use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::RuntimeLimits;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionUiMethod {
    Select,
    Confirm,
    Input,
    Editor,
    Notify,
    SetStatus,
    SetWidget,
    SetTitle,
    SetEditorText,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionNotifyType {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionWidgetPlacement {
    AboveEditor,
    BelowEditor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ExtensionDialogKind {
    Select {
        title: String,
        options: Vec<String>,
    },
    Confirm {
        title: String,
        message: String,
    },
    Input {
        title: String,
        placeholder: Option<String>,
    },
    Editor {
        title: String,
        prefill: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionDialogRequest {
    pub id: String,
    pub timeout_ms: Option<u64>,
    pub kind: ExtensionDialogKind,
}

impl ExtensionDialogRequest {
    #[must_use]
    pub fn resident_bytes(&self) -> usize {
        let payload = match &self.kind {
            ExtensionDialogKind::Select { title, options } => {
                options.iter().fold(title.len(), |total, option| {
                    total.saturating_add(option.len())
                })
            }
            ExtensionDialogKind::Confirm { title, message } => {
                title.len().saturating_add(message.len())
            }
            ExtensionDialogKind::Input { title, placeholder } => title
                .len()
                .saturating_add(placeholder.as_ref().map_or(0, String::len)),
            ExtensionDialogKind::Editor { title, prefill } => title
                .len()
                .saturating_add(prefill.as_ref().map_or(0, String::len)),
        };
        self.id.len().saturating_add(payload)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionFireAndForget {
    Notify {
        message: String,
        notify_type: ExtensionNotifyType,
    },
    SetStatus {
        key: String,
        text: Option<String>,
    },
    SetWidget {
        key: String,
        lines: Option<Vec<String>>,
        placement: ExtensionWidgetPlacement,
    },
    SetTitle {
        title: String,
    },
    SetEditorText {
        text: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionUiRequest {
    Dialog(ExtensionDialogRequest),
    FireAndForget(ExtensionFireAndForget),
    Unknown(ExtensionUiRequestMeta),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionUiDisposition {
    Dialog,
    FireAndForget,
    Unknown,
}

impl ExtensionUiRequest {
    pub fn parse_bounded(
        raw: &Value,
        limits: RuntimeLimits,
    ) -> Result<Self, ExtensionUiParseError> {
        let meta = ExtensionUiRequestMeta::parse(raw)?;
        let object = raw
            .as_object()
            .ok_or(ExtensionUiParseError::ExpectedObject)?;
        let mut budget = ExtensionPayloadBudget::new(limits.max_extension_ui_bytes_per_run);
        budget.add(meta.id.len())?;

        match meta.method {
            ExtensionUiMethod::Select => {
                let title = required_string(object, "title")?;
                let options = required_string_array(object, "options")?;
                if options.len() > limits.max_extension_ui_entries_per_run {
                    return Err(ExtensionUiParseError::TooManyOptions {
                        actual: options.len(),
                        limit: limits.max_extension_ui_entries_per_run,
                    });
                }
                budget.add(title.len())?;
                for option in &options {
                    budget.add(option.len())?;
                }
                Ok(Self::Dialog(ExtensionDialogRequest {
                    id: meta.id,
                    timeout_ms: meta.timeout_ms,
                    kind: ExtensionDialogKind::Select { title, options },
                }))
            }
            ExtensionUiMethod::Confirm => {
                let title = required_string(object, "title")?;
                let message = required_string(object, "message")?;
                budget.add(title.len())?;
                budget.add(message.len())?;
                Ok(Self::Dialog(ExtensionDialogRequest {
                    id: meta.id,
                    timeout_ms: meta.timeout_ms,
                    kind: ExtensionDialogKind::Confirm { title, message },
                }))
            }
            ExtensionUiMethod::Input => {
                let title = required_string(object, "title")?;
                let placeholder = optional_string(object, "placeholder")?;
                budget.add(title.len())?;
                budget.add(placeholder.as_ref().map_or(0, String::len))?;
                Ok(Self::Dialog(ExtensionDialogRequest {
                    id: meta.id,
                    timeout_ms: meta.timeout_ms,
                    kind: ExtensionDialogKind::Input { title, placeholder },
                }))
            }
            ExtensionUiMethod::Editor => {
                let title = required_string(object, "title")?;
                let prefill = optional_string(object, "prefill")?;
                budget.add(title.len())?;
                budget.add(prefill.as_ref().map_or(0, String::len))?;
                Ok(Self::Dialog(ExtensionDialogRequest {
                    id: meta.id,
                    timeout_ms: meta.timeout_ms,
                    kind: ExtensionDialogKind::Editor { title, prefill },
                }))
            }
            ExtensionUiMethod::Notify => {
                let message = required_string(object, "message")?;
                let notify_type = match optional_string(object, "notifyType")?.as_deref() {
                    None | Some("info") => ExtensionNotifyType::Info,
                    Some("warning") => ExtensionNotifyType::Warning,
                    Some("error") => ExtensionNotifyType::Error,
                    Some(_) => return Err(ExtensionUiParseError::InvalidNotifyType),
                };
                budget.add(message.len())?;
                Ok(Self::FireAndForget(ExtensionFireAndForget::Notify {
                    message,
                    notify_type,
                }))
            }
            ExtensionUiMethod::SetStatus => {
                let key = required_string(object, "statusKey")?;
                let text = optional_string(object, "statusText")?;
                budget.add(key.len())?;
                budget.add(text.as_ref().map_or(0, String::len))?;
                Ok(Self::FireAndForget(ExtensionFireAndForget::SetStatus {
                    key,
                    text,
                }))
            }
            ExtensionUiMethod::SetWidget => {
                let key = required_string(object, "widgetKey")?;
                let lines = optional_string_array(object, "widgetLines")?;
                if lines
                    .as_ref()
                    .is_some_and(|lines| lines.len() > limits.max_extension_ui_entries_per_run)
                {
                    return Err(ExtensionUiParseError::TooManyWidgetLines {
                        limit: limits.max_extension_ui_entries_per_run,
                    });
                }
                let placement = match optional_string(object, "widgetPlacement")?.as_deref() {
                    None | Some("aboveEditor") => ExtensionWidgetPlacement::AboveEditor,
                    Some("belowEditor") => ExtensionWidgetPlacement::BelowEditor,
                    Some(_) => return Err(ExtensionUiParseError::InvalidWidgetPlacement),
                };
                budget.add(key.len())?;
                if let Some(lines) = &lines {
                    for line in lines {
                        budget.add(line.len())?;
                    }
                }
                Ok(Self::FireAndForget(ExtensionFireAndForget::SetWidget {
                    key,
                    lines,
                    placement,
                }))
            }
            ExtensionUiMethod::SetTitle => {
                let title = required_string(object, "title")?;
                budget.add(title.len())?;
                Ok(Self::FireAndForget(ExtensionFireAndForget::SetTitle {
                    title,
                }))
            }
            ExtensionUiMethod::SetEditorText => {
                let text = required_string(object, "text")?;
                if text.len() > limits.max_draft_bytes_per_session {
                    return Err(ExtensionUiParseError::EditorTextTooLarge {
                        actual: text.len(),
                        limit: limits.max_draft_bytes_per_session,
                    });
                }
                budget.add(text.len())?;
                Ok(Self::FireAndForget(ExtensionFireAndForget::SetEditorText {
                    text,
                }))
            }
            ExtensionUiMethod::Unknown(_) => Ok(Self::Unknown(meta)),
        }
    }
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<String, ExtensionUiParseError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(ExtensionUiParseError::MissingPayloadField { field })
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, ExtensionUiParseError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ExtensionUiParseError::InvalidPayloadField { field }),
    }
}

fn required_string_array(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Vec<String>, ExtensionUiParseError> {
    optional_string_array(object, field)?
        .ok_or(ExtensionUiParseError::MissingPayloadField { field })
}

fn optional_string_array(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<Vec<String>>, ExtensionUiParseError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let values = value
        .as_array()
        .ok_or(ExtensionUiParseError::InvalidPayloadField { field })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(ExtensionUiParseError::InvalidPayloadField { field })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

struct ExtensionPayloadBudget {
    used: usize,
    limit: usize,
}

impl ExtensionPayloadBudget {
    const fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    fn add(&mut self, bytes: usize) -> Result<(), ExtensionUiParseError> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limit {
            return Err(ExtensionUiParseError::PayloadTooLarge {
                attempted: self.used,
                limit: self.limit,
            });
        }
        Ok(())
    }
}

impl ExtensionUiMethod {
    #[must_use]
    pub const fn disposition(&self) -> ExtensionUiDisposition {
        match self {
            Self::Select | Self::Confirm | Self::Input | Self::Editor => {
                ExtensionUiDisposition::Dialog
            }
            Self::Notify
            | Self::SetStatus
            | Self::SetWidget
            | Self::SetTitle
            | Self::SetEditorText => ExtensionUiDisposition::FireAndForget,
            Self::Unknown(_) => ExtensionUiDisposition::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionUiRequestMeta {
    pub id: String,
    pub method: ExtensionUiMethod,
    pub timeout_ms: Option<u64>,
}

impl ExtensionUiRequestMeta {
    pub fn parse(raw: &Value) -> Result<Self, ExtensionUiParseError> {
        let object = raw
            .as_object()
            .ok_or(ExtensionUiParseError::ExpectedObject)?;
        if object.get("type").and_then(Value::as_str) != Some("extension_ui_request") {
            return Err(ExtensionUiParseError::WrongType);
        }

        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ExtensionUiParseError::MissingId)?
            .to_owned();
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ExtensionUiParseError::MissingMethod)?;
        let method = match method {
            "select" => ExtensionUiMethod::Select,
            "confirm" => ExtensionUiMethod::Confirm,
            "input" => ExtensionUiMethod::Input,
            "editor" => ExtensionUiMethod::Editor,
            "notify" => ExtensionUiMethod::Notify,
            "setStatus" => ExtensionUiMethod::SetStatus,
            "setWidget" => ExtensionUiMethod::SetWidget,
            "setTitle" => ExtensionUiMethod::SetTitle,
            "set_editor_text" => ExtensionUiMethod::SetEditorText,
            unknown => ExtensionUiMethod::Unknown(unknown.to_owned()),
        };

        let timeout_ms = match object.get("timeout") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|value| *value > 0)
                    .ok_or(ExtensionUiParseError::InvalidTimeout)?,
            ),
        };

        Ok(Self {
            id,
            method,
            timeout_ms,
        })
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ExtensionUiParseError {
    #[error("extension UI request is not an object")]
    ExpectedObject,
    #[error("value is not an extension_ui_request")]
    WrongType,
    #[error("extension UI request is missing a non-empty id")]
    MissingId,
    #[error("extension UI request is missing a non-empty method")]
    MissingMethod,
    #[error("extension UI timeout must be a positive integer in milliseconds")]
    InvalidTimeout,
    #[error("extension UI request is missing payload field {field}")]
    MissingPayloadField { field: &'static str },
    #[error("extension UI request has invalid payload field {field}")]
    InvalidPayloadField { field: &'static str },
    #[error("extension UI select has {actual} options, exceeding limit {limit}")]
    TooManyOptions { actual: usize, limit: usize },
    #[error("extension UI widget line count exceeds limit {limit}")]
    TooManyWidgetLines { limit: usize },
    #[error("extension UI notifyType must be info, warning, or error")]
    InvalidNotifyType,
    #[error("extension UI widgetPlacement must be aboveEditor or belowEditor")]
    InvalidWidgetPlacement,
    #[error("extension UI payload would use {attempted} bytes, exceeding limit {limit}")]
    PayloadTooLarge { attempted: usize, limit: usize },
    #[error("extension set_editor_text is {actual} bytes, exceeding draft limit {limit}")]
    EditorTextTooLarge { actual: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn dialog_request_preserves_agent_side_timeout_metadata() {
        let meta = ExtensionUiRequestMeta::parse(&json!({
            "type": "extension_ui_request",
            "id": "dialog-1",
            "method": "confirm",
            "timeout": 5000
        }))
        .expect("dialog request");

        assert_eq!(meta.id, "dialog-1");
        assert_eq!(meta.method, ExtensionUiMethod::Confirm);
        assert_eq!(meta.method.disposition(), ExtensionUiDisposition::Dialog);
        assert_eq!(meta.timeout_ms, Some(5000));
    }

    #[test]
    fn fire_and_forget_request_is_not_misclassified_as_pending_dialog() {
        let meta = ExtensionUiRequestMeta::parse(&json!({
            "type": "extension_ui_request",
            "id": "status-1",
            "method": "setStatus",
            "statusKey": "build",
            "statusText": "running"
        }))
        .expect("status request");

        assert_eq!(meta.method, ExtensionUiMethod::SetStatus);
        assert_eq!(
            meta.method.disposition(),
            ExtensionUiDisposition::FireAndForget
        );
    }

    #[test]
    fn unknown_method_remains_explicit_instead_of_becoming_an_actionable_dialog() {
        let meta = ExtensionUiRequestMeta::parse(&json!({
            "type": "extension_ui_request",
            "id": "future-1",
            "method": "futureCustomUi"
        }))
        .expect("forward compatible envelope");

        assert_eq!(
            meta.method,
            ExtensionUiMethod::Unknown("futureCustomUi".to_owned())
        );
        assert_eq!(meta.method.disposition(), ExtensionUiDisposition::Unknown);
    }

    #[test]
    fn invalid_timeout_fails_closed() {
        assert_eq!(
            ExtensionUiRequestMeta::parse(&json!({
                "type": "extension_ui_request",
                "id": "dialog-1",
                "method": "input",
                "timeout": 0
            })),
            Err(ExtensionUiParseError::InvalidTimeout)
        );
    }

    #[test]
    fn typed_dialog_and_fire_and_forget_payloads_are_distinct() {
        let limits = RuntimeLimits::default();
        let select = ExtensionUiRequest::parse_bounded(
            &json!({
                "type":"extension_ui_request","id":"select-1","method":"select",
                "title":"Choose","options":["A","B"],"timeout":1000
            }),
            limits,
        )
        .expect("select");
        assert!(matches!(
            select,
            ExtensionUiRequest::Dialog(ExtensionDialogRequest {
                kind: ExtensionDialogKind::Select { .. },
                ..
            })
        ));

        let widget = ExtensionUiRequest::parse_bounded(
            &json!({
                "type":"extension_ui_request","id":"widget-1","method":"setWidget",
                "widgetKey":"build","widgetLines":["one","two"],"widgetPlacement":"belowEditor"
            }),
            limits,
        )
        .expect("widget");
        assert!(matches!(
            widget,
            ExtensionUiRequest::FireAndForget(ExtensionFireAndForget::SetWidget {
                placement: ExtensionWidgetPlacement::BelowEditor,
                ..
            })
        ));
    }

    #[test]
    fn extension_payload_and_editor_text_limits_fail_before_runtime_projection() {
        let limits = RuntimeLimits {
            max_extension_ui_bytes_per_run: 8,
            max_draft_bytes_per_session: 4,
            ..RuntimeLimits::default()
        };
        assert!(matches!(
            ExtensionUiRequest::parse_bounded(
                &json!({"type":"extension_ui_request","id":"n","method":"notify","message":"0123456789"}),
                limits,
            ),
            Err(ExtensionUiParseError::PayloadTooLarge { .. })
        ));
        assert_eq!(
            ExtensionUiRequest::parse_bounded(
                &json!({"type":"extension_ui_request","id":"e","method":"set_editor_text","text":"12345"}),
                limits,
            ),
            Err(ExtensionUiParseError::EditorTextTooLarge {
                actual: 5,
                limit: 4
            })
        );
    }
}
