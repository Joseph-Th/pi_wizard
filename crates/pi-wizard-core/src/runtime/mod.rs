mod capabilities;
mod coalescing;
pub use capabilities::RunCapabilities;
mod extension_ui;
mod hydration;
mod manager;
mod process_actor;
mod projection;
mod rpc_controller;
mod session_sync;
mod state;
mod stop;

pub use crate::worktree::GitWorktreeIdentity;
pub use coalescing::{
    UiBacklogError, UiBacklogFrame, UiBacklogPush, UiBacklogStats, UiCoalesceKey, UiEventBacklog,
};
pub use extension_ui::{
    ExtensionStatusSnapshot, ExtensionUiError, ExtensionUiSnapshot, ExtensionUiState,
    ExtensionWidget, ExtensionWidgetSnapshot, WidgetPlacement,
};
pub use hydration::{
    PendingExtensionDialogSnapshot, RUNTIME_HYDRATION_SCHEMA_VERSION, RunHydrationSnapshot,
    RunRpcHydrationSnapshot, RuntimeHydrationSnapshot,
};
pub use manager::{
    ComposerAction, ComposerSubmitResult, ManagedRpcCompletion, RunStartSpec, RuntimeManagerError,
    RuntimeManagerHandle, RuntimeManagerSignal, RuntimeShutdownReport, RuntimeStopResult,
    RuntimeUiDrain, RuntimeUiEvent, SessionReplacementResult, spawn_runtime_manager,
    spawn_runtime_manager_with_draft_persistence,
};
pub use process_actor::{
    ProcessTerminationReport, RunProcessCommandError, RunProcessEnvelope, RunProcessEvent,
    RunProcessHandle, spawn_run_process_actor,
};
pub use projection::{
    AssistantContentBlock, AssistantContentKind, AssistantContentSnapshot, DirectBashSnapshot,
    LiveProjection, LiveProjectionSnapshot, ProjectionError, ToolPreview, ToolPreviewSnapshot,
};
pub use rpc_controller::{
    CancelledRpcRequest, CompletedRpcRequest, RunRpcController, RunRpcControllerError,
    RunRpcEffect, SessionSyncCompletion,
};
pub use session_sync::{SessionSyncApplied, SessionSyncError, SessionSyncResync, SessionSyncState};
pub use state::{
    ActivityState, ComposerAvailability, ExecutionIsolation, ProcessState, QueueState, RunFailure,
    RunFailureKind, RunModelState, RunMutation, RunRecord, RunSessionState, RunStateObservation,
    RuntimeError, RuntimeStore,
};
pub use stop::{
    StopDirective, StopEscalationReason, StopPhase, StopTransaction, StopTransactionError,
};
