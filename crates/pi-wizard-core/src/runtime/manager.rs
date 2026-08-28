use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::draft::{
    DraftClearOutcome, DraftDurability, DraftError, DraftImageData, DraftSnapshot, DraftStoreError,
    SessionDraftStore,
};
use crate::draft_persistence::{
    DraftPersistenceEvent, DraftPersistenceWorkerHandle, spawn_draft_persistence_worker,
};
use crate::environment::ResolvedLaunchEnvironment;
use crate::launch::{ResolvedPiLaunchSpec, SessionLaunch};
use crate::process::spawn_pi_process;
use crate::rpc::{
    ClearQueueResult, CompactionResult, ExtensionUiResponse, InboundMessage, RpcCommand,
    RpcConcurrencyClass, RpcRequest, RpcResponse, RpcResponseOutcome, SessionEntriesPage,
    SessionStats, ThinkingLevel,
};
use crate::worktree::GitWorktreeIdentity;
use crate::{DraftImageId, ProjectId, RequestId, RunId, RuntimeLimits};

use super::{
    AssistantContentSnapshot, ComposerAvailability, DirectBashSnapshot, ExecutionIsolation,
    ProcessState, ProcessTerminationReport, RunFailure, RunFailureKind, RunMutation,
    RunProcessEnvelope, RunProcessEvent, RunProcessHandle, RunRecord, RunRpcController,
    RunRpcEffect, RuntimeHydrationSnapshot, RuntimeStore, SessionSyncCompletion, StopDirective,
    StopPhase, StopTransaction, ToolPreviewSnapshot, UiBacklogError, UiBacklogFrame, UiCoalesceKey,
    UiEventBacklog, spawn_run_process_actor,
};

/// Fully resolved launch input for one live runtime. Environment values remain
/// backend-only and never appear in hydration or UI event payloads.
#[derive(Clone)]
pub struct RunStartSpec {
    pub project_id: ProjectId,
    pub execution_isolation: ExecutionIsolation,
    pub worktree: Option<GitWorktreeIdentity>,
    pub launch: ResolvedPiLaunchSpec,
    pub environment: ResolvedLaunchEnvironment,
}

struct PendingSessionReplacement {
    request: RpcRequest,
    session_id: String,
    deadline: Instant,
    reply: oneshot::Sender<Result<ManagedRpcCompletion, String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComposerAction {
    Send,
    Steer,
    FollowUp,
    /// Execute a discovered Pi extension command through `prompt` while the
    /// agent is already streaming. Pi explicitly rejects extension commands
    /// sent through `steer` or `follow_up`.
    RunCommand,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerSubmitResult {
    pub action: ComposerAction,
    pub accepted: bool,
    pub draft_cleared: bool,
    pub error: Option<String>,
}

struct StartupHandshake {
    request_id: RequestId,
    deadline: Instant,
}

#[derive(Clone, Debug)]
pub struct ManagedRpcCompletion {
    pub response: RpcResponse,
    pub session_entries: Option<SessionEntriesPage>,
    pub session_resync_required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReplacementResult {
    pub cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStopResult {
    pub recovered_steering: Vec<String>,
    pub recovered_follow_up: Vec<String>,
    pub draft_restored: bool,
    pub draft_restore_error: Option<String>,
    pub process_terminated: bool,
    pub quarantined: bool,
}

impl RuntimeStopResult {
    fn normal(recovered: ClearQueueResult) -> Self {
        Self {
            recovered_steering: recovered.steering,
            recovered_follow_up: recovered.follow_up,
            draft_restored: false,
            draft_restore_error: None,
            process_terminated: false,
            quarantined: false,
        }
    }

    fn terminated(recovered: Option<&ClearQueueResult>, quarantined: bool) -> Self {
        Self {
            recovered_steering: recovered
                .map(|value| value.steering.clone())
                .unwrap_or_default(),
            recovered_follow_up: recovered
                .map(|value| value.follow_up.clone())
                .unwrap_or_default(),
            draft_restored: false,
            draft_restore_error: None,
            process_terminated: true,
            quarantined,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeShutdownReport {
    pub terminal_runs: usize,
    pub quarantined_runs: usize,
    pub draft_flush_failed_sessions: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum RuntimeUiEvent {
    StateChanged {
        run_id: RunId,
        runtime_revision: u64,
    },
    AssistantMessageReset {
        run_id: RunId,
    },
    AssistantBlockUpdated {
        run_id: RunId,
        block: AssistantContentSnapshot,
    },
    ToolUpdated {
        run_id: RunId,
        tool: ToolPreviewSnapshot,
    },
    DirectBashUpdated {
        run_id: RunId,
        bash: DirectBashSnapshot,
    },
    ToolFinished {
        run_id: RunId,
        tool_call_id: String,
        tool_name: String,
        output: String,
        dropped_bytes: u64,
        is_error: bool,
    },
    CapabilitiesChanged {
        run_id: RunId,
        revision: u64,
    },
    SessionSyncChanged {
        run_id: RunId,
        revision: u64,
        resync_required: bool,
    },
    ExtensionDialogsChanged {
        run_id: RunId,
    },
    ExtensionNotification {
        run_id: RunId,
        message: String,
        notify_type: crate::rpc::ExtensionNotifyType,
    },
    ExtensionUiStateChanged {
        run_id: RunId,
    },
    EditorTextChanged {
        run_id: RunId,
    },
    DraftChanged {
        run_id: RunId,
    },
    ComposerChanged {
        run_id: RunId,
    },
    ProcessTerminal {
        run_id: RunId,
        process: ProcessState,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeUiDrain {
    pub events: Vec<RuntimeUiEvent>,
    pub rehydrate_required: bool,
    pub pending_editor_text: Option<String>,
    pub has_more: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum RuntimeManagerSignal {
    RunDirty { run_id: RunId },
}

#[derive(Clone, Debug)]
pub struct RuntimeManagerHandle {
    commands: mpsc::Sender<RuntimeManagerCommand>,
    controls: mpsc::Sender<RuntimeManagerControlCommand>,
    signals: broadcast::Sender<RuntimeManagerSignal>,
    limits: RuntimeLimits,
}

impl RuntimeManagerHandle {
    pub async fn hydrate(&self) -> Result<RuntimeHydrationSnapshot, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::Hydrate { reply }).await?;
        receive(response).await
    }

    pub async fn attach_draft_image(
        &self,
        run_id: RunId,
        file_name: String,
        mime_type: String,
        data: String,
    ) -> Result<DraftSnapshot, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::AttachDraftImage {
            run_id,
            file_name,
            mime_type,
            data,
            reply,
        })
        .await?;
        receive(response).await
    }

    pub async fn remove_draft_image(
        &self,
        run_id: RunId,
        image_id: DraftImageId,
    ) -> Result<DraftSnapshot, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::RemoveDraftImage {
            run_id,
            image_id,
            reply,
        })
        .await?;
        receive(response).await
    }

    /// Seeds the bounded incremental session tail from an app-validated cold
    /// history observation. Existing healthy live synchronization is never
    /// rewound by a later renderer history refresh.
    pub async fn bootstrap_session_sync(
        &self,
        run_id: RunId,
        expected_session_id: String,
        cursor: String,
        leaf_id: Option<String>,
    ) -> Result<(), RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::BootstrapSessionSync {
            run_id,
            expected_session_id,
            cursor,
            leaf_id,
            reply,
        })
        .await?;
        receive(response).await
    }

    /// Applies a model change through Pi and then refreshes the model-dependent
    /// thinking capability plus authoritative runtime state. The renderer must
    /// not keep an optimistic model selection as its source of truth.
    pub async fn set_model(
        &self,
        run_id: RunId,
        provider: String,
        model_id: String,
    ) -> Result<(), RuntimeManagerError> {
        let changed = self
            .request(
                run_id,
                RpcRequest::new(RpcCommand::SetModel { provider, model_id }),
            )
            .await?;
        require_accepted("set model", &changed)?;

        let thinking = self
            .request(
                run_id,
                RpcRequest::new(RpcCommand::GetAvailableThinkingLevels),
            )
            .await?;
        require_accepted("refresh thinking levels after model change", &thinking)?;
        let state = self
            .request(run_id, RpcRequest::new(RpcCommand::GetState))
            .await?;
        require_accepted("reconcile model state", &state)
    }

    /// Applies a supported thinking level and waits for authoritative Pi state
    /// before the desktop considers the operation complete.
    pub async fn set_thinking_level(
        &self,
        run_id: RunId,
        level: ThinkingLevel,
    ) -> Result<(), RuntimeManagerError> {
        let changed = self
            .request(
                run_id,
                RpcRequest::new(RpcCommand::SetThinkingLevel { level }),
            )
            .await?;
        require_accepted("set thinking level", &changed)?;
        let state = self
            .request(run_id, RpcRequest::new(RpcCommand::GetState))
            .await?;
        require_accepted("reconcile thinking state", &state)
    }

    /// Enables or disables Pi's own automatic compaction policy and waits for
    /// authoritative `get_state` reconciliation. Pi remains the sole owner of
    /// when and how compaction occurs; the desktop only controls its native
    /// setting.
    pub async fn set_auto_compaction(
        &self,
        run_id: RunId,
        enabled: bool,
    ) -> Result<(), RuntimeManagerError> {
        let changed = self
            .request(
                run_id,
                RpcRequest::new(RpcCommand::SetAutoCompaction { enabled }),
            )
            .await?;
        require_accepted("set automatic compaction", &changed)?;
        let state = self
            .request(run_id, RpcRequest::new(RpcCommand::GetState))
            .await?;
        require_accepted("reconcile automatic compaction state", &state)
    }

    /// Runs Pi's native manual compaction and reconciles authoritative state
    /// before returning. Pi remains responsible for the actual compaction
    /// policy and summary generation.
    pub async fn compact_session(
        &self,
        run_id: RunId,
    ) -> Result<CompactionResult, RuntimeManagerError> {
        let compacted = self
            .request(
                run_id,
                RpcRequest::new(RpcCommand::Compact {
                    custom_instructions: None,
                }),
            )
            .await?;
        require_accepted("compact session", &compacted)?;
        let result = compacted
            .response
            .compaction_result(self.limits)
            .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
        let state = self
            .request(run_id, RpcRequest::new(RpcCommand::GetState))
            .await?;
        require_accepted("reconcile compacted session", &state)?;
        Ok(result)
    }

    /// Reads Pi's current session usage on demand. This remains an explicit
    /// request rather than a polling loop so an idle desktop stays idle.
    pub async fn session_stats(&self, run_id: RunId) -> Result<SessionStats, RuntimeManagerError> {
        let completion = self
            .request(run_id, RpcRequest::new(RpcCommand::GetSessionStats))
            .await?;
        require_accepted("get session stats", &completion)?;
        completion
            .response
            .session_stats(self.limits)
            .map_err(|error| RuntimeManagerError::Operation(error.to_string()))
    }

    /// Sets or clears the Pi session display name and waits for the resulting
    /// session identity metadata to be observed through `get_state`.
    pub async fn set_session_name(
        &self,
        run_id: RunId,
        name: String,
    ) -> Result<(), RuntimeManagerError> {
        let changed = self
            .request(run_id, RpcRequest::new(RpcCommand::SetSessionName { name }))
            .await?;
        require_accepted("set session name", &changed)?;
        let state = self
            .request(run_id, RpcRequest::new(RpcCommand::GetState))
            .await?;
        require_accepted("reconcile session name", &state)
    }

    /// Clones the active Pi branch. Accepted replacement responses already
    /// queue an internal `get_state`; this explicit second observation waits
    /// behind it so a caller can hydrate immediately without seeing the old
    /// session identity.
    pub async fn clone_session(
        &self,
        run_id: RunId,
    ) -> Result<SessionReplacementResult, RuntimeManagerError> {
        let cloned = self
            .request(run_id, RpcRequest::new(RpcCommand::Clone))
            .await?;
        match cloned.response.outcome() {
            RpcResponseOutcome::Rejected => Err(rejected_operation("clone session", &cloned)),
            RpcResponseOutcome::Cancelled => Ok(SessionReplacementResult { cancelled: true }),
            RpcResponseOutcome::Accepted => {
                let state = self
                    .request(run_id, RpcRequest::new(RpcCommand::GetState))
                    .await?;
                require_accepted("reconcile cloned session", &state)?;
                Ok(SessionReplacementResult { cancelled: false })
            }
        }
    }

    /// Forks the active Pi session at one persisted entry. As with clone and
    /// switch, the session-replacement transaction owns draft durability and
    /// the accepted response is followed by authoritative state reconciliation.
    pub async fn fork_session(
        &self,
        run_id: RunId,
        entry_id: String,
    ) -> Result<SessionReplacementResult, RuntimeManagerError> {
        let forked = self
            .request(run_id, RpcRequest::new(RpcCommand::Fork { entry_id }))
            .await?;
        match forked.response.outcome() {
            RpcResponseOutcome::Rejected => Err(rejected_operation("fork session", &forked)),
            RpcResponseOutcome::Cancelled => Ok(SessionReplacementResult { cancelled: true }),
            RpcResponseOutcome::Accepted => {
                let state = self
                    .request(run_id, RpcRequest::new(RpcCommand::GetState))
                    .await?;
                require_accepted("reconcile forked session", &state)?;
                Ok(SessionReplacementResult { cancelled: false })
            }
        }
    }

    pub async fn edit_draft(
        &self,
        run_id: RunId,
        text: String,
    ) -> Result<DraftSnapshot, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::EditDraft {
            run_id,
            text,
            reply,
        })
        .await?;
        receive(response).await
    }

    pub async fn submit_draft(
        &self,
        run_id: RunId,
        action: ComposerAction,
    ) -> Result<ComposerSubmitResult, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::SubmitDraft {
            run_id,
            action,
            reply,
        })
        .await?;
        receive(response).await
    }

    /// Atomically rebuilds authoritative runtime state for one desynchronized
    /// renderer backlog and discards only that run's stale queued display/event
    /// frames. Ordinary hydration is deliberately non-destructive.
    pub async fn recover_ui(
        &self,
        run_id: RunId,
    ) -> Result<RuntimeHydrationSnapshot, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::RecoverUi { run_id, reply })
            .await?;
        receive(response).await
    }

    pub async fn start_run(&self, spec: RunStartSpec) -> Result<RunId, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::StartRun {
            spec: Box::new(spec),
            reply,
        })
        .await?;
        receive(response).await
    }

    pub async fn request(
        &self,
        run_id: RunId,
        request: RpcRequest,
    ) -> Result<ManagedRpcCompletion, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::Request {
            run_id,
            request,
            reply,
        })
        .await?;
        receive(response).await
    }

    pub async fn respond_extension_ui(
        &self,
        run_id: RunId,
        response_value: ExtensionUiResponse,
    ) -> Result<(), RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send_control(RuntimeManagerControlCommand::RespondExtensionUi {
            run_id,
            response: response_value,
            reply,
        })
        .await?;
        receive(response).await
    }

    pub async fn stop_run(&self, run_id: RunId) -> Result<RuntimeStopResult, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send_control(RuntimeManagerControlCommand::StopRun { run_id, reply })
            .await?;
        receive(response).await
    }

    pub async fn drain_ui(
        &self,
        run_id: RunId,
        max_events: usize,
    ) -> Result<RuntimeUiDrain, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeManagerCommand::DrainUi {
            run_id,
            max_events,
            reply,
        })
        .await?;
        receive(response).await
    }

    pub async fn shutdown(&self) -> Result<RuntimeShutdownReport, RuntimeManagerError> {
        let (reply, response) = oneshot::channel();
        self.send_control(RuntimeManagerControlCommand::Shutdown { reply })
            .await?;
        receive(response).await
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeManagerSignal> {
        self.signals.subscribe()
    }

    async fn send(&self, command: RuntimeManagerCommand) -> Result<(), RuntimeManagerError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| RuntimeManagerError::ManagerClosed)
    }

    async fn send_control(
        &self,
        command: RuntimeManagerControlCommand,
    ) -> Result<(), RuntimeManagerError> {
        self.controls
            .send(command)
            .await
            .map_err(|_| RuntimeManagerError::ManagerClosed)
    }
}

fn require_accepted(
    operation: &str,
    completion: &ManagedRpcCompletion,
) -> Result<(), RuntimeManagerError> {
    match completion.response.outcome() {
        RpcResponseOutcome::Accepted => Ok(()),
        RpcResponseOutcome::Rejected | RpcResponseOutcome::Cancelled => {
            Err(rejected_operation(operation, completion))
        }
    }
}

fn rejected_operation(operation: &str, completion: &ManagedRpcCompletion) -> RuntimeManagerError {
    let detail = completion.response.error.as_deref().unwrap_or_else(|| {
        match completion.response.outcome() {
            RpcResponseOutcome::Cancelled => "operation was cancelled by a Pi extension",
            RpcResponseOutcome::Rejected => "Pi rejected the operation",
            RpcResponseOutcome::Accepted => "operation did not complete as expected",
        }
    });
    RuntimeManagerError::Operation(format!("{operation}: {detail}"))
}

