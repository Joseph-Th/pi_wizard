use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bounded::BoundedText;
use crate::{RequestId, RuntimeLimits};

/// Bounded hot data for the currently streaming portion of one run.
#[derive(Debug)]
pub struct LiveProjection {
    assistant_blocks: BTreeMap<usize, AssistantContentBlock>,
    assistant_resident_bytes: usize,
    max_assistant_bytes: usize,
    max_assistant_blocks: usize,
    active_tools: HashMap<String, ToolPreview>,
    max_active_tools: usize,
    max_tool_preview_bytes: usize,
    direct_bash: HashMap<RequestId, BoundedText>,
    max_active_direct_bash: usize,
}

impl LiveProjection {
    #[must_use]
    pub fn new(limits: RuntimeLimits) -> Self {
        Self {
            assistant_blocks: BTreeMap::new(),
            assistant_resident_bytes: 0,
            max_assistant_bytes: limits.max_stream_text_bytes_per_run,
            max_assistant_blocks: limits.max_stream_content_blocks_per_message,
            active_tools: HashMap::new(),
            max_active_tools: limits.max_active_tools_per_run,
            max_tool_preview_bytes: limits.max_tool_preview_bytes,
            direct_bash: HashMap::new(),
            max_active_direct_bash: limits.max_pending_rpc_requests_per_run,
        }
    }

    /// Starts a bounded live preview for a direct RPC `bash` command.
    ///
    /// Pi can stream more direct Bash output through `bash_execution_update`
    /// than it returns in the final response. The request ID is therefore the
    /// only safe correlation key. The preview uses the same byte ceiling as a
    /// tool output preview but has separate identity and lifecycle state.
    pub fn start_direct_bash(&mut self, request_id: RequestId) -> Result<(), ProjectionError> {
        if self.direct_bash.contains_key(&request_id) {
            return Err(ProjectionError::DuplicateDirectBash { request_id });
        }
        if self.direct_bash.len() >= self.max_active_direct_bash {
            return Err(ProjectionError::TooManyDirectBash {
                limit: self.max_active_direct_bash,
            });
        }
        self.direct_bash
            .insert(request_id, BoundedText::new(self.max_tool_preview_bytes));
        Ok(())
    }

    pub fn append_direct_bash_delta(
        &mut self,
        request_id: &RequestId,
        delta: &str,
    ) -> Result<(), ProjectionError> {
        let preview = self.direct_bash.get_mut(request_id).ok_or_else(|| {
            ProjectionError::UnknownDirectBash {
                request_id: request_id.clone(),
            }
        })?;
        preview.append(delta);
        Ok(())
    }

    pub fn finish_direct_bash(
        &mut self,
        request_id: &RequestId,
    ) -> Result<BoundedText, ProjectionError> {
        self.direct_bash
            .remove(request_id)
            .ok_or_else(|| ProjectionError::UnknownDirectBash {
                request_id: request_id.clone(),
            })
    }

    pub fn cancel_direct_bash(&mut self, request_id: &RequestId) -> Option<BoundedText> {
        self.direct_bash.remove(request_id)
    }

    #[must_use]
    pub fn direct_bash_preview(&self, request_id: &RequestId) -> Option<&BoundedText> {
        self.direct_bash.get(request_id)
    }

    #[must_use]
    pub fn active_direct_bash_count(&self) -> usize {
        self.direct_bash.len()
    }

    pub fn start_assistant_block(
        &mut self,
        content_index: usize,
        kind: AssistantContentKind,
    ) -> Result<(), ProjectionError> {
        if self.assistant_blocks.contains_key(&content_index) {
            return Err(ProjectionError::DuplicateAssistantBlock { content_index });
        }
        if self.assistant_blocks.len() >= self.max_assistant_blocks {
            return Err(ProjectionError::TooManyAssistantBlocks {
                limit: self.max_assistant_blocks,
            });
        }

        self.assistant_blocks.insert(
            content_index,
            AssistantContentBlock {
                content_index,
                kind,
                content: BoundedText::new(self.max_assistant_bytes),
                complete: false,
            },
        );
        Ok(())
    }

