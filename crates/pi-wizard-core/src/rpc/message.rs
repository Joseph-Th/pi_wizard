use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use super::command::ThinkingLevel;
use crate::RequestId;

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

    /// Returns queue sizes without copying queued user text into hot runtime
    /// state. `clear_queue` is the explicit operation that transfers the text
    /// when Stop needs to preserve it.
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