async fn receive<T>(
    response: oneshot::Receiver<Result<T, String>>,
) -> Result<T, RuntimeManagerError> {
    response
        .await
        .map_err(|_| RuntimeManagerError::ManagerClosed)?
        .map_err(RuntimeManagerError::Operation)
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RuntimeManagerError {
    #[error("runtime manager requires an active Tokio runtime")]
    AsyncRuntimeUnavailable,
    #[error("runtime manager is closed")]
    ManagerClosed,
    #[error("Git worktree execution root is already owned by a live run: {execution_root}")]
    WorktreeAlreadyActive { execution_root: PathBuf },
    #[error("runtime manager operation failed: {0}")]
    Operation(String),
}

enum RuntimeManagerCommand {
    Hydrate {
        reply: oneshot::Sender<Result<RuntimeHydrationSnapshot, String>>,
    },
    RecoverUi {
        run_id: RunId,
        reply: oneshot::Sender<Result<RuntimeHydrationSnapshot, String>>,
    },
    StartRun {
        spec: Box<RunStartSpec>,
        reply: oneshot::Sender<Result<RunId, String>>,
    },
    Request {
        run_id: RunId,
        request: RpcRequest,
        reply: oneshot::Sender<Result<ManagedRpcCompletion, String>>,
    },
    BootstrapSessionSync {
        run_id: RunId,
        expected_session_id: String,
        cursor: String,
        leaf_id: Option<String>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    EditDraft {
        run_id: RunId,
        text: String,
        reply: oneshot::Sender<Result<DraftSnapshot, String>>,
    },
    AttachDraftImage {
        run_id: RunId,
        file_name: String,
        mime_type: String,
        data: String,
        reply: oneshot::Sender<Result<DraftSnapshot, String>>,
    },
    RemoveDraftImage {
        run_id: RunId,
        image_id: DraftImageId,
        reply: oneshot::Sender<Result<DraftSnapshot, String>>,
    },
    SubmitDraft {
        run_id: RunId,
        action: ComposerAction,
        reply: oneshot::Sender<Result<ComposerSubmitResult, String>>,
    },
    DrainUi {
        run_id: RunId,
        max_events: usize,
        reply: oneshot::Sender<Result<RuntimeUiDrain, String>>,
    },
}

struct ActiveComposerSubmission {
    request_id: RequestId,
    action: ComposerAction,
    draft_generation: u64,
    reply: oneshot::Sender<Result<ComposerSubmitResult, String>>,
}

enum RuntimeManagerControlCommand {
    RespondExtensionUi {
        run_id: RunId,
        response: ExtensionUiResponse,
        reply: oneshot::Sender<Result<(), String>>,
    },
    StopRun {
        run_id: RunId,
        reply: oneshot::Sender<Result<RuntimeStopResult, String>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<RuntimeShutdownReport, String>>,
    },
}

struct RuntimeUiQueue {
    backlog: UiEventBacklog,
    rehydrate_required: bool,
    pending_editor_text: Option<String>,
    dirty_signaled: bool,
}

impl RuntimeUiQueue {
    fn new(limits: RuntimeLimits) -> Self {
        Self {
            backlog: UiEventBacklog::from_limits(limits),
            rehydrate_required: false,
            pending_editor_text: None,
            dirty_signaled: false,
        }
    }

    fn push_semantic(&mut self, event: RuntimeUiEvent) -> bool {
        let Ok(payload) = serde_json::to_vec(&event) else {
            self.rehydrate_required = true;
            return self.mark_dirty();
        };
        if matches!(
            self.backlog.push_semantic(payload),
            Err(UiBacklogError::FrameTooLarge { .. }
                | UiBacklogError::SemanticCapacityExhausted { .. })
        ) {
            self.rehydrate_required = true;
        }
        self.mark_dirty()
    }

    fn push_coalescible(&mut self, key: UiCoalesceKey, event: RuntimeUiEvent) -> bool {
        let Ok(payload) = serde_json::to_vec(&event) else {
            return false;
        };
        self.backlog.push_coalescible(key, payload);
        self.mark_dirty()
    }

    fn has_pending_delivery(&self) -> bool {
        self.backlog.stats().frame_count > 0
            || self.pending_editor_text.is_some()
            || self.rehydrate_required
    }

    fn reset_after_recovery(&mut self) -> bool {
        self.backlog.clear();
        self.rehydrate_required = false;
        self.dirty_signaled = false;
        self.pending_editor_text.is_some()
    }

    fn drain(&mut self, max_events: usize) -> RuntimeUiDrain {
        let mut events = Vec::with_capacity(max_events.min(self.backlog.stats().frame_count));
        for _ in 0..max_events {
            let Some(frame) = self.backlog.pop_front() else {
                break;
            };
            let payload = match frame {
                UiBacklogFrame::Semantic(payload) | UiBacklogFrame::Coalescible { payload, .. } => {
                    payload
                }
            };
            if let Ok(event) = serde_json::from_slice(&payload) {
                events.push(event);
            } else {
                self.rehydrate_required = true;
            }
        }
        let has_more = self.backlog.stats().frame_count > 0;
        let pending_editor_text = self.pending_editor_text.take();
        if !has_more {
            self.dirty_signaled = false;
        }
        RuntimeUiDrain {
            events,
            rehydrate_required: self.rehydrate_required,
            pending_editor_text,
            has_more,
        }
    }

    fn mark_dirty(&mut self) -> bool {
        if self.dirty_signaled {
            false
        } else {
            self.dirty_signaled = true;
            true
        }
    }
}

struct ActiveStop {
    transaction: StopTransaction,
    reply: oneshot::Sender<Result<RuntimeStopResult, String>>,
}

struct ShutdownState {
    reply: oneshot::Sender<Result<RuntimeShutdownReport, String>>,
    target_runs: Vec<RunId>,
    draft_deadline: Instant,
}

struct RuntimeManagerTask {
    limits: RuntimeLimits,
    store: RuntimeStore,
    drafts: SessionDraftStore,
    controllers: HashMap<RunId, RunRpcController>,
    processes: HashMap<RunId, RunProcessHandle>,
    ui: HashMap<RunId, RuntimeUiQueue>,
    request_waiters:
        HashMap<(RunId, String), oneshot::Sender<Result<ManagedRpcCompletion, String>>>,
    extension_waiters: HashMap<(RunId, String), oneshot::Sender<Result<(), String>>>,
    composer_submissions: HashMap<RunId, ActiveComposerSubmission>,
    pending_session_replacements: HashMap<RunId, PendingSessionReplacement>,
    draft_persistence: Option<DraftPersistenceWorkerHandle>,
    draft_events: mpsc::Receiver<DraftPersistenceEvent>,
    draft_events_open: bool,
    draft_load_attempted: HashSet<String>,
    draft_load_pending: HashSet<String>,
    draft_save_deadlines: HashMap<String, Instant>,
    startups: HashMap<RunId, StartupHandshake>,
    stops: HashMap<RunId, ActiveStop>,
    pending_failures: HashMap<RunId, RunFailure>,
    shutdown: Option<ShutdownState>,
    commands: mpsc::Receiver<RuntimeManagerCommand>,
    controls: mpsc::Receiver<RuntimeManagerControlCommand>,
    process_events: mpsc::Receiver<RunProcessEnvelope>,
    process_events_tx: mpsc::Sender<RunProcessEnvelope>,
    signals: broadcast::Sender<RuntimeManagerSignal>,
}

enum ManagerInput {
    Command(Option<RuntimeManagerCommand>),
    Control(Option<RuntimeManagerControlCommand>),
    Process(Option<RunProcessEnvelope>),
    Draft(Option<DraftPersistenceEvent>),
    Deadline,
}

pub fn spawn_runtime_manager(
    limits: RuntimeLimits,
) -> Result<RuntimeManagerHandle, RuntimeManagerError> {
    spawn_runtime_manager_inner(limits, None)
}

pub fn spawn_runtime_manager_with_draft_persistence(
    limits: RuntimeLimits,
    root: impl AsRef<Path>,
) -> Result<RuntimeManagerHandle, RuntimeManagerError> {
    spawn_runtime_manager_inner(limits, Some(root.as_ref().to_path_buf()))
}

fn spawn_runtime_manager_inner(
    limits: RuntimeLimits,
    draft_root: Option<PathBuf>,
) -> Result<RuntimeManagerHandle, RuntimeManagerError> {
    let limits = limits
        .validate()
        .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| RuntimeManagerError::AsyncRuntimeUnavailable)?;
    let (command_tx, command_rx) = mpsc::channel(limits.max_runtime_command_queue);
    let (control_tx, control_rx) = mpsc::channel(limits.max_pending_ui_requests_per_run.max(4));
    let (process_events_tx, process_events) = mpsc::channel(limits.max_process_event_queue);
    let (draft_events_tx, draft_events) = mpsc::channel(limits.max_runtime_command_queue);
    let draft_persistence = draft_root
        .map(|root| spawn_draft_persistence_worker(root, limits, draft_events_tx))
        .transpose()
        .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
    let (signals, _) = broadcast::channel(limits.max_runtime_command_queue);
    let task = RuntimeManagerTask {
        limits,
        store: RuntimeStore::new(limits),
        drafts: SessionDraftStore::new(limits),
        controllers: HashMap::new(),
        processes: HashMap::new(),
        ui: HashMap::new(),
        request_waiters: HashMap::new(),
        extension_waiters: HashMap::new(),
        composer_submissions: HashMap::new(),
        pending_session_replacements: HashMap::new(),
        draft_events,
        draft_events_open: draft_persistence.is_some(),
        draft_persistence,
        draft_load_attempted: HashSet::new(),
        draft_load_pending: HashSet::new(),
        draft_save_deadlines: HashMap::new(),
        startups: HashMap::new(),
        stops: HashMap::new(),
        pending_failures: HashMap::new(),
        shutdown: None,
        commands: command_rx,
        controls: control_rx,
        process_events,
        process_events_tx,
        signals: signals.clone(),
    };
    runtime.spawn(task.run());
    Ok(RuntimeManagerHandle {
        commands: command_tx,
        controls: control_tx,
        signals,
        limits,
    })
}

impl RuntimeManagerTask {
    async fn run(mut self) {
        let mut commands_open = true;
        let mut controls_open = true;
        loop {
            let next_deadline = self.next_deadline();
            let input = if let Some(deadline) = next_deadline {
                tokio::select! {
                    biased;
                    control = self.controls.recv(), if controls_open => ManagerInput::Control(control),
                    command = self.commands.recv(), if commands_open => ManagerInput::Command(command),
                    event = self.process_events.recv() => ManagerInput::Process(event),
                    event = self.draft_events.recv(), if self.draft_events_open => ManagerInput::Draft(event),
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => ManagerInput::Deadline,
                }
            } else {
                tokio::select! {
                    biased;
                    control = self.controls.recv(), if controls_open => ManagerInput::Control(control),
                    command = self.commands.recv(), if commands_open => ManagerInput::Command(command),
                    event = self.process_events.recv() => ManagerInput::Process(event),
                    event = self.draft_events.recv(), if self.draft_events_open => ManagerInput::Draft(event),
                }
            };

            let keep_running = match input {
                ManagerInput::Control(Some(control)) => self.handle_control(control),
                ManagerInput::Control(None) => {
                    controls_open = false;
                    if !commands_open {
                        self.begin_unobserved_shutdown();
                    }
                    true
                }
                ManagerInput::Command(Some(command)) => self.handle_command(command),
                ManagerInput::Command(None) => {
                    commands_open = false;
                    if !controls_open {
                        self.begin_unobserved_shutdown();
                    }
                    true
                }
                ManagerInput::Process(Some(event)) => {
                    self.handle_process_event(event);
                    true
                }
                ManagerInput::Process(None) => false,
                ManagerInput::Draft(Some(event)) => {
                    self.handle_draft_persistence_event(event);
                    true
                }
                ManagerInput::Draft(None) => {
                    self.draft_events_open = false;
                    self.fail_draft_persistence_worker("draft persistence worker closed");
                    true
                }
                ManagerInput::Deadline => {
                    self.handle_deadlines();
                    true
                }
            };
            if !keep_running || self.finish_shutdown_if_ready() {
                return;
            }
        }
    }

    fn handle_command(&mut self, command: RuntimeManagerCommand) -> bool {
        match command {
            RuntimeManagerCommand::Hydrate { reply } => {
                let snapshot = self.hydration_snapshot();
                // A renderer can subscribe after a previous dirty wake-up was
                // emitted. Hydration therefore re-announces pending delivery,
                // but never consumes it. This also keeps transient events that
                // are intentionally absent from the hydration snapshot.
                let resignal: Vec<_> = self
                    .ui
                    .iter()
                    .filter_map(|(run_id, queue)| queue.has_pending_delivery().then_some(*run_id))
                    .collect();
                for run_id in resignal {
                    self.resignal_dirty(run_id);
                }
                let _ = reply.send(Ok(snapshot));
            }
            RuntimeManagerCommand::RecoverUi { run_id, reply } => {
                let result = if let Some(queue) = self.ui.get_mut(&run_id) {
                    let preserve_editor_text = queue.reset_after_recovery();
                    let snapshot = self.hydration_snapshot();
                    if preserve_editor_text {
                        self.signal_dirty(run_id);
                    }
                    Ok(snapshot)
                } else {
                    Err(format!("run {run_id} has no UI backlog"))
                };
                let _ = reply.send(result);
            }
            RuntimeManagerCommand::AttachDraftImage {
                run_id,
                file_name,
                mime_type,
                data,
                reply,
            } => {
                let result = if self.shutdown.is_some() {
                    Err("runtime manager is shutting down".to_owned())
                } else {
                    self.attach_draft_image(run_id, file_name, mime_type, data)
                };
                let _ = reply.send(result);
            }
            RuntimeManagerCommand::RemoveDraftImage {
                run_id,
                image_id,
                reply,
            } => {
                let result = if self.shutdown.is_some() {
                    Err("runtime manager is shutting down".to_owned())
                } else {
                    self.remove_draft_image(run_id, image_id)
                };
                let _ = reply.send(result);
            }
            RuntimeManagerCommand::StartRun { spec, reply } => {
                if self.shutdown.is_some() {
                    let _ = reply.send(Err("runtime manager is shutting down".to_owned()));
                } else {
                    let _ = reply.send(self.start_run(*spec).map_err(|error| error.to_string()));
                }
            }
            RuntimeManagerCommand::Request {
                run_id,
                request,
                reply,
            } => {
                if self.shutdown.is_some() {
                    let _ = reply.send(Err("runtime manager is shutting down".to_owned()));
                } else {
                    self.begin_managed_request(run_id, request, reply);
                }
            }
            RuntimeManagerCommand::BootstrapSessionSync {
                run_id,
                expected_session_id,
                cursor,
                leaf_id,
                reply,
            } => {
                let result = self.bootstrap_session_sync_if_needed(
                    run_id,
                    &expected_session_id,
                    cursor,
                    leaf_id,
                );
                let _ = reply.send(result);
            }
            RuntimeManagerCommand::EditDraft {
                run_id,
                text,
                reply,
            } => {
                let result = if self.shutdown.is_some() {
                    Err("runtime manager is shutting down".to_owned())
                } else {
                    self.edit_draft(run_id, text)
                };
                let _ = reply.send(result);
            }
            RuntimeManagerCommand::SubmitDraft {
                run_id,
                action,
                reply,
            } => {
                if self.shutdown.is_some() {
                    let _ = reply.send(Err("runtime manager is shutting down".to_owned()));
                } else {
                    self.begin_composer_submission(run_id, action, reply);
                }
            }
            RuntimeManagerCommand::DrainUi {
                run_id,
                max_events,
                reply,
            } => {
                let result = self
                    .ui
                    .get_mut(&run_id)
                    .map(|queue| queue.drain(max_events.min(self.limits.max_runtime_command_queue)))
                    .ok_or_else(|| format!("run {run_id} has no UI backlog"));
                let _ = reply.send(result);
            }
        }
        true
    }

    fn handle_control(&mut self, command: RuntimeManagerControlCommand) -> bool {
        match command {
            RuntimeManagerControlCommand::RespondExtensionUi {
                run_id,
                response,
                reply,
            } => {
                let request_id = extension_response_id(&response).to_owned();
                if self.shutdown.is_some() {
                    let _ = reply.send(Err("runtime manager is shutting down".to_owned()));
                } else if let Err(error) = self.queue_extension_response(run_id, response) {
                    let _ = reply.send(Err(error));
                } else {
                    self.extension_waiters.insert((run_id, request_id), reply);
                }
            }
            RuntimeManagerControlCommand::StopRun { run_id, reply } => {
                if self.shutdown.is_some() {
                    let _ = reply.send(Err("runtime manager is shutting down".to_owned()));
                } else {
                    self.begin_stop(run_id, reply);
                }
            }
            RuntimeManagerControlCommand::Shutdown { reply } => {
                if self.shutdown.is_some() {
                    let _ = reply.send(Err("runtime manager is already shutting down".to_owned()));
                } else {
                    self.begin_shutdown(reply);
                }
            }
        }
        true
    }

    fn start_run(&mut self, spec: RunStartSpec) -> Result<RunId, RuntimeManagerError> {
        if spec.execution_isolation == ExecutionIsolation::GitWorktree {
            let execution_root = spec.launch.cwd();
            if self.store.records().any(|run| {
                !run.process_state().is_terminal() && run.execution_root() == execution_root
            }) {
                return Err(RuntimeManagerError::WorktreeAlreadyActive {
                    execution_root: execution_root.to_path_buf(),
                });
            }
        }
        let run_id = RunId::new();
        let record = RunRecord::starting_with_worktree(
            run_id,
            spec.project_id,
            spec.launch.cwd().to_path_buf(),
            spec.execution_isolation,
            spec.launch.project_trust,
            spec.worktree.clone(),
        )
        .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
        let initial_session_id = match &spec.launch.session {
            SessionLaunch::NewWithId(session_id) => Some(session_id.to_string()),
            SessionLaunch::New | SessionLaunch::Ephemeral | SessionLaunch::Resume(_) => None,
        };
        let process = spawn_pi_process(&spec.launch, &spec.environment, self.limits)
            .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
        self.drafts
            .register_run(run_id, initial_session_id)
            .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
        self.store
            .register(record)
            .map_err(|error| RuntimeManagerError::Operation(error.to_string()))?;
        self.controllers
            .insert(run_id, RunRpcController::new(run_id, self.limits));
        self.ui.insert(run_id, RuntimeUiQueue::new(self.limits));
        let handle =
            spawn_run_process_actor(run_id, process, self.process_events_tx.clone(), self.limits);
        self.processes.insert(run_id, handle);
        self.queue_draft_load_for_run(run_id);
        self.push_state_changed(run_id);

        let startup_state = RpcRequest::new(RpcCommand::GetState);
        self.startups.insert(
            run_id,
            StartupHandshake {
                request_id: startup_state.id.clone(),
                deadline: Instant::now()
                    + Duration::from_millis(self.limits.startup_rpc_deadline_ms),
            },
        );
        for request in [
            startup_state,
            RpcRequest::new(RpcCommand::GetAvailableModels),
            RpcRequest::new(RpcCommand::GetAvailableThinkingLevels),
            RpcRequest::new(RpcCommand::GetCommands),
        ] {
            if let Err(error) = self.send_request(run_id, request) {
                self.begin_transport_failure(
                    run_id,
                    RunFailureKind::Protocol,
                    &format!("failed queuing startup RPC probe: {error}"),
                );
                break;
            }
        }
        Ok(run_id)
    }

    fn edit_draft(&mut self, run_id: RunId, text: String) -> Result<DraftSnapshot, String> {
        self.store
            .get(run_id)
            .ok_or_else(|| format!("run {run_id} is not registered"))?;
        if self.pending_session_replacements.contains_key(&run_id) {
            return Err("session replacement is waiting for draft durability".to_owned());
        }
        if self.run_draft_restore_pending(run_id) {
            return Err("session draft restore is still pending".to_owned());
        }
        self.drafts
            .edit_run(run_id, text)
            .map_err(|error| error.to_string())?;
        self.schedule_draft_save_for_run(run_id);
        let snapshot = self
            .drafts
            .snapshot_run(run_id)
            .ok_or_else(|| format!("run {run_id} has no active draft"))?;
        self.push_semantic(run_id, RuntimeUiEvent::DraftChanged { run_id });
        Ok(snapshot)
    }

    fn attach_draft_image(
        &mut self,
        run_id: RunId,
        file_name: String,
        mime_type: String,
        data: String,
    ) -> Result<DraftSnapshot, String> {
        self.store
            .get(run_id)
            .ok_or_else(|| format!("run {run_id} is not registered"))?;
        if self.pending_session_replacements.contains_key(&run_id) {
            return Err("session replacement is waiting for draft durability".to_owned());
        }
        if self.run_draft_restore_pending(run_id) {
            return Err("session draft restore is still pending".to_owned());
        }
        let image = DraftImageData::try_new(file_name, mime_type, data, self.limits)
            .map_err(|error| error.to_string())?;
        self.drafts
            .attach_image_run(run_id, image)
            .map_err(|error| error.to_string())?;
        self.schedule_draft_save_for_run(run_id);
        let snapshot = self
            .drafts
            .snapshot_run(run_id)
            .ok_or_else(|| format!("run {run_id} has no active draft"))?;
        self.push_semantic(run_id, RuntimeUiEvent::DraftChanged { run_id });
        Ok(snapshot)
    }

    fn remove_draft_image(
        &mut self,
        run_id: RunId,
        image_id: DraftImageId,
    ) -> Result<DraftSnapshot, String> {
        self.store
            .get(run_id)
            .ok_or_else(|| format!("run {run_id} is not registered"))?;
        if self.pending_session_replacements.contains_key(&run_id) {
            return Err("session replacement is waiting for draft durability".to_owned());
        }
        if self.run_draft_restore_pending(run_id) {
            return Err("session draft restore is still pending".to_owned());
        }
        self.drafts
            .remove_image_run(run_id, image_id)
            .map_err(|error| error.to_string())?;
        self.schedule_draft_save_for_run(run_id);
        let snapshot = self
            .drafts
            .snapshot_run(run_id)
            .ok_or_else(|| format!("run {run_id} has no active draft"))?;
        self.push_semantic(run_id, RuntimeUiEvent::DraftChanged { run_id });
        Ok(snapshot)
    }

    fn begin_managed_request(
        &mut self,
        run_id: RunId,
        request: RpcRequest,
        reply: oneshot::Sender<Result<ManagedRpcCompletion, String>>,
    ) {
        if self.pending_session_replacements.contains_key(&run_id) {
            let _ = reply.send(Err(format!(
                "run {run_id} is waiting for draft durability before session replacement"
            )));
            return;
        }
        if request.command.concurrency_class() == RpcConcurrencyClass::SessionReplacement
            && self.draft_persistence.is_some()
        {
            self.begin_durable_session_replacement(run_id, request, reply);
            return;
        }
        self.dispatch_request_with_waiter(run_id, request, reply);
    }

    fn dispatch_request_with_waiter(
        &mut self,
        run_id: RunId,
        request: RpcRequest,
        reply: oneshot::Sender<Result<ManagedRpcCompletion, String>>,
    ) {
        if let Err(error) = self.send_request(run_id, request.clone()) {
            let _ = reply.send(Err(error));
        } else {
            self.request_waiters
                .insert((run_id, request.id.as_str().to_owned()), reply);
        }
    }

    fn begin_durable_session_replacement(
        &mut self,
        run_id: RunId,
        request: RpcRequest,
        reply: oneshot::Sender<Result<ManagedRpcCompletion, String>>,
    ) {
        if self.run_draft_restore_pending(run_id) {
            let _ = reply.send(Err("session draft restore is still pending".to_owned()));
            return;
        }
        let Some(session_id) = self.drafts.current_session_id(run_id).map(str::to_owned) else {
            let _ = reply.send(Err(format!(
                "run {run_id} has no authoritative session id for durable replacement"
            )));
            return;
        };
        let Some(snapshot) = self.drafts.snapshot_session(&session_id) else {
            let _ = reply.send(Err(format!("session {session_id} has no draft record")));
            return;
        };
        if snapshot.durability == DraftDurability::Saved {
            self.dispatch_request_with_waiter(run_id, request, reply);
            return;
        }

        self.pending_session_replacements.insert(
            run_id,
            PendingSessionReplacement {
                request,
                session_id: session_id.clone(),
                deadline: Instant::now()
                    + Duration::from_millis(self.limits.draft_flush_deadline_ms),
                reply,
            },
        );
        self.draft_save_deadlines.remove(&session_id);
        if snapshot.durability != DraftDurability::Saving {
            self.start_draft_save(&session_id);
        }
        self.continue_pending_session_replacements(&session_id);
    }

    fn continue_pending_session_replacements(&mut self, session_id: &str) {
        let run_ids: Vec<_> = self
            .pending_session_replacements
            .iter()
            .filter_map(|(run_id, pending)| (pending.session_id == session_id).then_some(*run_id))
            .collect();
        for run_id in run_ids {
            let Some(snapshot) = self.drafts.snapshot_session(session_id) else {
                self.fail_pending_session_replacement(
                    run_id,
                    &format!("session {session_id} draft disappeared during replacement flush"),
                );
                continue;
            };
            match snapshot.durability {
                DraftDurability::Saved => {
                    let pending = self
                        .pending_session_replacements
                        .remove(&run_id)
                        .expect("pending replacement exists");
                    self.dispatch_request_with_waiter(run_id, pending.request, pending.reply);
                }
                DraftDurability::Dirty => self.start_draft_save(session_id),
                DraftDurability::Saving => {}
                DraftDurability::Failed => {
                    let detail = snapshot
                        .persistence_error
                        .as_deref()
                        .unwrap_or("draft persistence failed before session replacement");
                    self.fail_pending_session_replacement(run_id, detail);
                }
            }
        }
    }

    fn fail_pending_session_replacement(&mut self, run_id: RunId, detail: &str) {
        if let Some(pending) = self.pending_session_replacements.remove(&run_id) {
            let _ = pending.reply.send(Err(detail.to_owned()));
        }
    }

    fn queue_draft_load_for_run(&mut self, run_id: RunId) {
        let Some(worker) = self.draft_persistence.as_ref() else {
            return;
        };
        let Some(session_id) = self.drafts.current_session_id(run_id).map(str::to_owned) else {
            return;
        };
        if !self.draft_load_attempted.insert(session_id.clone()) {
            return;
        }
        if let Err(error) = worker.try_load(session_id.clone()) {
            let detail = error.to_string();
            let _ = self
                .drafts
                .mark_session_persistence_failed(&session_id, &detail);
            self.signal_draft_session(&session_id);
        } else {
            self.draft_load_pending.insert(session_id);
        }
    }

    fn run_draft_restore_pending(&self, run_id: RunId) -> bool {
        self.drafts
            .current_session_id(run_id)
            .is_some_and(|session_id| self.draft_load_pending.contains(session_id))
    }

    fn schedule_draft_save_for_run(&mut self, run_id: RunId) {
        if self.draft_persistence.is_none() {
            return;
        }
        let Some(snapshot) = self.drafts.snapshot_run(run_id) else {
            return;
        };
        if snapshot.durability != DraftDurability::Dirty {
            return;
        }
        let Some(session_id) = self.drafts.current_session_id(run_id).map(str::to_owned) else {
            return;
        };
        self.draft_save_deadlines.insert(
            session_id,
            Instant::now() + Duration::from_millis(self.limits.draft_save_debounce_ms),
        );
    }

    fn start_draft_save(&mut self, session_id: &str) {
        let Some(worker) = self.draft_persistence.as_ref() else {
            return;
        };
        let save = match self.drafts.begin_save_session(session_id) {
            Ok(save) => save,
            Err(DraftStoreError::Draft(
                DraftError::AlreadySaved { .. } | DraftError::SaveInFlight { .. },
            )) => return,
            Err(error) => {
                let detail = error.to_string();
                let _ = self
                    .drafts
                    .mark_session_persistence_failed(session_id, &detail);
                self.signal_draft_session(session_id);
                return;
            }
        };
        if let Err(error) = worker.try_save(
            session_id.to_owned(),
            save.generation,
            save.text,
            save.images,
        ) {
            let detail = error.to_string();
            let _ = self.drafts.complete_save_session(
                session_id,
                save.generation,
                Err(detail.as_str()),
            );
        }
        self.signal_draft_session(session_id);
    }

    fn handle_draft_persistence_event(&mut self, event: DraftPersistenceEvent) {
        match event {
            DraftPersistenceEvent::Loaded { session_id, result } => {
                self.draft_load_pending.remove(&session_id);
                match result {
                    Ok(Some(loaded)) => {
                        let _ = self.drafts.restore_session_if_unedited(&session_id, loaded);
                    }
                    Ok(None) => {}
                    Err(detail) => {
                        let _ = self
                            .drafts
                            .mark_session_persistence_failed(&session_id, &detail);
                    }
                }
                self.signal_draft_session(&session_id);
            }
            DraftPersistenceEvent::Saved {
                session_id,
                generation,
                result,
            } => {
                let completion = match &result {
                    Ok(()) => Ok(()),
                    Err(detail) => Err(detail.as_str()),
                };
                match self
                    .drafts
                    .complete_save_session(&session_id, generation, completion)
                {
                    Ok(()) | Err(DraftStoreError::Draft(DraftError::StaleCompletion { .. })) => {}
                    Err(error) => {
                        let detail = error.to_string();
                        let _ = self
                            .drafts
                            .mark_session_persistence_failed(&session_id, &detail);
                    }
                }
                self.signal_draft_session(&session_id);
                self.continue_pending_session_replacements(&session_id);
            }
        }
    }

    fn signal_draft_session(&mut self, session_id: &str) {
        for run_id in self.drafts.run_ids_for_session(session_id) {
            self.push_semantic(run_id, RuntimeUiEvent::DraftChanged { run_id });
        }
    }

    fn fail_draft_persistence_worker(&mut self, detail: &str) {
        self.draft_persistence = None;
        self.draft_load_pending.clear();
        self.draft_save_deadlines.clear();
        let pending_replacements: Vec<_> =
            self.pending_session_replacements.keys().copied().collect();
        for run_id in pending_replacements {
            self.fail_pending_session_replacement(run_id, detail);
        }
        let session_ids = self.drafts.unsaved_sessions();
        for session_id in session_ids {
            let _ = self
                .drafts
                .mark_session_persistence_failed(&session_id, detail);
            self.signal_draft_session(&session_id);
        }
        let in_flight = self.drafts.fail_in_flight_saves(detail);
        for session_id in in_flight {
            self.signal_draft_session(&session_id);
        }
    }

    fn begin_composer_submission(
        &mut self,
        run_id: RunId,
        action: ComposerAction,
        reply: oneshot::Sender<Result<ComposerSubmitResult, String>>,
    ) {
        if self.composer_submissions.contains_key(&run_id) {
            let _ = reply.send(Err(format!(
                "run {run_id} already has a composer submission in flight"
            )));
            return;
        }
        if self.pending_session_replacements.contains_key(&run_id) {
            let _ = reply.send(Err(
                "session replacement is waiting for draft durability".to_owned()
            ));
            return;
        }
        if self.run_draft_restore_pending(run_id) {
            let _ = reply.send(Err("session draft restore is still pending".to_owned()));
            return;
        }

        let (availability, model_supports_images) = match self.store.get(run_id) {
            Some(run) => (
                run.composer_availability(),
                run.session_state()
                    .model
                    .as_ref()
                    .and_then(|model| model.supports_images),
            ),
            None => {
                let _ = reply.send(Err(format!("run {run_id} is not registered")));
                return;
            }
        };
        let draft = match self.drafts.submission_run(run_id) {
            Ok(draft) => draft,
            Err(error) => {
                let _ = reply.send(Err(error.to_string()));
                return;
            }
        };
        if draft.text.trim().is_empty() && draft.images.is_empty() {
            let _ = reply.send(Err("composer draft is empty".to_owned()));
            return;
        }
        if !draft.images.is_empty() && model_supports_images == Some(false) {
            let _ = reply.send(Err(
                "current Pi model is configured for text-only input; remove images or switch to an image-capable model"
                    .to_owned(),
            ));
            return;
        }

        let is_extension_command = self.controllers.get(&run_id).is_some_and(|controller| {
            Self::draft_is_extension_command(&draft.text, controller.capabilities())
        });
        let action_allowed = match availability {
            ComposerAvailability::Ready => action == ComposerAction::Send,
            ComposerAvailability::AgentWorking if is_extension_command => {
                action == ComposerAction::RunCommand
            }
            ComposerAvailability::AgentWorking => {
                matches!(action, ComposerAction::Steer | ComposerAction::FollowUp)
            }
            ComposerAvailability::BlockedByCompaction | ComposerAvailability::Unavailable => false,
        };
        if !action_allowed {
            let detail = if availability == ComposerAvailability::AgentWorking
                && is_extension_command
            {
                "Pi extension commands cannot be queued through steer/follow-up; use Run command"
                    .to_owned()
            } else if availability == ComposerAvailability::AgentWorking
                && action == ComposerAction::RunCommand
            {
                "Run command is reserved for discovered Pi extension commands while the agent is working"
                    .to_owned()
            } else {
                format!(
                    "composer action {action:?} is not valid while availability is {availability:?}"
                )
            };
            let _ = reply.send(Err(detail));
            return;
        }

        let command = match action {
            ComposerAction::Send => RpcCommand::Prompt {
                message: draft.text,
                images: draft.images,
                streaming_behavior: None,
            },
            ComposerAction::Steer => RpcCommand::Steer {
                message: draft.text,
                images: draft.images,
            },
            ComposerAction::FollowUp => RpcCommand::FollowUp {
                message: draft.text,
                images: draft.images,
            },
            ComposerAction::RunCommand => RpcCommand::Prompt {
                message: draft.text,
                images: draft.images,
                streaming_behavior: None,
            },
        };
        let request = RpcRequest::new(command);
        if let Err(error) = self.send_request(run_id, request.clone()) {
            let _ = reply.send(Err(error));
            return;
        }
        self.composer_submissions.insert(
            run_id,
            ActiveComposerSubmission {
                request_id: request.id,
                action,
                draft_generation: draft.generation,
                reply,
            },
        );
        self.push_semantic(run_id, RuntimeUiEvent::ComposerChanged { run_id });
    }

    fn draft_is_extension_command(text: &str, capabilities: &super::RunCapabilities) -> bool {
        let Some(name) = Self::slash_command_name(text) else {
            return false;
        };
        capabilities.commands().is_some_and(|commands| {
            commands
                .iter()
                .any(|command| command.source == "extension" && command.name == name)
        })
    }

    fn slash_command_name(text: &str) -> Option<&str> {
        let command = text.trim_start().strip_prefix('/')?;
        let end = command.find(char::is_whitespace).unwrap_or(command.len());
        (end > 0).then_some(&command[..end])
    }

    fn send_request(&mut self, run_id: RunId, request: RpcRequest) -> Result<(), String> {
        let controller = self
            .controllers
            .get_mut(&run_id)
            .ok_or_else(|| format!("run {run_id} has no RPC controller"))?;
        controller
            .begin_request(&request, None)
            .map_err(|error| error.to_string())?;
        let result = self
            .processes
            .get(&run_id)
            .ok_or_else(|| format!("run {run_id} has no live process"))?
            .send_request(request.clone())
            .map_err(|error| error.to_string());
        if result.is_err() {
            controller.cancel_request(&request.id);
        }
        result
    }

    fn bootstrap_session_sync_if_needed(
        &mut self,
        run_id: RunId,
        expected_session_id: &str,
        cursor: String,
        leaf_id: Option<String>,
    ) -> Result<(), String> {
        let current_session_id = self
            .store
            .get(run_id)
            .and_then(|run| run.session_state().session_id.as_deref())
            .ok_or_else(|| format!("run {run_id} has no authoritative session id"))?;
        if current_session_id != expected_session_id {
            return Err(format!(
                "run {run_id} changed Pi sessions before history synchronization could start"
            ));
        }
        let revision = {
            let controller = self
                .controllers
                .get_mut(&run_id)
                .ok_or_else(|| format!("run {run_id} has no RPC controller"))?;
            let sync = controller.session_sync_state();
            if sync.initialized() && !sync.resync_required() {
                return Ok(());
            }
            controller
                .seed_session_sync(Some(cursor.clone()), leaf_id)
                .map_err(|error| error.to_string())?;
            controller.session_sync_state().revision()
        };
        self.push_semantic(
            run_id,
            RuntimeUiEvent::SessionSyncChanged {
                run_id,
                revision,
                resync_required: false,
            },
        );
        // Catch the narrow append race between the file snapshot and seed. The
        // actor queues this while it still owns the same session identity, so
        // a replacement command cannot cross between seed and request.
        self.send_request(
            run_id,
            RpcRequest::new(RpcCommand::GetEntries {
                since: Some(cursor),
            }),
        )?;
        Ok(())
    }

    fn queue_incremental_session_sync(&mut self, run_id: RunId) {
        if self.shutdown.is_some() || self.stops.contains_key(&run_id) {
            return;
        }
        let cursor = self.controllers.get(&run_id).and_then(|controller| {
            let sync = controller.session_sync_state();
            (sync.initialized() && !sync.resync_required())
                .then(|| sync.cursor().map(str::to_owned))
                .flatten()
        });
        let Some(cursor) = cursor else {
            return;
        };
        let _ = self.send_request(
            run_id,
            RpcRequest::new(RpcCommand::GetEntries {
                since: Some(cursor),
            }),
        );
    }

    fn queue_extension_response(
        &mut self,
        run_id: RunId,
        response: ExtensionUiResponse,
    ) -> Result<(), String> {
        let request_id = extension_response_id(&response);
        let pending = self
            .controllers
            .get(&run_id)
            .ok_or_else(|| format!("run {run_id} has no RPC controller"))?
            .pending_extension_dialogs()
            .any(|dialog| dialog.id == request_id);
        if !pending {
            return Err(format!(
                "extension dialog {request_id} is not pending for run {run_id}"
            ));
        }
        if self
            .extension_waiters
            .contains_key(&(run_id, request_id.to_owned()))
        {
            return Err(format!(
                "extension dialog {request_id} already has a response write in flight"
            ));
        }
        self.processes
            .get(&run_id)
            .ok_or_else(|| format!("run {run_id} has no live process"))?
            .send_extension_ui_response(response)
            .map_err(|error| error.to_string())
    }

    fn begin_stop(
        &mut self,
        run_id: RunId,
        reply: oneshot::Sender<Result<RuntimeStopResult, String>>,
    ) {
        if self.stops.contains_key(&run_id) {
            let _ = reply.send(Err(format!("run {run_id} already has a Stop transaction")));
            return;
        }
        self.fail_pending_session_replacement(
            run_id,
            "Stop superseded the pending session replacement",
        );
        match StopTransaction::begin(run_id, Instant::now(), self.limits, &mut self.store) {
            Ok((transaction, directive)) => {
                // Stop owns the run from this point. A startup handshake timer
                // must not race the intentional termination path and relabel it
                // as a protocol failure.
                self.startups.remove(&run_id);
                self.stops.insert(run_id, ActiveStop { transaction, reply });
                self.push_state_changed(run_id);
                self.apply_stop_directive(run_id, directive);
            }
            Err(error) => {
                let _ = reply.send(Err(error.to_string()));
            }
        }
    }

    fn resignal_dirty(&mut self, run_id: RunId) {
        if let Some(queue) = self.ui.get_mut(&run_id) {
            queue.dirty_signaled = true;
            let _ = self.signals.send(RuntimeManagerSignal::RunDirty { run_id });
        }
    }

    fn apply_stop_directive(&mut self, run_id: RunId, directive: StopDirective) {
        match directive {
            StopDirective::Send(request) => {
                if let Err(error) = self.send_request(run_id, request) {
                    self.begin_transport_failure(
                        run_id,
                        RunFailureKind::Stop,
                        &format!("Stop RPC could not be queued: {error}"),
                    );
                }
            }
            StopDirective::WaitForAgentSettled | StopDirective::None => {}
            StopDirective::Complete { recovered } => {
                self.finish_stop(run_id, RuntimeStopResult::normal(recovered));
            }
            StopDirective::TerminateProcess {
                termination_deadline,
                ..
            } => {
                self.push_state_changed(run_id);
                if let Some(process) = self.processes.get(&run_id) {
                    let _ = process.terminate(termination_deadline);
                }
            }
        }
    }

    fn handle_process_event(&mut self, envelope: RunProcessEnvelope) {
        let run_id = envelope.run_id;
        match envelope.event {
            RunProcessEvent::Inbound(message) => {
                if let Err(error) = self.handle_inbound(run_id, message) {
                    self.begin_transport_failure(run_id, RunFailureKind::Protocol, &error);
                }
            }
            RunProcessEvent::RequestWriteFailed { request_id, detail } => {
                if let Some(controller) = self.controllers.get_mut(&run_id) {
                    controller.cancel_request(&request_id);
                }
                self.fail_request_waiter(run_id, &request_id, &detail);
                self.fail_composer_submission(run_id, Some(&request_id), &detail);
                self.begin_transport_failure(
                    run_id,
                    RunFailureKind::Protocol,
                    &format!("Pi RPC write failed: {detail}"),
                );
            }
            RunProcessEvent::ExtensionUiResponseWritten { request_id } => {
                let result = self
                    .controllers
                    .get_mut(&run_id)
                    .ok_or_else(|| format!("run {run_id} has no RPC controller"))
                    .and_then(|controller| {
                        controller
                            .complete_extension_ui_response(&request_id, &mut self.store)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    });
                if result.is_ok() {
                    self.push_semantic(run_id, RuntimeUiEvent::ExtensionDialogsChanged { run_id });
                    self.push_state_changed(run_id);
                }
                if let Some(reply) = self.extension_waiters.remove(&(run_id, request_id)) {
                    let _ = reply.send(result);
                }
            }
            RunProcessEvent::ExtensionUiResponseWriteFailed { request_id, detail } => {
                if let Some(reply) = self.extension_waiters.remove(&(run_id, request_id)) {
                    let _ = reply.send(Err(detail.clone()));
                }
                self.begin_transport_failure(
                    run_id,
                    RunFailureKind::Protocol,
                    &format!("extension UI response write failed: {detail}"),
                );
            }
            RunProcessEvent::Exited { code, .. } => self.finalize_natural_exit(run_id, code),
            RunProcessEvent::ProtocolFailure {
                detail,
                termination,
            } => {
                self.pending_failures.insert(
                    run_id,
                    RunFailure::from_runtime_limits(RunFailureKind::Protocol, &detail, self.limits),
                );
                self.finalize_termination_report(run_id, termination);
            }
            RunProcessEvent::TerminationFinished { result } => match result {
                Ok(report) => self.finalize_termination_report(run_id, report),
                Err(detail) => self.finalize_uncertain_termination(run_id, &detail),
            },
        }
    }

    fn handle_inbound(&mut self, run_id: RunId, message: InboundMessage) -> Result<(), String> {
        match message {
            InboundMessage::Event(event) => {
                let settled = event.kind == crate::rpc::RpcEventKind::AgentSettled;
                let effect = self
                    .controllers
                    .get_mut(&run_id)
                    .ok_or_else(|| format!("run {run_id} has no RPC controller"))?
                    .apply_event(&event, &mut self.store)
                    .map_err(|error| error.to_string())?;
                self.apply_rpc_effect(run_id, effect);
                if settled {
                    self.handle_stop_settled(run_id);
                    self.queue_incremental_session_sync(run_id);
                }
            }
            InboundMessage::Response(response) => self.handle_response(run_id, response)?,
        }
        Ok(())
    }

    fn handle_response(&mut self, run_id: RunId, response: RpcResponse) -> Result<(), String> {
        let response_id = response
            .id
            .clone()
            .ok_or_else(|| "correlated Pi response is missing id".to_owned())?;
        let completed = self
            .controllers
            .get_mut(&run_id)
            .ok_or_else(|| format!("run {run_id} has no RPC controller"))?
            .complete_response(&response, &mut self.store)
            .map_err(|error| error.to_string())?;
        let completes_startup = self.startups.get(&run_id).is_some_and(|startup| {
            startup.request_id.as_str() == response_id && response.command == "get_state"
        });
        if completes_startup {
            self.startups.remove(&run_id);
            if completed.outcome != RpcResponseOutcome::Accepted {
                self.begin_transport_failure(
                    run_id,
                    RunFailureKind::Protocol,
                    "Pi rejected the startup get_state handshake",
                );
                return Ok(());
            }
        }
        if completed.outcome == RpcResponseOutcome::Accepted && response.command == "get_state" {
            let session_id = self
                .store
                .get(run_id)
                .and_then(|run| run.session_state().session_id.clone())
                .ok_or_else(|| format!("run {run_id} get_state did not establish a session id"))?;
            self.drafts
                .reconcile_session(run_id, session_id)
                .map_err(|error| error.to_string())?;
            self.queue_draft_load_for_run(run_id);
            self.schedule_draft_save_for_run(run_id);
        }
        if completes_startup {
            self.store
                .apply(run_id, RunMutation::ProcessReady)
                .map_err(|error| error.to_string())?;
        }
        let reconcile_replaced_session = completed.outcome == RpcResponseOutcome::Accepted
            && completed.active.class == RpcConcurrencyClass::SessionReplacement;

        if completed.capabilities_changed {
            let revision = self
                .controllers
                .get(&run_id)
                .expect("controller exists after response")
                .capabilities()
                .revision();
            self.push_semantic(
                run_id,
                RuntimeUiEvent::CapabilitiesChanged { run_id, revision },
            );
        }
        let mut session_entries = None;
        let mut session_resync_required = false;
        if let Some(sync) = completed.session_sync {
            match sync {
                SessionSyncCompletion::Page { page, applied } => {
                    session_entries = Some(page);
                    self.push_semantic(
                        run_id,
                        RuntimeUiEvent::SessionSyncChanged {
                            run_id,
                            revision: applied.revision,
                            resync_required: false,
                        },
                    );
                }
                SessionSyncCompletion::ResyncRequired { resync, .. } => {
                    session_resync_required = true;
                    self.push_semantic(
                        run_id,
                        RuntimeUiEvent::SessionSyncChanged {
                            run_id,
                            revision: resync.revision,
                            resync_required: true,
                        },
                    );
                }
                SessionSyncCompletion::Rejected { .. } => {}
            }
        }

        self.push_state_changed(run_id);
        self.handle_stop_response(run_id, &response);
        self.complete_composer_submission(run_id, &response, completed.outcome)?;

        // Pi's replacement responses report cancellation status but not the
        // identity of the session that is now active. Queue an authoritative
        // state observation before releasing the original completion to its
        // caller so any following command is ordered behind this reconciliation
        // in the process writer. Extension-cancelled replacements skip it.
        if reconcile_replaced_session
            && let Err(error) = self.send_request(run_id, RpcRequest::new(RpcCommand::GetState))
        {
            self.begin_transport_failure(
                run_id,
                RunFailureKind::Protocol,
                &format!("failed queuing post-session-replacement state reconciliation: {error}"),
            );
        }

        if let Some(reply) = self.request_waiters.remove(&(run_id, response_id)) {
            let _ = reply.send(Ok(ManagedRpcCompletion {
                response,
                session_entries,
                session_resync_required,
            }));
        }
        Ok(())
    }

    fn apply_rpc_effect(&mut self, run_id: RunId, effect: RunRpcEffect) {
        match effect {
            RunRpcEffect::None | RunRpcEffect::ForwardCompatibleIgnored => {}
            RunRpcEffect::AssistantMessageReset => {
                self.push_semantic(run_id, RuntimeUiEvent::AssistantMessageReset { run_id })
            }
            RunRpcEffect::AssistantBlockUpdated { content_index } => {
                if let Some(block) = self.controllers.get(&run_id).and_then(|controller| {
                    controller
                        .live_projection()
                        .assistant_block_snapshot(content_index)
                }) {
                    self.push_coalescible(
                        run_id,
                        UiCoalesceKey::AssistantBlock(content_index),
                        RuntimeUiEvent::AssistantBlockUpdated { run_id, block },
                    );
                }
            }
            RunRpcEffect::ToolUpdated { tool_call_id } => {
                if let Some(tool) = self.controllers.get(&run_id).and_then(|controller| {
                    controller.live_projection().tool_snapshot(&tool_call_id)
                }) {
                    self.push_coalescible(
                        run_id,
                        UiCoalesceKey::ToolPreview(tool_call_id),
                        RuntimeUiEvent::ToolUpdated { run_id, tool },
                    );
                }
            }
            RunRpcEffect::DirectBashUpdated { request_id } => {
                if let Some(bash) = self.controllers.get(&run_id).and_then(|controller| {
                    controller
                        .live_projection()
                        .direct_bash_snapshot(&request_id)
                }) {
                    self.push_coalescible(
                        run_id,
                        UiCoalesceKey::DirectBash(request_id),
                        RuntimeUiEvent::DirectBashUpdated { run_id, bash },
                    );
                }
            }
            RunRpcEffect::SemanticStateChanged => self.push_state_changed(run_id),
            RunRpcEffect::ToolFinished {
                tool_call_id,
                tool_name,
                preview,
                is_error,
            } => self.push_semantic(
                run_id,
                RuntimeUiEvent::ToolFinished {
                    run_id,
                    tool_call_id,
                    tool_name,
                    output: preview.output.as_str().to_owned(),
                    dropped_bytes: preview.output.dropped_bytes(),
                    is_error,
                },
            ),
            RunRpcEffect::ExtensionDialogRequested(_) => {
                self.push_semantic(run_id, RuntimeUiEvent::ExtensionDialogsChanged { run_id });
                self.push_state_changed(run_id);
            }
            RunRpcEffect::ExtensionNotification {
                message,
                notify_type,
            } => self.push_semantic(
                run_id,
                RuntimeUiEvent::ExtensionNotification {
                    run_id,
                    message,
                    notify_type,
                },
            ),
            RunRpcEffect::ExtensionUiStateChanged => {
                self.push_semantic(run_id, RuntimeUiEvent::ExtensionUiStateChanged { run_id })
            }
            RunRpcEffect::SetEditorText { text } => {
                if let Err(error) = self.drafts.edit_run(run_id, text.clone()) {
                    self.begin_transport_failure(
                        run_id,
                        RunFailureKind::Internal,
                        &format!("failed applying extension editor text to session draft: {error}"),
                    );
                    return;
                }
                self.schedule_draft_save_for_run(run_id);
                if let Some(queue) = self.ui.get_mut(&run_id) {
                    queue.pending_editor_text = Some(text);
                }
                self.push_semantic(run_id, RuntimeUiEvent::EditorTextChanged { run_id });
            }
        }
    }

    fn hydration_snapshot(&self) -> RuntimeHydrationSnapshot {
        let mut snapshot =
            RuntimeHydrationSnapshot::build(&self.store, &self.controllers, Instant::now());
        for run in &mut snapshot.runs {
            run.draft = self.drafts.snapshot_run(run.run.id());
            run.composer_submission_pending = self.composer_submissions.contains_key(&run.run.id());
            run.draft_restore_pending = self.run_draft_restore_pending(run.run.id());
        }
        snapshot
    }

    fn complete_composer_submission(
        &mut self,
        run_id: RunId,
        response: &RpcResponse,
        outcome: RpcResponseOutcome,
    ) -> Result<(), String> {
        let owns_response = self
            .composer_submissions
            .get(&run_id)
            .is_some_and(|submission| {
                response.id.as_deref() == Some(submission.request_id.as_str())
            });
        if !owns_response {
            return Ok(());
        }
        let submission = self
            .composer_submissions
            .remove(&run_id)
            .expect("composer submission ownership checked");
        let accepted = outcome == RpcResponseOutcome::Accepted;
        let draft_cleared = if accepted {
            matches!(
                self.drafts
                    .clear_run_if_generation(run_id, submission.draft_generation)
                    .map_err(|error| error.to_string())?,
                DraftClearOutcome::Cleared
            )
        } else {
            false
        };
        let result = ComposerSubmitResult {
            action: submission.action,
            accepted,
            draft_cleared,
            error: if accepted {
                None
            } else {
                response.error.clone()
            },
        };
        if draft_cleared {
            self.schedule_draft_save_for_run(run_id);
        }
        let _ = submission.reply.send(Ok(result));
        self.push_semantic(run_id, RuntimeUiEvent::ComposerChanged { run_id });
        Ok(())
    }

    fn handle_stop_response(&mut self, run_id: RunId, response: &RpcResponse) {
        let owns_response = self.stops.get(&run_id).is_some_and(|active| {
            let expected = match active.transaction.phase() {
                StopPhase::AwaitingClearQueue { request_id }
                | StopPhase::AwaitingAbort { request_id } => Some(request_id.as_str()),
                _ => None,
            };
            expected == response.id.as_deref()
        });
        if !owns_response {
            return;
        }
        let mut active = self.stops.remove(&run_id).expect("stop exists");
        match active.transaction.on_response(response, &mut self.store) {
            Ok(directive) => {
                self.stops.insert(run_id, active);
                self.push_state_changed(run_id);
                self.apply_stop_directive(run_id, directive);
            }
            Err(error) => {
                let _ = active.reply.send(Err(error.to_string()));
                self.begin_transport_failure(run_id, RunFailureKind::Stop, &error.to_string());
            }
        }
    }

    fn fail_composer_submission(
        &mut self,
        run_id: RunId,
        request_id: Option<&RequestId>,
        detail: &str,
    ) {
        let should_fail = self
            .composer_submissions
            .get(&run_id)
            .is_some_and(|submission| {
                request_id.is_none_or(|request_id| submission.request_id == *request_id)
            });
        if !should_fail {
            return;
        }
        if let Some(submission) = self.composer_submissions.remove(&run_id) {
            let _ = submission.reply.send(Err(detail.to_owned()));
            self.push_semantic(run_id, RuntimeUiEvent::ComposerChanged { run_id });
        }
    }

    fn handle_stop_settled(&mut self, run_id: RunId) {
        let Some(mut active) = self.stops.remove(&run_id) else {
            return;
        };
        let directive = active.transaction.on_agent_settled();
        self.stops.insert(run_id, active);
        self.apply_stop_directive(run_id, directive);
    }

    fn handle_deadlines(&mut self) {
        let now = Instant::now();
        let expired_replacements: Vec<_> = self
            .pending_session_replacements
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in expired_replacements {
            self.fail_pending_session_replacement(
                run_id,
                "draft flush deadline expired before session replacement",
            );
        }

        let due_drafts: Vec<_> = self
            .draft_save_deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(session_id, _)| session_id.clone())
            .collect();
        for session_id in due_drafts {
            self.draft_save_deadlines.remove(&session_id);
            self.start_draft_save(&session_id);
        }

        let startup_ids: Vec<_> = self
            .startups
            .iter()
            .filter(|(_, startup)| startup.deadline <= now)
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in startup_ids {
            self.startups.remove(&run_id);
            self.begin_transport_failure(
                run_id,
                RunFailureKind::Protocol,
                "Pi RPC startup handshake exceeded its deadline",
            );
        }

        let stop_ids: Vec<_> = self
            .stops
            .iter()
            .filter(|(_, stop)| stop.transaction.rpc_deadline() <= now)
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in stop_ids {
            let Some(mut active) = self.stops.remove(&run_id) else {
                continue;
            };
            match active.transaction.on_deadline(now, &mut self.store) {
                Ok(directive) => {
                    self.stops.insert(run_id, active);
                    self.push_state_changed(run_id);
                    self.apply_stop_directive(run_id, directive);
                }
                Err(error) => {
                    let _ = active.reply.send(Err(error.to_string()));
                    self.begin_transport_failure(run_id, RunFailureKind::Stop, &error.to_string());
                }
            }
        }

        let dialog_ids: Vec<_> = self
            .controllers
            .iter()
            .filter(|(_, controller)| {
                controller
                    .next_extension_dialog_expiry()
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in dialog_ids {
            let result = self
                .controllers
                .get_mut(&run_id)
                .expect("controller exists")
                .expire_extension_dialogs(now, &mut self.store);
            match result {
                Ok(expired) if !expired.is_empty() => {
                    for request_id in expired {
                        if let Some(reply) = self.extension_waiters.remove(&(run_id, request_id)) {
                            let _ = reply.send(Err("extension dialog timed out in Pi".to_owned()));
                        }
                    }
                    self.push_semantic(run_id, RuntimeUiEvent::ExtensionDialogsChanged { run_id });
                    self.push_state_changed(run_id);
                }
                Ok(_) => {}
                Err(error) => self.begin_transport_failure(
                    run_id,
                    RunFailureKind::Protocol,
                    &error.to_string(),
                ),
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.stops
            .values()
            .map(|stop| stop.transaction.rpc_deadline())
            .chain(self.startups.values().map(|startup| startup.deadline))
            .chain(
                self.pending_session_replacements
                    .values()
                    .map(|pending| pending.deadline),
            )
            .chain(self.draft_save_deadlines.values().copied())
            .chain(
                self.shutdown
                    .as_ref()
                    .map(|shutdown| shutdown.draft_deadline),
            )
            .chain(
                self.controllers
                    .values()
                    .filter_map(RunRpcController::next_extension_dialog_expiry),
            )
            .min()
    }

    fn begin_transport_failure(&mut self, run_id: RunId, kind: RunFailureKind, detail: &str) {
        self.startups.remove(&run_id);
        let process = self.store.get(run_id).map(RunRecord::process_state);
        if process.is_none() || process.is_some_and(ProcessState::is_terminal) {
            return;
        }
        self.pending_failures.insert(
            run_id,
            RunFailure::from_runtime_limits(kind, detail, self.limits),
        );
        if matches!(process, Some(ProcessState::Starting | ProcessState::Ready)) {
            let _ = self.store.apply(run_id, RunMutation::BeginStop);
        }
        if let Some(controller) = self.controllers.get_mut(&run_id) {
            controller.process_ended();
        }
        self.fail_run_waiters(run_id, detail);
        self.push_state_changed(run_id);
        if let Some(process) = self.processes.get(&run_id) {
            let _ = process.terminate(Duration::from_millis(
                self.limits.stop_termination_deadline_ms,
            ));
        }
    }

    fn finalize_natural_exit(&mut self, run_id: RunId, code: Option<i32>) {
        self.startups.remove(&run_id);
        let process_state = self.store.get(run_id).map(RunRecord::process_state);
        if process_state.is_none() || process_state.is_some_and(ProcessState::is_terminal) {
            self.processes.remove(&run_id);
            return;
        }
        if let Some(controller) = self.controllers.get_mut(&run_id) {
            controller.process_ended();
        }
        let mutation = if let Some(failure) = self.pending_failures.remove(&run_id) {
            RunMutation::ProcessFailed { failure }
        } else if process_state == Some(ProcessState::Stopping) {
            RunMutation::ProcessExited { code }
        } else {
            RunMutation::ProcessFailed {
                failure: RunFailure::from_runtime_limits(
                    RunFailureKind::UnexpectedExit,
                    &format!("Pi process exited unexpectedly with code {code:?}"),
                    self.limits,
                ),
            }
        };
        let _ = self.store.apply(run_id, mutation);
        self.processes.remove(&run_id);
        self.fail_run_waiters(run_id, "Pi process exited");
        self.finish_terminal_ui(run_id);
        self.finish_hard_stop(run_id, false);
    }

    fn finalize_termination_report(&mut self, run_id: RunId, report: ProcessTerminationReport) {
        match report {
            ProcessTerminationReport::Exited { code, .. } => {
                self.finalize_natural_exit(run_id, code);
            }
            ProcessTerminationReport::Unconfirmed { .. } => {
                self.finalize_uncertain_termination(
                    run_id,
                    "Pi process termination could not be confirmed",
                );
            }
        }
    }

    fn finalize_uncertain_termination(&mut self, run_id: RunId, detail: &str) {
        self.startups.remove(&run_id);
        let process = self.store.get(run_id).map(RunRecord::process_state);
        if process.is_none() || process.is_some_and(ProcessState::is_terminal) {
            self.processes.remove(&run_id);
            return;
        }
        if matches!(process, Some(ProcessState::Starting | ProcessState::Ready)) {
            let _ = self.store.apply(run_id, RunMutation::BeginStop);
        }
        if let Some(controller) = self.controllers.get_mut(&run_id) {
            controller.process_ended();
        }
        let failure = self.pending_failures.remove(&run_id).unwrap_or_else(|| {
            RunFailure::from_runtime_limits(RunFailureKind::Stop, detail, self.limits)
        });
        let _ = self
            .store
            .apply(run_id, RunMutation::ProcessQuarantined { failure });
        self.processes.remove(&run_id);
        self.fail_run_waiters(run_id, detail);
        self.finish_terminal_ui(run_id);
        self.finish_hard_stop(run_id, true);
    }

    fn finish_terminal_ui(&mut self, run_id: RunId) {
        if let Some(process) = self.store.get(run_id).map(RunRecord::process_state) {
            self.push_semantic(run_id, RuntimeUiEvent::ProcessTerminal { run_id, process });
            self.push_state_changed(run_id);
        }
    }

    fn finish_stop(&mut self, run_id: RunId, mut result: RuntimeStopResult) {
        self.restore_stop_recovery_into_draft(run_id, &mut result);
        if let Some(active) = self.stops.remove(&run_id) {
            let _ = active.reply.send(Ok(result));
        }
    }

    fn finish_hard_stop(&mut self, run_id: RunId, quarantined: bool) {
        if let Some(active) = self.stops.remove(&run_id) {
            let mut result =
                RuntimeStopResult::terminated(active.transaction.recovered(), quarantined);
            self.restore_stop_recovery_into_draft(run_id, &mut result);
            let _ = active.reply.send(Ok(result));
        }
    }

    fn restore_stop_recovery_into_draft(&mut self, run_id: RunId, result: &mut RuntimeStopResult) {
        let message_count = result
            .recovered_steering
            .len()
            .saturating_add(result.recovered_follow_up.len());
        if message_count == 0 {
            result.draft_restored = true;
            return;
        }
        if self.run_draft_restore_pending(run_id) {
            result.draft_restore_error = Some("session draft restore is still pending".to_owned());
            return;
        }
        let Some(draft) = self.drafts.snapshot_run(run_id) else {
            result.draft_restore_error = Some("active session draft is unavailable".to_owned());
            return;
        };
        let recovered_bytes = result
            .recovered_steering
            .iter()
            .chain(&result.recovered_follow_up)
            .fold(0usize, |total, message| total.saturating_add(message.len()));
        let separator_bytes = message_count
            .saturating_sub(1)
            .saturating_add(usize::from(!draft.text.is_empty()));
        let attempted = recovered_bytes
            .saturating_add(separator_bytes)
            .saturating_add(draft.text.len());
        if attempted > self.limits.max_draft_bytes_per_session {
            result.draft_restore_error = Some(format!(
                "recovered queue plus current draft is {attempted} bytes, exceeding draft limit {}",
                self.limits.max_draft_bytes_per_session
            ));
            return;
        }

        let mut merged = String::with_capacity(attempted);
        let mut first = true;
        for message in result
            .recovered_steering
            .iter()
            .chain(&result.recovered_follow_up)
        {
            if !first {
                merged.push('\n');
            }
            merged.push_str(message);
            first = false;
        }
        if !draft.text.is_empty() {
            if !first {
                merged.push('\n');
            }
            merged.push_str(&draft.text);
        }

        match self.drafts.edit_run(run_id, merged) {
            Ok(_) => {
                result.draft_restored = true;
                self.schedule_draft_save_for_run(run_id);
                self.push_semantic(run_id, RuntimeUiEvent::DraftChanged { run_id });
            }
            Err(error) => result.draft_restore_error = Some(error.to_string()),
        }
    }

    fn fail_request_waiter(&mut self, run_id: RunId, request_id: &RequestId, detail: &str) {
        if let Some(reply) = self
            .request_waiters
            .remove(&(run_id, request_id.as_str().to_owned()))
        {
            let _ = reply.send(Err(detail.to_owned()));
        }
    }

    fn fail_run_waiters(&mut self, run_id: RunId, detail: &str) {
        let request_keys: Vec<_> = self
            .request_waiters
            .keys()
            .filter(|(candidate, _)| *candidate == run_id)
            .cloned()
            .collect();
        for key in request_keys {
            if let Some(reply) = self.request_waiters.remove(&key) {
                let _ = reply.send(Err(detail.to_owned()));
            }
        }
        let extension_keys: Vec<_> = self
            .extension_waiters
            .keys()
            .filter(|(candidate, _)| *candidate == run_id)
            .cloned()
            .collect();
        for key in extension_keys {
            if let Some(reply) = self.extension_waiters.remove(&key) {
                let _ = reply.send(Err(detail.to_owned()));
            }
        }
        self.fail_pending_session_replacement(run_id, detail);
        self.fail_composer_submission(run_id, None, detail);
    }

    fn push_state_changed(&mut self, run_id: RunId) {
        let revision = self.store.revision();
        self.push_semantic(
            run_id,
            RuntimeUiEvent::StateChanged {
                run_id,
                runtime_revision: revision,
            },
        );
    }

    fn push_semantic(&mut self, run_id: RunId, event: RuntimeUiEvent) {
        let signal = self
            .ui
            .get_mut(&run_id)
            .is_some_and(|queue| queue.push_semantic(event));
        if signal {
            self.signal_dirty(run_id);
        }
    }

    fn push_coalescible(&mut self, run_id: RunId, key: UiCoalesceKey, event: RuntimeUiEvent) {
        let signal = self
            .ui
            .get_mut(&run_id)
            .is_some_and(|queue| queue.push_coalescible(key, event));
        if signal {
            self.signal_dirty(run_id);
        }
    }

    fn signal_dirty(&self, run_id: RunId) {
        let _ = self.signals.send(RuntimeManagerSignal::RunDirty { run_id });
    }

    fn begin_shutdown(&mut self, reply: oneshot::Sender<Result<RuntimeShutdownReport, String>>) {
        let target_runs = self.processes.keys().copied().collect();
        self.shutdown = Some(ShutdownState {
            reply,
            target_runs,
            draft_deadline: Instant::now()
                + Duration::from_millis(self.limits.draft_flush_deadline_ms),
        });
        self.flush_all_drafts_for_shutdown();
        self.terminate_all_for_shutdown();
    }

    fn flush_all_drafts_for_shutdown(&mut self) {
        if self.draft_persistence.is_none() {
            return;
        }
        let sessions = self.drafts.unsaved_sessions();
        for session_id in sessions {
            self.draft_save_deadlines.remove(&session_id);
            self.start_draft_save(&session_id);
        }
    }

    fn begin_unobserved_shutdown(&mut self) {
        if self.shutdown.is_some() {
            return;
        }
        let (reply, _receiver) = oneshot::channel();
        self.begin_shutdown(reply);
    }

    fn terminate_all_for_shutdown(&mut self) {
        let run_ids: Vec<_> = self.processes.keys().copied().collect();
        for run_id in run_ids {
            // Native app shutdown supersedes startup readiness deadlines.
            self.startups.remove(&run_id);
            let state = self.store.get(run_id).map(RunRecord::process_state);
            if matches!(state, Some(ProcessState::Starting | ProcessState::Ready)) {
                let _ = self.store.apply(run_id, RunMutation::BeginStop);
                self.push_state_changed(run_id);
            }
            if let Some(process) = self.processes.get(&run_id) {
                let _ = process.terminate(Duration::from_millis(
                    self.limits.stop_termination_deadline_ms,
                ));
            }
        }
    }

    fn finish_shutdown_if_ready(&mut self) -> bool {
        if self.shutdown.is_none() || !self.processes.is_empty() {
            return false;
        }
        let draft_deadline = self
            .shutdown
            .as_ref()
            .expect("checked shutdown")
            .draft_deadline;
        let draft_work_pending =
            self.drafts.has_saves_in_flight() || !self.draft_save_deadlines.is_empty();
        if draft_work_pending && Instant::now() < draft_deadline {
            return false;
        }
        if draft_work_pending {
            self.draft_save_deadlines.clear();
            let failed = self
                .drafts
                .fail_in_flight_saves("draft flush deadline expired during application shutdown");
            for session_id in failed {
                self.signal_draft_session(&session_id);
            }
        }
        let shutdown = self.shutdown.take().expect("checked shutdown");
        let report = RuntimeShutdownReport {
            terminal_runs: shutdown.target_runs.len(),
            quarantined_runs: shutdown
                .target_runs
                .iter()
                .filter(|run_id| {
                    self.store
                        .get(**run_id)
                        .is_some_and(|run| run.process_state() == ProcessState::Quarantined)
                })
                .count(),
            draft_flush_failed_sessions: self.drafts.failed_session_count(),
        };
        let _ = shutdown.reply.send(Ok(report));
        true
    }
}

fn extension_response_id(response: &ExtensionUiResponse) -> &str {
    match response {
        ExtensionUiResponse::Value { id, .. }
        | ExtensionUiResponse::Confirmation { id, .. }
        | ExtensionUiResponse::Cancelled { id } => id,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use tokio::sync::broadcast;
    use tokio::time::timeout;

    use super::*;
    use crate::draft_persistence::DraftFileStore;
    use crate::environment::{LaunchEnvironmentInput, resolve_launch_environment};
    use crate::launch::{PiLaunchSpec, ProjectTrustPolicy};
    use crate::rpc::RpcResponseOutcome;
    use crate::runtime::ActivityState;

    struct ManagerFixture {
        root: PathBuf,
        fake_pi: PathBuf,
    }

    impl ManagerFixture {
        fn new(name: &str) -> Self {
            Self::new_with_script(name, FAKE_PI_JS)
        }

        fn new_with_script(name: &str, source: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("pi-wizard-manager-{name}-{}", RunId::new()));
            fs::create_dir_all(&root).expect("create manager fixture root");
            let script = root.join("fake-pi.js");
            fs::write(&script, source).expect("write fake Pi JavaScript");

            #[cfg(windows)]
            let fake_pi = {
                let path = root.join("pi.cmd");
                fs::write(&path, "@echo off\r\nnode \"%~dp0fake-pi.js\"\r\n")
                    .expect("write fake Pi wrapper");
                path
            };

            #[cfg(not(windows))]
            let fake_pi = {
                use std::os::unix::fs::PermissionsExt;
                let path = root.join("pi");
                fs::write(
                    &path,
                    "#!/bin/sh\nexec node \"$(dirname \"$0\")/fake-pi.js\"\n",
                )
                .expect("write fake Pi wrapper");
                let mut permissions = fs::metadata(&path).expect("wrapper metadata").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&path, permissions).expect("wrapper permissions");
                path
            };

            Self { root, fake_pi }
        }

        fn environment(&self) -> ResolvedLaunchEnvironment {
            let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
            resolve_launch_environment(LaunchEnvironmentInput {
                configured_pi: Some(self.fake_pi.clone()),
                desktop_environment,
                ..LaunchEnvironmentInput::default()
            })
            .expect("resolve fake Pi environment")
        }

        fn launch(&self) -> ResolvedPiLaunchSpec {
            PiLaunchSpec::new(
                self.fake_pi.clone(),
                self.root.clone(),
                ProjectTrustPolicy::Ignore,
            )
            .resolve()
            .expect("resolve fake Pi launch")
        }

        fn start_spec(&self) -> RunStartSpec {
            RunStartSpec {
                project_id: ProjectId::new(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: self.launch(),
                environment: self.environment(),
            }
        }
    }

    impl Drop for ManagerFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    async fn synchronize(manager: &RuntimeManagerHandle, run_id: RunId, suffix: &str) {
        let completion = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire(format!("sync-{suffix}")),
                    RpcCommand::GetState,
                ),
            )
            .await
            .expect("synchronize through get_state");
        assert_eq!(completion.response.outcome(), RpcResponseOutcome::Accepted);
    }

    async fn drain_all(manager: &RuntimeManagerHandle, run_id: RunId) -> Vec<RuntimeUiEvent> {
        let mut events = Vec::new();
        loop {
            let drained = manager.drain_ui(run_id, 64).await.expect("drain UI");
            events.extend(drained.events);
            if !drained.has_more {
                return events;
            }
        }
    }

    async fn next_dirty(
        signals: &mut broadcast::Receiver<RuntimeManagerSignal>,
        expected_run: RunId,
    ) {
        let signal = timeout(Duration::from_secs(2), signals.recv())
            .await
            .expect("dirty signal deadline")
            .expect("dirty signal channel");
        assert_eq!(
            signal,
            RuntimeManagerSignal::RunDirty {
                run_id: expected_run
            }
        );
    }

    async fn wait_for_draft_durability(
        manager: &RuntimeManagerHandle,
        run_id: RunId,
        expected: DraftDurability,
        expected_text: Option<&str>,
    ) -> DraftSnapshot {
        timeout(Duration::from_secs(2), async {
            let mut signals = manager.subscribe();
            loop {
                let snapshot = manager.hydrate().await.expect("draft hydration");
                let run = &snapshot.runs[0];
                let draft = run.draft.clone().expect("draft");
                let text_matches = expected_text.is_none_or(|text| draft.text == text);
                if !run.draft_restore_pending && draft.durability == expected && text_matches {
                    return draft;
                }
                next_dirty(&mut signals, run_id).await;
                let _ = drain_all(manager, run_id).await;
            }
        })
        .await
        .expect("draft durability deadline")
    }

    #[test]
    fn manager_spawn_without_async_runtime_is_an_explicit_error() {
        assert!(matches!(
            spawn_runtime_manager(RuntimeLimits::default()),
            Err(RuntimeManagerError::AsyncRuntimeUnavailable)
        ));
    }

    #[tokio::test]
    async fn startup_rpc_handshake_must_succeed_before_ready_and_is_deadline_bounded() {
        let fixture = ManagerFixture::new_with_script("startup-timeout", SILENT_PI_JS);
        let limits = RuntimeLimits {
            startup_rpc_deadline_ms: 40,
            stop_termination_deadline_ms: 500,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("manager");
        let mut signals = manager.subscribe();
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("spawn silent child");

        let starting = manager.hydrate().await.expect("starting hydration");
        assert_eq!(starting.runs[0].run.process_state(), ProcessState::Starting);
        assert_eq!(
            starting.runs[0].run.composer_availability(),
            crate::runtime::ComposerAvailability::Unavailable
        );
        let _ = drain_all(&manager, run_id).await;
        while signals.try_recv().is_ok() {}

        // This is only the harness observation window. The production
        // readiness and termination deadlines above remain 40ms and 500ms.
        // Real-Git fixture tests run concurrently in the full suite and can
        // briefly starve this observer on loaded Windows CI/dev machines.
        timeout(Duration::from_secs(15), async {
            loop {
                let signal = signals.recv().await.expect("runtime signal");
                if signal != (RuntimeManagerSignal::RunDirty { run_id }) {
                    continue;
                }
                let events = drain_all(&manager, run_id).await;
                if events.iter().any(|event| {
                    matches!(
                        event,
                        RuntimeUiEvent::ProcessTerminal {
                            process: ProcessState::Failed,
                            ..
                        }
                    )
                }) {
                    break;
                }
            }
        })
        .await
        .expect("startup timeout reaches terminal failure");

        let failed = manager.hydrate().await.expect("failed hydration");
        assert_eq!(failed.runs[0].run.process_state(), ProcessState::Failed);
        manager.shutdown().await.expect("shutdown manager");
    }

    #[tokio::test]
    async fn manager_hydration_survives_renderer_style_reload_without_restarting_child() {
        let fixture = ManagerFixture::new("hydrate");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let mut signals = manager.subscribe();
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        next_dirty(&mut signals, run_id).await;
        synchronize(&manager, run_id, "startup").await;

        let first = manager.hydrate().await.expect("first hydration");
        let second = manager.hydrate().await.expect("second hydration");
        assert_eq!(first, second);
        assert_eq!(first.runs.len(), 1);
        let run = &first.runs[0];
        assert_eq!(run.run.id(), run_id);
        assert_eq!(run.run.process_state(), ProcessState::Ready);
        assert_eq!(run.run.activity_state(), ActivityState::Idle);
        assert_eq!(
            run.run.session_state().session_id.as_deref(),
            Some("fake-session")
        );
        let rpc = run.rpc.as_ref().expect("live RPC hydration");
        assert_eq!(
            rpc.capabilities.models().expect("models")[0].id,
            "fake-model"
        );
        assert_eq!(
            rpc.capabilities.commands().expect("commands")[0].name,
            "fake-command"
        );

        let prompt = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("prompt-after-reload"),
                    RpcCommand::Prompt {
                        message: "after reload".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("prompt after repeated hydration");
        assert_eq!(prompt.response.outcome(), RpcResponseOutcome::Accepted);
        synchronize(&manager, run_id, "post-reload").await;
        let after = manager.hydrate().await.expect("hydrate after prompt");
        assert_eq!(after.runs[0].run.process_state(), ProcessState::Ready);

        let report = manager.shutdown().await.expect("shutdown");
        assert_eq!(report.terminal_runs, 1);
        // This test owns renderer-style hydration behavior, not OS shutdown
        // certainty. Dedicated shutdown tests assert the non-quarantine path;
        // coupling that assertion here made parallel core runs timing-sensitive.
    }

    #[tokio::test]
    async fn composer_acceptance_clears_only_the_submitted_draft_generation() {
        let fixture = ManagerFixture::new("composer-generation");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "composer-generation-startup").await;
        let _ = drain_all(&manager, run_id).await;
        let mut signals = manager.subscribe();
        while signals.try_recv().is_ok() {}

        manager
            .edit_draft(run_id, "delayed-accept".to_owned())
            .await
            .expect("initial draft");
        let _ = drain_all(&manager, run_id).await;
        while signals.try_recv().is_ok() {}

        let submitting_manager = manager.clone();
        let submission = tokio::spawn(async move {
            submitting_manager
                .submit_draft(run_id, ComposerAction::Send)
                .await
        });
        next_dirty(&mut signals, run_id).await;
        let pending = manager.hydrate().await.expect("pending hydration");
        assert!(pending.runs[0].composer_submission_pending);

        manager
            .edit_draft(run_id, "typed while sending".to_owned())
            .await
            .expect("newer draft");
        let result = submission
            .await
            .expect("submission task")
            .expect("submission result");
        assert!(result.accepted);
        assert!(!result.draft_cleared);

        let after = manager.hydrate().await.expect("after submission");
        assert!(!after.runs[0].composer_submission_pending);
        assert_eq!(
            after.runs[0].draft.as_ref().expect("draft").text,
            "typed while sending"
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn second_live_run_cannot_reuse_same_worktree_execution_root() {
        let fixture = ManagerFixture::new("worktree-exclusive");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let canonical_root = fixture.root.canonicalize().expect("canonical fixture root");
        let identity = GitWorktreeIdentity {
            repository_root: canonical_root.clone(),
            worktree_root: canonical_root.clone(),
            branch: "agent/exclusive".to_owned(),
            base_commit: "abc123".to_owned(),
        };
        let mut spec = fixture.start_spec();
        spec.execution_isolation = ExecutionIsolation::GitWorktree;
        spec.worktree = Some(identity);
        let first = manager
            .start_run(spec.clone())
            .await
            .expect("first worktree run");
        synchronize(&manager, first, "worktree-exclusive-startup").await;

        let second = manager.start_run(spec).await;
        assert!(matches!(
            second,
            Err(RuntimeManagerError::Operation(message))
                if message.contains("already owned by a live run")
        ));
        let snapshot = manager.hydrate().await.expect("exclusive hydration");
        assert_eq!(snapshot.runs.len(), 1);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn text_only_model_rejects_image_submission_without_clearing_draft() {
        let fixture = ManagerFixture::new("composer-text-only-image");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "text-only-image-startup").await;
        manager
            .set_model(run_id, "fake-alt".to_owned(), "alt-model".to_owned())
            .await
            .expect("set text-only model");
        manager
            .attach_draft_image(
                run_id,
                "diagram.png".to_owned(),
                "image/png".to_owned(),
                "YWJj".to_owned(),
            )
            .await
            .expect("attachment remains valid draft data");

        let error = manager
            .submit_draft(run_id, ComposerAction::Send)
            .await
            .expect_err("text-only model must not silently omit draft image");
        assert!(error.to_string().contains("text-only input"));
        let after = manager.hydrate().await.expect("preserved draft hydration");
        let draft = after.runs[0].draft.as_ref().expect("draft");
        assert_eq!(draft.images.len(), 1);
        assert_eq!(
            after.runs[0]
                .run
                .session_state()
                .model
                .as_ref()
                .and_then(|model| model.supports_images),
            Some(false)
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn image_only_composer_submission_is_accepted_and_clears_attachment() {
        let fixture = ManagerFixture::new("composer-image-only");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "image-only-startup").await;
        let attached = manager
            .attach_draft_image(
                run_id,
                "screen.png".to_owned(),
                "image/png".to_owned(),
                "aGVsbG8=".to_owned(),
            )
            .await
            .expect("attach image");
        assert_eq!(attached.images.len(), 1);
        assert!(attached.text.is_empty());

        let submitted = manager
            .submit_draft(run_id, ComposerAction::Send)
            .await
            .expect("submit image-only draft");
        assert!(submitted.accepted);
        assert!(submitted.draft_cleared);
        let after = manager.hydrate().await.expect("after submit");
        assert!(
            after.runs[0]
                .draft
                .as_ref()
                .expect("draft")
                .images
                .is_empty()
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn attachment_added_while_submission_is_pending_survives_acceptance() {
        let fixture = ManagerFixture::new("composer-image-generation");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "image-generation-startup").await;
        manager
            .edit_draft(run_id, "delayed-accept".to_owned())
            .await
            .expect("draft");
        let _ = drain_all(&manager, run_id).await;

        let mut signals = manager.subscribe();
        while signals.try_recv().is_ok() {}
        let submitting_manager = manager.clone();
        let submission = tokio::spawn(async move {
            submitting_manager
                .submit_draft(run_id, ComposerAction::Send)
                .await
        });
        next_dirty(&mut signals, run_id).await;
        manager
            .attach_draft_image(
                run_id,
                "new.png".to_owned(),
                "image/png".to_owned(),
                "YWJj".to_owned(),
            )
            .await
            .expect("newer attachment");

        let result = submission
            .await
            .expect("submission task")
            .expect("submission result");
        assert!(result.accepted);
        assert!(!result.draft_cleared);
        let after = manager.hydrate().await.expect("after submission");
        let draft = after.runs[0].draft.as_ref().expect("draft");
        assert_eq!(draft.text, "delayed-accept");
        assert_eq!(draft.images.len(), 1);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn fork_session_returns_only_after_new_session_identity_is_observable() {
        let fixture = ManagerFixture::new("fork-control");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "fork-control-startup").await;

        let result = manager
            .fork_session(run_id, "fork-entry".to_owned())
            .await
            .expect("fork session");
        assert!(!result.cancelled);
        let snapshot = manager.hydrate().await.expect("forked hydration");
        assert_eq!(
            snapshot.runs[0].run.session_state().session_id.as_deref(),
            Some("forked-session")
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn run_controls_reconcile_model_thinking_and_session_name_from_pi() {
        let fixture = ManagerFixture::new("run-controls");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "run-controls-startup").await;

        manager
            .set_model(run_id, "fake-alt".to_owned(), "alt-model".to_owned())
            .await
            .expect("set model");
        manager
            .set_thinking_level(run_id, ThinkingLevel::High)
            .await
            .expect("set thinking");
        manager
            .set_auto_compaction(run_id, false)
            .await
            .expect("disable automatic compaction");
        manager
            .set_session_name(run_id, "Control Surface".to_owned())
            .await
            .expect("set name");

        let snapshot = manager.hydrate().await.expect("control hydration");
        let run = &snapshot.runs[0];
        let model = run
            .run
            .session_state()
            .model
            .as_ref()
            .expect("authoritative model");
        assert_eq!(model.provider, "fake-alt");
        assert_eq!(model.id, "alt-model");
        assert_eq!(model.supports_images, Some(false));
        assert_eq!(
            run.run.session_state().thinking_level,
            Some(ThinkingLevel::High)
        );
        assert_eq!(
            run.run.session_state().session_name.as_deref(),
            Some("Control Surface")
        );
        assert_eq!(run.run.session_state().auto_compaction_enabled, Some(false));
        assert_eq!(
            run.rpc
                .as_ref()
                .expect("rpc")
                .capabilities
                .thinking_levels()
                .expect("thinking levels"),
            &[
                ThinkingLevel::Off,
                ThinkingLevel::Medium,
                ThinkingLevel::High
            ]
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn manual_compaction_uses_pi_rpc_and_reconciles_idle_state() {
        let fixture = ManagerFixture::new("manual-compaction-control");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "manual-compaction-startup").await;

        let result = manager
            .compact_session(run_id)
            .await
            .expect("native Pi compaction");
        assert_eq!(result.first_kept_entry_id, "kept-entry");
        assert_eq!(result.tokens_before, 100);
        assert_eq!(result.estimated_tokens_after, 40);
        let snapshot = manager.hydrate().await.expect("post-compaction hydration");
        assert!(!snapshot.runs[0].run.is_compacting());
        assert_eq!(
            snapshot.runs[0].run.composer_availability(),
            ComposerAvailability::Ready
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn session_stats_are_requested_explicitly_and_keep_pi_context_usage() {
        let fixture = ManagerFixture::new("session-stats-control");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "session-stats-startup").await;

        let stats = manager.session_stats(run_id).await.expect("session stats");
        assert_eq!(stats.session_id, "fake-session");
        assert_eq!(stats.tokens.total, 150);
        assert_eq!(stats.context_usage.expect("context").percent, Some(25.0));
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn worktree_launch_retains_branch_base_and_root_identity_in_hydration() {
        let fixture = ManagerFixture::new("worktree-identity");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let canonical_root = fixture.root.canonicalize().expect("canonical fixture root");
        let identity = GitWorktreeIdentity {
            repository_root: canonical_root.clone(),
            worktree_root: canonical_root.clone(),
            branch: "agent/feature".to_owned(),
            base_commit: "abc123".to_owned(),
        };
        let mut spec = fixture.start_spec();
        spec.execution_isolation = ExecutionIsolation::GitWorktree;
        spec.worktree = Some(identity.clone());
        let run_id = manager.start_run(spec).await.expect("start worktree run");
        synchronize(&manager, run_id, "worktree-identity-startup").await;

        let snapshot = manager.hydrate().await.expect("worktree hydration");
        let run = snapshot
            .runs
            .iter()
            .find(|run| run.run.id() == run_id)
            .expect("run");
        assert_eq!(run.run.worktree_identity(), Some(&identity));
        assert_eq!(run.run.execution_root(), &canonical_root);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn clone_session_returns_only_after_new_session_identity_is_observable() {
        let fixture = ManagerFixture::new("clone-control");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "clone-control-startup").await;

        let result = manager.clone_session(run_id).await.expect("clone session");
        assert!(!result.cancelled);
        let snapshot = manager.hydrate().await.expect("cloned hydration");
        assert_eq!(
            snapshot.runs[0].run.session_state().session_id.as_deref(),
            Some("cloned-session")
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn working_extension_commands_use_prompt_instead_of_queue_rpcs() {
        let fixture = ManagerFixture::new("composer-extension-command");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "extension-command-startup").await;

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("extension-command-hold"),
                    RpcCommand::Prompt {
                        message: "hold".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("start working turn");
        synchronize(&manager, run_id, "extension-command-working").await;
        manager
            .edit_draft(run_id, " /fake-command inspect".to_owned())
            .await
            .expect("extension command draft");

        let queued = manager
            .submit_draft(run_id, ComposerAction::Steer)
            .await
            .expect_err("extension command must not use steer");
        assert!(queued.to_string().contains("cannot be queued"));
        let preserved = manager.hydrate().await.expect("preserved command draft");
        assert_eq!(
            preserved.runs[0].draft.as_ref().expect("draft").text,
            " /fake-command inspect"
        );

        let executed = manager
            .submit_draft(run_id, ComposerAction::RunCommand)
            .await
            .expect("extension command prompt accepted");
        assert!(executed.accepted);
        assert!(executed.draft_cleared);
        assert_eq!(executed.action, ComposerAction::RunCommand);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn composer_rejection_preserves_submitted_text_and_reports_failure() {
        let fixture = ManagerFixture::new("composer-reject");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "composer-reject-startup").await;
        manager
            .edit_draft(run_id, "reject".to_owned())
            .await
            .expect("draft");

        let result = manager
            .submit_draft(run_id, ComposerAction::Send)
            .await
            .expect("protocol rejection is a completed composer result");
        assert!(!result.accepted);
        assert!(!result.draft_cleared);
        assert_eq!(result.error.as_deref(), Some("fixture prompt rejection"));
        assert_eq!(
            manager.hydrate().await.expect("hydration").runs[0]
                .draft
                .as_ref()
                .expect("draft")
                .text,
            "reject"
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn persistent_manager_restores_session_draft_across_manager_restart() {
        let fixture = ManagerFixture::new("draft-restart");
        let persistence_root = fixture.root.join("app-state");
        let limits = RuntimeLimits {
            draft_save_debounce_ms: 20,
            draft_flush_deadline_ms: 500,
            ..RuntimeLimits::default()
        };

        let first = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("first persistent manager");
        let first_run = first
            .start_run(fixture.start_spec())
            .await
            .expect("first run");
        synchronize(&first, first_run, "draft-restart-first").await;
        wait_for_draft_durability(&first, first_run, DraftDurability::Saved, Some("")).await;
        first
            .edit_draft(first_run, "survive restart".to_owned())
            .await
            .expect("edit persistent draft");
        let saved = wait_for_draft_durability(
            &first,
            first_run,
            DraftDurability::Saved,
            Some("survive restart"),
        )
        .await;
        assert_eq!(saved.text, "survive restart");
        let first_report = first.shutdown().await.expect("first shutdown");
        assert_eq!(first_report.draft_flush_failed_sessions, 0);

        let second = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("second persistent manager");
        let second_run = second
            .start_run(fixture.start_spec())
            .await
            .expect("second run");
        synchronize(&second, second_run, "draft-restart-second").await;
        let restored = wait_for_draft_durability(
            &second,
            second_run,
            DraftDurability::Saved,
            Some("survive restart"),
        )
        .await;
        assert_eq!(restored.text, "survive restart");
        assert!(restored.persistence_error.is_none());
        second.shutdown().await.expect("second shutdown");
    }

    #[tokio::test]
    async fn persistent_manager_restores_image_attachment_across_restart() {
        let fixture = ManagerFixture::new("draft-image-restart");
        let persistence_root = fixture.root.join("app-state");
        let limits = RuntimeLimits {
            draft_save_debounce_ms: 20,
            draft_flush_deadline_ms: 500,
            ..RuntimeLimits::default()
        };
        let first = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("first manager");
        let first_run = first
            .start_run(fixture.start_spec())
            .await
            .expect("first run");
        synchronize(&first, first_run, "image-restart-first").await;
        wait_for_draft_durability(&first, first_run, DraftDurability::Saved, Some("")).await;
        first
            .attach_draft_image(
                first_run,
                "persist.png".to_owned(),
                "image/png".to_owned(),
                "aGVsbG8=".to_owned(),
            )
            .await
            .expect("attach image");
        let saved =
            wait_for_draft_durability(&first, first_run, DraftDurability::Saved, Some("")).await;
        assert_eq!(saved.images.len(), 1);
        first.shutdown().await.expect("first shutdown");

        let second = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("second manager");
        let second_run = second
            .start_run(fixture.start_spec())
            .await
            .expect("second run");
        synchronize(&second, second_run, "image-restart-second").await;
        let restored =
            wait_for_draft_durability(&second, second_run, DraftDurability::Saved, Some("")).await;
        assert_eq!(restored.images.len(), 1);
        assert_eq!(restored.images[0].file_name, "persist.png");
        assert_eq!(restored.images[0].decoded_bytes, 5);
        second.shutdown().await.expect("second shutdown");
    }

    #[tokio::test]
    async fn shutdown_flushes_dirty_draft_without_waiting_for_normal_debounce() {
        let fixture = ManagerFixture::new("draft-shutdown-flush");
        let persistence_root = fixture.root.join("app-state");
        let limits = RuntimeLimits {
            draft_save_debounce_ms: 60_000,
            draft_flush_deadline_ms: 1_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("persistent manager");
        let run_id = manager.start_run(fixture.start_spec()).await.expect("run");
        synchronize(&manager, run_id, "draft-shutdown-startup").await;
        manager
            .edit_draft(run_id, "flush on exit".to_owned())
            .await
            .expect("dirty draft");

        let report = manager.shutdown().await.expect("bounded shutdown");
        assert_eq!(report.draft_flush_failed_sessions, 0);
        let store = DraftFileStore::open(&persistence_root, limits).expect("file store");
        assert_eq!(
            store
                .load("fake-session")
                .expect("load flushed draft")
                .as_ref()
                .map(|draft| draft.text.as_str()),
            Some("flush on exit")
        );
    }

    #[tokio::test]
    async fn corrupt_persisted_draft_is_visible_failure_without_failing_pi_runtime() {
        let fixture = ManagerFixture::new("draft-corrupt-runtime");
        let persistence_root = fixture.root.join("app-state");
        let limits = RuntimeLimits {
            draft_save_debounce_ms: 20,
            ..RuntimeLimits::default()
        };
        let store = DraftFileStore::open(&persistence_root, limits).expect("file store");
        fs::write(store.path_for_session("fake-session"), b"{corrupt")
            .expect("corrupt draft fixture");

        let manager = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("persistent manager");
        let run_id = manager.start_run(fixture.start_spec()).await.expect("run");
        synchronize(&manager, run_id, "draft-corrupt-startup").await;
        let failed =
            wait_for_draft_durability(&manager, run_id, DraftDurability::Failed, None).await;
        assert!(
            failed
                .persistence_error
                .as_deref()
                .is_some_and(|detail| detail.contains("quarantined"))
        );
        assert_eq!(
            manager.hydrate().await.expect("runtime hydration").runs[0]
                .run
                .process_state(),
            ProcessState::Ready
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn session_replacement_flushes_current_draft_before_pi_switches_sessions() {
        let fixture = ManagerFixture::new("draft-before-session-replacement");
        let persistence_root = fixture.root.join("app-state");
        let limits = RuntimeLimits {
            draft_save_debounce_ms: 60_000,
            draft_flush_deadline_ms: 1_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("persistent manager");
        let run_id = manager.start_run(fixture.start_spec()).await.expect("run");
        synchronize(&manager, run_id, "durable-switch-startup").await;
        wait_for_draft_durability(&manager, run_id, DraftDurability::Saved, Some("")).await;
        manager
            .edit_draft(run_id, "old session draft".to_owned())
            .await
            .expect("dirty old-session draft");

        let replacement = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("durable-switch"),
                    RpcCommand::SwitchSession {
                        session_path: PathBuf::from("switched-session.jsonl"),
                    },
                ),
            )
            .await
            .expect("replacement waits for persistence then succeeds");
        assert_eq!(replacement.response.outcome(), RpcResponseOutcome::Accepted);

        let store = DraftFileStore::open(&persistence_root, limits).expect("file store");
        assert_eq!(
            store
                .load("fake-session")
                .expect("load old-session draft")
                .as_ref()
                .map(|draft| draft.text.as_str()),
            Some("old session draft")
        );
        synchronize(&manager, run_id, "durable-switch-reconciled").await;
        let after = manager.hydrate().await.expect("switched hydration");
        assert_eq!(
            after.runs[0].run.session_state().session_id.as_deref(),
            Some("switched-session")
        );
        assert_eq!(after.runs[0].draft.as_ref().expect("new draft").text, "");
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn failed_draft_flush_blocks_session_replacement_and_preserves_old_binding() {
        let fixture = ManagerFixture::new("draft-switch-save-failure");
        let persistence_root = fixture.root.join("app-state");
        let limits = RuntimeLimits {
            draft_save_debounce_ms: 60_000,
            draft_flush_deadline_ms: 1_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager_with_draft_persistence(limits, &persistence_root)
            .expect("persistent manager");
        let run_id = manager.start_run(fixture.start_spec()).await.expect("run");
        synchronize(&manager, run_id, "failed-switch-startup").await;
        wait_for_draft_durability(&manager, run_id, DraftDurability::Saved, Some("")).await;
        manager
            .edit_draft(run_id, "must not be lost".to_owned())
            .await
            .expect("dirty draft");

        let drafts_dir = persistence_root.join("drafts");
        fs::remove_dir_all(&drafts_dir).expect("remove drafts directory");
        fs::write(&drafts_dir, b"not a directory").expect("replace drafts directory with file");

        let error = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("blocked-switch"),
                    RpcCommand::SwitchSession {
                        session_path: PathBuf::from("switched-session.jsonl"),
                    },
                ),
            )
            .await
            .expect_err("replacement must fail when old draft cannot be persisted");
        assert!(error.to_string().contains("draft"));

        synchronize(&manager, run_id, "blocked-switch-still-old").await;
        let after = manager.hydrate().await.expect("old-session hydration");
        assert_eq!(
            after.runs[0].run.session_state().session_id.as_deref(),
            Some("fake-session")
        );
        let draft = after.runs[0].draft.as_ref().expect("preserved draft");
        assert_eq!(draft.text, "must not be lost");
        assert_eq!(draft.durability, DraftDurability::Failed);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn composer_actions_follow_runtime_activity_instead_of_renderer_intent() {
        let fixture = ManagerFixture::new("composer-actions");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "composer-actions-startup").await;
        manager
            .edit_draft(run_id, "not yet".to_owned())
            .await
            .expect("idle draft");
        assert!(
            manager
                .submit_draft(run_id, ComposerAction::Steer)
                .await
                .expect_err("idle steer must be rejected")
                .to_string()
                .contains("not valid")
        );

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("composer-hold"),
                    RpcCommand::Prompt {
                        message: "hold".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("start working turn");
        synchronize(&manager, run_id, "composer-working").await;
        assert_eq!(
            manager.hydrate().await.expect("working").runs[0]
                .run
                .composer_availability(),
            ComposerAvailability::AgentWorking
        );
        manager
            .edit_draft(run_id, "change direction".to_owned())
            .await
            .expect("steer draft");
        let steered = manager
            .submit_draft(run_id, ComposerAction::Steer)
            .await
            .expect("steer accepted");
        assert!(steered.accepted);
        assert!(steered.draft_cleared);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn ordinary_hydration_preserves_transient_delivery_and_rewakes_late_subscriber() {
        let fixture = ManagerFixture::new("hydrate-delivery");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "hydrate-delivery-startup").await;
        let _ = drain_all(&manager, run_id).await;

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("notify-prompt"),
                    RpcCommand::Prompt {
                        message: "notify".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("notification prompt accepted");
        synchronize(&manager, run_id, "notify-delivered-to-manager").await;

        // Subscribe only after the original dirty notification has already
        // happened, matching a renderer reload/listener replacement.
        let mut late_signals = manager.subscribe();
        let snapshot = manager.hydrate().await.expect("non-destructive hydration");
        assert_eq!(snapshot.runs.len(), 1);
        next_dirty(&mut late_signals, run_id).await;

        let events = drain_all(&manager, run_id).await;
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeUiEvent::ExtensionNotification { message, .. }
                if message == "fixture notification"
        )));
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn extension_editor_text_is_session_scoped_across_runtime_session_switches() {
        let fixture = ManagerFixture::new("session-draft");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "draft-startup").await;

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("editor-prompt"),
                    RpcCommand::Prompt {
                        message: "editor".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("editor prompt accepted");
        synchronize(&manager, run_id, "editor-applied").await;
        let original = manager.hydrate().await.expect("original draft hydration");
        assert_eq!(
            original.runs[0].draft.as_ref().expect("session draft").text,
            "extension draft"
        );

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("switch-draft-session"),
                    RpcCommand::SwitchSession {
                        session_path: PathBuf::from("switched-session.jsonl"),
                    },
                ),
            )
            .await
            .expect("switch session");
        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("after-draft-switch"),
                    RpcCommand::GetCommands,
                ),
            )
            .await
            .expect("order after switch");
        let switched = manager.hydrate().await.expect("switched draft hydration");
        assert_eq!(
            switched.runs[0]
                .draft
                .as_ref()
                .expect("new session draft")
                .text,
            ""
        );

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("switch-original-session"),
                    RpcCommand::SwitchSession {
                        session_path: PathBuf::from("original-session.jsonl"),
                    },
                ),
            )
            .await
            .expect("switch original session");
        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("after-original-switch"),
                    RpcCommand::GetCommands,
                ),
            )
            .await
            .expect("order after original switch");
        let restored = manager.hydrate().await.expect("restored draft hydration");
        assert_eq!(
            restored.runs[0]
                .draft
                .as_ref()
                .expect("restored draft")
                .text,
            "extension draft"
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn accepted_session_replacement_automatically_reconciles_new_session_identity() {
        let fixture = ManagerFixture::new("session-replacement");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "replacement-startup").await;
        assert_eq!(
            manager.hydrate().await.expect("initial hydration").runs[0]
                .run
                .session_state()
                .session_id
                .as_deref(),
            Some("fake-session")
        );

        let replacement = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("switch-session"),
                    RpcCommand::SwitchSession {
                        session_path: PathBuf::from("switched-session.jsonl"),
                    },
                ),
            )
            .await
            .expect("switch session response");
        assert_eq!(replacement.response.outcome(), RpcResponseOutcome::Accepted);

        // The manager queues its internal get_state before completing the
        // replacement waiter. A later writer command therefore completes only
        // after the fake Pi has emitted the reconciled state response.
        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("after-switch-capabilities"),
                    RpcCommand::GetCommands,
                ),
            )
            .await
            .expect("post-switch command");
        let snapshot = manager.hydrate().await.expect("replacement hydration");
        assert_eq!(
            snapshot.runs[0].run.session_state().session_id.as_deref(),
            Some("switched-session")
        );
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn explicit_ui_recovery_is_the_operation_that_discards_stale_run_backlog() {
        let fixture = ManagerFixture::new("recover-ui");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "recover-startup").await;

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("recover-stream"),
                    RpcCommand::Prompt {
                        message: "stream before recover".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("stream prompt accepted");
        synchronize(&manager, run_id, "recover-stream-settled").await;

        let recovered = manager.recover_ui(run_id).await.expect("recover run UI");
        assert_eq!(recovered.runs[0].run.id(), run_id);
        let drained = manager
            .drain_ui(run_id, 64)
            .await
            .expect("drain after recovery");
        assert!(drained.events.is_empty());
        assert!(!drained.rehydrate_required);
        assert!(!drained.has_more);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn manager_coalesces_stream_wakeup_and_drains_latest_assistant_projection() {
        let fixture = ManagerFixture::new("stream");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let mut signals = manager.subscribe();
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        next_dirty(&mut signals, run_id).await;
        synchronize(&manager, run_id, "stream-startup").await;
        manager.hydrate().await.expect("baseline hydration");

        let prompt = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("stream-prompt"),
                    RpcCommand::Prompt {
                        message: "stream".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("prompt accepted");
        assert_eq!(prompt.response.outcome(), RpcResponseOutcome::Accepted);
        synchronize(&manager, run_id, "stream-finished").await;
        next_dirty(&mut signals, run_id).await;

        assert!(matches!(
            signals.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        let events = drain_all(&manager, run_id).await;
        let latest = events.iter().find_map(|event| match event {
            RuntimeUiEvent::AssistantBlockUpdated { block, .. } => Some(block),
            _ => None,
        });
        assert_eq!(latest.expect("assistant block event").text, "Hello world");

        let second = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("second-prompt"),
                    RpcCommand::Prompt {
                        message: "second".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("second prompt");
        assert_eq!(second.response.outcome(), RpcResponseOutcome::Accepted);
        next_dirty(&mut signals, run_id).await;
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn manager_stop_recovers_queue_then_keeps_normal_child_reusable() {
        let fixture = ManagerFixture::new("stop");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "stop-startup").await;
        manager.hydrate().await.expect("baseline hydration");

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("hold-prompt"),
                    RpcCommand::Prompt {
                        message: "hold".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("hold prompt accepted");
        synchronize(&manager, run_id, "holding").await;
        let working = manager.hydrate().await.expect("working hydration");
        assert_eq!(working.runs[0].run.activity_state(), ActivityState::Working);
        manager
            .edit_draft(run_id, "already typed".to_owned())
            .await
            .expect("unsent editor draft");

        let stopped = manager.stop_run(run_id).await.expect("normal Stop");
        assert_eq!(stopped.recovered_steering, ["recover steering"]);
        assert_eq!(stopped.recovered_follow_up, ["recover follow up"]);
        assert!(stopped.draft_restored);
        assert!(stopped.draft_restore_error.is_none());
        assert!(!stopped.process_terminated);
        assert!(!stopped.quarantined);

        let idle = manager.hydrate().await.expect("post-stop hydration");
        assert_eq!(idle.runs[0].run.process_state(), ProcessState::Ready);
        assert_eq!(idle.runs[0].run.activity_state(), ActivityState::Idle);
        assert_eq!(
            idle.runs[0].draft.as_ref().expect("restored draft").text,
            "recover steering\nrecover follow up\nalready typed"
        );

        let after_stop = manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("after-stop"),
                    RpcCommand::Prompt {
                        message: "after stop".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("same child accepts prompt after Stop");
        assert_eq!(after_stop.response.outcome(), RpcResponseOutcome::Accepted);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn stop_never_destroys_existing_draft_when_recovered_queue_would_overflow_it() {
        let fixture = ManagerFixture::new("stop-draft-overflow");
        let limits = RuntimeLimits {
            max_draft_bytes_per_session: 64,
            max_recovered_queue_bytes_per_run: 64,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "stop-overflow-startup").await;
        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("stop-overflow-hold"),
                    RpcCommand::Prompt {
                        message: "hold".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("hold prompt accepted");
        synchronize(&manager, run_id, "stop-overflow-working").await;
        let original = "x".repeat(48);
        manager
            .edit_draft(run_id, original.clone())
            .await
            .expect("bounded existing draft");

        let stopped = manager.stop_run(run_id).await.expect("Stop completes");
        assert!(!stopped.draft_restored);
        assert!(
            stopped
                .draft_restore_error
                .as_deref()
                .is_some_and(|detail| detail.contains("exceeding draft limit"))
        );
        let after = manager.hydrate().await.expect("post-stop hydration");
        assert_eq!(after.runs[0].draft.as_ref().expect("draft").text, original);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn extension_dialog_response_uses_control_plane_and_clears_backend_ownership() {
        let fixture = ManagerFixture::new("dialog");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let run_id = manager
            .start_run(fixture.start_spec())
            .await
            .expect("start run");
        synchronize(&manager, run_id, "dialog-startup").await;

        manager
            .request(
                run_id,
                RpcRequest::with_id(
                    RequestId::from_wire("dialog-prompt"),
                    RpcCommand::Prompt {
                        message: "dialog".to_owned(),
                        images: Vec::new(),
                        streaming_behavior: None,
                    },
                ),
            )
            .await
            .expect("dialog prompt accepted");
        synchronize(&manager, run_id, "dialog-visible").await;
        let waiting = manager.hydrate().await.expect("dialog hydration");
        let rpc = waiting.runs[0].rpc.as_ref().expect("RPC state");
        assert_eq!(rpc.pending_dialogs.len(), 1);
        assert_eq!(
            waiting.runs[0].run.activity_state(),
            ActivityState::WaitingForInput
        );

        manager
            .respond_extension_ui(
                run_id,
                ExtensionUiResponse::Confirmation {
                    id: "dialog-1".to_owned(),
                    confirmed: true,
                },
            )
            .await
            .expect("extension dialog response written");
        synchronize(&manager, run_id, "dialog-resolved").await;
        let resolved = manager.hydrate().await.expect("resolved hydration");
        assert!(
            resolved.runs[0]
                .rpc
                .as_ref()
                .expect("RPC state")
                .pending_dialogs
                .is_empty()
        );
        assert_eq!(resolved.runs[0].run.activity_state(), ActivityState::Idle);
        manager.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn shutdown_waits_for_all_owned_children_and_closes_manager() {
        let first_fixture = ManagerFixture::new("shutdown-a");
        let second_fixture = ManagerFixture::new("shutdown-b");
        let manager = spawn_runtime_manager(RuntimeLimits::default()).expect("manager");
        let first = manager
            .start_run(first_fixture.start_spec())
            .await
            .expect("first run");
        let second = manager
            .start_run(second_fixture.start_spec())
            .await
            .expect("second run");
        synchronize(&manager, first, "shutdown-a").await;
        synchronize(&manager, second, "shutdown-b").await;

        let report = manager.shutdown().await.expect("manager shutdown");
        assert_eq!(report.terminal_runs, 2);
        assert_eq!(report.quarantined_runs, 0);
        assert!(matches!(
            manager.hydrate().await,
            Err(RuntimeManagerError::ManagerClosed)
        ));
    }

    #[test]
    fn ui_queue_editor_delivery_rearms_dirty_signal_after_drain() {
        let mut queue = RuntimeUiQueue::new(RuntimeLimits::default());
        queue.pending_editor_text = Some("first".to_owned());
        assert!(queue.mark_dirty());
        let drained = queue.drain(8);
        assert_eq!(drained.pending_editor_text.as_deref(), Some("first"));
        assert!(!drained.has_more);
        queue.pending_editor_text = Some("second".to_owned());
        assert!(queue.mark_dirty());
    }

    #[test]
    fn explicit_ui_recovery_preserves_out_of_snapshot_editor_text_for_redelivery() {
        let mut queue = RuntimeUiQueue::new(RuntimeLimits::default());
        queue.pending_editor_text = Some("extension text".to_owned());
        queue.push_semantic(RuntimeUiEvent::EditorTextChanged {
            run_id: RunId::new(),
        });
        assert!(queue.reset_after_recovery());
        let drained = queue.drain(8);
        assert!(drained.events.is_empty());
        assert_eq!(
            drained.pending_editor_text.as_deref(),
            Some("extension text")
        );
    }

    #[test]
    fn ui_drain_reports_remaining_bounded_work() {
        let mut queue = RuntimeUiQueue::new(RuntimeLimits::default());
        queue.push_semantic(RuntimeUiEvent::AssistantMessageReset {
            run_id: RunId::new(),
        });
        queue.push_semantic(RuntimeUiEvent::AssistantMessageReset {
            run_id: RunId::new(),
        });
        let first = queue.drain(1);
        assert_eq!(first.events.len(), 1);
        assert!(first.has_more);
        let second = queue.drain(1);
        assert_eq!(second.events.len(), 1);
        assert!(!second.has_more);
    }

    #[test]
    fn runtime_signal_and_ui_event_use_stable_camel_case_tags() {
        let run_id = RunId::new();
        let signal = serde_json::to_value(RuntimeManagerSignal::RunDirty { run_id })
            .expect("serialize dirty signal");
        assert_eq!(signal["kind"], "runDirty");
        assert_eq!(signal["runId"], run_id.to_string());

        let event = serde_json::to_value(RuntimeUiEvent::StateChanged {
            run_id,
            runtime_revision: 7,
        })
        .expect("serialize UI event");
        assert_eq!(event["kind"], "stateChanged");
        assert_eq!(event["runId"], run_id.to_string());
        assert_eq!(event["runtimeRevision"], 7);
    }

    const FAKE_PI_JS: &str = r#"
let buffer = "";
let working = false;
let sessionId = "fake-session";
let modelProvider = "fake";
let modelId = "fake-model";
let modelName = "Fake Model";
let thinkingLevel = "medium";
let sessionName = "Fake Session";
let autoCompactionEnabled = true;

function emit(value) {
  process.stdout.write(JSON.stringify(value) + "\n");
}

function respond(request, data) {
  const value = {
    id: request.id,
    type: "response",
    command: request.type,
    success: true,
  };
  if (data !== undefined) value.data = data;
  emit(value);
}

function reject(request, error) {
  emit({
    id: request.id,
    type: "response",
    command: request.type,
    success: false,
    error,
  });
}

function streamAssistant() {
  emit({type: "message_start", message: {role: "assistant", content: []}});
  emit({type: "message_update", assistantMessageEvent: {type: "text_start", contentIndex: 0}});
  emit({type: "message_update", assistantMessageEvent: {type: "text_delta", contentIndex: 0, delta: "Hello "}});
  emit({type: "message_update", assistantMessageEvent: {type: "text_delta", contentIndex: 0, delta: "world"}});
  emit({type: "message_update", assistantMessageEvent: {type: "text_end", contentIndex: 0, content: "Hello world"}});
  emit({type: "message_end", message: {role: "assistant", content: [{type: "text", text: "Hello world"}]}});
}

function handle(request) {
  if (request.type === "extension_ui_response") {
    working = false;
    emit({type: "agent_settled"});
    return;
  }
  switch (request.type) {
    case "get_state":
      respond(request, {
        model: {
          provider: modelProvider,
          id: modelId,
          name: modelName,
          input: modelId === "alt-model" ? ["text"] : ["text", "image"],
        },
        thinkingLevel,
        isStreaming: working,
        isCompacting: false,
        steeringMode: "all",
        followUpMode: "one-at-a-time",
        sessionFile: null,
        sessionId,
        sessionName,
        autoCompactionEnabled,
        messageCount: 1,
        pendingMessageCount: 0,
      });
      break;
    case "get_available_models":
      respond(request, {models: [
        {provider: "fake", id: "fake-model", name: "Fake Model", input: ["text", "image"]},
        {provider: "fake-alt", id: "alt-model", name: "Alternate Model", input: ["text"]},
      ]});
      break;
    case "get_available_thinking_levels":
      respond(request, {levels: ["off", "medium", "high"]});
      break;
    case "get_commands":
      respond(request, {commands: [{name: "fake-command", description: "fixture", source: "extension"}]});
      break;
    case "set_model":
      modelProvider = String(request.provider);
      modelId = String(request.modelId);
      modelName = modelId === "alt-model" ? "Alternate Model" : modelId;
      respond(request, {provider: modelProvider, id: modelId, name: modelName});
      break;
    case "set_thinking_level":
      thinkingLevel = String(request.level);
      respond(request);
      emit({type: "thinking_level_changed", level: thinkingLevel});
      break;
    case "set_auto_compaction":
      autoCompactionEnabled = Boolean(request.enabled);
      respond(request);
      break;
    case "set_session_name":
      sessionName = String(request.name);
      respond(request);
      emit({type: "session_info_changed", name: sessionName || null});
      break;
    case "compact":
      emit({type: "compaction_start"});
      respond(request, {summary: "fixture compaction", firstKeptEntryId: "kept-entry", tokensBefore: 100, estimatedTokensAfter: 40});
      emit({type: "compaction_end"});
      break;
    case "get_session_stats":
      respond(request, {
        sessionFile: "fake-session.jsonl",
        sessionId,
        userMessages: 2,
        assistantMessages: 2,
        toolCalls: 1,
        toolResults: 1,
        totalMessages: 6,
        tokens: {input: 100, output: 20, cacheRead: 25, cacheWrite: 5, total: 150},
        cost: 0.01,
        contextUsage: {tokens: 50000, contextWindow: 200000, percent: 25},
      });
      break;
    case "clone":
      sessionId = "cloned-session";
      respond(request, {cancelled: false});
      break;
    case "fork":
      sessionId = "forked-session";
      respond(request, {cancelled: false, text: "forked"});
      break;
    case "switch_session":
      if (String(request.sessionPath || "").includes("cancel")) {
        respond(request, {cancelled: true});
      } else {
        sessionId = String(request.sessionPath || "").includes("original")
          ? "fake-session"
          : "switched-session";
        respond(request, {cancelled: false});
      }
      break;
    case "prompt":
      if (request.message === "reject") {
        reject(request, "fixture prompt rejection");
        break;
      }
      if (request.message === "delayed-accept") {
        setTimeout(() => {
          respond(request);
          working = true;
          emit({type: "agent_start"});
          streamAssistant();
          working = false;
          emit({type: "agent_settled"});
        }, 60);
        break;
      }
      respond(request);
      working = true;
      emit({type: "agent_start"});
      if (request.message === "notify") {
        emit({
          type: "extension_ui_request",
          id: "notify-1",
          method: "notify",
          message: "fixture notification",
          notifyType: "info",
        });
        working = false;
        emit({type: "agent_settled"});
        break;
      }
      if (request.message === "dialog") {
        emit({
          type: "extension_ui_request",
          id: "dialog-1",
          method: "confirm",
          title: "Continue?",
          message: "Fixture dialog",
          timeout: 30000,
        });
        break;
      }
      if (request.message === "editor") {
        emit({
          type: "extension_ui_request",
          id: "editor-1",
          method: "set_editor_text",
          text: "extension draft",
        });
        working = false;
        emit({type: "agent_settled"});
        break;
      }
      streamAssistant();
      if (request.message === "hold") {
        emit({type: "queue_update", steering: ["recover steering"], followUp: ["recover follow up"]});
      } else {
        working = false;
        emit({type: "agent_settled"});
      }
      break;
    case "steer":
    case "follow_up":
      respond(request);
      break;
    case "clear_queue":
      respond(request, {steering: ["recover steering"], followUp: ["recover follow up"]});
      emit({type: "queue_update", steering: [], followUp: []});
      break;
    case "abort":
      respond(request);
      working = false;
      emit({type: "agent_settled"});
      break;
    default:
      respond(request);
      break;
  }
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  while (true) {
    const newline = buffer.indexOf("\n");
    if (newline < 0) break;
    const line = buffer.slice(0, newline).replace(/\r$/, "");
    buffer = buffer.slice(newline + 1);
    if (!line) continue;
    handle(JSON.parse(line));
  }
});
"#;

    const SILENT_PI_JS: &str = r#"
process.stdin.resume();
setInterval(() => {}, 1000);
"#;
}
