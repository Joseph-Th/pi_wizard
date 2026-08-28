use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use super::{QueueMode, RpcResponse, ThinkingLevel};
use crate::RuntimeLimits;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
    /// `None` preserves compatibility with older/partial Pi model payloads.
    /// `Some(false)` means Pi explicitly declares the model text-only.
    pub supports_images: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextUsage {
    pub tokens: Option<usize>,
    pub context_window: usize,
    pub percent: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTokenUsage {
    pub input: usize,
    pub output: usize,
    pub cache_read: usize,
    pub cache_write: usize,
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub session_file: PathBuf,
    pub session_id: String,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub total_messages: usize,
    pub tokens: SessionTokenUsage,
    pub cost: f64,
    pub context_usage: Option<SessionContextUsage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub first_kept_entry_id: String,
    pub tokens_before: usize,
    pub estimated_tokens_after: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntryEnvelope {
    pub id: String,
    pub parent_id: Option<String>,
    pub entry_type: String,
    pub timestamp: Option<String>,
    pub raw: Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntriesPage {
    pub entries: Vec<SessionEntryEnvelope>,
    pub leaf_id: Option<String>,
    pub encoded_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSummary {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub location: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcStateSnapshot {
    pub model: Option<ModelSummary>,
    pub thinking_level: ThinkingLevel,
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub session_file: Option<PathBuf>,
    pub session_id: String,
    pub session_name: Option<String>,
    pub auto_compaction_enabled: bool,
    pub message_count: usize,
    pub pending_message_count: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClearQueueResult {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

impl ClearQueueResult {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_up.is_empty()
    }

    #[must_use]
    pub fn message_count(&self) -> usize {
        self.steering.len().saturating_add(self.follow_up.len())
    }

    #[must_use]
    pub fn text_bytes(&self) -> usize {
        self.steering
            .iter()
            .chain(&self.follow_up)
            .fold(0usize, |total, message| total.saturating_add(message.len()))
    }
}

fn validate_session_cursor(
    cursor: &str,
    limits: RuntimeLimits,
) -> Result<(), RpcResponsePayloadError> {
    if cursor.is_empty() || cursor.len() > limits.max_session_cursor_bytes {
        return Err(RpcResponsePayloadError::InvalidSessionCursor {
            actual: cursor.len(),
            limit: limits.max_session_cursor_bytes,
        });
    }
    Ok(())
}

impl RpcResponse {
    pub fn entries_page(
        &self,
        limits: RuntimeLimits,
    ) -> Result<SessionEntriesPage, RpcResponsePayloadError> {
        let data = accepted_data(self, "get_entries")?;
        let entries = required_array(data, "get_entries", "entries")?;
        if entries.len() > limits.max_session_entry_page_entries {
            return Err(RpcResponsePayloadError::SessionEntryLimit {
                actual: entries.len(),
                limit: limits.max_session_entry_page_entries,
            });
        }

        let leaf_id = optional_string(data, "get_entries", "leafId")?;
        if let Some(leaf_id) = &leaf_id {
            validate_session_cursor(leaf_id, limits)?;
        }

        let mut encoded_bytes = 0usize;
        let mut parsed = Vec::with_capacity(entries.len());
        for value in entries {
            let object = value
                .as_object()
                .ok_or(RpcResponsePayloadError::InvalidObjectArray {
                    command: "get_entries",
                    field: "entries",
                })?;
            let id = required_string(object, "get_entries", "id")?;
            validate_session_cursor(&id, limits)?;
            let parent_id = optional_string(object, "get_entries", "parentId")?;
            if let Some(parent_id) = &parent_id {
                validate_session_cursor(parent_id, limits)?;
            }
            let entry_type = required_string(object, "get_entries", "type")?;
            let timestamp = optional_string(object, "get_entries", "timestamp")?;
            let encoded = serde_json::to_vec(value)
                .map_err(|_| RpcResponsePayloadError::InvalidSessionEntryEncoding)?;
            encoded_bytes = encoded_bytes.saturating_add(encoded.len());
            if encoded_bytes > limits.max_session_entry_page_bytes {
                return Err(RpcResponsePayloadError::SessionEntryByteLimit {
                    attempted: encoded_bytes,
                    limit: limits.max_session_entry_page_bytes,
                });
            }
            parsed.push(SessionEntryEnvelope {
                id,
                parent_id,
                entry_type,
                timestamp,
                raw: value.clone(),
            });
        }

        Ok(SessionEntriesPage {
            entries: parsed,
            leaf_id,
            encoded_bytes,
        })
    }

    pub fn available_models(
        &self,
        limits: RuntimeLimits,
    ) -> Result<Vec<ModelSummary>, RpcResponsePayloadError> {
        let data = accepted_data(self, "get_available_models")?;
        let models = required_array(data, "get_available_models", "models")?;
        enforce_entry_limit(models.len(), limits)?;
        let mut budget = CapabilityBudget::new(limits.max_capability_bytes_per_run);
        models
            .iter()
            .map(|value| {
                let object =
                    value
                        .as_object()
                        .ok_or(RpcResponsePayloadError::InvalidObjectArray {
                            command: "get_available_models",
                            field: "models",
                        })?;
                let model = ModelSummary {
                    provider: required_string(object, "get_available_models", "provider")?,
                    id: required_string(object, "get_available_models", "id")?,
                    name: optional_string(object, "get_available_models", "name")?,
                    supports_images: parse_model_image_support(object, "get_available_models")?,
                };
                budget.add(model.provider.len())?;
                budget.add(model.id.len())?;
                budget.add(model.name.as_ref().map_or(0, String::len))?;
                Ok(model)
            })
            .collect()
    }

    pub fn available_thinking_levels(
        &self,
        limits: RuntimeLimits,
    ) -> Result<Vec<ThinkingLevel>, RpcResponsePayloadError> {
        let data = accepted_data(self, "get_available_thinking_levels")?;
        let levels = required_array(data, "get_available_thinking_levels", "levels")?;
        enforce_entry_limit(levels.len(), limits)?;
        levels
            .iter()
            .map(|level| {
                serde_json::from_value(level.clone()).map_err(|_| {
                    RpcResponsePayloadError::InvalidEnumValue {
                        command: "get_available_thinking_levels",
                        field: "levels",
                    }
                })
            })
            .collect()
    }

    pub fn available_commands(
        &self,
        limits: RuntimeLimits,
    ) -> Result<Vec<CommandSummary>, RpcResponsePayloadError> {
        let data = accepted_data(self, "get_commands")?;
        let commands = required_array(data, "get_commands", "commands")?;
        enforce_entry_limit(commands.len(), limits)?;
        let mut budget = CapabilityBudget::new(limits.max_capability_bytes_per_run);
        commands
            .iter()
            .map(|value| {
                let object =
                    value
                        .as_object()
                        .ok_or(RpcResponsePayloadError::InvalidObjectArray {
                            command: "get_commands",
                            field: "commands",
                        })?;
                let command = CommandSummary {
                    name: required_string(object, "get_commands", "name")?,
                    description: optional_string(object, "get_commands", "description")?,
                    source: required_string(object, "get_commands", "source")?,
                    location: optional_string(object, "get_commands", "location")?,
                    path: optional_path(object, "get_commands", "path")?,
                };
                budget.add(command.name.len())?;
                budget.add(command.description.as_ref().map_or(0, String::len))?;
                budget.add(command.source.len())?;
                budget.add(command.location.as_ref().map_or(0, String::len))?;
                budget.add(
                    command
                        .path
                        .as_ref()
                        .map_or(0, |path| path.as_os_str().len()),
                )?;
                Ok(command)
            })
            .collect()
    }
}

fn required_array<'a>(
    data: &'a Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<&'a Vec<Value>, RpcResponsePayloadError> {
    data.get(field)
        .and_then(Value::as_array)
        .ok_or(RpcResponsePayloadError::MissingArray { command, field })
}

fn enforce_entry_limit(
    actual: usize,
    limits: RuntimeLimits,
) -> Result<(), RpcResponsePayloadError> {
    if actual > limits.max_capability_entries_per_run {
        return Err(RpcResponsePayloadError::CapabilityEntryLimit {
            actual,
            limit: limits.max_capability_entries_per_run,
        });
    }
    Ok(())
}

struct CapabilityBudget {
    used: usize,
    limit: usize,
}

impl CapabilityBudget {
    const fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    fn add(&mut self, bytes: usize) -> Result<(), RpcResponsePayloadError> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limit {
            return Err(RpcResponsePayloadError::CapabilityByteLimit {
                attempted: self.used,
                limit: self.limit,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BashCommandResult {
    pub output: String,
    pub exit_code: i64,
    pub cancelled: bool,
    pub truncated: bool,
    pub full_output_path: Option<PathBuf>,
}

impl RpcResponse {
    pub fn state_snapshot(
        &self,
        limits: RuntimeLimits,
    ) -> Result<RpcStateSnapshot, RpcResponsePayloadError> {
        let data = accepted_data(self, "get_state")?;
        let snapshot = RpcStateSnapshot {
            model: parse_model(data.get("model"))?,
            thinking_level: parse_enum(data, "get_state", "thinkingLevel")?,
            is_streaming: required_bool(data, "get_state", "isStreaming")?,
            is_compacting: required_bool(data, "get_state", "isCompacting")?,
            steering_mode: parse_enum(data, "get_state", "steeringMode")?,
            follow_up_mode: parse_enum(data, "get_state", "followUpMode")?,
            session_file: optional_path(data, "get_state", "sessionFile")?,
            session_id: required_string(data, "get_state", "sessionId")?,
            session_name: optional_string(data, "get_state", "sessionName")?,
            auto_compaction_enabled: required_bool(data, "get_state", "autoCompactionEnabled")?,
            message_count: required_usize(data, "get_state", "messageCount")?,
            pending_message_count: required_usize(data, "get_state", "pendingMessageCount")?,
        };
        if snapshot.session_id.is_empty()
            || snapshot.session_id.len() > limits.max_session_cursor_bytes
        {
            return Err(RpcResponsePayloadError::InvalidSessionCursor {
                actual: snapshot.session_id.len(),
                limit: limits.max_session_cursor_bytes,
            });
        }
        let retained_bytes = snapshot
            .model
            .as_ref()
            .map_or(0, |model| {
                model
                    .provider
                    .len()
                    .saturating_add(model.id.len())
                    .saturating_add(model.name.as_ref().map_or(0, String::len))
            })
            .saturating_add(
                snapshot
                    .session_file
                    .as_ref()
                    .map_or(0, |path| path.as_os_str().to_string_lossy().len()),
            )
            .saturating_add(snapshot.session_id.len())
            .saturating_add(snapshot.session_name.as_ref().map_or(0, String::len));
        if retained_bytes > limits.max_runtime_state_bytes_per_run {
            return Err(RpcResponsePayloadError::RuntimeStateByteLimit {
                attempted: retained_bytes,
                limit: limits.max_runtime_state_bytes_per_run,
            });
        }
        Ok(snapshot)
    }

    pub fn clear_queue_result(
        &self,
        limits: RuntimeLimits,
    ) -> Result<ClearQueueResult, RpcResponsePayloadError> {
        let data = accepted_data(self, "clear_queue")?;
        let steering = required_string_array(data, "clear_queue", "steering")?;
        let follow_up = required_string_array(data, "clear_queue", "followUp")?;
        let message_count = steering.len().saturating_add(follow_up.len());
        if message_count > limits.max_recovered_queue_messages_per_run {
            return Err(RpcResponsePayloadError::RecoveredQueueMessageLimit {
                actual: message_count,
                limit: limits.max_recovered_queue_messages_per_run,
            });
        }
        let mut text_bytes = 0usize;
        let mut collect = |values: &Vec<Value>| -> Result<Vec<String>, RpcResponsePayloadError> {
            values
                .iter()
                .map(|value| {
                    let text =
                        value
                            .as_str()
                            .ok_or(RpcResponsePayloadError::InvalidStringArray {
                                command: "clear_queue",
                                field: "queue",
                            })?;
                    text_bytes = text_bytes.saturating_add(text.len());
                    if text_bytes > limits.max_recovered_queue_bytes_per_run {
                        return Err(RpcResponsePayloadError::RecoveredQueueByteLimit {
                            attempted: text_bytes,
                            limit: limits.max_recovered_queue_bytes_per_run,
                        });
                    }
                    Ok(text.to_owned())
                })
                .collect()
        };
        Ok(ClearQueueResult {
            steering: collect(steering)?,
            follow_up: collect(follow_up)?,
        })
    }

    pub fn bash_result(&self) -> Result<BashCommandResult, RpcResponsePayloadError> {
        let data = accepted_data(self, "bash")?;
        Ok(BashCommandResult {
            output: required_string(data, "bash", "output")?,
            exit_code: data.get("exitCode").and_then(Value::as_i64).ok_or(
                RpcResponsePayloadError::MissingInteger {
                    command: "bash",
                    field: "exitCode",
                },
            )?,
            cancelled: required_bool(data, "bash", "cancelled")?,
            truncated: required_bool(data, "bash", "truncated")?,
            full_output_path: optional_path(data, "bash", "fullOutputPath")?,
        })
    }

    pub fn session_stats(
        &self,
        limits: RuntimeLimits,
    ) -> Result<SessionStats, RpcResponsePayloadError> {
        let data = accepted_data(self, "get_session_stats")?;
        let session_file =
            PathBuf::from(required_string(data, "get_session_stats", "sessionFile")?);
        let session_id = required_string(data, "get_session_stats", "sessionId")?;
        validate_session_cursor(&session_id, limits)?;
        if session_file.as_os_str().to_string_lossy().len() > limits.max_runtime_state_bytes_per_run
        {
            return Err(RpcResponsePayloadError::RuntimeStateByteLimit {
                attempted: session_file.as_os_str().to_string_lossy().len(),
                limit: limits.max_runtime_state_bytes_per_run,
            });
        }
        let token_data = data.get("tokens").and_then(Value::as_object).ok_or(
            RpcResponsePayloadError::MissingObject {
                command: "get_session_stats",
                field: "tokens",
            },
        )?;
        let tokens = SessionTokenUsage {
            input: required_usize(token_data, "get_session_stats", "input")?,
            output: required_usize(token_data, "get_session_stats", "output")?,
            cache_read: required_usize(token_data, "get_session_stats", "cacheRead")?,
            cache_write: required_usize(token_data, "get_session_stats", "cacheWrite")?,
            total: required_usize(token_data, "get_session_stats", "total")?,
        };
        let context_usage = match data.get("contextUsage") {
            None | Some(Value::Null) => None,
            Some(Value::Object(context)) => Some(SessionContextUsage {
                tokens: optional_usize(context, "get_session_stats", "tokens")?,
                context_window: required_usize(context, "get_session_stats", "contextWindow")?,
                percent: optional_f64(context, "get_session_stats", "percent")?,
            }),
            Some(_) => {
                return Err(RpcResponsePayloadError::InvalidOptionalObject {
                    command: "get_session_stats",
                    field: "contextUsage",
                });
            }
        };
        let cost = required_f64(data, "get_session_stats", "cost")?;
        if !cost.is_finite() || cost < 0.0 {
            return Err(RpcResponsePayloadError::InvalidNumber {
                command: "get_session_stats",
                field: "cost",
            });
        }
        Ok(SessionStats {
            session_file,
            session_id,
            user_messages: required_usize(data, "get_session_stats", "userMessages")?,
            assistant_messages: required_usize(data, "get_session_stats", "assistantMessages")?,
            tool_calls: required_usize(data, "get_session_stats", "toolCalls")?,
            tool_results: required_usize(data, "get_session_stats", "toolResults")?,
            total_messages: required_usize(data, "get_session_stats", "totalMessages")?,
            tokens,
            cost,
            context_usage,
        })
    }

    pub fn compaction_result(
        &self,
        limits: RuntimeLimits,
    ) -> Result<CompactionResult, RpcResponsePayloadError> {
        let data = accepted_data(self, "compact")?;
        let first_kept_entry_id = required_string(data, "compact", "firstKeptEntryId")?;
        validate_session_cursor(&first_kept_entry_id, limits)?;
        Ok(CompactionResult {
            first_kept_entry_id,
            tokens_before: required_usize(data, "compact", "tokensBefore")?,
            estimated_tokens_after: required_usize(data, "compact", "estimatedTokensAfter")?,
        })
    }
}

fn required_string_array<'a>(
    data: &'a Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<&'a Vec<Value>, RpcResponsePayloadError> {
    required_array(data, command, field)
}

fn optional_usize(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<Option<usize>, RpcResponsePayloadError> {
    match data.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => {
            let raw =
                value
                    .as_u64()
                    .ok_or(RpcResponsePayloadError::InvalidOptionalUnsignedInteger {
                        command,
                        field,
                    })?;
            usize::try_from(raw)
                .map(Some)
                .map_err(|_| RpcResponsePayloadError::IntegerOutOfRange {
                    command,
                    field,
                    value: raw,
                })
        }
        Some(_) => Err(RpcResponsePayloadError::InvalidOptionalUnsignedInteger { command, field }),
    }
}

fn required_f64(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<f64, RpcResponsePayloadError> {
    data.get(field)
        .and_then(Value::as_f64)
        .ok_or(RpcResponsePayloadError::MissingNumber { command, field })
}

fn optional_f64(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<Option<f64>, RpcResponsePayloadError> {
    match data.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or(RpcResponsePayloadError::InvalidOptionalNumber { command, field }),
    }
}

fn accepted_data<'a>(
    response: &'a RpcResponse,
    command: &'static str,
) -> Result<&'a Map<String, Value>, RpcResponsePayloadError> {
    if response.command != command {
        return Err(RpcResponsePayloadError::WrongCommand {
            expected: command,
            actual: response.command.clone(),
        });
    }
    if !response.success {
        return Err(RpcResponsePayloadError::Rejected {
            command,
            error: response.error.clone(),
        });
    }
    response
        .data
        .as_ref()
        .and_then(Value::as_object)
        .ok_or(RpcResponsePayloadError::MissingDataObject { command })
}

fn required_string(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<String, RpcResponsePayloadError> {
    data.get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(RpcResponsePayloadError::MissingString { command, field })
}

fn optional_string(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<Option<String>, RpcResponsePayloadError> {
    match data.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RpcResponsePayloadError::InvalidOptionalString { command, field }),
    }
}

fn optional_path(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<Option<PathBuf>, RpcResponsePayloadError> {
    optional_string(data, command, field).map(|value| value.map(PathBuf::from))
}

fn required_bool(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<bool, RpcResponsePayloadError> {
    data.get(field)
        .and_then(Value::as_bool)
        .ok_or(RpcResponsePayloadError::MissingBoolean { command, field })
}

fn required_usize(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<usize, RpcResponsePayloadError> {
    let value = data
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(RpcResponsePayloadError::MissingUnsignedInteger { command, field })?;
    usize::try_from(value).map_err(|_| RpcResponsePayloadError::IntegerOutOfRange {
        command,
        field,
        value,
    })
}

fn parse_enum<T>(
    data: &Map<String, Value>,
    command: &'static str,
    field: &'static str,
) -> Result<T, RpcResponsePayloadError>
where
    T: serde::de::DeserializeOwned,
{
    let value = data
        .get(field)
        .cloned()
        .ok_or(RpcResponsePayloadError::MissingString { command, field })?;
    serde_json::from_value(value)
        .map_err(|_| RpcResponsePayloadError::InvalidEnumValue { command, field })
}

fn parse_model(value: Option<&Value>) -> Result<Option<ModelSummary>, RpcResponsePayloadError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or(RpcResponsePayloadError::InvalidModelObject)?;
    Ok(Some(ModelSummary {
        provider: required_string(object, "get_state", "provider")?,
        id: required_string(object, "get_state", "id")?,
        name: optional_string(object, "get_state", "name")?,
        supports_images: parse_model_image_support(object, "get_state")?,
    }))
}

fn parse_model_image_support(
    object: &Map<String, Value>,
    command: &'static str,
) -> Result<Option<bool>, RpcResponsePayloadError> {
    let Some(input) = object.get("input") else {
        return Ok(None);
    };
    let values = input
        .as_array()
        .ok_or(RpcResponsePayloadError::InvalidStringArray {
            command,
            field: "input",
        })?;
    let mut supports_images = false;
    for value in values {
        let value = value
            .as_str()
            .ok_or(RpcResponsePayloadError::InvalidStringArray {
                command,
                field: "input",
            })?;
        supports_images |= value == "image";
    }
    Ok(Some(supports_images))
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RpcResponsePayloadError {
    #[error("RPC payload decoder expected response command {expected}, got {actual}")]
    WrongCommand {
        expected: &'static str,
        actual: String,
    },
    #[error("RPC command {command} field {field} must contain only objects")]
    InvalidObjectArray {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} was rejected: {error:?}")]
    Rejected {
        command: &'static str,
        error: Option<String>,
    },
    #[error("successful RPC command {command} is missing an object data payload")]
    MissingDataObject { command: &'static str },
    #[error("RPC command {command} is missing string field {field}")]
    MissingString {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} optional field {field} must be a string when present")]
    InvalidOptionalString {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} is missing boolean field {field}")]
    MissingBoolean {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} is missing integer field {field}")]
    MissingInteger {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} is missing unsigned integer field {field}")]
    MissingUnsignedInteger {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} is missing number field {field}")]
    MissingNumber {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} optional field {field} must be an unsigned integer or null")]
    InvalidOptionalUnsignedInteger {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} optional field {field} must be a finite number or null")]
    InvalidOptionalNumber {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} field {field} must be a valid finite number")]
    InvalidNumber {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} is missing object field {field}")]
    MissingObject {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} optional field {field} must be an object or null")]
    InvalidOptionalObject {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} field {field} value {value} does not fit this platform")]
    IntegerOutOfRange {
        command: &'static str,
        field: &'static str,
        value: u64,
    },
    #[error("RPC command {command} is missing array field {field}")]
    MissingArray {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} field {field} must contain only strings")]
    InvalidStringArray {
        command: &'static str,
        field: &'static str,
    },
    #[error("RPC command {command} field {field} contains an unsupported enum value")]
    InvalidEnumValue {
        command: &'static str,
        field: &'static str,
    },
    #[error("get_state model must be an object or null")]
    InvalidModelObject,
    #[error("capability response contains {actual} entries, exceeding limit {limit}")]
    CapabilityEntryLimit { actual: usize, limit: usize },
    #[error("capability projection would use {attempted} bytes, exceeding limit {limit}")]
    CapabilityByteLimit { attempted: usize, limit: usize },
    #[error("get_state projection would use {attempted} bytes, exceeding limit {limit}")]
    RuntimeStateByteLimit { attempted: usize, limit: usize },
    #[error("get_entries returned {actual} entries, exceeding page limit {limit}")]
    SessionEntryLimit { actual: usize, limit: usize },
    #[error("get_entries page would use {attempted} bytes, exceeding limit {limit}")]
    SessionEntryByteLimit { attempted: usize, limit: usize },
    #[error("session entry cursor/id is {actual} bytes; expected 1..={limit}")]
    InvalidSessionCursor { actual: usize, limit: usize },
    #[error("clear_queue returned {actual} messages, exceeding recovery limit {limit}")]
    RecoveredQueueMessageLimit { actual: usize, limit: usize },
    #[error("clear_queue recovery would retain {attempted} bytes, exceeding limit {limit}")]
    RecoveredQueueByteLimit { attempted: usize, limit: usize },
    #[error("get_entries returned a session entry that could not be encoded deterministically")]
    InvalidSessionEntryEncoding,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn response(command: &str, data: Value) -> RpcResponse {
        RpcResponse {
            id: Some("req".to_owned()),
            command: command.to_owned(),
            success: true,
            data: Some(data),
            error: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn get_state_decodes_current_runtime_identity_and_modes() {
        let snapshot = response(
            "get_state",
            json!({
                "model": {"provider":"openai","id":"gpt-5.6","name":"GPT-5.6","input":["text","image"],"future":true},
                "thinkingLevel":"xhigh",
                "isStreaming":true,
                "isCompacting":false,
                "steeringMode":"one-at-a-time",
                "followUpMode":"all",
                "sessionFile":"/sessions/one.jsonl",
                "sessionId":"session-1",
                "sessionName":"feature work",
                "autoCompactionEnabled":true,
                "messageCount":17,
                "pendingMessageCount":2
            }),
        )
        .state_snapshot(RuntimeLimits::default())
        .expect("state snapshot");

        assert_eq!(
            snapshot.model,
            Some(ModelSummary {
                provider: "openai".to_owned(),
                id: "gpt-5.6".to_owned(),
                name: Some("GPT-5.6".to_owned()),
                supports_images: Some(true),
            })
        );
        assert_eq!(snapshot.thinking_level, ThinkingLevel::Xhigh);
        assert!(snapshot.is_streaming);
        assert_eq!(snapshot.steering_mode, QueueMode::OneAtATime);
        assert_eq!(snapshot.follow_up_mode, QueueMode::All);
        assert_eq!(snapshot.session_id, "session-1");
        assert_eq!(snapshot.message_count, 17);
        assert_eq!(snapshot.pending_message_count, 2);
    }

    #[test]
    fn get_state_projection_bounds_session_identity_and_retained_text() {
        let cursor_limits = RuntimeLimits {
            max_session_cursor_bytes: 4,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            response(
                "get_state",
                json!({
                    "model": null,
                    "thinkingLevel":"medium",
                    "isStreaming":false,
                    "isCompacting":false,
                    "steeringMode":"all",
                    "followUpMode":"one-at-a-time",
                    "sessionFile":null,
                    "sessionId":"oversized",
                    "sessionName":null,
                    "autoCompactionEnabled":true,
                    "messageCount":0,
                    "pendingMessageCount":0
                }),
            )
            .state_snapshot(cursor_limits),
            Err(RpcResponsePayloadError::InvalidSessionCursor {
                actual: 9,
                limit: 4,
            })
        );

        let state_limits = RuntimeLimits {
            max_runtime_state_bytes_per_run: 8,
            ..RuntimeLimits::default()
        };
        assert!(matches!(
            response(
                "get_state",
                json!({
                    "model": null,
                    "thinkingLevel":"medium",
                    "isStreaming":false,
                    "isCompacting":false,
                    "steeringMode":"all",
                    "followUpMode":"one-at-a-time",
                    "sessionFile":null,
                    "sessionId":"id",
                    "sessionName":"0123456789",
                    "autoCompactionEnabled":true,
                    "messageCount":0,
                    "pendingMessageCount":0
                }),
            )
            .state_snapshot(state_limits),
            Err(RpcResponsePayloadError::RuntimeStateByteLimit { .. })
        ));
    }

    #[test]
    fn session_stats_preserve_context_usage_without_guessing_unknown_post_compaction_values() {
        let stats = response(
            "get_session_stats",
            json!({
                "sessionFile":"/sessions/one.jsonl",
                "sessionId":"session-1",
                "userMessages":5,
                "assistantMessages":5,
                "toolCalls":12,
                "toolResults":12,
                "totalMessages":22,
                "tokens":{"input":50000,"output":10000,"cacheRead":40000,"cacheWrite":5000,"total":105000},
                "cost":0.45,
                "contextUsage":{"tokens":null,"contextWindow":200000,"percent":null}
            }),
        )
        .session_stats(RuntimeLimits::default())
        .expect("session stats");

        assert_eq!(stats.session_id, "session-1");
        assert_eq!(stats.tokens.total, 105_000);
        assert_eq!(stats.cost, 0.45);
        let context = stats.context_usage.expect("context usage");
        assert_eq!(context.tokens, None);
        assert_eq!(context.context_window, 200_000);
        assert_eq!(context.percent, None);
    }

    #[test]
    fn compaction_result_keeps_only_bounded_reconciliation_metadata() {
        let result = response(
            "compact",
            json!({
                "summary":"a potentially very large summary that the renderer does not need",
                "firstKeptEntryId":"kept-1",
                "tokensBefore":150000,
                "estimatedTokensAfter":32000,
                "usage":{"input":32000}
            }),
        )
        .compaction_result(RuntimeLimits::default())
        .expect("compaction result");

        assert_eq!(result.first_kept_entry_id, "kept-1");
        assert_eq!(result.tokens_before, 150_000);
        assert_eq!(result.estimated_tokens_after, 32_000);
    }

    #[test]
    fn clear_queue_preserves_exact_steering_and_follow_up_text() {
        let result = response(
            "clear_queue",
            json!({"steering":["one","two"],"followUp":["after"]}),
        )
        .clear_queue_result(RuntimeLimits::default())
        .expect("clear queue");

        assert_eq!(result.steering, ["one", "two"]);
        assert_eq!(result.follow_up, ["after"]);
        assert_eq!(result.message_count(), 3);
        assert_eq!(result.text_bytes(), 11);
    }

    #[test]
    fn clear_queue_recovery_is_bounded_before_text_is_cloned() {
        let message_limits = RuntimeLimits {
            max_recovered_queue_messages_per_run: 2,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            response(
                "clear_queue",
                json!({"steering":["one","two"],"followUp":["three"]}),
            )
            .clear_queue_result(message_limits),
            Err(RpcResponsePayloadError::RecoveredQueueMessageLimit {
                actual: 3,
                limit: 2,
            })
        );

        let byte_limits = RuntimeLimits {
            max_recovered_queue_bytes_per_run: 5,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            response(
                "clear_queue",
                json!({"steering":["1234"],"followUp":["56"]}),
            )
            .clear_queue_result(byte_limits),
            Err(RpcResponsePayloadError::RecoveredQueueByteLimit {
                attempted: 6,
                limit: 5,
            })
        );
    }

    #[test]
    fn bash_result_retains_truncation_metadata_without_guessing_from_output() {
        let result = response(
            "bash",
            json!({
                "output":"tail",
                "exitCode":130,
                "cancelled":true,
                "truncated":true,
                "fullOutputPath":"/tmp/full.log"
            }),
        )
        .bash_result()
        .expect("bash result");

        assert_eq!(result.exit_code, 130);
        assert!(result.cancelled);
        assert!(result.truncated);
        assert_eq!(
            result.full_output_path,
            Some(PathBuf::from("/tmp/full.log"))
        );
    }

    #[test]
    fn rejected_response_is_not_mistaken_for_malformed_success_data() {
        let rejected = RpcResponse {
            id: Some("req".to_owned()),
            command: "clear_queue".to_owned(),
            success: false,
            data: None,
            error: Some("nope".to_owned()),
            extra: BTreeMap::new(),
        };

        assert_eq!(
            rejected.clear_queue_result(RuntimeLimits::default()),
            Err(RpcResponsePayloadError::Rejected {
                command: "clear_queue",
                error: Some("nope".to_owned())
            })
        );
    }

    #[test]
    fn capability_responses_keep_only_bounded_ui_fields() {
        let limits = RuntimeLimits::default();
        let models = response(
            "get_available_models",
            json!({"models":[
                {"provider":"openai","id":"gpt-5.6","name":"GPT-5.6","input":["text","image"],"contextWindow":999999,"secretFutureField":"ignored"},
                {"provider":"local","id":"text-only","name":"Text only","input":["text"]},
                {"provider":"legacy","id":"unknown","name":"Legacy"}
            ]}),
        )
        .available_models(limits)
        .expect("models");
        assert_eq!(models[0].id, "gpt-5.6");
        assert_eq!(models[0].supports_images, Some(true));
        assert_eq!(models[1].supports_images, Some(false));
        assert_eq!(models[2].supports_images, None);

        let levels = response(
            "get_available_thinking_levels",
            json!({"levels":["off","high","max"]}),
        )
        .available_thinking_levels(limits)
        .expect("thinking levels");
        assert_eq!(
            levels,
            [ThinkingLevel::Off, ThinkingLevel::High, ThinkingLevel::Max]
        );

        let commands = response(
            "get_commands",
            json!({"commands":[{"name":"fix-tests","description":"Fix tests","source":"prompt","location":"project","path":"/p/fix-tests.md"}]}),
        )
        .available_commands(limits)
        .expect("commands");
        assert_eq!(commands[0].name, "fix-tests");
        assert_eq!(commands[0].source, "prompt");
    }

    #[test]
    fn capability_projection_enforces_entry_and_byte_ceilings() {
        let entry_limits = RuntimeLimits {
            max_capability_entries_per_run: 1,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            response(
                "get_available_thinking_levels",
                json!({"levels":["off","high"]}),
            )
            .available_thinking_levels(entry_limits),
            Err(RpcResponsePayloadError::CapabilityEntryLimit {
                actual: 2,
                limit: 1,
            })
        );

        let byte_limits = RuntimeLimits {
            max_capability_bytes_per_run: 4,
            ..RuntimeLimits::default()
        };
        assert!(matches!(
            response(
                "get_commands",
                json!({"commands":[{"name":"long-name","source":"extension"}]}),
            )
            .available_commands(byte_limits),
            Err(RpcResponsePayloadError::CapabilityByteLimit { .. })
        ));
    }

    #[test]
    fn get_entries_preserves_bounded_raw_entries_and_leaf_identity() {
        let page = response(
            "get_entries",
            json!({
                "entries":[
                    {"type":"message","id":"def456","parentId":"abc123","timestamp":"2026-08-27T12:00:00Z","message":{"role":"user","content":"hello"}},
                    {"type":"custom","id":"ghi789","parentId":"def456","customType":"marker","data":{"x":1}}
                ],
                "leafId":"ghi789"
            }),
        )
        .entries_page(RuntimeLimits::default())
        .expect("entries page");

        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].id, "def456");
        assert_eq!(page.entries[0].parent_id.as_deref(), Some("abc123"));
        assert_eq!(page.entries[1].entry_type, "custom");
        assert_eq!(page.leaf_id.as_deref(), Some("ghi789"));
        assert!(page.encoded_bytes > 0);
        assert_eq!(page.entries[0].raw["message"]["role"], "user");
    }

    #[test]
    fn get_entries_rejects_unbounded_page_and_cursor_shapes() {
        let entry_limits = RuntimeLimits {
            max_session_entry_page_entries: 1,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            response(
                "get_entries",
                json!({
                    "entries":[
                        {"type":"message","id":"a"},
                        {"type":"message","id":"b"}
                    ],
                    "leafId":"b"
                }),
            )
            .entries_page(entry_limits),
            Err(RpcResponsePayloadError::SessionEntryLimit {
                actual: 2,
                limit: 1,
            })
        );

        let cursor_limits = RuntimeLimits {
            max_session_cursor_bytes: 3,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            response(
                "get_entries",
                json!({"entries":[{"type":"message","id":"abcd"}],"leafId":"abcd"}),
            )
            .entries_page(cursor_limits),
            Err(RpcResponsePayloadError::InvalidSessionCursor {
                actual: 4,
                limit: 3,
            })
        );

        let byte_limits = RuntimeLimits {
            max_session_entry_page_bytes: 16,
            ..RuntimeLimits::default()
        };
        assert!(matches!(
            response(
                "get_entries",
                json!({"entries":[{"type":"message","id":"a","payload":"01234567890123456789"}],"leafId":"a"}),
            )
            .entries_page(byte_limits),
            Err(RpcResponsePayloadError::SessionEntryByteLimit { .. })
        ));
    }
}