    pub fn append_assistant_delta(
        &mut self,
        content_index: usize,
        kind: AssistantContentKind,
        delta: &str,
    ) -> Result<(), ProjectionError> {
        let before = {
            let block = self.assistant_block_mut(content_index, kind)?;
            let before = block.content.len_bytes();
            block.content.append(delta);
            before
        };
        let after = self
            .assistant_blocks
            .get(&content_index)
            .expect("assistant block exists after mutation")
            .content
            .len_bytes();
        self.assistant_resident_bytes = self
            .assistant_resident_bytes
            .saturating_sub(before)
            .saturating_add(after);
        self.enforce_assistant_budget();
        Ok(())
    }

    /// Marks a streamed content block complete. Text/thinking end events can
    /// provide their accumulated content, which replaces prior deltas before
    /// the aggregate hot-state budget is re-applied. `message_end.message`
    /// remains authoritative for the completed assistant message.
    pub fn finish_assistant_block(
        &mut self,
        content_index: usize,
        kind: AssistantContentKind,
        authoritative_content: Option<&str>,
    ) -> Result<(), ProjectionError> {
        let before = {
            let block = self.assistant_block_mut(content_index, kind)?;
            let before = block.content.len_bytes();
            if let Some(content) = authoritative_content {
                block.content.replace(content);
            }
            block.complete = true;
            before
        };
        let after = self
            .assistant_blocks
            .get(&content_index)
            .expect("assistant block exists after mutation")
            .content
            .len_bytes();
        self.assistant_resident_bytes = self
            .assistant_resident_bytes
            .saturating_sub(before)
            .saturating_add(after);
        self.enforce_assistant_budget();
        Ok(())
    }

    pub fn clear_assistant_message(&mut self) {
        self.assistant_blocks.clear();
        self.assistant_resident_bytes = 0;
    }

    /// Replaces the transient streamed assistant message with Pi's completed
    /// authoritative content in one bounded transaction.
    pub fn reconcile_assistant_message<I>(&mut self, blocks: I) -> Result<(), ProjectionError>
    where
        I: IntoIterator<Item = (usize, AssistantContentKind, String)>,
    {
        let mut next = BTreeMap::new();
        let mut resident_bytes = 0usize;
        for (content_index, kind, content) in blocks {
            if next.contains_key(&content_index) {
                return Err(ProjectionError::DuplicateAssistantBlock { content_index });
            }
            if next.len() >= self.max_assistant_blocks {
                return Err(ProjectionError::TooManyAssistantBlocks {
                    limit: self.max_assistant_blocks,
                });
            }
            let mut bounded = BoundedText::new(self.max_assistant_bytes);
            bounded.replace(&content);
            resident_bytes = resident_bytes.saturating_add(bounded.len_bytes());
            next.insert(
                content_index,
                AssistantContentBlock {
                    content_index,
                    kind,
                    content: bounded,
                    complete: true,
                },
            );
        }
        self.assistant_blocks = next;
        self.assistant_resident_bytes = resident_bytes;
        self.enforce_assistant_budget();
        Ok(())
    }

