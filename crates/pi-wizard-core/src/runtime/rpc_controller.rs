use std::collections::HashMap;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::bounded::BoundedText;
use crate::rpc::{
    ActiveRpcCommand, AssistantMessageBlockKind, AssistantMessageUpdate, ExtensionDialogRequest,
    ExtensionFireAndForget, ExtensionNotifyType, ExtensionUiParseError, ExtensionUiRequest,
    ExtensionWidgetPlacement, PendingRequest, PendingRequestError, PendingRequests, RpcCommand,
    RpcCommandGate, RpcEvent, RpcEventKind, RpcEventPayloadError, RpcGateError, RpcRequest,
    RpcResponse, RpcResponseOutcome, RpcResponsePayloadError, SessionEntriesPage,
};
use crate::{RequestId, RunId, RuntimeLimits};

use super::{
    AssistantContentKind, ExtensionUiError, ExtensionUiState, ExtensionWidget, LiveProjection,
    PendingExtensionDialogSnapshot, ProjectionError, QueueState, RunCapabilities, RunModelState,
    RunMutation, RunRpcHydrationSnapshot, RunStateObservation, RuntimeError, RuntimeStore,
    SessionSyncApplied, SessionSyncError, SessionSyncResync, SessionSyncState, ToolPreview,
    WidgetPlacement,
};

/// Tauri-independent owner for one live run's RPC correlation and transient
/// projection state.
///
/// The desktop host may own the child process and I/O tasks, but it should not
/// independently reconstruct request registration, command barriers, stream
/// assembly, or runtime-store mutations. This controller keeps those pieces in
/// one deterministic, testable boundary.
#[derive(Debug)]
pub struct RunRpcController {
    run_id: RunId,
    pending: PendingRequests,
    gate: RpcCommandGate,
    live: LiveProjection,
    capabilities: RunCapabilities,
    extension_ui: ExtensionUiState,
    pending_dialogs: HashMap<String, PendingExtensionDialog>,
    pending_dialog_bytes: usize,
    session_sync: SessionSyncState,
    session_sync_request: Option<PendingSessionSyncRequest>,
    limits: RuntimeLimits,
}

