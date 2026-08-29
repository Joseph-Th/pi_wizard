use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{RequestId, RuntimeLimits};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    Steer,
    FollowUp,
}

impl RpcCommand {
    #[must_use]
    pub const fn wire_type(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Steer { .. } => "steer",
            Self::FollowUp { .. } => "follow_up",
            Self::Abort => "abort",
            Self::ClearQueue => "clear_queue",
            Self::GetState => "get_state",
            Self::GetMessages => "get_messages",
            Self::GetCommands => "get_commands",
            Self::GetAvailableModels => "get_available_models",
            Self::CycleModel => "cycle_model",
            Self::GetAvailableThinkingLevels => "get_available_thinking_levels",
            Self::CycleThinkingLevel => "cycle_thinking_level",
            Self::SetModel { .. } => "set_model",
            Self::SetThinkingLevel { .. } => "set_thinking_level",
            Self::SetSteeringMode { .. } => "set_steering_mode",
            Self::SetFollowUpMode { .. } => "set_follow_up_mode",
            Self::Compact { .. } => "compact",
            Self::SetAutoCompaction { .. } => "set_auto_compaction",
            Self::SetAutoRetry { .. } => "set_auto_retry",
            Self::AbortRetry => "abort_retry",
            Self::Bash { .. } => "bash",
            Self::AbortBash => "abort_bash",
            Self::NewSession { .. } => "new_session",
            Self::SwitchSession { .. } => "switch_session",
            Self::Fork { .. } => "fork",
            Self::Clone => "clone",
            Self::GetForkMessages => "get_fork_messages",
            Self::GetEntries { .. } => "get_entries",
            Self::GetTree => "get_tree",
            Self::GetLastAssistantText => "get_last_assistant_text",
            Self::GetSessionStats => "get_session_stats",
            Self::ExportHtml { .. } => "export_html",
            Self::SetSessionName { .. } => "set_session_name",
        }
    }

    /// Commands Pi Wizard must not send while a manual compaction request is
    /// in flight, even before Pi emits `compaction_start`.
    ///
    /// Pi dispatches RPC input frames asynchronously. Without this client-side
    /// barrier, a composer submission can race the compaction request before
    /// RuntimeStore learns about compaction from the event stream. Commands
    /// that mutate session-visible state are kept behind the same boundary.
    #[must_use]
    pub const fn blocked_by_manual_compaction(&self) -> bool {
        matches!(
            self,
            Self::Prompt { .. }
                | Self::Steer { .. }
                | Self::FollowUp { .. }
                | Self::Bash { .. }
                | Self::SetModel { .. }
                | Self::SetThinkingLevel { .. }
                | Self::SetAutoCompaction { .. }
                | Self::SetAutoRetry { .. }
                | Self::SetSessionName { .. }
        )
    }

    /// Commands Pi Wizard must not start while one of its direct Bash
    /// requests is active for the same run. Direct Bash operates against the
    /// exact execution root outside model context, so overlapping it with a
    /// model/session mutation would create two independent writers to the same
    /// owned runtime. Read-only probes/export and `abort_bash` remain allowed.
    #[must_use]
    pub const fn blocked_by_direct_bash(&self) -> bool {
        matches!(
            self,
            Self::Prompt { .. }
                | Self::Steer { .. }
                | Self::FollowUp { .. }
                | Self::Bash { .. }
                | Self::CycleModel
                | Self::CycleThinkingLevel
                | Self::SetModel { .. }
                | Self::SetThinkingLevel { .. }
                | Self::SetSteeringMode { .. }
                | Self::SetFollowUpMode { .. }
                | Self::Compact { .. }
                | Self::SetAutoCompaction { .. }
                | Self::SetAutoRetry { .. }
                | Self::NewSession { .. }
                | Self::SwitchSession { .. }
                | Self::Fork { .. }
                | Self::Clone
                | Self::SetSessionName { .. }
        )
    }

    /// RPC concurrency category used by Pi Wizard's client-side command gate.
    ///
    /// Current Pi dispatches RPC frames asynchronously, while session replacement
    /// mutates one shared runtime. Treat replacements as a barrier even when Pi
    /// itself accepts overlapping frames.
    #[must_use]
    pub const fn concurrency_class(&self) -> RpcConcurrencyClass {
        match self {
            Self::NewSession { .. }
            | Self::SwitchSession { .. }
            | Self::Fork { .. }
            | Self::Clone => RpcConcurrencyClass::SessionReplacement,
            Self::Compact { .. } => RpcConcurrencyClass::ManualCompaction,
            _ => RpcConcurrencyClass::Ordinary,
        }
    }

    fn images(&self) -> &[ImageContent] {
        match self {
            Self::Prompt { images, .. }
            | Self::Steer { images, .. }
            | Self::FollowUp { images, .. } => images,
            _ => &[],
        }
    }

    fn validate(&self, limits: RuntimeLimits) -> Result<(), AttachmentError> {
        let images = self.images();
        if images.len() > limits.max_attachments_per_prompt {
            return Err(AttachmentError::TooMany {
                actual: images.len(),
                limit: limits.max_attachments_per_prompt,
            });
        }

        let mut total = 0usize;
        for image in images {
            if image.decoded_bytes > limits.max_attachment_bytes_per_image {
                return Err(AttachmentError::ImageTooLarge {
                    actual: image.decoded_bytes,
                    limit: limits.max_attachment_bytes_per_image,
                });
            }
            total = total.saturating_add(image.decoded_bytes);
            if total > limits.max_attachment_bytes_per_prompt {
                return Err(AttachmentError::PromptTooLarge {
                    actual: total,
                    limit: limits.max_attachment_bytes_per_prompt,
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcConcurrencyClass {
    Ordinary,
    ManualCompaction,
    SessionReplacement,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueMode {
    All,
    OneAtATime,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum ImageContentType {
    #[serde(rename = "image")]
    Image,
}

/// Validated Pi `ImageContent` payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    #[serde(rename = "type")]
    kind: ImageContentType,
    data: String,
    mime_type: String,
    #[serde(skip)]
    decoded_bytes: usize,
}

impl ImageContent {
    pub fn try_new(
        data: String,
        mime_type: String,
        limits: RuntimeLimits,
    ) -> Result<Self, AttachmentError> {
        if !mime_type.starts_with("image/") || mime_type.len() > 128 {
            return Err(AttachmentError::InvalidMimeType { mime_type });
        }
        let decoded_bytes = base64_decoded_len(&data)?;
        if decoded_bytes == 0 {
            return Err(AttachmentError::EmptyImage);
        }
        if decoded_bytes > limits.max_attachment_bytes_per_image {
            return Err(AttachmentError::ImageTooLarge {
                actual: decoded_bytes,
                limit: limits.max_attachment_bytes_per_image,
            });
        }

        Ok(Self {
            kind: ImageContentType::Image,
            data,
            mime_type,
            decoded_bytes,
        })
    }

    #[must_use]
    pub const fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }
}

fn base64_decoded_len(data: &str) -> Result<usize, AttachmentError> {
    let bytes = data.as_bytes();
    if bytes.is_empty() {
        return Ok(0);
    }

    let first_padding = bytes.iter().position(|byte| *byte == b'=');
    let content_len = first_padding.unwrap_or(bytes.len());
    if bytes[content_len..].iter().any(|byte| *byte != b'=')
        || bytes.len().saturating_sub(content_len) > 2
    {
        return Err(AttachmentError::InvalidBase64);
    }
    if bytes[..content_len]
        .iter()
        .any(|byte| !matches!(*byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/'))
    {
        return Err(AttachmentError::InvalidBase64);
    }

    let padding = bytes.len() - content_len;
    if padding > 0 && !bytes.len().is_multiple_of(4) {
        return Err(AttachmentError::InvalidBase64);
    }
    let remainder = content_len % 4;
    if remainder == 1 {
        return Err(AttachmentError::InvalidBase64);
    }

    if padding > 0 {
        return Ok((bytes.len() / 4).saturating_mul(3).saturating_sub(padding));
    }
    Ok((content_len / 4)
        .saturating_mul(3)
        .saturating_add(match remainder {
            0 => 0,
            2 => 1,
            3 => 2,
            _ => return Err(AttachmentError::InvalidBase64),
        }))
}

/// Pi RPC command body. Request correlation is added by [`RpcRequest`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcCommand {
    Prompt {
        message: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageContent>,
        #[serde(rename = "streamingBehavior", skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    Steer {
        message: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageContent>,
    },
    FollowUp {
        message: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageContent>,
    },
    Abort,
    ClearQueue,
    GetState,
    GetMessages,
    GetCommands,
    GetAvailableModels,
    CycleModel,
    GetAvailableThinkingLevels,
    CycleThinkingLevel,
    SetModel {
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },
    SetThinkingLevel {
        level: ThinkingLevel,
    },
    SetSteeringMode {
        mode: QueueMode,
    },
    SetFollowUpMode {
        mode: QueueMode,
    },
    Compact {
        #[serde(rename = "customInstructions", skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    SetAutoCompaction {
        enabled: bool,
    },
    SetAutoRetry {
        enabled: bool,
    },
    AbortRetry,
    Bash {
        command: String,
        #[serde(rename = "excludeFromContext", skip_serializing_if = "Option::is_none")]
        exclude_from_context: Option<bool>,
    },
    AbortBash,
    NewSession {
        #[serde(rename = "parentSession", skip_serializing_if = "Option::is_none")]
        parent_session: Option<PathBuf>,
    },
    SwitchSession {
        #[serde(rename = "sessionPath")]
        session_path: PathBuf,
    },
    Fork {
        #[serde(rename = "entryId")]
        entry_id: String,
    },
    Clone,
    GetForkMessages,
    GetEntries {
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    GetTree,
    GetLastAssistantText,
    GetSessionStats,
    ExportHtml {
        #[serde(rename = "outputPath", skip_serializing_if = "Option::is_none")]
        output_path: Option<PathBuf>,
    },
    SetSessionName {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcRequest {
    pub id: RequestId,
    pub command: RpcCommand,
}

impl RpcRequest {
    #[must_use]
    pub fn new(command: RpcCommand) -> Self {
        Self {
            id: RequestId::new(),
            command,
        }
    }

    #[must_use]
    pub fn with_id(id: RequestId, command: RpcCommand) -> Self {
        Self { id, command }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionUiResponse {
    Value { id: String, value: String },
    Confirmation { id: String, confirmed: bool },
    Cancelled { id: String },
}

pub fn encode_request(
    request: &RpcRequest,
    limits: RuntimeLimits,
) -> Result<Vec<u8>, OutboundEncodeError> {
    validate_session_command_cursor(&request.command, limits)?;
    request.command.validate(limits)?;
    let mut value = serde_json::to_value(&request.command)?;
    let Value::Object(ref mut object) = value else {
        return Err(OutboundEncodeError::InternalShape);
    };
    object.insert(
        "id".to_owned(),
        Value::String(request.id.as_str().to_owned()),
    );
    encode_value(value, limits.max_outbound_rpc_bytes)
}

fn validate_session_command_cursor(
    command: &RpcCommand,
    limits: RuntimeLimits,
) -> Result<(), OutboundEncodeError> {
    let cursor = match command {
        RpcCommand::Fork { entry_id } => Some(entry_id.as_str()),
        RpcCommand::GetEntries { since: Some(since) } => Some(since.as_str()),
        _ => None,
    };
    if let Some(cursor) = cursor
        && (cursor.is_empty() || cursor.len() > limits.max_session_cursor_bytes)
    {
        return Err(OutboundEncodeError::InvalidSessionCursor {
            actual: cursor.len(),
            limit: limits.max_session_cursor_bytes,
        });
    }
    Ok(())
}

pub fn encode_extension_ui_response(
    response: &ExtensionUiResponse,
    max_bytes: usize,
) -> Result<Vec<u8>, OutboundEncodeError> {
    let mut object = Map::new();
    object.insert(
        "type".to_owned(),
        Value::String("extension_ui_response".to_owned()),
    );
    match response {
        ExtensionUiResponse::Value { id, value } => {
            object.insert("id".to_owned(), Value::String(id.clone()));
            object.insert("value".to_owned(), Value::String(value.clone()));
        }
        ExtensionUiResponse::Confirmation { id, confirmed } => {
            object.insert("id".to_owned(), Value::String(id.clone()));
            object.insert("confirmed".to_owned(), Value::Bool(*confirmed));
        }
        ExtensionUiResponse::Cancelled { id } => {
            object.insert("id".to_owned(), Value::String(id.clone()));
            object.insert("cancelled".to_owned(), Value::Bool(true));
        }
    }
    encode_value(Value::Object(object), max_bytes)
}

fn encode_value(value: Value, max_bytes: usize) -> Result<Vec<u8>, OutboundEncodeError> {
    if max_bytes == 0 {
        return Err(OutboundEncodeError::ZeroLimit);
    }
    let mut encoded = serde_json::to_vec(&value)?;
    if encoded.len() > max_bytes {
        return Err(OutboundEncodeError::TooLarge {
            actual: encoded.len(),
            limit: max_bytes,
        });
    }
    encoded.push(b'\n');
    Ok(encoded)
}

#[derive(Debug, Error)]
pub enum OutboundEncodeError {
    #[error("outbound RPC byte limit must be non-zero")]
    ZeroLimit,
    #[error("outbound RPC payload is {actual} bytes, exceeding limit {limit}")]
    TooLarge { actual: usize, limit: usize },
    #[error("RPC command serialized to an unexpected non-object shape")]
    InternalShape,
    #[error("failed to serialize RPC command: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("outbound session entry id/cursor is {actual} bytes; expected 1..={limit}")]
    InvalidSessionCursor { actual: usize, limit: usize },
    #[error(transparent)]
    Attachment(#[from] AttachmentError),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AttachmentError {
    #[error("attachment MIME type is not a bounded image type: {mime_type}")]
    InvalidMimeType { mime_type: String },
    #[error("image payload is not valid standard base64")]
    InvalidBase64,
    #[error("image payload is empty")]
    EmptyImage,
    #[error("decoded image is {actual} bytes, exceeding per-image limit {limit}")]
    ImageTooLarge { actual: usize, limit: usize },
    #[error("prompt has {actual} images, exceeding limit {limit}")]
    TooMany { actual: usize, limit: usize },
    #[error("decoded prompt images total {actual} bytes, exceeding limit {limit}")]
    PromptTooLarge { actual: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(encoded: &[u8]) -> Value {
        serde_json::from_slice(encoded).expect("encoded request should be JSON")
    }

    #[test]
    fn prompt_uses_pi_streaming_behavior_spelling_and_request_id() {
        let request = RpcRequest::with_id(
            RequestId::from_wire("req-1"),
            RpcCommand::Prompt {
                message: "continue".to_owned(),
                images: Vec::new(),
                streaming_behavior: Some(StreamingBehavior::FollowUp),
            },
        );

        let value = decode(
            &encode_request(&request, RuntimeLimits::default()).expect("request should encode"),
        );
        assert_eq!(value["type"], "prompt");
        assert_eq!(value["id"], "req-1");
        assert_eq!(value["message"], "continue");
        assert_eq!(value["streamingBehavior"], "followUp");
    }

    #[test]
    fn thinking_level_includes_current_max_level() {
        let request = RpcRequest::with_id(
            RequestId::from_wire("req-2"),
            RpcCommand::SetThinkingLevel {
                level: ThinkingLevel::Max,
            },
        );
        let value = decode(
            &encode_request(&request, RuntimeLimits::default()).expect("request should encode"),
        );

        assert_eq!(value["type"], "set_thinking_level");
        assert_eq!(value["level"], "max");
    }

    #[test]
    fn extension_ui_confirmation_matches_subprotocol() {
        let value = decode(
            &encode_extension_ui_response(
                &ExtensionUiResponse::Confirmation {
                    id: "uuid-2".to_owned(),
                    confirmed: true,
                },
                1024,
            )
            .expect("response should encode"),
        );

        assert_eq!(value["type"], "extension_ui_response");
        assert_eq!(value["id"], "uuid-2");
        assert_eq!(value["confirmed"], true);
    }

    #[test]
    fn outbound_payload_is_bounded_before_lf_is_appended() {
        let request = RpcRequest::with_id(RequestId::from_wire("x"), RpcCommand::GetState);
        let mut limits = RuntimeLimits::default();
        let encoded = encode_request(&request, limits).expect("fixture should encode");
        let payload_len = encoded.len() - 1;

        limits.max_outbound_rpc_bytes = payload_len - 1;
        let error = encode_request(&request, limits).expect_err("limit must reject");
        assert!(matches!(error, OutboundEncodeError::TooLarge { .. }));
    }

    #[test]
    fn first_class_steer_serializes_bounded_images() {
        let limits = RuntimeLimits::default();
        let image = ImageContent::try_new("aGVsbG8=".to_owned(), "image/png".to_owned(), limits)
            .expect("bounded image");
        let request = RpcRequest::with_id(
            RequestId::from_wire("steer-1"),
            RpcCommand::Steer {
                message: "inspect this".to_owned(),
                images: vec![image],
            },
        );

        let value = decode(&encode_request(&request, limits).expect("steer should encode"));
        assert_eq!(value["type"], "steer");
        assert_eq!(value["images"][0]["type"], "image");
        assert_eq!(value["images"][0]["mimeType"], "image/png");
    }

    #[test]
    fn get_entries_uses_stable_since_cursor() {
        let request = RpcRequest::with_id(
            RequestId::from_wire("entries-1"),
            RpcCommand::GetEntries {
                since: Some("abc123".to_owned()),
            },
        );
        let value = decode(
            &encode_request(&request, RuntimeLimits::default()).expect("request should encode"),
        );

        assert_eq!(value["type"], "get_entries");
        assert_eq!(value["since"], "abc123");
    }

    #[test]
    fn fork_and_incremental_history_reject_unbounded_entry_ids_before_serialization() {
        let limits = RuntimeLimits {
            max_session_cursor_bytes: 4,
            ..RuntimeLimits::default()
        };
        for command in [
            RpcCommand::Fork {
                entry_id: "12345".to_owned(),
            },
            RpcCommand::GetEntries {
                since: Some("12345".to_owned()),
            },
        ] {
            let request = RpcRequest::with_id(RequestId::from_wire("bounded-cursor"), command);
            assert!(matches!(
                encode_request(&request, limits),
                Err(OutboundEncodeError::InvalidSessionCursor {
                    actual: 5,
                    limit: 4
                })
            ));
        }
    }

    #[test]
    fn attachment_aggregate_is_revalidated_at_rpc_boundary() {
        let limits = RuntimeLimits {
            max_attachment_bytes_per_image: 4,
            max_attachment_bytes_per_prompt: 5,
            ..RuntimeLimits::default()
        };
        let image = ImageContent::try_new("YWJjZA==".to_owned(), "image/png".to_owned(), limits)
            .expect("four byte image");
        let request = RpcRequest::with_id(
            RequestId::from_wire("images-1"),
            RpcCommand::FollowUp {
                message: "two images".to_owned(),
                images: vec![image.clone(), image],
            },
        );

        assert!(matches!(
            encode_request(&request, limits),
            Err(OutboundEncodeError::Attachment(
                AttachmentError::PromptTooLarge {
                    actual: 8,
                    limit: 5
                }
            ))
        ));
    }

    #[test]
    fn malformed_base64_is_rejected_before_rpc_serialization() {
        assert_eq!(
            ImageContent::try_new(
                "not base64!".to_owned(),
                "image/png".to_owned(),
                RuntimeLimits::default(),
            ),
            Err(AttachmentError::InvalidBase64)
        );
    }

    #[test]
    fn direct_shell_commands_match_current_pi_rpc_wire_names() {
        let limits = RuntimeLimits::default();
        let bash = RpcRequest::with_id(
            RequestId::from_wire("bash-1"),
            RpcCommand::Bash {
                command: "git status --short".to_owned(),
                exclude_from_context: Some(true),
            },
        );
        let abort = RpcRequest::with_id(RequestId::from_wire("bash-2"), RpcCommand::AbortBash);

        let bash_value = decode(&encode_request(&bash, limits).expect("bash request"));
        let abort_value = decode(&encode_request(&abort, limits).expect("abort bash request"));

        assert_eq!(bash_value["type"], "bash");
        assert_eq!(bash_value["command"], "git status --short");
        assert_eq!(bash_value["excludeFromContext"], true);
        assert_eq!(abort_value["type"], "abort_bash");
    }

    #[test]
    fn auto_compaction_uses_pi_native_wire_shape_and_respects_manual_compaction_barrier() {
        let request = RpcRequest::with_id(
            RequestId::from_wire("auto-compact-1"),
            RpcCommand::SetAutoCompaction { enabled: false },
        );
        let value = decode(
            &encode_request(&request, RuntimeLimits::default())
                .expect("automatic compaction request"),
        );
        assert_eq!(value["type"], "set_auto_compaction");
        assert_eq!(value["enabled"], false);
        assert!(request.command.blocked_by_manual_compaction());
        assert_eq!(
            request.command.concurrency_class(),
            RpcConcurrencyClass::Ordinary
        );
    }

    #[test]
    fn auto_retry_uses_pi_native_wire_shape_and_respects_manual_compaction_barrier() {
        let request = RpcRequest::with_id(
            RequestId::from_wire("auto-retry-1"),
            RpcCommand::SetAutoRetry { enabled: false },
        );
        let value = decode(
            &encode_request(&request, RuntimeLimits::default()).expect("automatic retry request"),
        );
        assert_eq!(value["type"], "set_auto_retry");
        assert_eq!(value["enabled"], false);
        assert!(request.command.blocked_by_manual_compaction());
        assert_eq!(
            request.command.concurrency_class(),
            RpcConcurrencyClass::Ordinary
        );
    }

    #[test]
    fn session_replacements_and_manual_compaction_are_classified_for_client_side_serialization() {
        assert_eq!(
            RpcCommand::SwitchSession {
                session_path: PathBuf::from("session.jsonl")
            }
            .concurrency_class(),
            RpcConcurrencyClass::SessionReplacement
        );
        assert_eq!(
            RpcCommand::Compact {
                custom_instructions: None
            }
            .concurrency_class(),
            RpcConcurrencyClass::ManualCompaction
        );
        assert_eq!(
            RpcCommand::GetState.concurrency_class(),
            RpcConcurrencyClass::Ordinary
        );
    }

    #[test]
    fn direct_bash_blocks_mutating_session_commands_but_not_read_or_cancel_operations() {
        let mutating = [
            RpcCommand::Prompt {
                message: "continue".to_owned(),
                images: Vec::new(),
                streaming_behavior: None,
            },
            RpcCommand::Compact {
                custom_instructions: None,
            },
            RpcCommand::SetModel {
                provider: "fake".to_owned(),
                model_id: "model".to_owned(),
            },
            RpcCommand::SwitchSession {
                session_path: PathBuf::from("other.jsonl"),
            },
        ];
        assert!(mutating.iter().all(RpcCommand::blocked_by_direct_bash));

        let safe = [
            RpcCommand::GetState,
            RpcCommand::GetSessionStats,
            RpcCommand::ExportHtml { output_path: None },
            RpcCommand::AbortBash,
        ];
        assert!(safe.iter().all(|command| !command.blocked_by_direct_bash()));
    }
}
