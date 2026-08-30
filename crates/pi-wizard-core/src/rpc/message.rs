use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use super::command::ThinkingLevel;
use super::response::ClearQueueResult;
use crate::{RequestId, RuntimeLimits};

#[derive(Clone, Debug, PartialEq)]
pub enum InboundMessage {
    Response(RpcResponse),
    Event(RpcEvent),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RpcResponse {
    #[serde(default)]
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcResponseOutcome {
    Rejected,
    Accepted,
    Cancelled,
}

impl RpcResponse {
    /// Semantic outcome for a command response.
    ///
    /// Some Pi session mutations can be cancelled by extensions while still
    /// returning protocol-level `success: true`. Callers must not mutate their
    /// local session binding until this outcome is `Accepted`.
    #[must_use]
    pub fn outcome(&self) -> RpcResponseOutcome {
        if !self.success {
            return RpcResponseOutcome::Rejected;
        }
        if is_cancellable_session_command(&self.command)
            && self
                .data
                .as_ref()
                .and_then(|data| data.get("cancelled"))
                .and_then(Value::as_bool)
                == Some(true)
        {
            RpcResponseOutcome::Cancelled
        } else {
            RpcResponseOutcome::Accepted
        }
    }
}

fn is_cancellable_session_command(command: &str) -> bool {
    matches!(command, "new_session" | "switch_session" | "fork" | "clone")
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcEvent {
    pub kind: RpcEventKind,
    /// Original object retained so protocol additions do not require lossy parsing.
    pub raw: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssistantMessageBlockKind {
    Text,
    Thinking,
    ToolCall,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssistantStopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Unknown(String),
}

impl AssistantStopReason {
    fn from_wire(value: &str) -> Self {
        match value {
            "stop" => Self::Stop,
            "length" => Self::Length,
            "toolUse" => Self::ToolUse,
            "error" => Self::Error,
            "aborted" => Self::Aborted,
            unknown => Self::Unknown(unknown.to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCallStartMeta {
    pub id: String,
    pub tool_name: String,
}

/// Typed view of Pi's nested `assistantMessageEvent` stream payload.
///
/// `message_end.message` remains authoritative. This type exists only to make
/// bounded live assembly explicit and content-index aware while streaming.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssistantMessageUpdate {
    Start {
        content_index: usize,
        kind: AssistantMessageBlockKind,
        tool_call: Option<ToolCallStartMeta>,
    },
    Delta {
        content_index: usize,
        kind: AssistantMessageBlockKind,
        delta: String,
    },
    End {
        content_index: usize,
        kind: AssistantMessageBlockKind,
        /// Present for current text/thinking end events. Tool-call completion
        /// is intentionally not copied here because Pi includes the complete
        /// call object and the final message is authoritative anyway.
        content: Option<String>,
    },
    /// Preserve compatibility if Pi adds a new nested stream event before Pi
    /// Wizard learns how to project it.
    Unknown {
        event_type: String,
        content_index: Option<usize>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantMessageFinalBlock {
    pub content_index: usize,
    pub kind: AssistantMessageBlockKind,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantMessageEnd {
    pub blocks: Vec<AssistantMessageFinalBlock>,
    pub stop_reason: AssistantStopReason,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BashExecutionUpdate {
    pub request_id: Option<RequestId>,
    pub delta: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecutionStart {
    pub tool_call_id: String,
    pub tool_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecutionUpdate {
    pub tool_call_id: String,
    pub tool_name: String,
    /// Current Pi tool progress is accumulated, not a delta. Consumers replace
    /// their prior preview with this value.
    pub accumulated_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecutionEnd {
    pub tool_call_id: String,
    pub tool_name: String,
    pub final_text: String,
    pub is_error: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueUpdateCounts {
    pub steering: usize,
    pub follow_up: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionInfoChanged {
    pub name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionStartEvent {
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionEndEvent {
    pub reason: String,
    pub aborted: bool,
    pub will_retry: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoRetryStart {
    pub attempt: usize,
    pub max_attempts: usize,
    pub delay_ms: u64,
    pub error_message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoRetryEnd {
    pub success: bool,
    pub attempt: usize,
    pub final_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SummarizationRetryScheduled {
    pub attempt: usize,
    pub max_attempts: usize,
    pub delay_ms: u64,
    pub error_message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SummarizationRetryAttemptStart {
    pub source: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionErrorEvent {
    pub extension_path: String,
    pub event: String,
    pub error: String,
}

impl RpcEvent {
    /// Parses a current Pi `message_update` without cloning the cumulative raw
    /// event object or pretending it is a complete assistant message.
    pub fn assistant_message_update(
        &self,
    ) -> Result<Option<AssistantMessageUpdate>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::MessageUpdate {
            return Ok(None);
        }
        let nested = self
            .raw
            .get("assistantMessageEvent")
            .and_then(Value::as_object)
            .ok_or(RpcEventPayloadError::MissingObject {
                event: "message_update",
                field: "assistantMessageEvent",
            })?;
        let event_type = nested.get("type").and_then(Value::as_str).ok_or(
            RpcEventPayloadError::MissingString {
                event: "message_update",
                field: "assistantMessageEvent.type",
            },
        )?;

        let content_index = || -> Result<usize, RpcEventPayloadError> {
            let value = nested.get("contentIndex").and_then(Value::as_u64).ok_or(
                RpcEventPayloadError::MissingUnsignedInteger {
                    event: "message_update",
                    field: "assistantMessageEvent.contentIndex",
                },
            )?;
            usize::try_from(value).map_err(|_| RpcEventPayloadError::IntegerOutOfRange {
                event: "message_update",
                field: "assistantMessageEvent.contentIndex",
                value,
            })
        };
        let required_string = |field: &'static str| -> Result<String, RpcEventPayloadError> {
            nested
                .get(
                    field
                        .rsplit('.')
                        .next()
                        .expect("static field has a component"),
                )
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(RpcEventPayloadError::MissingString {
                    event: "message_update",
                    field,
                })
        };

        let update = match event_type {
            "text_start" => AssistantMessageUpdate::Start {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::Text,
                tool_call: None,
            },
            "thinking_start" => AssistantMessageUpdate::Start {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::Thinking,
                tool_call: None,
            },
            "toolcall_start" => AssistantMessageUpdate::Start {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::ToolCall,
                tool_call: Some(ToolCallStartMeta {
                    id: required_string("assistantMessageEvent.id")?,
                    tool_name: required_string("assistantMessageEvent.toolName")?,
                }),
            },
            "text_delta" => AssistantMessageUpdate::Delta {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::Text,
                delta: required_string("assistantMessageEvent.delta")?,
            },
            "thinking_delta" => AssistantMessageUpdate::Delta {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::Thinking,
                delta: required_string("assistantMessageEvent.delta")?,
            },
            "toolcall_delta" => AssistantMessageUpdate::Delta {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::ToolCall,
                delta: required_string("assistantMessageEvent.delta")?,
            },
            "text_end" => AssistantMessageUpdate::End {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::Text,
                content: Some(required_string("assistantMessageEvent.content")?),
            },
            "thinking_end" => AssistantMessageUpdate::End {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::Thinking,
                content: Some(required_string("assistantMessageEvent.content")?),
            },
            "toolcall_end" => AssistantMessageUpdate::End {
                content_index: content_index()?,
                kind: AssistantMessageBlockKind::ToolCall,
                content: None,
            },
            unknown => AssistantMessageUpdate::Unknown {
                event_type: unknown.to_owned(),
                content_index: nested
                    .get("contentIndex")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok()),
            },
        };
        Ok(Some(update))
    }

    /// Parses Pi's authoritative completed assistant message. Pi emits
    /// `message_end` for every AgentMessage, including tool results, so a
    /// non-assistant message is not an assistant completion.
    pub fn assistant_message_end(
        &self,
    ) -> Result<Option<AssistantMessageEnd>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::MessageEnd {
            return Ok(None);
        }
        let message = self.raw.get("message").and_then(Value::as_object).ok_or(
            RpcEventPayloadError::MissingObject {
                event: "message_end",
                field: "message",
            },
        )?;
        let role = message.get("role").and_then(Value::as_str).ok_or(
            RpcEventPayloadError::MissingString {
                event: "message_end",
                field: "message.role",
            },
        )?;
        if role != "assistant" {
            return Ok(None);
        }
        let stop_reason = message.get("stopReason").and_then(Value::as_str).ok_or(
            RpcEventPayloadError::MissingString {
                event: "message_end",
                field: "message.stopReason",
            },
        )?;
        let error_message = match message.get("errorMessage") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => {
                return Err(RpcEventPayloadError::InvalidOptionalString {
                    event: "message_end",
                    field: "message.errorMessage",
                });
            }
        };
        let content = message.get("content").and_then(Value::as_array).ok_or(
            RpcEventPayloadError::MissingArray {
                event: "message_end",
                field: "message.content",
            },
        )?;
        let mut blocks = Vec::with_capacity(content.len());
        for (content_index, value) in content.iter().enumerate() {
            let block = value
                .as_object()
                .ok_or(RpcEventPayloadError::InvalidContentItem {
                    event: "message_end",
                })?;
            let Some(block_type) = block.get("type").and_then(Value::as_str) else {
                return Err(RpcEventPayloadError::MissingString {
                    event: "message_end",
                    field: "message.content[].type",
                });
            };
            let (kind, content) = match block_type {
                "text" => (
                    AssistantMessageBlockKind::Text,
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(RpcEventPayloadError::MissingString {
                            event: "message_end",
                            field: "message.content[].text",
                        })?
                        .to_owned(),
                ),
                "thinking" => (
                    AssistantMessageBlockKind::Thinking,
                    block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or(RpcEventPayloadError::MissingString {
                            event: "message_end",
                            field: "message.content[].thinking",
                        })?
                        .to_owned(),
                ),
                "toolCall" => (
                    AssistantMessageBlockKind::ToolCall,
                    block
                        .get("arguments")
                        .map(Value::to_string)
                        .unwrap_or_else(|| value.to_string()),
                ),
                _ => continue,
            };
            blocks.push(AssistantMessageFinalBlock {
                content_index,
                kind,
                content,
            });
        }
        Ok(Some(AssistantMessageEnd {
            blocks,
            stop_reason: AssistantStopReason::from_wire(stop_reason),
            error_message,
        }))
    }

    /// Typed view of direct RPC bash output. Pi includes the originating
    /// request ID when the command had one; Pi Wizard always sends IDs, so the
    /// process adapter can require correlation without guessing by chronology.
    pub fn bash_execution_update(
        &self,
    ) -> Result<Option<BashExecutionUpdate>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::BashExecutionUpdate {
            return Ok(None);
        }
        let delta = self.raw.get("delta").and_then(Value::as_str).ok_or(
            RpcEventPayloadError::MissingString {
                event: "bash_execution_update",
                field: "delta",
            },
        )?;
        let request_id = match self.raw.get("id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) => Some(RequestId::from_wire(id.clone())),
            Some(_) => {
                return Err(RpcEventPayloadError::InvalidOptionalString {
                    event: "bash_execution_update",
                    field: "id",
                });
            }
        };
        Ok(Some(BashExecutionUpdate {
            request_id,
            delta: delta.to_owned(),
        }))
    }

    pub fn tool_execution_start(&self) -> Result<Option<ToolExecutionStart>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::ToolExecutionStart {
            return Ok(None);
        }
        Ok(Some(ToolExecutionStart {
            tool_call_id: required_string(&self.raw, "tool_execution_start", "toolCallId")?,
            tool_name: required_string(&self.raw, "tool_execution_start", "toolName")?,
        }))
    }

    pub fn tool_execution_update(
        &self,
    ) -> Result<Option<ToolExecutionUpdate>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::ToolExecutionUpdate {
            return Ok(None);
        }
        Ok(Some(ToolExecutionUpdate {
            tool_call_id: required_string(&self.raw, "tool_execution_update", "toolCallId")?,
            tool_name: required_string(&self.raw, "tool_execution_update", "toolName")?,
            accumulated_text: tool_result_text(
                &self.raw,
                "tool_execution_update",
                "partialResult",
            )?,
        }))
    }

    pub fn tool_execution_end(&self) -> Result<Option<ToolExecutionEnd>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::ToolExecutionEnd {
            return Ok(None);
        }
        let is_error = self.raw.get("isError").and_then(Value::as_bool).ok_or(
            RpcEventPayloadError::MissingBoolean {
                event: "tool_execution_end",
                field: "isError",
            },
        )?;
        Ok(Some(ToolExecutionEnd {
            tool_call_id: required_string(&self.raw, "tool_execution_end", "toolCallId")?,
            tool_name: required_string(&self.raw, "tool_execution_end", "toolName")?,
            final_text: tool_result_text(&self.raw, "tool_execution_end", "result")?,
            is_error,
        }))
    }

    /// Returns queue sizes without copying queued user text. The controller's
    /// separate bounded recovery parser is the only event path allowed to
    /// retain queue strings, and it keeps them outside runtime hydration.
    pub fn queue_update_counts(&self) -> Result<Option<QueueUpdateCounts>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::QueueUpdate {
            return Ok(None);
        }
        let steering = required_string_array(&self.raw, "queue_update", "steering")?;
        let follow_up = required_string_array(&self.raw, "queue_update", "followUp")?;
        Ok(Some(QueueUpdateCounts {
            steering: steering.len(),
            follow_up: follow_up.len(),
        }))
    }

    /// Returns the current user-visible Pi queues under the same hard limits
    /// used by Stop recovery. This copy is suitable only for a private
    /// emergency recovery shadow; `queue_update` remains an event projection,
    /// not queue authority.
    pub fn queue_update_recovery(
        &self,
        limits: RuntimeLimits,
    ) -> Result<Option<ClearQueueResult>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::QueueUpdate {
            return Ok(None);
        }
        let steering = required_string_array(&self.raw, "queue_update", "steering")?;
        let follow_up = required_string_array(&self.raw, "queue_update", "followUp")?;
        let message_count = steering.len().saturating_add(follow_up.len());
        if message_count > limits.max_recovered_queue_messages_per_run {
            return Err(RpcEventPayloadError::RecoveredQueueMessageLimit {
                actual: message_count,
                limit: limits.max_recovered_queue_messages_per_run,
            });
        }
        let mut text_bytes = 0usize;
        for value in steering.iter().chain(follow_up) {
            let text = value
                .as_str()
                .expect("required_string_array validates queue strings");
            text_bytes = text_bytes.saturating_add(text.len());
            if text_bytes > limits.max_recovered_queue_bytes_per_run {
                return Err(RpcEventPayloadError::RecoveredQueueByteLimit {
                    attempted: text_bytes,
                    limit: limits.max_recovered_queue_bytes_per_run,
                });
            }
        }
        Ok(Some(ClearQueueResult {
            steering: steering
                .iter()
                .map(|value| value.as_str().expect("validated").to_owned())
                .collect(),
            follow_up: follow_up
                .iter()
                .map(|value| value.as_str().expect("validated").to_owned())
                .collect(),
        }))
    }

    pub fn session_info_changed(&self) -> Result<Option<SessionInfoChanged>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::SessionInfoChanged {
            return Ok(None);
        }
        let name = match self.raw.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => {
                return Err(RpcEventPayloadError::InvalidOptionalString {
                    event: "session_info_changed",
                    field: "name",
                });
            }
        };
        Ok(Some(SessionInfoChanged { name }))
    }

    pub fn compaction_start(&self) -> Result<Option<CompactionStartEvent>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::CompactionStart {
            return Ok(None);
        }
        Ok(Some(CompactionStartEvent {
            reason: required_string(&self.raw, "compaction_start", "reason")?,
        }))
    }

    pub fn compaction_end(&self) -> Result<Option<CompactionEndEvent>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::CompactionEnd {
            return Ok(None);
        }
        Ok(Some(CompactionEndEvent {
            reason: required_string(&self.raw, "compaction_end", "reason")?,
            aborted: required_bool(&self.raw, "compaction_end", "aborted")?,
            will_retry: required_bool(&self.raw, "compaction_end", "willRetry")?,
            error_message: optional_string(&self.raw, "compaction_end", "errorMessage")?,
        }))
    }

    pub fn auto_retry_start(&self) -> Result<Option<AutoRetryStart>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::AutoRetryStart {
            return Ok(None);
        }
        Ok(Some(AutoRetryStart {
            attempt: required_usize(&self.raw, "auto_retry_start", "attempt")?,
            max_attempts: required_usize(&self.raw, "auto_retry_start", "maxAttempts")?,
            delay_ms: required_u64(&self.raw, "auto_retry_start", "delayMs")?,
            error_message: required_string(&self.raw, "auto_retry_start", "errorMessage")?,
        }))
    }

    pub fn auto_retry_end(&self) -> Result<Option<AutoRetryEnd>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::AutoRetryEnd {
            return Ok(None);
        }
        Ok(Some(AutoRetryEnd {
            success: required_bool(&self.raw, "auto_retry_end", "success")?,
            attempt: required_usize(&self.raw, "auto_retry_end", "attempt")?,
            final_error: optional_string(&self.raw, "auto_retry_end", "finalError")?,
        }))
    }

    pub fn summarization_retry_scheduled(
        &self,
    ) -> Result<Option<SummarizationRetryScheduled>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::SummarizationRetryScheduled {
            return Ok(None);
        }
        Ok(Some(SummarizationRetryScheduled {
            attempt: required_usize(&self.raw, "summarization_retry_scheduled", "attempt")?,
            max_attempts: required_usize(
                &self.raw,
                "summarization_retry_scheduled",
                "maxAttempts",
            )?,
            delay_ms: required_u64(&self.raw, "summarization_retry_scheduled", "delayMs")?,
            error_message: required_string(
                &self.raw,
                "summarization_retry_scheduled",
                "errorMessage",
            )?,
        }))
    }

    pub fn summarization_retry_attempt_start(
        &self,
    ) -> Result<Option<SummarizationRetryAttemptStart>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::SummarizationRetryAttemptStart {
            return Ok(None);
        }
        Ok(Some(SummarizationRetryAttemptStart {
            source: required_string(&self.raw, "summarization_retry_attempt_start", "source")?,
            reason: optional_string(&self.raw, "summarization_retry_attempt_start", "reason")?,
        }))
    }

    pub fn extension_error(&self) -> Result<Option<ExtensionErrorEvent>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::ExtensionError {
            return Ok(None);
        }
        Ok(Some(ExtensionErrorEvent {
            extension_path: required_string(&self.raw, "extension_error", "extensionPath")?,
            event: required_string(&self.raw, "extension_error", "event")?,
            error: required_string(&self.raw, "extension_error", "error")?,
        }))
    }

    pub fn thinking_level_changed(&self) -> Result<Option<ThinkingLevel>, RpcEventPayloadError> {
        if self.kind != RpcEventKind::ThinkingLevelChanged {
            return Ok(None);
        }
        let value = self
            .raw
            .get("level")
            .cloned()
            .ok_or(RpcEventPayloadError::MissingString {
                event: "thinking_level_changed",
                field: "level",
            })?;
        serde_json::from_value(value).map(Some).map_err(|_| {
            RpcEventPayloadError::InvalidEnumValue {
                event: "thinking_level_changed",
                field: "level",
            }
        })
    }
}

fn required_u64(
    value: &Value,
    event: &'static str,
    field: &'static str,
) -> Result<u64, RpcEventPayloadError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(RpcEventPayloadError::MissingUnsignedInteger { event, field })
}

fn required_usize(
    value: &Value,
    event: &'static str,
    field: &'static str,
) -> Result<usize, RpcEventPayloadError> {
    let value = required_u64(value, event, field)?;
    usize::try_from(value).map_err(|_| RpcEventPayloadError::IntegerOutOfRange {
        event,
        field,
        value,
    })
}

fn required_bool(
    value: &Value,
    event: &'static str,
    field: &'static str,
) -> Result<bool, RpcEventPayloadError> {
    value
        .get(field)
        .and_then(Value::as_bool)
        .ok_or(RpcEventPayloadError::MissingBoolean { event, field })
}

fn optional_string(
    value: &Value,
    event: &'static str,
    field: &'static str,
) -> Result<Option<String>, RpcEventPayloadError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RpcEventPayloadError::InvalidOptionalString { event, field }),
    }
}