impl RunRpcController {
    #[must_use]
    pub fn new(run_id: RunId, limits: RuntimeLimits) -> Self {
        Self {
            run_id,
            pending: PendingRequests::from_limits(limits),
            gate: RpcCommandGate::from_limits(limits),
            live: LiveProjection::new(limits),
            capabilities: RunCapabilities::default(),
            extension_ui: ExtensionUiState::new(limits),
            pending_dialogs: HashMap::new(),
            pending_dialog_bytes: 0,
            session_sync: SessionSyncState::default(),
            session_sync_request: None,
            limits,
        }
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Registers all app-owned request state before a frame is handed to the
    /// writer. If writing fails, the caller must invoke [`Self::cancel_request`]
    /// so capacity/barriers are released immediately.
    pub fn begin_request(
        &mut self,
        request: &RpcRequest,
        expires_at: Option<Instant>,
    ) -> Result<(), RunRpcControllerError> {
        if let RpcCommand::GetEntries { since } = &request.command {
            self.session_sync
                .validate_request(since.as_deref(), self.limits)?;
            if let Some(active) = &self.session_sync_request {
                return Err(RunRpcControllerError::SessionSyncInFlight {
                    request_id: active.request_id.clone(),
                });
            }
        }

        self.gate.begin(request)?;

        if let Err(error) = self.pending.register(request, expires_at) {
            self.gate.finish(&request.id);
            return Err(error.into());
        }

        if matches!(request.command, RpcCommand::Bash { .. })
            && let Err(error) = self.live.start_direct_bash(request.id.clone())
        {
            self.pending.cancel(&request.id);
            self.gate.finish(&request.id);
            return Err(error.into());
        }

        if let RpcCommand::GetEntries { since } = &request.command {
            self.session_sync_request = Some(PendingSessionSyncRequest {
                request_id: request.id.clone(),
                since: since.clone(),
            });
        }

        Ok(())
    }

    /// Releases all controller-owned state for an outbound request that never
    /// reached Pi or that is being abandoned by a higher-level timeout owner.
    pub fn cancel_request(&mut self, id: &RequestId) -> CancelledRpcRequest {
        let session_sync_cancelled = self
            .session_sync_request
            .as_ref()
            .is_some_and(|request| request.request_id == *id);
        if session_sync_cancelled {
            self.session_sync_request = None;
        }
        CancelledRpcRequest {
            pending: self.pending.cancel(id),
            active: self.gate.finish(id),
            direct_bash_preview: self.live.cancel_direct_bash(id),
            session_sync_cancelled,
        }
    }

    /// Completes exact request correlation, releases command barriers, and
    /// applies authoritative `get_state` reconciliation when present.
    pub fn complete_response(
        &mut self,
        response: &RpcResponse,
        store: &mut RuntimeStore,
    ) -> Result<CompletedRpcRequest, RunRpcControllerError> {
        let pending = self.pending.complete(response)?;
        let active = self.gate.finish(&pending.id).ok_or_else(|| {
            RunRpcControllerError::MissingGateEntry {
                request_id: pending.id.clone(),
            }
        })?;

        let direct_bash_preview = if pending.command == "bash" {
            Some(self.live.finish_direct_bash(&pending.id)?)
        } else {
            None
        };

        let outcome = response.outcome();
        let mut capabilities_changed = false;
        let mut session_sync = None;
        if pending.command == "get_entries" {
            let request = self.take_session_sync_request(&pending.id)?;
            session_sync = if response.success {
                let page = response.entries_page(self.limits)?;
                let applied =
                    self.session_sync
                        .apply_page(request.since.as_deref(), &page, self.limits)?;
                Some(SessionSyncCompletion::Page { page, applied })
            } else if let Some(rejected_since) = request.since {
                let resync = self
                    .session_sync
                    .mark_resync_required(&rejected_since, self.limits)?;
                Some(SessionSyncCompletion::ResyncRequired {
                    resync,
                    error: response.error.clone(),
                })
            } else {
                Some(SessionSyncCompletion::Rejected {
                    error: response.error.clone(),
                })
            };
        }
        if outcome == RpcResponseOutcome::Accepted {
            match pending.command {
                "get_state" => {
                    let snapshot = response.state_snapshot(self.limits)?;
                    store.apply(
                        self.run_id,
                        RunMutation::StateObserved(RunStateObservation {
                            model: snapshot.model.map(|model| RunModelState {
                                provider: model.provider,
                                id: model.id,
                                name: model.name,
                                supports_images: model.supports_images,
                            }),
                            thinking_level: snapshot.thinking_level,
                            is_streaming: snapshot.is_streaming,
                            is_compacting: snapshot.is_compacting,
                            steering_mode: snapshot.steering_mode,
                            follow_up_mode: snapshot.follow_up_mode,
                            session_file: snapshot.session_file,
                            session_id: snapshot.session_id,
                            session_name: snapshot.session_name,
                            auto_compaction_enabled: snapshot.auto_compaction_enabled,
                            message_count: snapshot.message_count,
                            pending_message_count: snapshot.pending_message_count,
                        }),
                    )?;
                }
                "get_available_models" => {
                    self.capabilities
                        .replace_models(response.available_models(self.limits)?);
                    capabilities_changed = true;
                }
                "get_available_thinking_levels" => {
                    self.capabilities
                        .replace_thinking_levels(response.available_thinking_levels(self.limits)?);
                    capabilities_changed = true;
                }
                "get_commands" => {
                    self.capabilities
                        .replace_commands(response.available_commands(self.limits)?);
                    capabilities_changed = true;
                }
                "new_session" | "switch_session" | "fork" | "clone" => {
                    self.session_sync.reset_for_session_replacement();
                }
                _ => {}
            }
        }

        Ok(CompletedRpcRequest {
            pending,
            active,
            outcome,
            direct_bash_preview,
            capabilities_changed,
            session_sync,
        })
    }

    /// Applies one typed Pi event to the per-run hot projection and runtime
    /// store. Unknown/optional event kinds remain forward-compatible and do not
    /// manufacture state transitions.
    pub fn apply_event(
        &mut self,
        event: &RpcEvent,
        store: &mut RuntimeStore,
    ) -> Result<RunRpcEffect, RunRpcControllerError> {
        let effect = match event.kind {
            RpcEventKind::AgentStart => {
                self.live.clear_assistant_message();
                store.apply(self.run_id, RunMutation::AgentStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::AgentSettled => {
                store.apply(self.run_id, RunMutation::AgentSettled)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::CompactionStart => {
                store.apply(self.run_id, RunMutation::CompactionStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::CompactionEnd => {
                store.apply(self.run_id, RunMutation::CompactionEnded)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::QueueUpdate => {
                let counts = event
                    .queue_update_counts()?
                    .expect("queue_update parser is gated by event kind");
                store.apply(
                    self.run_id,
                    RunMutation::QueueChanged(QueueState {
                        steering: counts.steering,
                        follow_up: counts.follow_up,
                    }),
                )?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::SessionInfoChanged => {
                let info = event
                    .session_info_changed()?
                    .expect("session-info parser is gated by event kind");
                let retained_bytes = store
                    .get(self.run_id)
                    .ok_or(RuntimeError::UnknownRun {
                        run_id: self.run_id,
                    })?
                    .retained_runtime_state_bytes_with_name(info.name.as_deref());
                if retained_bytes > self.limits.max_runtime_state_bytes_per_run {
                    return Err(RpcResponsePayloadError::RuntimeStateByteLimit {
                        attempted: retained_bytes,
                        limit: self.limits.max_runtime_state_bytes_per_run,
                    }
                    .into());
                }
                store.apply(self.run_id, RunMutation::SessionNameChanged(info.name))?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::ThinkingLevelChanged => {
                let level = event
                    .thinking_level_changed()?
                    .expect("thinking parser is gated by event kind");
                store.apply(self.run_id, RunMutation::ThinkingLevelChanged(level))?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::MessageStart => {
                self.live.clear_assistant_message();
                RunRpcEffect::AssistantMessageReset
            }
            RpcEventKind::MessageUpdate => {
                let update = event
                    .assistant_message_update()?
                    .expect("message-update parser is gated by event kind");
                let content_index = match update {
                    AssistantMessageUpdate::Start {
                        content_index,
                        kind,
                        ..
                    } => {
                        self.live
                            .start_assistant_block(content_index, assistant_kind(kind))?;
                        content_index
                    }
                    AssistantMessageUpdate::Delta {
                        content_index,
                        kind,
                        delta,
                    } => {
                        self.live.append_assistant_delta(
                            content_index,
                            assistant_kind(kind),
                            &delta,
                        )?;
                        content_index
                    }
                    AssistantMessageUpdate::End {
                        content_index,
                        kind,
                        content,
                    } => {
                        self.live.finish_assistant_block(
                            content_index,
                            assistant_kind(kind),
                            content.as_deref(),
                        )?;
                        content_index
                    }
                    AssistantMessageUpdate::Unknown { .. } => {
                        return Ok(RunRpcEffect::ForwardCompatibleIgnored);
                    }
                };
                RunRpcEffect::AssistantBlockUpdated { content_index }
            }
            RpcEventKind::ToolExecutionStart => {
                let update = event
                    .tool_execution_start()?
                    .expect("tool-start parser is gated by event kind");
                let tool_call_id = update.tool_call_id;
                self.live
                    .start_tool(tool_call_id.clone(), update.tool_name)?;
                RunRpcEffect::ToolUpdated { tool_call_id }
            }
            RpcEventKind::ToolExecutionUpdate => {
                let update = event
                    .tool_execution_update()?
                    .expect("tool-update parser is gated by event kind");
                self.live
                    .replace_tool_output(&update.tool_call_id, &update.accumulated_text)?;
                RunRpcEffect::ToolUpdated {
                    tool_call_id: update.tool_call_id,
                }
            }
            RpcEventKind::ToolExecutionEnd => {
                let update = event
                    .tool_execution_end()?
                    .expect("tool-end parser is gated by event kind");
                self.live
                    .replace_tool_output(&update.tool_call_id, &update.final_text)?;
                let preview = self.live.finish_tool(&update.tool_call_id)?;
                RunRpcEffect::ToolFinished {
                    tool_call_id: update.tool_call_id,
                    tool_name: update.tool_name,
                    preview,
                    is_error: update.is_error,
                }
            }
            RpcEventKind::BashExecutionUpdate => {
                let update = event
                    .bash_execution_update()?
                    .expect("bash-update parser is gated by event kind");
                let request_id = update
                    .request_id
                    .ok_or(RunRpcControllerError::MissingBashRequestId)?;
                self.live
                    .append_direct_bash_delta(&request_id, &update.delta)?;
                RunRpcEffect::DirectBashUpdated { request_id }
            }
            RpcEventKind::ExtensionUiRequest => {
                let request = ExtensionUiRequest::parse_bounded(&event.raw, self.limits)?;
                self.apply_extension_ui_request(request, store, Instant::now())?
            }
            _ => RunRpcEffect::None,
        };

        Ok(effect)
    }

    #[must_use]
    pub const fn live_projection(&self) -> &LiveProjection {
        &self.live
    }

    #[must_use]
    pub fn pending_request_count(&self) -> usize {
        self.pending.len()
    }

    #[must_use]
    pub fn active_command_count(&self) -> usize {
        self.gate.len()
    }

    #[must_use]
    pub const fn capabilities(&self) -> &RunCapabilities {
        &self.capabilities
    }

    #[must_use]
    pub const fn extension_ui_state(&self) -> &ExtensionUiState {
        &self.extension_ui
    }

    #[must_use]
    pub const fn session_sync_state(&self) -> &SessionSyncState {
        &self.session_sync
    }

    pub fn seed_session_sync(
        &mut self,
        cursor: Option<String>,
        leaf_id: Option<String>,
    ) -> Result<(), RunRpcControllerError> {
        if let Some(active) = &self.session_sync_request {
            return Err(RunRpcControllerError::SessionSyncInFlight {
                request_id: active.request_id.clone(),
            });
        }
        self.session_sync.seed(cursor, leaf_id, self.limits)?;
        Ok(())
    }

    #[must_use]
    pub fn hydration_snapshot(&self, now: Instant) -> RunRpcHydrationSnapshot {
        let mut pending_dialogs: Vec<_> = self
            .pending_dialogs
            .values()
            .map(|pending| PendingExtensionDialogSnapshot {
                request: pending.request.clone(),
                remaining_timeout_ms: pending.expires_at.map(|deadline| {
                    deadline
                        .saturating_duration_since(now)
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64
                }),
            })
            .collect();
        pending_dialogs.sort_by(|left, right| left.request.id.cmp(&right.request.id));
        RunRpcHydrationSnapshot {
            run_id: self.run_id,
            capabilities: self.capabilities.clone(),
            session_sync: self.session_sync.clone(),
            live: self.live.snapshot(),
            extension_ui: self.extension_ui.snapshot(),
            pending_dialogs,
        }
    }

    pub fn pending_extension_dialogs(&self) -> impl Iterator<Item = &ExtensionDialogRequest> {
        self.pending_dialogs
            .values()
            .map(|pending| &pending.request)
    }

    #[must_use]
    pub fn next_extension_dialog_expiry(&self) -> Option<Instant> {
        self.pending_dialogs
            .values()
            .filter_map(|pending| pending.expires_at)
            .min()
    }

    /// Revokes all live transport-owned state after the child becomes terminal.
    /// Durable session/capability metadata remains available for inspection,
    /// but no pending request, dialog, tool, stream, or direct Bash preview may
    /// survive as if it were still actionable.
    pub fn process_ended(&mut self) {
        self.pending = PendingRequests::from_limits(self.limits);
        self.gate = RpcCommandGate::from_limits(self.limits);
        self.live = LiveProjection::new(self.limits);
        self.pending_dialogs.clear();
        self.pending_dialog_bytes = 0;
        self.session_sync_request = None;
    }

    /// Call only after an extension_ui_response was successfully written. The
    /// subprotocol has no acknowledgement response, so write success is the
    /// local completion boundary.
    pub fn complete_extension_ui_response(
        &mut self,
        request_id: &str,
        store: &mut RuntimeStore,
    ) -> Result<ExtensionDialogRequest, RunRpcControllerError> {
        let pending = self.pending_dialogs.remove(request_id).ok_or_else(|| {
            RunRpcControllerError::UnknownExtensionDialog {
                request_id: request_id.to_owned(),
            }
        })?;
        if let Err(error) = store.apply(
            self.run_id,
            RunMutation::UiRequestClosed {
                request_id: request_id.to_owned(),
            },
        ) {
            self.pending_dialogs.insert(request_id.to_owned(), pending);
            return Err(error.into());
        }
        self.pending_dialog_bytes = self
            .pending_dialog_bytes
            .saturating_sub(pending.request.resident_bytes());
        Ok(pending.request)
    }

    /// Removes locally actionable dialogs whose Pi-owned timeout has elapsed.
    /// The desktop adapter should schedule a one-shot expiry from the request
    /// timeout rather than poll.
    pub fn expire_extension_dialogs(
        &mut self,
        now: Instant,
        store: &mut RuntimeStore,
    ) -> Result<Vec<String>, RunRpcControllerError> {
        let expired: Vec<String> = self
            .pending_dialogs
            .iter()
            .filter(|(_, pending)| pending.expires_at.is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.complete_extension_ui_response(id, store)?;
        }
        Ok(expired)
    }

    fn apply_extension_ui_request(
        &mut self,
        request: ExtensionUiRequest,
        store: &mut RuntimeStore,
        now: Instant,
    ) -> Result<RunRpcEffect, RunRpcControllerError> {
        match request {
            ExtensionUiRequest::Dialog(request) => {
                let bytes = request.resident_bytes();
                let attempted = self.pending_dialog_bytes.saturating_add(bytes);
                if attempted > self.limits.max_extension_ui_bytes_per_run {
                    return Err(RunRpcControllerError::ExtensionDialogByteLimit {
                        attempted,
                        limit: self.limits.max_extension_ui_bytes_per_run,
                    });
                }
                store.apply(
                    self.run_id,
                    RunMutation::UiRequestOpened {
                        request_id: request.id.clone(),
                    },
                )?;
                let expires_at = request
                    .timeout_ms
                    .and_then(|timeout_ms| now.checked_add(Duration::from_millis(timeout_ms)));
                self.pending_dialog_bytes = attempted;
                self.pending_dialogs.insert(
                    request.id.clone(),
                    PendingExtensionDialog {
                        request: request.clone(),
                        expires_at,
                    },
                );
                Ok(RunRpcEffect::ExtensionDialogRequested(request))
            }
            ExtensionUiRequest::FireAndForget(action) => match action {
                ExtensionFireAndForget::Notify {
                    message,
                    notify_type,
                } => Ok(RunRpcEffect::ExtensionNotification {
                    message,
                    notify_type,
                }),
                ExtensionFireAndForget::SetStatus { key, text } => {
                    self.extension_ui.set_status(key, text)?;
                    Ok(RunRpcEffect::ExtensionUiStateChanged)
                }
                ExtensionFireAndForget::SetWidget {
                    key,
                    lines,
                    placement,
                } => {
                    let widget = lines.map(|lines| ExtensionWidget {
                        lines,
                        placement: match placement {
                            ExtensionWidgetPlacement::AboveEditor => WidgetPlacement::AboveEditor,
                            ExtensionWidgetPlacement::BelowEditor => WidgetPlacement::BelowEditor,
                        },
                    });
                    self.extension_ui.set_widget(key, widget)?;
                    Ok(RunRpcEffect::ExtensionUiStateChanged)
                }
                ExtensionFireAndForget::SetTitle { title } => {
                    self.extension_ui.set_title(Some(title))?;
                    Ok(RunRpcEffect::ExtensionUiStateChanged)
                }
                ExtensionFireAndForget::SetEditorText { text } => {
                    Ok(RunRpcEffect::SetEditorText { text })
                }
            },
            ExtensionUiRequest::Unknown(_) => Ok(RunRpcEffect::ForwardCompatibleIgnored),
        }
    }

    fn take_session_sync_request(
        &mut self,
        request_id: &RequestId,
    ) -> Result<PendingSessionSyncRequest, RunRpcControllerError> {
        let pending = self.session_sync_request.take().ok_or_else(|| {
            RunRpcControllerError::MissingSessionSyncRequest {
                request_id: request_id.clone(),
            }
        })?;
        if pending.request_id != *request_id {
            let actual = pending.request_id.clone();
            self.session_sync_request = Some(pending);
            return Err(RunRpcControllerError::SessionSyncResponseMismatch {
                expected: request_id.clone(),
                actual,
            });
        }
        Ok(pending)
    }
}

#[derive(Debug)]
struct PendingExtensionDialog {
    request: ExtensionDialogRequest,
    expires_at: Option<Instant>,
}

#[derive(Debug)]
struct PendingSessionSyncRequest {
    request_id: RequestId,
    since: Option<String>,
}

fn assistant_kind(kind: AssistantMessageBlockKind) -> AssistantContentKind {
    match kind {
        AssistantMessageBlockKind::Text => AssistantContentKind::Text,
        AssistantMessageBlockKind::Thinking => AssistantContentKind::Thinking,
        AssistantMessageBlockKind::ToolCall => AssistantContentKind::ToolCall,
    }
}

#[derive(Debug)]
pub struct CompletedRpcRequest {
    pub pending: PendingRequest,
    pub active: ActiveRpcCommand,
    pub outcome: RpcResponseOutcome,
    pub direct_bash_preview: Option<BoundedText>,
    pub capabilities_changed: bool,
    pub session_sync: Option<SessionSyncCompletion>,
}

#[derive(Debug)]
pub struct CancelledRpcRequest {
    pub pending: Option<PendingRequest>,
    pub active: Option<ActiveRpcCommand>,
    pub direct_bash_preview: Option<BoundedText>,
    pub session_sync_cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionSyncCompletion {
    Page {
        page: SessionEntriesPage,
        applied: SessionSyncApplied,
    },
    ResyncRequired {
        resync: SessionSyncResync,
        error: Option<String>,
    },
    Rejected {
        error: Option<String>,
    },
}

#[derive(Debug)]
pub enum RunRpcEffect {
    None,
    AssistantMessageReset,
    AssistantBlockUpdated {
        content_index: usize,
    },
    ToolUpdated {
        tool_call_id: String,
    },
    DirectBashUpdated {
        request_id: RequestId,
    },
    SemanticStateChanged,
    ForwardCompatibleIgnored,
    ToolFinished {
        tool_call_id: String,
        tool_name: String,
        preview: ToolPreview,
        is_error: bool,
    },
    ExtensionDialogRequested(ExtensionDialogRequest),
    ExtensionNotification {
        message: String,
        notify_type: ExtensionNotifyType,
    },
    ExtensionUiStateChanged,
    SetEditorText {
        text: String,
    },
}

#[derive(Debug, Error)]
pub enum RunRpcControllerError {
    #[error(transparent)]
    Gate(#[from] RpcGateError),
    #[error(transparent)]
    Pending(#[from] PendingRequestError),
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    EventPayload(#[from] RpcEventPayloadError),
    #[error(transparent)]
    ResponsePayload(#[from] RpcResponsePayloadError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("pending RPC request {request_id} had no matching active command gate entry")]
    MissingGateEntry { request_id: RequestId },
    #[error("direct bash stream update is missing its originating RPC request id")]
    MissingBashRequestId,
    #[error(transparent)]
    ExtensionUiParse(#[from] ExtensionUiParseError),
    #[error(transparent)]
    ExtensionUiState(#[from] ExtensionUiError),
    #[error(transparent)]
    SessionSync(#[from] SessionSyncError),
    #[error("extension dialog payloads would use {attempted} bytes, exceeding limit {limit}")]
    ExtensionDialogByteLimit { attempted: usize, limit: usize },
    #[error("extension dialog {request_id} is not pending")]
    UnknownExtensionDialog { request_id: String },
    #[error("get_entries request {request_id} is already in flight")]
    SessionSyncInFlight { request_id: RequestId },
    #[error("get_entries response {request_id} has no controller-owned synchronization request")]
    MissingSessionSyncRequest { request_id: RequestId },
    #[error("get_entries response {expected} crossed active synchronization request {actual}")]
    SessionSyncResponseMismatch {
        expected: RequestId,
        actual: RequestId,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::ProjectId;
    use crate::launch::ProjectTrustPolicy;
    use crate::rpc::{InboundMessage, parse_frame};
    use crate::runtime::{ActivityState, ExecutionIsolation, ProcessState, RunRecord};

    fn ready_store(run_id: RunId) -> RuntimeStore {
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store
            .register(
                RunRecord::starting(
                    run_id,
                    ProjectId::new(),
                    PathBuf::from("project"),
                    ExecutionIsolation::LocalCheckout,
                    ProjectTrustPolicy::Ignore,
                )
                .expect("local run"),
            )
            .expect("register");
        store
            .apply(run_id, RunMutation::ProcessReady)
            .expect("ready");
        store
    }

    #[test]
    fn extension_dialog_survives_projection_until_exact_response() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        let effect = controller
            .apply_event(
                &event(br#"{"type":"extension_ui_request","id":"confirm-1","method":"confirm","title":"Continue?","message":"Proceed","timeout":1000}"#),
                &mut store,
            )
            .expect("dialog");
        assert!(matches!(effect, RunRpcEffect::ExtensionDialogRequested(_)));
        assert_eq!(store.get(run_id).expect("run").pending_ui_requests(), 1);
        assert_eq!(controller.pending_extension_dialogs().count(), 1);

        let resolved = controller
            .complete_extension_ui_response("confirm-1", &mut store)
            .expect("resolve exact dialog");
        assert_eq!(resolved.id, "confirm-1");
        assert_eq!(store.get(run_id).expect("run").pending_ui_requests(), 0);
    }

    #[test]
    fn extension_fire_and_forget_updates_state_without_pending_dialog() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        controller
            .apply_event(
                &event(br#"{"type":"extension_ui_request","id":"status-1","method":"setStatus","statusKey":"build","statusText":"running"}"#),
                &mut store,
            )
            .expect("status");

        assert_eq!(store.get(run_id).expect("run").pending_ui_requests(), 0);
        assert_eq!(controller.extension_ui_state().status_count(), 1);
    }

    #[test]
    fn timed_extension_dialog_can_be_expired_without_polling_semantics() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        let before = Instant::now();
        controller
            .apply_event(
                &event(br#"{"type":"extension_ui_request","id":"input-1","method":"input","title":"Value","timeout":1}"#),
                &mut store,
            )
            .expect("dialog");
        let expired = controller
            .expire_extension_dialogs(before + Duration::from_secs(1), &mut store)
            .expect("expire");
        assert_eq!(expired, ["input-1"]);
        assert_eq!(controller.pending_extension_dialogs().count(), 0);
    }

    #[test]
    fn session_name_event_cannot_bypass_retained_runtime_state_budget() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let limits = RuntimeLimits {
            max_runtime_state_bytes_per_run: 8,
            ..RuntimeLimits::default()
        };
        let mut controller = RunRpcController::new(run_id, limits);
        assert!(matches!(
            controller.apply_event(
                &event(br#"{"type":"session_info_changed","name":"0123456789"}"#),
                &mut store,
            ),
            Err(RunRpcControllerError::ResponsePayload(
                RpcResponsePayloadError::RuntimeStateByteLimit { limit: 8, .. }
            ))
        ));
        assert!(
            store
                .get(run_id)
                .expect("run")
                .session_state()
                .session_name
                .is_none()
        );
    }

    #[test]
    fn capability_responses_replace_revisioned_backend_projection() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        let request = RpcRequest::with_id(
            RequestId::from_wire("models"),
            RpcCommand::GetAvailableModels,
        );
        controller
            .begin_request(&request, None)
            .expect("begin models");
        let completed = controller
            .complete_response(
                &response(
                    "models",
                    "get_available_models",
                    json!({"models":[{"provider":"openai","id":"gpt-5.6","name":"GPT-5.6","input":["text","image"]}]}),
                ),
                &mut store,
            )
            .expect("models response");

        assert!(completed.capabilities_changed);
        assert_eq!(controller.capabilities().revision(), 1);
        assert_eq!(
            controller.capabilities().models().expect("models")[0].id,
            "gpt-5.6"
        );
        assert_eq!(
            controller.capabilities().models().expect("models")[0].supports_images,
            Some(true)
        );
    }

    fn event(value: &[u8]) -> RpcEvent {
        let InboundMessage::Event(event) = parse_frame(value).expect("event fixture") else {
            panic!("expected event");
        };
        event
    }

    fn response(id: &str, command: &str, data: serde_json::Value) -> RpcResponse {
        RpcResponse {
            id: Some(id.to_owned()),
            command: command.to_owned(),
            success: true,
            data: Some(data),
            error: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn writer_failure_cancellation_releases_all_request_owners() {
        let run_id = RunId::new();
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        let request = RpcRequest::with_id(
            RequestId::from_wire("bash-1"),
            RpcCommand::Bash {
                command: "pwd".to_owned(),
                exclude_from_context: None,
            },
        );
        controller.begin_request(&request, None).expect("begin");
        assert_eq!(controller.pending_request_count(), 1);
        assert_eq!(controller.active_command_count(), 1);
        assert_eq!(controller.live_projection().active_direct_bash_count(), 1);

        let cancelled = controller.cancel_request(&request.id);
        assert!(cancelled.pending.is_some());
        assert!(cancelled.active.is_some());
        assert!(cancelled.direct_bash_preview.is_some());
        assert!(!cancelled.session_sync_cancelled);
        assert_eq!(controller.pending_request_count(), 0);
        assert_eq!(controller.active_command_count(), 0);
        assert_eq!(controller.live_projection().active_direct_bash_count(), 0);
    }

    #[test]
    fn get_entries_advances_exact_cursor_and_exposes_bounded_page_without_retaining_history() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        controller
            .seed_session_sync(Some("a".to_owned()), Some("a".to_owned()))
            .expect("seed cursor");
        let request = RpcRequest::with_id(
            RequestId::from_wire("entries-1"),
            RpcCommand::GetEntries {
                since: Some("a".to_owned()),
            },
        );
        controller
            .begin_request(&request, None)
            .expect("begin entries");

        let completed = controller
            .complete_response(
                &response(
                    "entries-1",
                    "get_entries",
                    json!({
                        "entries":[
                            {"type":"message","id":"b","parentId":"a","message":{"role":"user","content":"one"}},
                            {"type":"message","id":"c","parentId":"b","message":{"role":"assistant","content":[]}}
                        ],
                        "leafId":"b"
                    }),
                ),
                &mut store,
            )
            .expect("entries response");

        let Some(SessionSyncCompletion::Page { page, applied }) = completed.session_sync else {
            panic!("expected session page");
        };
        assert_eq!(page.entries.len(), 2);
        assert_eq!(applied.appended_entries, 2);
        assert_eq!(controller.session_sync_state().cursor(), Some("c"));
        assert_eq!(controller.session_sync_state().leaf_id(), Some("b"));
    }

    #[test]
    fn rejected_incremental_cursor_requires_explicit_resync_instead_of_full_message_fallback() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        controller
            .seed_session_sync(Some("gone".to_owned()), None)
            .expect("seed");
        let request = RpcRequest::with_id(
            RequestId::from_wire("entries-stale"),
            RpcCommand::GetEntries {
                since: Some("gone".to_owned()),
            },
        );
        controller.begin_request(&request, None).expect("begin");
        let rejected = RpcResponse {
            id: Some("entries-stale".to_owned()),
            command: "get_entries".to_owned(),
            success: false,
            data: None,
            error: Some("since entry not found".to_owned()),
            extra: BTreeMap::new(),
        };
        let completed = controller
            .complete_response(&rejected, &mut store)
            .expect("classified rejection");

        assert!(matches!(
            completed.session_sync,
            Some(SessionSyncCompletion::ResyncRequired { .. })
        ));
        assert!(controller.session_sync_state().resync_required());
        assert!(matches!(
            controller.begin_request(
                &RpcRequest::with_id(
                    RequestId::from_wire("entries-again"),
                    RpcCommand::GetEntries {
                        since: Some("gone".to_owned())
                    },
                ),
                None,
            ),
            Err(RunRpcControllerError::SessionSync(
                SessionSyncError::ResyncRequired
            ))
        ));
    }

    #[test]
    fn concurrent_get_entries_is_rejected_before_cross_response_can_exist() {
        let run_id = RunId::new();
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        let first = RpcRequest::with_id(
            RequestId::from_wire("entries-1"),
            RpcCommand::GetEntries { since: None },
        );
        let second = RpcRequest::with_id(
            RequestId::from_wire("entries-2"),
            RpcCommand::GetEntries { since: None },
        );
        controller.begin_request(&first, None).expect("first");
        assert!(matches!(
            controller.begin_request(&second, None),
            Err(RunRpcControllerError::SessionSyncInFlight {
                request_id
            })
                if request_id == RequestId::from_wire("entries-1")
        ));
        let cancelled = controller.cancel_request(&first.id);
        assert!(cancelled.session_sync_cancelled);
    }

    #[test]
    fn direct_bash_stream_and_response_are_correlated_to_exact_request() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let limits = RuntimeLimits {
            max_tool_preview_bytes: 6,
            ..RuntimeLimits::default()
        };
        let mut controller = RunRpcController::new(run_id, limits);
        let request = RpcRequest::with_id(
            RequestId::from_wire("bash-7"),
            RpcCommand::Bash {
                command: "long command".to_owned(),
                exclude_from_context: Some(true),
            },
        );
        controller
            .begin_request(&request, None)
            .expect("begin bash");
        controller
            .apply_event(
                &event(br#"{"type":"bash_execution_update","id":"bash-7","delta":"0123456789"}"#),
                &mut store,
            )
            .expect("bash update");

        let completed = controller
            .complete_response(
                &response(
                    "bash-7",
                    "bash",
                    json!({"output":"tail","exitCode":0,"cancelled":false,"truncated":true}),
                ),
                &mut store,
            )
            .expect("bash response");
        assert_eq!(completed.outcome, RpcResponseOutcome::Accepted);
        assert_eq!(
            completed
                .direct_bash_preview
                .expect("stream preview")
                .as_str(),
            "456789"
        );
        assert_eq!(controller.pending_request_count(), 0);
    }

    #[test]
    fn get_state_response_reconciles_runtime_after_hydration_gap() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        let request = RpcRequest::with_id(RequestId::from_wire("state"), RpcCommand::GetState);
        controller
            .begin_request(&request, None)
            .expect("begin state");
        controller
            .complete_response(
                &response(
                    "state",
                    "get_state",
                    json!({
                        "model":{"provider":"openai","id":"gpt-5.6","name":"GPT-5.6"},
                        "thinkingLevel":"high",
                        "isStreaming":true,
                        "isCompacting":false,
                        "steeringMode":"all",
                        "followUpMode":"one-at-a-time",
                        "sessionFile":"session.jsonl",
                        "sessionId":"session-9",
                        "sessionName":"resume me",
                        "autoCompactionEnabled":true,
                        "messageCount":99,
                        "pendingMessageCount":1
                    }),
                ),
                &mut store,
            )
            .expect("state response");

        let run = store.get(run_id).expect("run");
        assert_eq!(run.process_state(), ProcessState::Ready);
        assert_eq!(run.activity_state(), ActivityState::Working);
        assert_eq!(run.session_state().session_id.as_deref(), Some("session-9"));
        assert_eq!(run.session_state().message_count, Some(99));
    }

    #[test]
    fn event_projection_preserves_accumulated_tool_output_and_semantic_state() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        controller
            .apply_event(&event(br#"{"type":"agent_start"}"#), &mut store)
            .expect("agent start");
        assert_eq!(
            store.get(run_id).expect("run").activity_state(),
            ActivityState::Working
        );

        controller
            .apply_event(
                &event(br#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"bash","args":{}}"#),
                &mut store,
            )
            .expect("tool start");
        controller
            .apply_event(
                &event(br#"{"type":"tool_execution_update","toolCallId":"call-1","toolName":"bash","partialResult":{"content":[{"type":"text","text":"first"}]}}"#),
                &mut store,
            )
            .expect("tool update");
        controller
            .apply_event(
                &event(br#"{"type":"tool_execution_update","toolCallId":"call-1","toolName":"bash","partialResult":{"content":[{"type":"text","text":"accumulated"}]}}"#),
                &mut store,
            )
            .expect("tool replacement");
        let effect = controller
            .apply_event(
                &event(br#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"bash","result":{"content":[{"type":"text","text":"final"}]},"isError":false}"#),
                &mut store,
            )
            .expect("tool end");
        let RunRpcEffect::ToolFinished { preview, .. } = effect else {
            panic!("expected finished tool");
        };
        assert_eq!(preview.output.as_str(), "final");

        controller
            .apply_event(
                &event(br#"{"type":"queue_update","steering":["one"],"followUp":["two","three"]}"#),
                &mut store,
            )
            .expect("queue update");
        assert_eq!(
            store.get(run_id).expect("run").queue(),
            QueueState {
                steering: 1,
                follow_up: 2
            }
        );
        controller
            .apply_event(&event(br#"{"type":"agent_settled"}"#), &mut store)
            .expect("settled");
        assert_eq!(
            store.get(run_id).expect("run").activity_state(),
            ActivityState::Idle
        );
    }

    #[test]
    fn missing_direct_bash_request_id_never_falls_back_to_oldest_request() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());

        assert!(matches!(
            controller.apply_event(
                &event(br#"{"type":"bash_execution_update","delta":"uncorrelated"}"#),
                &mut store,
            ),
            Err(RunRpcControllerError::MissingBashRequestId)
        ));
    }
}