    pub fn start_tool(
        &mut self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Result<(), ProjectionError> {
        let tool_call_id = tool_call_id.into();
        if self.active_tools.contains_key(&tool_call_id) {
            return Err(ProjectionError::DuplicateTool { tool_call_id });
        }
        if self.active_tools.len() >= self.max_active_tools {
            return Err(ProjectionError::TooManyActiveTools {
                limit: self.max_active_tools,
            });
        }
        self.active_tools.insert(
            tool_call_id,
            ToolPreview {
                tool_name: tool_name.into(),
                output: BoundedText::new(self.max_tool_preview_bytes),
            },
        );
        Ok(())
    }

    /// Pi `tool_execution_update.partialResult` is accumulated, not a delta.
    pub fn replace_tool_output(
        &mut self,
        tool_call_id: &str,
        accumulated_output: &str,
    ) -> Result<(), ProjectionError> {
        let preview = self.active_tools.get_mut(tool_call_id).ok_or_else(|| {
            ProjectionError::UnknownTool {
                tool_call_id: tool_call_id.to_owned(),
            }
        })?;
        preview.output.replace(accumulated_output);
        Ok(())
    }

    pub fn finish_tool(&mut self, tool_call_id: &str) -> Result<ToolPreview, ProjectionError> {
        self.active_tools
            .remove(tool_call_id)
            .ok_or_else(|| ProjectionError::UnknownTool {
                tool_call_id: tool_call_id.to_owned(),
            })
    }

    #[must_use]
    pub fn assistant_block(&self, content_index: usize) -> Option<&AssistantContentBlock> {
        self.assistant_blocks.get(&content_index)
    }

    #[must_use]
    pub fn assistant_block_snapshot(
        &self,
        content_index: usize,
    ) -> Option<AssistantContentSnapshot> {
        self.assistant_blocks
            .get(&content_index)
            .map(|block| AssistantContentSnapshot {
                content_index: block.content_index,
                kind: block.kind,
                text: block.content.as_str().to_owned(),
                dropped_bytes: block.content.dropped_bytes(),
                complete: block.complete,
            })
    }

    pub fn assistant_blocks(&self) -> impl Iterator<Item = &AssistantContentBlock> {
        self.assistant_blocks.values()
    }

    #[must_use]
    pub const fn assistant_resident_bytes(&self) -> usize {
        self.assistant_resident_bytes
    }

    #[must_use]
    pub fn active_tool_count(&self) -> usize {
        self.active_tools.len()
    }

    #[must_use]
    pub fn tool_snapshot(&self, tool_call_id: &str) -> Option<ToolPreviewSnapshot> {
        self.active_tools
            .get(tool_call_id)
            .map(|preview| ToolPreviewSnapshot {
                tool_call_id: tool_call_id.to_owned(),
                tool_name: preview.tool_name.clone(),
                output: preview.output.as_str().to_owned(),
                dropped_bytes: preview.output.dropped_bytes(),
            })
    }

    #[must_use]
    pub fn direct_bash_snapshot(&self, request_id: &RequestId) -> Option<DirectBashSnapshot> {
        self.direct_bash
            .get(request_id)
            .map(|preview| DirectBashSnapshot {
                request_id: request_id.clone(),
                output: preview.as_str().to_owned(),
                dropped_bytes: preview.dropped_bytes(),
            })
    }

    #[must_use]
    pub fn assistant_block_count(&self) -> usize {
        self.assistant_blocks.len()
    }

    #[must_use]
    pub fn snapshot(&self) -> LiveProjectionSnapshot {
        let assistant_blocks = self
            .assistant_blocks
            .values()
            .map(|block| AssistantContentSnapshot {
                content_index: block.content_index,
                kind: block.kind,
                text: block.content.as_str().to_owned(),
                dropped_bytes: block.content.dropped_bytes(),
                complete: block.complete,
            })
            .collect();
        let mut active_tools: Vec<_> = self
            .active_tools
            .iter()
            .map(|(tool_call_id, preview)| ToolPreviewSnapshot {
                tool_call_id: tool_call_id.clone(),
                tool_name: preview.tool_name.clone(),
                output: preview.output.as_str().to_owned(),
                dropped_bytes: preview.output.dropped_bytes(),
            })
            .collect();
        active_tools.sort_by(|left, right| left.tool_call_id.cmp(&right.tool_call_id));
        let mut direct_bash: Vec<_> = self
            .direct_bash
            .iter()
            .map(|(request_id, preview)| DirectBashSnapshot {
                request_id: request_id.clone(),
                output: preview.as_str().to_owned(),
                dropped_bytes: preview.dropped_bytes(),
            })
            .collect();
        direct_bash.sort_by(|left, right| left.request_id.as_str().cmp(right.request_id.as_str()));
        LiveProjectionSnapshot {
            assistant_blocks,
            active_tools,
            direct_bash,
        }
    }

    fn assistant_block_mut(
        &mut self,
        content_index: usize,
        expected_kind: AssistantContentKind,
    ) -> Result<&mut AssistantContentBlock, ProjectionError> {
        let block = self
            .assistant_blocks
            .get_mut(&content_index)
            .ok_or(ProjectionError::UnknownAssistantBlock { content_index })?;
        if block.kind != expected_kind {
            return Err(ProjectionError::AssistantBlockKindMismatch {
                content_index,
                expected: expected_kind,
                actual: block.kind,
            });
        }
        Ok(block)
    }

    fn enforce_assistant_budget(&mut self) {
        let mut excess = self
            .assistant_resident_bytes
            .saturating_sub(self.max_assistant_bytes);
        if excess == 0 {
            return;
        }

        for block in self.assistant_blocks.values_mut() {
            if excess == 0 {
                break;
            }
            let dropped = block.content.drop_oldest_bytes(excess);
            self.assistant_resident_bytes = self.assistant_resident_bytes.saturating_sub(dropped);
            excess = self
                .assistant_resident_bytes
                .saturating_sub(self.max_assistant_bytes);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantContentKind {
    Text,
    Thinking,
    ToolCall,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProjectionSnapshot {
    pub assistant_blocks: Vec<AssistantContentSnapshot>,
    pub active_tools: Vec<ToolPreviewSnapshot>,
    pub direct_bash: Vec<DirectBashSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantContentSnapshot {
    pub content_index: usize,
    pub kind: AssistantContentKind,
    pub text: String,
    pub dropped_bytes: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPreviewSnapshot {
    pub tool_call_id: String,
    pub tool_name: String,
    pub output: String,
    pub dropped_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectBashSnapshot {
    pub request_id: RequestId,
    pub output: String,
    pub dropped_bytes: u64,
}

#[derive(Debug)]
pub struct AssistantContentBlock {
    pub content_index: usize,
    pub kind: AssistantContentKind,
    pub content: BoundedText,
    pub complete: bool,
}

#[derive(Debug)]
pub struct ToolPreview {
    pub tool_name: String,
    pub output: BoundedText,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ProjectionError {
    #[error("assistant content block {content_index} is already active")]
    DuplicateAssistantBlock { content_index: usize },
    #[error("assistant content block limit {limit} reached")]
    TooManyAssistantBlocks { limit: usize },
    #[error("assistant content block {content_index} has not started")]
    UnknownAssistantBlock { content_index: usize },
    #[error(
        "assistant content block {content_index} kind mismatch: expected {expected:?}, got {actual:?}"
    )]
    AssistantBlockKindMismatch {
        content_index: usize,
        expected: AssistantContentKind,
        actual: AssistantContentKind,
    },
    #[error("tool {tool_call_id} is already active")]
    DuplicateTool { tool_call_id: String },
    #[error("active tool limit {limit} reached")]
    TooManyActiveTools { limit: usize },
    #[error("tool {tool_call_id} is not active")]
    UnknownTool { tool_call_id: String },
    #[error("direct bash request {request_id} is already active")]
    DuplicateDirectBash { request_id: RequestId },
    #[error("active direct bash limit {limit} reached")]
    TooManyDirectBash { limit: usize },
    #[error("direct bash request {request_id} is not active")]
    UnknownDirectBash { request_id: RequestId },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_stream_preserves_content_index_and_kind() {
        let mut projection = LiveProjection::new(RuntimeLimits::default());
        projection
            .start_assistant_block(2, AssistantContentKind::Thinking)
            .expect("thinking block");
        projection
            .start_assistant_block(0, AssistantContentKind::Text)
            .expect("text block");
        projection
            .start_assistant_block(1, AssistantContentKind::ToolCall)
            .expect("tool-call block");

        projection
            .append_assistant_delta(2, AssistantContentKind::Thinking, "reason")
            .expect("thinking delta");
        projection
            .append_assistant_delta(0, AssistantContentKind::Text, "answer")
            .expect("text delta");
        projection
            .append_assistant_delta(1, AssistantContentKind::ToolCall, "{\"path\":")
            .expect("tool-call delta");

        let blocks: Vec<_> = projection.assistant_blocks().collect();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].content_index, 0);
        assert_eq!(blocks[0].kind, AssistantContentKind::Text);
        assert_eq!(blocks[0].content.as_str(), "answer");
        assert_eq!(blocks[1].content_index, 1);
        assert_eq!(blocks[1].kind, AssistantContentKind::ToolCall);
        assert_eq!(blocks[2].content_index, 2);
        assert_eq!(blocks[2].kind, AssistantContentKind::Thinking);
        assert_eq!(blocks[2].content.as_str(), "reason");
    }

    #[test]
    fn assistant_stream_aggregate_never_exceeds_byte_limit() {
        let limits = RuntimeLimits {
            max_stream_text_bytes_per_run: 8,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        projection
            .start_assistant_block(0, AssistantContentKind::Text)
            .expect("text block");
        projection
            .append_assistant_delta(0, AssistantContentKind::Text, "abcd")
            .expect("text delta");
        projection
            .start_assistant_block(1, AssistantContentKind::Thinking)
            .expect("thinking block");
        projection
            .append_assistant_delta(1, AssistantContentKind::Thinking, "uvwxyz")
            .expect("thinking delta");

        assert_eq!(projection.assistant_resident_bytes(), 8);
        assert_eq!(
            projection
                .assistant_block(0)
                .expect("text")
                .content
                .as_str(),
            "cd"
        );
        assert_eq!(
            projection
                .assistant_block(1)
                .expect("thinking")
                .content
                .as_str(),
            "uvwxyz"
        );
    }

    #[test]
    fn assistant_block_kind_mismatch_is_rejected() {
        let mut projection = LiveProjection::new(RuntimeLimits::default());
        projection
            .start_assistant_block(0, AssistantContentKind::Text)
            .expect("text block");

        assert_eq!(
            projection.append_assistant_delta(0, AssistantContentKind::Thinking, "wrong"),
            Err(ProjectionError::AssistantBlockKindMismatch {
                content_index: 0,
                expected: AssistantContentKind::Thinking,
                actual: AssistantContentKind::Text,
            })
        );
    }

    #[test]
    fn assistant_block_count_is_bounded_without_sparse_index_allocation() {
        let limits = RuntimeLimits {
            max_stream_content_blocks_per_message: 1,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        projection
            .start_assistant_block(10_000_000, AssistantContentKind::Text)
            .expect("sparse content index should not allocate a huge vector");

        assert_eq!(
            projection.start_assistant_block(1, AssistantContentKind::Thinking),
            Err(ProjectionError::TooManyAssistantBlocks { limit: 1 })
        );
    }

    #[test]
    fn assistant_end_content_replaces_streamed_delta_before_final_message() {
        let mut projection = LiveProjection::new(RuntimeLimits::default());
        projection
            .start_assistant_block(0, AssistantContentKind::Text)
            .expect("text block");
        projection
            .append_assistant_delta(0, AssistantContentKind::Text, "partial")
            .expect("partial");
        projection
            .finish_assistant_block(0, AssistantContentKind::Text, Some("authoritative block"))
            .expect("finish");

        let block = projection.assistant_block(0).expect("block");
        assert!(block.complete);
        assert_eq!(block.content.as_str(), "authoritative block");
    }

    #[test]
    fn completed_assistant_message_replaces_stream_preview_atomically_and_stays_bounded() {
        let limits = RuntimeLimits {
            max_stream_text_bytes_per_run: 10,
            max_stream_content_blocks_per_message: 2,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        projection
            .start_assistant_block(0, AssistantContentKind::Text)
            .expect("stream block");
        projection
            .append_assistant_delta(0, AssistantContentKind::Text, "stale")
            .expect("stream delta");

        projection
            .reconcile_assistant_message([
                (0, AssistantContentKind::Text, "final".to_owned()),
                (1, AssistantContentKind::Thinking, "reasoning".to_owned()),
            ])
            .expect("final message");
        assert_eq!(projection.assistant_block_count(), 2);
        assert_eq!(
            projection
                .assistant_block(0)
                .expect("text")
                .content
                .as_str(),
            "l"
        );
        assert_eq!(
            projection
                .assistant_block(1)
                .expect("thinking")
                .content
                .as_str(),
            "reasoning"
        );
        assert!(projection.assistant_blocks().all(|block| block.complete));
        assert_eq!(projection.assistant_resident_bytes(), 10);

        let before = projection.snapshot();
        assert_eq!(
            projection.reconcile_assistant_message([
                (0, AssistantContentKind::Text, "one".to_owned()),
                (1, AssistantContentKind::Thinking, "two".to_owned()),
                (2, AssistantContentKind::Text, "three".to_owned()),
            ]),
            Err(ProjectionError::TooManyAssistantBlocks { limit: 2 })
        );
        assert_eq!(projection.snapshot(), before);
    }

    #[test]
    fn tool_updates_replace_accumulated_output_and_remain_bounded() {
        let limits = RuntimeLimits {
            max_tool_preview_bytes: 6,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        projection.start_tool("call-1", "bash").expect("start tool");
        projection
            .replace_tool_output("call-1", "first")
            .expect("first update");
        projection
            .replace_tool_output("call-1", "0123456789")
            .expect("second update");

        let finished = projection.finish_tool("call-1").expect("finish tool");
        assert_eq!(finished.output.as_str(), "456789");
        assert_eq!(finished.output.dropped_bytes(), 4);
    }

    #[test]
    fn active_tool_count_is_bounded() {
        let limits = RuntimeLimits {
            max_active_tools_per_run: 1,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        projection.start_tool("a", "read").expect("first tool");

        assert_eq!(
            projection.start_tool("b", "write"),
            Err(ProjectionError::TooManyActiveTools { limit: 1 })
        );
    }

    #[test]
    fn direct_bash_stream_is_request_correlated_and_bounded() {
        let limits = RuntimeLimits {
            max_tool_preview_bytes: 6,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        let first = RequestId::from_wire("bash-1");
        let second = RequestId::from_wire("bash-2");
        projection
            .start_direct_bash(first.clone())
            .expect("first bash");
        projection
            .start_direct_bash(second.clone())
            .expect("second bash");

        projection
            .append_direct_bash_delta(&first, "0123456789")
            .expect("first output");
        projection
            .append_direct_bash_delta(&second, "other")
            .expect("second output");

        assert_eq!(
            projection
                .direct_bash_preview(&first)
                .expect("first preview")
                .as_str(),
            "456789"
        );
        assert_eq!(
            projection
                .direct_bash_preview(&second)
                .expect("second preview")
                .as_str(),
            "other"
        );
        assert_eq!(projection.active_direct_bash_count(), 2);

        let finished = projection.finish_direct_bash(&first).expect("finish first");
        assert_eq!(finished.as_str(), "456789");
        assert!(projection.direct_bash_preview(&first).is_none());
    }

    #[test]
    fn direct_bash_count_is_bounded_by_pending_rpc_capacity() {
        let limits = RuntimeLimits {
            max_pending_rpc_requests_per_run: 1,
            ..RuntimeLimits::default()
        };
        let mut projection = LiveProjection::new(limits);
        projection
            .start_direct_bash(RequestId::from_wire("one"))
            .expect("first bash");

        assert_eq!(
            projection.start_direct_bash(RequestId::from_wire("two")),
            Err(ProjectionError::TooManyDirectBash { limit: 1 })
        );
    }
}
