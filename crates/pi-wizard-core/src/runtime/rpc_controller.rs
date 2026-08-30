use std::collections::HashMap;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::bounded::BoundedText;
use crate::rpc::{
    ActiveRpcCommand, AssistantMessageBlockKind, AssistantMessageUpdate, ClearQueueResult,
    ExtensionDialogRequest, ExtensionFireAndForget, ExtensionNotifyType, ExtensionUiParseError,
    ExtensionUiRequest, ExtensionWidgetPlacement, PendingRequest, PendingRequestError,
    PendingRequests, RpcCommand, RpcCommandGate, RpcEvent, RpcEventKind, RpcEventPayloadError,
    RpcGateError, RpcRequest, RpcResponse, RpcResponseOutcome, RpcResponsePayloadError,
    SessionEntriesPage,
};
use crate::{RequestId, RunId, RuntimeLimits};

use super::{
    AssistantContentKind, ExtensionUiError, ExtensionUiState, ExtensionWidget, LiveProjection,
    PendingExtensionDialogSnapshot, ProjectionError, QueueState, RunCapabilities,
    RunCompactionSnapshot, RunExtensionErrorSnapshot, RunModelState, RunMutation, RunRetrySnapshot,
    RunRpcHydrationSnapshot, RunStateObservation, RunSummarizationRetrySnapshot, RuntimeError,
    RuntimeStore, SessionSyncApplied, SessionSyncError, SessionSyncState, ToolPreview,
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
    compaction: Option<RunCompactionSnapshot>,
    retry: Option<RunRetrySnapshot>,
    summarization_retry: Option<RunSummarizationRetrySnapshot>,
    last_extension_error: Option<RunExtensionErrorSnapshot>,
    stream_stalled: bool,
    observed_queue_recovery: ClearQueueResult,
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
            compaction: None,
            retry: None,
            summarization_retry: None,
            last_extension_error: None,
            stream_stalled: false,
            observed_queue_recovery: ClearQueueResult::default(),
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

        if pending.command == "bash" {
            // A shell command may have mutated the execution root even when
            // its eventual response is rejected/cancelled or reports failure.
            store.apply(self.run_id, RunMutation::ChangesMayHaveChanged)?;
        }

        let outcome = response.outcome();
        let mut capabilities_changed = false;
        let mut session_sync = None;
        if pending.command == "get_entries" {
            let request = self.take_session_sync_request(&pending.id)?;
            session_sync = if response.success {
                match response.entries_page(self.limits) {
                    Ok(page) => {
                        let applied = self.session_sync.apply_page(
                            request.since.as_deref(),
                            &page,
                            self.limits,
                        )?;
                        Some(SessionSyncCompletion::Page { page, applied })
                    }
                    Err(
                        error @ (RpcResponsePayloadError::SessionEntryByteLimit { .. }
                        | RpcResponsePayloadError::SessionEntryLimit { .. }),
                    ) => {
                        // A successful Pi response can legitimately contain more
                        // incremental history than Pi Wizard retains in one hot
                        // page. Fall back to bounded file-backed history and
                        // reseed the cursor instead of killing a healthy process.
                        let revision = self.session_sync.mark_projection_resync_required();
                        Some(SessionSyncCompletion::ResyncRequired {
                            revision,
                            error: Some(error.to_string()),
                        })
                    }
                    Err(error) => return Err(error.into()),
                }
            } else if let Some(rejected_since) = request.since {
                let resync = self
                    .session_sync
                    .mark_resync_required(&rejected_since, self.limits)?;
                Some(SessionSyncCompletion::ResyncRequired {
                    revision: resync.revision,
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
                    store.apply(self.run_id, RunMutation::SessionReplacementAccepted)?;
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
                self.live.start_agent_turn();
                if self
                    .compaction
                    .as_ref()
                    .is_some_and(|compaction| compaction.finished && compaction.will_retry)
                {
                    self.compaction = None;
                }
                if self.retry.as_ref().is_some_and(|retry| retry.finished) {
                    self.retry = None;
                } else if let Some(retry) = self.retry.as_mut() {
                    retry.waiting = false;
                }
                store.apply(self.run_id, RunMutation::AgentStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::AgentSettled => {
                store.apply(self.run_id, RunMutation::AgentSettled)?;
                if self.live.settle_agent_turn() > 0 {
                    // Missing tool-end display traffic must not leave Git review
                    // looking current after Pi has authoritatively settled the
                    // turn. This is deliberately conservative because tool
                    // semantics are extension-owned and may have mutated files.
                    store.apply(self.run_id, RunMutation::ChangesMayHaveChanged)?;
                }
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::CompactionStart => {
                let started = event
                    .compaction_start()?
                    .expect("compaction-start parser is gated by event kind");
                let (reason, reason_truncated) = self.bound_detail(&started.reason);
                self.compaction = Some(RunCompactionSnapshot {
                    reason,
                    reason_truncated,
                    finished: false,
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                    error_truncated: false,
                });
                store.apply(self.run_id, RunMutation::CompactionStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::CompactionEnd => {
                let ended = event
                    .compaction_end()?
                    .expect("compaction-end parser is gated by event kind");
                let (reason, reason_truncated) = self.bound_detail(&ended.reason);
                let (error_message, error_truncated) = ended
                    .error_message
                    .as_deref()
                    .map(|value| self.bound_detail(value))
                    .map_or((None, false), |(value, truncated)| (Some(value), truncated));
                self.compaction = Some(RunCompactionSnapshot {
                    reason,
                    reason_truncated,
                    finished: true,
                    aborted: ended.aborted,
                    will_retry: ended.will_retry,
                    error_message,
                    error_truncated,
                });
                store.apply(self.run_id, RunMutation::CompactionEnded)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::AutoRetryStart => {
                let retry = event
                    .auto_retry_start()?
                    .expect("auto-retry parser is gated by event kind");
                let (error_message, error_truncated) = self.bound_detail(&retry.error_message);
                self.retry = Some(RunRetrySnapshot {
                    attempt: retry.attempt,
                    max_attempts: retry.max_attempts,
                    delay_ms: retry.delay_ms,
                    error_message,
                    error_truncated,
                    waiting: true,
                    finished: false,
                    success: None,
                    final_error: None,
                    final_error_truncated: false,
                });
                store.apply(self.run_id, RunMutation::AutoRetryStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::AutoRetryEnd => {
                let ended = event
                    .auto_retry_end()?
                    .expect("auto-retry-end parser is gated by event kind");
                let (final_error, final_error_truncated) = ended
                    .final_error
                    .as_deref()
                    .map(|value| self.bound_detail(value))
                    .map_or((None, false), |(value, truncated)| (Some(value), truncated));
                let retry = self.retry.get_or_insert_with(|| RunRetrySnapshot {
                    attempt: ended.attempt,
                    max_attempts: ended.attempt,
                    delay_ms: 0,
                    error_message: String::new(),
                    error_truncated: false,
                    waiting: false,
                    finished: false,
                    success: None,
                    final_error: None,
                    final_error_truncated: false,
                });
                retry.attempt = ended.attempt;
                retry.waiting = false;
                retry.finished = true;
                retry.success = Some(ended.success);
                retry.final_error = final_error;
                retry.final_error_truncated = final_error_truncated;
                store.apply(self.run_id, RunMutation::AutoRetryEnded)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::SummarizationRetryScheduled => {
                let retry = event
                    .summarization_retry_scheduled()?
                    .expect("summarization-retry parser is gated by event kind");
                let (error_message, error_truncated) = self.bound_detail(&retry.error_message);
                self.summarization_retry = Some(RunSummarizationRetrySnapshot {
                    attempt: retry.attempt,
                    max_attempts: retry.max_attempts,
                    delay_ms: retry.delay_ms,
                    error_message,
                    error_truncated,
                    source: None,
                    reason: None,
                    finished: false,
                });
                store.apply(self.run_id, RunMutation::SummarizationRetryStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::SummarizationRetryAttemptStart => {
                let attempt = event
                    .summarization_retry_attempt_start()?
                    .expect("summarization-attempt parser is gated by event kind");
                let (source, source_truncated) = self.bound_detail(&attempt.source);
                let (reason, reason_truncated) = attempt
                    .reason
                    .as_deref()
                    .map(|value| self.bound_detail(value))
                    .map_or((None, false), |(value, truncated)| (Some(value), truncated));
                let retry =
                    self.summarization_retry
                        .get_or_insert_with(|| RunSummarizationRetrySnapshot {
                            attempt: 0,
                            max_attempts: 0,
                            delay_ms: 0,
                            error_message: String::new(),
                            error_truncated: false,
                            source: None,
                            reason: None,
                            finished: false,
                        });
                retry.source = Some(source);
                retry.reason = reason;
                retry.error_truncated |= source_truncated || reason_truncated;
                retry.finished = false;
                store.apply(self.run_id, RunMutation::SummarizationRetryStarted)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::SummarizationRetryFinished => {
                if let Some(retry) = self.summarization_retry.as_mut() {
                    retry.finished = true;
                }
                store.apply(self.run_id, RunMutation::SummarizationRetryEnded)?;
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::ExtensionError => {
                let error = event
                    .extension_error()?
                    .expect("extension-error parser is gated by event kind");
                let (extension_path, path_truncated) = self.bound_detail(&error.extension_path);
                let (event_name, event_truncated) = self.bound_detail(&error.event);
                let (detail, detail_truncated) = self.bound_detail(&error.error);
                self.last_extension_error = Some(RunExtensionErrorSnapshot {
                    extension_path,
                    event: event_name,
                    error: detail,
                    detail_truncated: path_truncated || event_truncated || detail_truncated,
                });
                RunRpcEffect::SemanticStateChanged
            }
            RpcEventKind::QueueUpdate => {
                let observed = event
                    .queue_update_recovery(self.limits)?
                    .expect("queue_update parser is gated by event kind");
                store.apply(
                    self.run_id,
                    RunMutation::QueueChanged(QueueState {
                        steering: observed.steering.len(),
                        follow_up: observed.follow_up.len(),
                    }),
                )?;
                self.observed_queue_recovery = observed;
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
                self.live.start_assistant_message();
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
            RpcEventKind::MessageEnd => {
                let Some(message) = event.assistant_message_end()? else {
                    return Ok(RunRpcEffect::None);
                };
                let error_message = message
                    .error_message
                    .as_deref()
                    .map(|error| self.bound_detail(error).0);
                self.live
                    .reconcile_assistant_message(message.blocks.into_iter().map(|block| {
                        (
                            block.content_index,
                            assistant_kind(block.kind),
                            block.content,
                        )
                    }))?;
                store.apply(
                    self.run_id,
                    RunMutation::AssistantMessageCompleted {
                        stop_reason: message.stop_reason,
                        error_message,
                    },
                )?;
                RunRpcEffect::SemanticStateChanged
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
                // Tool semantics are Pi-owned and extensions may define tools,
                // so do not guess which names mutate files. Any completed tool
                // is a conservative repository-review invalidation boundary.
                store.apply(self.run_id, RunMutation::ChangesMayHaveChanged)?;
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
            compaction: self.compaction.clone(),
            retry: self.retry.clone(),
            summarization_retry: self.summarization_retry.clone(),
            last_extension_error: self.last_extension_error.clone(),
            stream_stalled: self.stream_stalled,
        }
    }

    pub fn pending_extension_dialogs(&self) -> impl Iterator<Item = &ExtensionDialogRequest> {
        self.pending_dialogs
            .values()
            .map(|pending| &pending.request)
    }

    #[must_use]
    pub fn has_pending_extension_dialogs(&self) -> bool {
        !self.pending_dialogs.is_empty()
    }

    /// Private bounded copy of the most recent user-visible Pi queue event.
    /// It is intentionally excluded from hydration and is used only when Pi
    /// explicitly rejects native `clear_queue`, after which Stop terminates
    /// the exact process so queued work cannot continue.
    #[must_use]
    pub fn observed_queue_recovery(&self) -> &ClearQueueResult {
        &self.observed_queue_recovery
    }

    /// Sets a local, non-authoritative quiet-stream advisory. This bit is
    /// presentation/recovery guidance only and never changes Pi commands or
    /// process lifecycle state.
    pub fn set_stream_stalled(&mut self, stalled: bool) -> bool {
        if self.stream_stalled == stalled {
            return false;
        }
        self.stream_stalled = stalled;
        true
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
        self.compaction = None;
        self.retry = None;
        self.summarization_retry = None;
        self.stream_stalled = false;
        self.observed_queue_recovery = ClearQueueResult::default();
    }

    fn bound_detail(&self, value: &str) -> (String, bool) {
        let mut bounded = BoundedText::new(self.limits.max_failure_detail_bytes);
        bounded.replace(value);
        (bounded.as_str().to_owned(), bounded.dropped_bytes() > 0)
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
        revision: u64,
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
    fn oversized_successful_get_entries_page_requests_cold_resync_instead_of_failing_protocol() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let limits = RuntimeLimits::default();
        let mut controller = RunRpcController::new(run_id, limits);
        controller
            .seed_session_sync(Some("a".to_owned()), Some("a".to_owned()))
            .expect("seed cursor");
        let request = RpcRequest::with_id(
            RequestId::from_wire("entries-large"),
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
                    "entries-large",
                    "get_entries",
                    json!({
                        "entries":[{
                            "type":"message",
                            "id":"b",
                            "parentId":"a",
                            "message":{"role":"assistant","content":"x".repeat(limits.max_session_entry_page_bytes)}
                        }],
                        "leafId":"b"
                    }),
                ),
                &mut store,
            )
            .expect("oversized app projection is recoverable");

        let Some(SessionSyncCompletion::ResyncRequired { revision, error }) =
            completed.session_sync
        else {
            panic!("expected bounded resync request");
        };
        assert_eq!(revision, 2);
        assert!(error.is_some_and(|detail| detail.contains("524288")));
        assert!(controller.session_sync_state().resync_required());
        assert_eq!(controller.session_sync_state().cursor(), Some("a"));
        assert_eq!(
            store.get(run_id).expect("run").process_state(),
            ProcessState::Ready
        );
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
        assert_eq!(store.get(run_id).expect("run").change_revision(), 1);
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
    fn message_end_reconciles_final_assistant_content_over_stream_preview() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());

        controller
            .apply_event(&event(br#"{"type":"message_start"}"#), &mut store)
            .expect("message start");
        controller
            .apply_event(
                &event(br#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#),
                &mut store,
            )
            .expect("text start");
        controller
            .apply_event(
                &event(br#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"stale preview"}}"#),
                &mut store,
            )
            .expect("text delta");

        let effect = controller
            .apply_event(
                &event(br#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"final answer"},{"type":"thinking","thinking":"final reasoning"}]}}"#),
                &mut store,
            )
            .expect("message end");
        assert!(matches!(effect, RunRpcEffect::SemanticStateChanged));
        assert_eq!(
            store
                .get(run_id)
                .expect("run")
                .assistant_message_generation(),
            1
        );
        assert_eq!(
            store.get(run_id).expect("run").last_assistant_stop_reason(),
            Some(&crate::rpc::AssistantStopReason::Stop)
        );
        assert!(
            serde_json::to_value(store.get(run_id).expect("run"))
                .expect("serialize run")
                .get("assistantMessageGeneration")
                .is_none(),
            "orchestration generation is backend-only and must not expand renderer hydration"
        );
        let snapshot = controller.live_projection().snapshot();
        assert_eq!(snapshot.assistant_blocks.len(), 2);
        assert_eq!(snapshot.assistant_blocks[0].text, "final answer");
        assert_eq!(
            snapshot.assistant_blocks[0].kind,
            AssistantContentKind::Text
        );
        assert!(snapshot.assistant_blocks[0].complete);
        assert_eq!(snapshot.assistant_blocks[1].text, "final reasoning");
        assert_eq!(
            snapshot.assistant_blocks[1].kind,
            AssistantContentKind::Thinking
        );
        assert!(snapshot.assistant_blocks[1].complete);
        assert_eq!(snapshot.reasoning, "final reasoning");

        controller
            .apply_event(&event(br#"{"type":"message_start"}"#), &mut store)
            .expect("next assistant message");
        let carried = controller.live_projection().snapshot();
        assert_eq!(carried.reasoning, "final reasoning");
        assert!(carried.assistant_blocks.is_empty());

        controller
            .apply_event(
                &event(br#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}"#),
                &mut store,
            )
            .expect("next thinking start");
        controller
            .apply_event(
                &event(br#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"next step"}}"#),
                &mut store,
            )
            .expect("next thinking delta");
        assert_eq!(
            controller.live_projection().snapshot().reasoning,
            "final reasoning\n\nnext step"
        );

        controller
            .apply_event(&event(br#"{"type":"agent_start"}"#), &mut store)
            .expect("new agent turn");
        assert!(controller.live_projection().snapshot().reasoning.is_empty());
    }

    #[test]
    fn tool_result_message_end_does_not_advance_assistant_generation_and_settlement_is_separate() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        controller
            .apply_event(&event(br#"{"type":"agent_start"}"#), &mut store)
            .expect("agent start");
        controller
            .apply_event(
                &event(br#"{"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"README.md"}}]}}"#),
                &mut store,
            )
            .expect("assistant tool-use message");
        assert_eq!(
            store
                .get(run_id)
                .expect("run")
                .assistant_message_generation(),
            1
        );
        assert_eq!(
            store.get(run_id).expect("run").agent_settled_generation(),
            0
        );

        let effect = controller
            .apply_event(
                &event(br#"{"type":"message_end","message":{"role":"toolResult","toolCallId":"call-1","toolName":"read","content":[{"type":"text","text":"done"}],"isError":false}}"#),
                &mut store,
            )
            .expect("tool result message");
        assert!(matches!(effect, RunRpcEffect::None));
        assert_eq!(
            store
                .get(run_id)
                .expect("run")
                .assistant_message_generation(),
            1
        );
        assert_eq!(
            store.get(run_id).expect("run").agent_settled_generation(),
            0
        );

        controller
            .apply_event(&event(br#"{"type":"agent_settled"}"#), &mut store)
            .expect("agent settled");
        assert_eq!(
            store.get(run_id).expect("run").agent_settled_generation(),
            1
        );
        assert_eq!(
            store.get(run_id).expect("run").activity_state(),
            ActivityState::Idle
        );
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
        assert_eq!(store.get(run_id).expect("run").change_revision(), 1);

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
        assert_eq!(controller.observed_queue_recovery().steering, ["one"]);
        assert_eq!(
            controller.observed_queue_recovery().follow_up,
            ["two", "three"]
        );
        assert_eq!(
            serde_json::to_value(controller.hydration_snapshot(Instant::now()))
                .expect("serialize hydration")
                .get("observedQueueRecovery"),
            None,
            "emergency queue text must never enter renderer hydration"
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
    fn agent_settled_discards_unfinished_tool_preview_and_invalidates_review() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let mut controller = RunRpcController::new(run_id, RuntimeLimits::default());
        controller
            .apply_event(&event(br#"{"type":"agent_start"}"#), &mut store)
            .expect("agent start");
        controller
            .apply_event(
                &event(br#"{"type":"tool_execution_start","toolCallId":"orphan-tool","toolName":"write","args":{}}"#),
                &mut store,
            )
            .expect("tool start");
        controller
            .apply_event(
                &event(br#"{"type":"tool_execution_update","toolCallId":"orphan-tool","toolName":"write","partialResult":{"content":[{"type":"text","text":"partial"}]}}"#),
                &mut store,
            )
            .expect("tool update");
        assert_eq!(controller.live_projection().active_tool_count(), 1);
        assert_eq!(store.get(run_id).expect("run").change_revision(), 0);

        controller
            .apply_event(&event(br#"{"type":"agent_settled"}"#), &mut store)
            .expect("agent settled");

        assert_eq!(controller.live_projection().active_tool_count(), 0);
        assert_eq!(
            store.get(run_id).expect("run").activity_state(),
            ActivityState::Idle
        );
        assert_eq!(
            store.get(run_id).expect("run").change_revision(),
            1,
            "an unfinished tool at semantic settlement conservatively invalidates Git review"
        );
    }

    #[test]
    fn retry_summarization_and_extension_errors_are_bounded_in_hydration() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let limits = RuntimeLimits {
            max_failure_detail_bytes: 8,
            ..RuntimeLimits::default()
        };
        let mut controller = RunRpcController::new(run_id, limits);

        controller
            .apply_event(
                &event(br#"{"type":"auto_retry_start","attempt":1,"maxAttempts":4,"delayMs":2000,"errorMessage":"retry-detail-is-long"}"#),
                &mut store,
            )
            .expect("retry scheduled");
        assert!(store.get(run_id).expect("run").is_retry_waiting());
        let snapshot = controller.hydration_snapshot(Instant::now());
        let retry = snapshot.retry.expect("retry snapshot");
        assert!(retry.waiting);
        assert_eq!(retry.attempt, 1);
        assert!(retry.error_truncated);
        assert!(retry.error_message.len() <= 8);

        controller
            .apply_event(&event(br#"{"type":"agent_start"}"#), &mut store)
            .expect("retry attempt start");
        assert!(!store.get(run_id).expect("run").is_retry_waiting());
        assert!(
            !controller
                .hydration_snapshot(Instant::now())
                .retry
                .expect("retry remains visible while attempt runs")
                .waiting
        );

        controller
            .apply_event(
                &event(br#"{"type":"summarization_retry_scheduled","attempt":2,"maxAttempts":3,"delayMs":500,"errorMessage":"summary-detail-is-long"}"#),
                &mut store,
            )
            .expect("summary retry");
        assert!(store.get(run_id).expect("run").has_summarization_retry());

        controller
            .apply_event(
                &event(br#"{"type":"extension_error","extensionPath":"C:/extensions/very-long-name.ts","event":"session_start","error":"extension-detail-is-long"}"#),
                &mut store,
            )
            .expect("extension error");
        let snapshot = controller.hydration_snapshot(Instant::now());
        let summary = snapshot
            .summarization_retry
            .expect("summary retry snapshot");
        assert!(summary.error_truncated);
        assert!(summary.error_message.len() <= 8);
        let extension = snapshot
            .last_extension_error
            .expect("extension error snapshot");
        assert!(extension.detail_truncated);
        assert!(extension.extension_path.len() <= 8);
        assert!(extension.error.len() <= 8);

        controller
            .apply_event(
                &event(br#"{"type":"summarization_retry_finished"}"#),
                &mut store,
            )
            .expect("summary retry finished");
        assert!(!store.get(run_id).expect("run").has_summarization_retry());
        assert!(
            controller
                .hydration_snapshot(Instant::now())
                .summarization_retry
                .expect("finished state retained for UI settlement")
                .finished
        );
    }

    #[test]
    fn compaction_reason_and_failure_are_bounded_and_overflow_retry_clears_on_agent_restart() {
        let run_id = RunId::new();
        let mut store = ready_store(run_id);
        let limits = RuntimeLimits {
            max_failure_detail_bytes: 8,
            ..RuntimeLimits::default()
        };
        let mut controller = RunRpcController::new(run_id, limits);

        controller
            .apply_event(
                &event(br#"{"type":"compaction_start","reason":"overflow-is-long"}"#),
                &mut store,
            )
            .expect("compaction start");
        let active = controller
            .hydration_snapshot(Instant::now())
            .compaction
            .expect("active compaction");
        assert!(!active.finished);
        assert!(active.reason_truncated);
        assert!(active.reason.len() <= 8);

        controller
            .apply_event(
                &event(br#"{"type":"compaction_end","reason":"overflow","result":null,"aborted":false,"willRetry":true,"errorMessage":"quota-error-is-long"}"#),
                &mut store,
            )
            .expect("compaction end");
        let ended = controller
            .hydration_snapshot(Instant::now())
            .compaction
            .expect("finished compaction");
        assert!(ended.finished);
        assert!(ended.will_retry);
        assert!(ended.error_truncated);
        assert!(
            ended
                .error_message
                .as_ref()
                .is_some_and(|value| value.len() <= 8)
        );

        controller
            .apply_event(&event(br#"{"type":"agent_start"}"#), &mut store)
            .expect("overflow retry agent start");
        assert!(
            controller
                .hydration_snapshot(Instant::now())
                .compaction
                .is_none()
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