fn required_string(
    value: &Value,
    event: &'static str,
    field: &'static str,
) -> Result<String, RpcEventPayloadError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(RpcEventPayloadError::MissingString { event, field })
}

fn required_string_array<'a>(
    value: &'a Value,
    event: &'static str,
    field: &'static str,
) -> Result<&'a [Value], RpcEventPayloadError> {
    let values = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or(RpcEventPayloadError::MissingArray { event, field })?;
    if values.iter().any(|value| !value.is_string()) {
        return Err(RpcEventPayloadError::InvalidStringArray { event, field });
    }
    Ok(values)
}

fn tool_result_text(
    event_value: &Value,
    event: &'static str,
    result_field: &'static str,
) -> Result<String, RpcEventPayloadError> {
    let result = event_value
        .get(result_field)
        .and_then(Value::as_object)
        .ok_or(RpcEventPayloadError::MissingObject {
            event,
            field: result_field,
        })?;
    let content = result.get("content").and_then(Value::as_array).ok_or(
        RpcEventPayloadError::MissingArray {
            event,
            field: "content",
        },
    )?;

    let mut text = String::new();
    for item in content {
        let object = item
            .as_object()
            .ok_or(RpcEventPayloadError::InvalidContentItem { event })?;
        match object.get("type").and_then(Value::as_str) {
            Some("text") => {
                let part = object.get("text").and_then(Value::as_str).ok_or(
                    RpcEventPayloadError::MissingString {
                        event,
                        field: "content[].text",
                    },
                )?;
                text.push_str(part);
            }
            Some(_) => {}
            None => return Err(RpcEventPayloadError::InvalidContentItem { event }),
        }
    }
    Ok(text)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RpcEventKind {
    AgentStart,
    AgentEnd,
    AgentSettled,
    TurnStart,
    TurnEnd,
    MessageStart,
    MessageUpdate,
    MessageEnd,
    BashExecutionUpdate,
    ToolExecutionStart,
    ToolExecutionUpdate,
    ToolExecutionEnd,
    QueueUpdate,
    EntryAppended,
    SessionInfoChanged,
    ThinkingLevelChanged,
    CompactionStart,
    CompactionEnd,
    AutoRetryStart,
    AutoRetryEnd,
    SummarizationRetryScheduled,
    SummarizationRetryAttemptStart,
    SummarizationRetryFinished,
    ExtensionError,
    ExtensionUiRequest,
    Unknown(String),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RpcEventPayloadError {
    #[error("{event} is missing object field {field}")]
    MissingObject {
        event: &'static str,
        field: &'static str,
    },
    #[error("{event} is missing boolean field {field}")]
    MissingBoolean {
        event: &'static str,
        field: &'static str,
    },
    #[error("{event} is missing array field {field}")]
    MissingArray {
        event: &'static str,
        field: &'static str,
    },
    #[error("{event} field {field} must contain only strings")]
    InvalidStringArray {
        event: &'static str,
        field: &'static str,
    },
    #[error("queue_update contains {actual} recoverable messages, above limit {limit}")]
    RecoveredQueueMessageLimit { actual: usize, limit: usize },
    #[error("queue_update recoverable text reached {attempted} bytes, above limit {limit}")]
    RecoveredQueueByteLimit { attempted: usize, limit: usize },
    #[error("{event} contains a malformed tool-result content item")]
    InvalidContentItem { event: &'static str },
    #[error("{event} field {field} contains an unsupported enum value")]
    InvalidEnumValue {
        event: &'static str,
        field: &'static str,
    },
    #[error("{event} is missing string field {field}")]
    MissingString {
        event: &'static str,
        field: &'static str,
    },
    #[error("{event} is missing unsigned integer field {field}")]
    MissingUnsignedInteger {
        event: &'static str,
        field: &'static str,
    },
    #[error("{event} field {field} value {value} does not fit this platform")]
    IntegerOutOfRange {
        event: &'static str,
        field: &'static str,
        value: u64,
    },
    #[error("{event} optional field {field} must be a string when present")]
    InvalidOptionalString {
        event: &'static str,
        field: &'static str,
    },
}

impl RpcEventKind {
    fn from_wire(value: &str) -> Self {
        match value {
            "agent_start" => Self::AgentStart,
            "agent_end" => Self::AgentEnd,
            "agent_settled" => Self::AgentSettled,
            "turn_start" => Self::TurnStart,
            "turn_end" => Self::TurnEnd,
            "message_start" => Self::MessageStart,
            "message_update" => Self::MessageUpdate,
            "message_end" => Self::MessageEnd,
            "bash_execution_update" => Self::BashExecutionUpdate,
            "tool_execution_start" => Self::ToolExecutionStart,
            "tool_execution_update" => Self::ToolExecutionUpdate,
            "tool_execution_end" => Self::ToolExecutionEnd,
            "queue_update" => Self::QueueUpdate,
            "entry_appended" => Self::EntryAppended,
            "session_info_changed" => Self::SessionInfoChanged,
            "thinking_level_changed" => Self::ThinkingLevelChanged,
            "compaction_start" => Self::CompactionStart,
            "compaction_end" => Self::CompactionEnd,
            "auto_retry_start" => Self::AutoRetryStart,
            "auto_retry_end" => Self::AutoRetryEnd,
            "summarization_retry_scheduled" => Self::SummarizationRetryScheduled,
            "summarization_retry_attempt_start" => Self::SummarizationRetryAttemptStart,
            "summarization_retry_finished" => Self::SummarizationRetryFinished,
            "extension_error" => Self::ExtensionError,
            "extension_ui_request" => Self::ExtensionUiRequest,
            unknown => Self::Unknown(unknown.to_owned()),
        }
    }

    /// High-frequency display updates that may be coalesced or superseded.
    ///
    /// These events must not directly trigger durable app-owned persistence.
    /// `message_end`/`tool_execution_end` and Pi's session entries are the
    /// authoritative semantic boundaries.
    #[must_use]
    pub const fn is_coalescible_stream_update(&self) -> bool {
        matches!(
            self,
            Self::MessageUpdate | Self::BashExecutionUpdate | Self::ToolExecutionUpdate
        )
    }

    /// Events that carry durable/session-semantic boundaries and must never be
    /// dropped merely to protect renderer throughput.
    #[must_use]
    pub const fn is_session_semantic(&self) -> bool {
        matches!(
            self,
            Self::AgentSettled
                | Self::MessageEnd
                | Self::ToolExecutionEnd
                | Self::QueueUpdate
                | Self::EntryAppended
                | Self::SessionInfoChanged
                | Self::ThinkingLevelChanged
                | Self::CompactionStart
                | Self::CompactionEnd
                | Self::AutoRetryStart
                | Self::AutoRetryEnd
                | Self::SummarizationRetryScheduled
                | Self::SummarizationRetryAttemptStart
                | Self::SummarizationRetryFinished
                | Self::ExtensionError
                | Self::ExtensionUiRequest
        )
    }
}

pub fn parse_frame(frame: &[u8]) -> Result<InboundMessage, RpcParseError> {
    let value: Value = serde_json::from_slice(frame)?;
    let object = value.as_object().ok_or(RpcParseError::ExpectedObject)?;
    let message_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or(RpcParseError::MissingType)?;

    if message_type == "response" {
        let response = serde_json::from_value(value)?;
        Ok(InboundMessage::Response(response))
    } else {
        Ok(InboundMessage::Event(RpcEvent {
            kind: RpcEventKind::from_wire(message_type),
            raw: value,
        }))
    }
}

#[derive(Debug, Error)]
pub enum RpcParseError {
    #[error("RPC frame is not a JSON object")]
    ExpectedObject,
    #[error("RPC JSON object is missing string field 'type'")]
    MissingType,
    #[error("invalid RPC JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_correlated_response_without_discarding_extra_fields() {
        let message = parse_frame(
            br#"{"id":"req-1","type":"response","command":"get_state","success":true,"data":{"isStreaming":false},"futureField":7}"#,
        )
        .expect("response should parse");

        let InboundMessage::Response(response) = message else {
            panic!("expected response");
        };
        assert_eq!(response.id.as_deref(), Some("req-1"));
        assert_eq!(response.command, "get_state");
        assert!(response.success);
        assert_eq!(response.extra.get("futureField"), Some(&Value::from(7)));
    }

    #[test]
    fn known_event_is_classified_but_raw_payload_is_preserved() {
        let message =
            parse_frame(br#"{"type":"queue_update","steering":["one"],"followUp":["two"]}"#)
                .expect("event should parse");

        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(event.kind, RpcEventKind::QueueUpdate);
        assert_eq!(event.raw["followUp"][0], "two");
    }

    #[test]
    fn unknown_event_is_forward_compatible() {
        let message = parse_frame(br#"{"type":"future_event","answer":42}"#)
            .expect("unknown event should remain valid");

        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(event.kind, RpcEventKind::Unknown("future_event".to_owned()));
        assert_eq!(event.raw["answer"], 42);
    }

    #[test]
    fn malformed_response_fails_closed() {
        let error = parse_frame(br#"{"type":"response","success":true}"#)
            .expect_err("response without command must fail");
        assert!(matches!(error, RpcParseError::Json(_)));
    }

    #[test]
    fn extension_cancelled_session_switch_is_not_treated_as_completed() {
        let message = parse_frame(
            br#"{"id":"req-2","type":"response","command":"switch_session","success":true,"data":{"cancelled":true}}"#,
        )
        .expect("response should parse");

        let InboundMessage::Response(response) = message else {
            panic!("expected response");
        };
        assert_eq!(response.outcome(), RpcResponseOutcome::Cancelled);
    }

    #[test]
    fn protocol_rejection_and_normal_acceptance_remain_distinct() {
        let rejected = RpcResponse {
            id: None,
            command: "prompt".to_owned(),
            success: false,
            data: None,
            error: Some("rejected".to_owned()),
            extra: BTreeMap::new(),
        };
        let accepted = RpcResponse {
            id: None,
            command: "switch_session".to_owned(),
            success: true,
            data: Some(serde_json::json!({"cancelled": false})),
            error: None,
            extra: BTreeMap::new(),
        };

        assert_eq!(rejected.outcome(), RpcResponseOutcome::Rejected);
        assert_eq!(accepted.outcome(), RpcResponseOutcome::Accepted);
    }

    #[test]
    fn only_high_frequency_stream_updates_are_coalescible() {
        assert!(RpcEventKind::MessageUpdate.is_coalescible_stream_update());
        assert!(RpcEventKind::ToolExecutionUpdate.is_coalescible_stream_update());
        assert!(RpcEventKind::BashExecutionUpdate.is_coalescible_stream_update());
        assert!(!RpcEventKind::MessageEnd.is_coalescible_stream_update());
        assert!(!RpcEventKind::AgentSettled.is_coalescible_stream_update());
        assert!(!RpcEventKind::QueueUpdate.is_coalescible_stream_update());
    }

    #[test]
    fn durable_entry_and_state_events_are_classified_as_non_droppable_semantics() {
        for kind in [
            RpcEventKind::EntryAppended,
            RpcEventKind::SessionInfoChanged,
            RpcEventKind::ThinkingLevelChanged,
            RpcEventKind::CompactionStart,
            RpcEventKind::CompactionEnd,
        ] {
            assert!(kind.is_session_semantic());
            assert!(!kind.is_coalescible_stream_update());
        }
    }

    #[test]
    fn message_update_parses_content_indexed_text_delta() {
        let message = parse_frame(
            br#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":3,"delta":"hello"}}"#,
        )
        .expect("message update");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.assistant_message_update().expect("typed update"),
            Some(AssistantMessageUpdate::Delta {
                content_index: 3,
                kind: AssistantMessageBlockKind::Text,
                delta: "hello".to_owned(),
            })
        );
    }

    #[test]
    fn message_end_exposes_authoritative_assistant_content_and_outcome() {
        let message = parse_frame(
            br#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"final answer"},{"type":"thinking","thinking":"final reasoning"},{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"README.md"}},{"type":"futureBlock","payload":true}]}}"#,
        )
        .expect("message end");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(
            event.assistant_message_end().expect("typed final message"),
            Some(AssistantMessageEnd {
                blocks: vec![
                    AssistantMessageFinalBlock {
                        content_index: 0,
                        kind: AssistantMessageBlockKind::Text,
                        content: "final answer".to_owned(),
                    },
                    AssistantMessageFinalBlock {
                        content_index: 1,
                        kind: AssistantMessageBlockKind::Thinking,
                        content: "final reasoning".to_owned(),
                    },
                    AssistantMessageFinalBlock {
                        content_index: 2,
                        kind: AssistantMessageBlockKind::ToolCall,
                        content: r#"{"path":"README.md"}"#.to_owned(),
                    },
                ],
                stop_reason: AssistantStopReason::Stop,
                error_message: None,
            })
        );
    }

    #[test]
    fn tool_result_message_end_is_not_an_assistant_completion() {
        let message = parse_frame(
            br#"{"type":"message_end","message":{"role":"toolResult","toolCallId":"call-1","toolName":"read","content":[{"type":"text","text":"done"}],"isError":false}}"#,
        )
        .expect("tool result message end");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(
            event.assistant_message_end().expect("typed message end"),
            None
        );
    }

    #[test]
    fn assistant_error_message_end_preserves_provider_error() {
        let message = parse_frame(
            br#"{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"rate limited","content":[]}}"#,
        )
        .expect("assistant error message end");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(
            event.assistant_message_end().expect("typed message end"),
            Some(AssistantMessageEnd {
                blocks: Vec::new(),
                stop_reason: AssistantStopReason::Error,
                error_message: Some("rate limited".to_owned()),
            })
        );
    }

    #[test]
    fn queue_update_recovery_copies_only_after_message_and_byte_bounds_pass() {
        let message = parse_frame(
            br#"{"type":"queue_update","steering":["one","two"],"followUp":["three"]}"#,
        )
        .expect("queue update");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        let recovered = event
            .queue_update_recovery(RuntimeLimits::default())
            .expect("bounded queue recovery")
            .expect("queue event");
        assert_eq!(recovered.steering, ["one", "two"]);
        assert_eq!(recovered.follow_up, ["three"]);

        let message_limits = RuntimeLimits {
            max_recovered_queue_messages_per_run: 2,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            event.queue_update_recovery(message_limits),
            Err(RpcEventPayloadError::RecoveredQueueMessageLimit {
                actual: 3,
                limit: 2,
            })
        );

        let byte_limits = RuntimeLimits {
            max_recovered_queue_bytes_per_run: 5,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            event.queue_update_recovery(byte_limits),
            Err(RpcEventPayloadError::RecoveredQueueByteLimit {
                attempted: 6,
                limit: 5,
            })
        );
    }

    #[test]
    fn toolcall_start_preserves_transient_call_identity_and_tool_name() {
        let message = parse_frame(
            br#"{"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":1,"id":"call-7","toolName":"write"}}"#,
        )
        .expect("tool call start");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.assistant_message_update().expect("typed update"),
            Some(AssistantMessageUpdate::Start {
                content_index: 1,
                kind: AssistantMessageBlockKind::ToolCall,
                tool_call: Some(ToolCallStartMeta {
                    id: "call-7".to_owned(),
                    tool_name: "write".to_owned(),
                }),
            })
        );
    }

    #[test]
    fn unknown_nested_assistant_update_remains_forward_compatible() {
        let message = parse_frame(
            br#"{"type":"message_update","assistantMessageEvent":{"type":"future_delta","contentIndex":9,"payload":true}}"#,
        )
        .expect("future update");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.assistant_message_update().expect("typed update"),
            Some(AssistantMessageUpdate::Unknown {
                event_type: "future_delta".to_owned(),
                content_index: Some(9),
            })
        );
    }

    #[test]
    fn malformed_known_assistant_delta_fails_closed() {
        let message = parse_frame(
            br#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0}}"#,
        )
        .expect("outer event remains valid");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.assistant_message_update(),
            Err(RpcEventPayloadError::MissingString {
                event: "message_update",
                field: "assistantMessageEvent.delta",
            })
        );
    }

    #[test]
    fn direct_bash_update_is_correlated_by_request_id() {
        let message =
            parse_frame(br#"{"type":"bash_execution_update","id":"req-7","delta":"line one\n"}"#)
                .expect("bash update");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.bash_execution_update().expect("typed bash update"),
            Some(BashExecutionUpdate {
                request_id: Some(RequestId::from_wire("req-7")),
                delta: "line one\n".to_owned(),
            })
        );
    }

    #[test]
    fn tool_progress_extracts_accumulated_text_and_ignores_non_text_content() {
        let message = parse_frame(
            br#"{"type":"tool_execution_update","toolCallId":"call-1","toolName":"bash","partialResult":{"content":[{"type":"text","text":"one"},{"type":"image","data":"x"},{"type":"text","text":" two"}],"details":{}}}"#,
        )
        .expect("tool update");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.tool_execution_update().expect("typed tool update"),
            Some(ToolExecutionUpdate {
                tool_call_id: "call-1".to_owned(),
                tool_name: "bash".to_owned(),
                accumulated_text: "one two".to_owned(),
            })
        );
    }

    #[test]
    fn tool_end_preserves_error_flag_and_final_text() {
        let message = parse_frame(
            br#"{"type":"tool_execution_end","toolCallId":"call-2","toolName":"read","result":{"content":[{"type":"text","text":"failed"}],"details":{}},"isError":true}"#,
        )
        .expect("tool end");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.tool_execution_end().expect("typed tool end"),
            Some(ToolExecutionEnd {
                tool_call_id: "call-2".to_owned(),
                tool_name: "read".to_owned(),
                final_text: "failed".to_owned(),
                is_error: true,
            })
        );
    }

    #[test]
    fn queue_update_counts_without_copying_queue_text() {
        let message = parse_frame(
            br#"{"type":"queue_update","steering":["one","two"],"followUp":["three"]}"#,
        )
        .expect("queue update");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.queue_update_counts().expect("queue counts"),
            Some(QueueUpdateCounts {
                steering: 2,
                follow_up: 1,
            })
        );
    }

    #[test]
    fn session_name_clear_and_thinking_change_are_typed() {
        let cleared = parse_frame(br#"{"type":"session_info_changed"}"#).expect("name clear");
        let InboundMessage::Event(cleared) = cleared else {
            panic!("expected event");
        };
        assert_eq!(
            cleared.session_info_changed().expect("session info"),
            Some(SessionInfoChanged { name: None })
        );

        let thinking = parse_frame(br#"{"type":"thinking_level_changed","level":"xhigh"}"#)
            .expect("thinking event");
        let InboundMessage::Event(thinking) = thinking else {
            panic!("expected event");
        };
        assert_eq!(
            thinking.thinking_level_changed().expect("thinking level"),
            Some(ThinkingLevel::Xhigh)
        );
    }

    #[test]
    fn retry_and_summarization_events_preserve_current_pi_fields() {
        let retry = parse_frame(
            br#"{"type":"auto_retry_start","attempt":2,"maxAttempts":5,"delayMs":1500,"errorMessage":"rate limited"}"#,
        )
        .expect("retry event");
        let InboundMessage::Event(retry) = retry else {
            panic!("expected event");
        };
        assert_eq!(
            retry.auto_retry_start().expect("typed retry"),
            Some(AutoRetryStart {
                attempt: 2,
                max_attempts: 5,
                delay_ms: 1_500,
                error_message: "rate limited".to_owned(),
            })
        );

        let summary = parse_frame(
            br#"{"type":"summarization_retry_attempt_start","source":"branchSummary","reason":"provider unavailable"}"#,
        )
        .expect("summary retry event");
        let InboundMessage::Event(summary) = summary else {
            panic!("expected event");
        };
        assert_eq!(
            summary
                .summarization_retry_attempt_start()
                .expect("typed summary retry"),
            Some(SummarizationRetryAttemptStart {
                source: "branchSummary".to_owned(),
                reason: Some("provider unavailable".to_owned()),
            })
        );
    }

    #[test]
    fn extension_error_event_preserves_exact_source_and_error() {
        let message = parse_frame(
            br#"{"type":"extension_error","extensionPath":"C:/pi/extensions/broken.ts","event":"session_start","error":"boom"}"#,
        )
        .expect("extension error");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(
            event.extension_error().expect("typed extension error"),
            Some(ExtensionErrorEvent {
                extension_path: "C:/pi/extensions/broken.ts".to_owned(),
                event: "session_start".to_owned(),
                error: "boom".to_owned(),
            })
        );
    }

    #[test]
    fn malformed_retry_event_fails_closed_at_typed_projection() {
        let message = parse_frame(
            br#"{"type":"auto_retry_start","attempt":1,"maxAttempts":5,"delayMs":"later","errorMessage":"x"}"#,
        )
        .expect("outer event");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };
        assert_eq!(
            event.auto_retry_start(),
            Err(RpcEventPayloadError::MissingUnsignedInteger {
                event: "auto_retry_start",
                field: "delayMs",
            })
        );
    }

    #[test]
    fn compaction_events_preserve_reason_abort_retry_and_error_semantics() {
        let start = parse_frame(br#"{"type":"compaction_start","reason":"overflow"}"#)
            .expect("compaction start");
        let InboundMessage::Event(start) = start else {
            panic!("expected event");
        };
        assert_eq!(
            start.compaction_start().expect("typed start"),
            Some(CompactionStartEvent {
                reason: "overflow".to_owned(),
            })
        );

        let end = parse_frame(
            br#"{"type":"compaction_end","reason":"overflow","result":null,"aborted":false,"willRetry":true,"errorMessage":"provider quota"}"#,
        )
        .expect("compaction end");
        let InboundMessage::Event(end) = end else {
            panic!("expected event");
        };
        assert_eq!(
            end.compaction_end().expect("typed end"),
            Some(CompactionEndEvent {
                reason: "overflow".to_owned(),
                aborted: false,
                will_retry: true,
                error_message: Some("provider quota".to_owned()),
            })
        );
    }

    #[test]
    fn malformed_tool_content_fails_closed() {
        let message = parse_frame(
            br#"{"type":"tool_execution_update","toolCallId":"call-1","toolName":"bash","partialResult":{"content":[42]}}"#,
        )
        .expect("outer event remains valid");
        let InboundMessage::Event(event) = message else {
            panic!("expected event");
        };

        assert_eq!(
            event.tool_execution_update(),
            Err(RpcEventPayloadError::InvalidContentItem {
                event: "tool_execution_update"
            })
        );
    }
}
