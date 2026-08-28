use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use pi_wizard_core::automation::{
    AutomationCatalogSnapshot, AutomationChain, AutomationExecutionSnapshot,
    AutomationExecutionStatus, AutomationStepStatus, AutomationStore,
};
use pi_wizard_core::compatibility::{PiVersion, probe_pi_version};
use pi_wizard_core::environment::{
    EnvironmentResolutionError, LaunchEnvironmentDiagnostics, LaunchEnvironmentInput,
    ResolvedLaunchEnvironment, probe_login_shell_environment, resolve_launch_environment,
};
use pi_wizard_core::git_review::{
    GitDiffCursor, GitFileDiff, GitFileDiffPage, GitReviewSummary, review_file_diff,
    review_file_diff_page, review_summary,
};
use pi_wizard_core::launch::{
    ContextFilesPolicy, ExtensionDiscoveryPolicy, PiLaunchSpec, ProjectTrustPolicy, SessionLaunch,
};
use pi_wizard_core::preferences::PreferencesStore;
use pi_wizard_core::process::{SpawnedPiProcess, TerminationOutcome, spawn_pi_process};
use pi_wizard_core::project::{ProjectBinding, ProjectRegisteredLocation};
use pi_wizard_core::project_registry::ProjectRegistry;
use pi_wizard_core::project_resources::{ProjectResourcePreflight, inspect_project_resources};
use pi_wizard_core::rpc::{
    CompactionResult, ExtensionUiResponse, InboundMessage, ModelSummary, RpcCommand, RpcRequest,
    RpcResponse, SessionStats, SessionTreeSnapshot, ThinkingLevel,
};
use pi_wizard_core::runtime::{
    ComposerAction, ComposerAvailability, ComposerSubmitResult, ExecutionIsolation,
    GitWorktreeIdentity, ProcessState, RunStartSpec, RuntimeCloseResult,
    RuntimeDiagnosticsSnapshot, RuntimeHydrationSnapshot, RuntimeManagerHandle,
    RuntimeManagerSignal, RuntimeStopResult, RuntimeUiDrain, SessionReplacementResult,
    spawn_runtime_manager, spawn_runtime_manager_with_draft_persistence,
};
use pi_wizard_core::session_catalog::{
    SessionCatalogCursor, SessionCatalogPage, list_project_sessions, validate_project_session,
};
use pi_wizard_core::session_history::{
    SessionHistoryCursor, SessionHistoryPage, read_session_history_page,
};
use pi_wizard_core::worktree::{
    WorktreeBaseSnapshot, WorktreeCleanupResult, WorktreeCreatePlan, WorktreeRecoveryProbe,
    cleanup_pristine_worktree, create_worktree, inspect_worktree_base, probe_worktree_recovery,
};
use pi_wizard_core::worktree_registry::{WorktreeRecoveryRecord, WorktreeRegistry};
use pi_wizard_core::{
    AutomationChainId, AutomationExecutionId, DraftImageId, PiSessionId, ProjectId, RunId,
    RuntimeLimits, WorktreeId,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::{AbortHandle, JoinHandle};

const RUNTIME_DIRTY_EVENT: &str = "runtime://dirty";
const RUNTIME_REHYDRATE_EVENT: &str = "runtime://rehydrate";
const AUTOMATION_CHANGED_EVENT: &str = "automation://changed";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum AutomationChangedSignal {
    Catalog,
    Executions,
}

#[derive(Clone)]
struct AutomationCoordinator {
    store: Arc<Mutex<AutomationStore>>,
    executions: Arc<Mutex<HashMap<AutomationExecutionId, ActiveAutomationExecution>>>,
    changed: broadcast::Sender<AutomationChangedSignal>,
    limits: RuntimeLimits,
}

struct ActiveAutomationExecution {
    snapshot: AutomationExecutionSnapshot,
    cancel: watch::Sender<bool>,
}

impl AutomationCoordinator {
    fn new(store: AutomationStore, limits: RuntimeLimits) -> Self {
        let (changed, _) = broadcast::channel(limits.max_runtime_command_queue);
        Self {
            store: Arc::new(Mutex::new(store)),
            executions: Arc::new(Mutex::new(HashMap::new())),
            changed,
            limits,
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<AutomationChangedSignal> {
        self.changed.subscribe()
    }

    fn signal_catalog_changed(&self) {
        let _ = self.changed.send(AutomationChangedSignal::Catalog);
    }

    fn signal_executions_changed(&self) {
        let _ = self.changed.send(AutomationChangedSignal::Executions);
    }

    async fn snapshot(&self) -> DesktopAutomationSnapshot {
        let catalog = self.store.lock().await.snapshot();
        let mut executions: Vec<_> = self
            .executions
            .lock()
            .await
            .values()
            .map(|execution| execution.snapshot.clone())
            .collect();
        executions.sort_by_key(|execution| std::cmp::Reverse(execution.id.to_string()));
        DesktopAutomationSnapshot {
            catalog,
            executions,
        }
    }

    async fn execution_snapshot(&self) -> Vec<AutomationExecutionSnapshot> {
        let mut executions: Vec<_> = self
            .executions
            .lock()
            .await
            .values()
            .map(|execution| execution.snapshot.clone())
            .collect();
        executions.sort_by_key(|execution| std::cmp::Reverse(execution.id.to_string()));
        executions
    }

    async fn insert_execution(
        &self,
        snapshot: AutomationExecutionSnapshot,
    ) -> Result<watch::Receiver<bool>, String> {
        let mut executions = self.executions.lock().await;
        let capacity = self
            .limits
            .max_live_runs
            .saturating_add(self.limits.max_retained_terminal_runs);
        if executions.len() >= capacity {
            let evict = executions
                .iter()
                .filter(|(_, execution)| execution.snapshot.status.is_terminal())
                .min_by_key(|(id, _)| id.to_string())
                .map(|(id, _)| *id)
                .ok_or_else(|| {
                    format!(
                        "automation execution capacity {capacity} is occupied by active workflows"
                    )
                })?;
            executions.remove(&evict);
        }
        let (cancel, receiver) = watch::channel(false);
        executions.insert(snapshot.id, ActiveAutomationExecution { snapshot, cancel });
        drop(executions);
        self.signal_executions_changed();
        Ok(receiver)
    }

    async fn mutate_execution(
        &self,
        id: AutomationExecutionId,
        mutate: impl FnOnce(&mut AutomationExecutionSnapshot),
    ) -> Result<(), String> {
        let mut executions = self.executions.lock().await;
        let execution = executions
            .get_mut(&id)
            .ok_or_else(|| format!("unknown automation execution {id}"))?;
        let before = execution.snapshot.clone();
        mutate(&mut execution.snapshot);
        let changed = execution.snapshot != before;
        drop(executions);
        if changed {
            self.signal_executions_changed();
        }
        Ok(())
    }

    async fn cancel(&self, id: AutomationExecutionId) -> Result<(), String> {
        let mut executions = self.executions.lock().await;
        let execution = executions
            .get_mut(&id)
            .ok_or_else(|| format!("unknown automation execution {id}"))?;
        if execution.snapshot.status.is_terminal() {
            return Ok(());
        }
        execution
            .cancel
            .send(true)
            .map_err(|_| format!("automation execution {id} is no longer running"))?;
        execution.snapshot.status = AutomationExecutionStatus::Cancelled;
        for step in &mut execution.snapshot.steps {
            if matches!(
                step.status,
                AutomationStepStatus::Queued | AutomationStepStatus::Starting
            ) {
                step.status = AutomationStepStatus::Cancelled;
            }
        }
        drop(executions);
        self.signal_executions_changed();
        Ok(())
    }
}

struct DesktopRuntime {
    manager: RuntimeManagerHandle,
    limits: RuntimeLimits,
    launch_cleanup_gate: Arc<Mutex<()>>,
    launch_profile: Mutex<Option<DesktopLaunchProfile>>,
    preferences: Mutex<PreferencesStore>,
    projects: Mutex<ProjectRegistry>,
    worktrees: Arc<Mutex<WorktreeRegistry>>,
    automation: AutomationCoordinator,
    git_review_jobs: Mutex<GitReviewJobRegistry>,
    session_catalog_jobs: AtomicUsize,
}

struct ActiveJobGuard<'a> {
    counter: &'a AtomicUsize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopAutomationSnapshot {
    catalog: AutomationCatalogSnapshot,
    executions: Vec<AutomationExecutionSnapshot>,
}

impl<'a> ActiveJobGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for ActiveJobGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

struct GitReviewJobState {
    generation: u64,
    touch: u64,
    abort: Option<AbortHandle>,
}

struct GitReviewJobRegistry {
    by_run: HashMap<RunId, GitReviewJobState>,
    capacity: usize,
    clock: u64,
}

impl GitReviewJobRegistry {
    fn new(capacity: usize) -> Self {
        Self {
            by_run: HashMap::new(),
            capacity: capacity.max(1),
            clock: 0,
        }
    }

    fn begin(&mut self, run_id: RunId) -> Result<u64, String> {
        self.clock = self.clock.saturating_add(1);
        let touch = self.clock;
        if let Some(state) = self.by_run.get_mut(&run_id) {
            if let Some(abort) = state.abort.take() {
                abort.abort();
            }
            state.generation = state.generation.saturating_add(1).max(1);
            state.touch = touch;
            return Ok(state.generation);
        }
        if self.by_run.len() >= self.capacity {
            let evict = self
                .by_run
                .iter()
                .filter(|(_, state)| state.abort.is_none())
                .min_by_key(|(_, state)| state.touch)
                .map(|(run_id, _)| *run_id)
                .ok_or_else(|| {
                    format!(
                        "Git review job capacity {} is fully occupied by active reviews",
                        self.capacity
                    )
                })?;
            self.by_run.remove(&evict);
        }
        self.by_run.insert(
            run_id,
            GitReviewJobState {
                generation: 1,
                touch,
                abort: None,
            },
        );
        Ok(1)
    }

    fn attach(&mut self, run_id: RunId, generation: u64, abort: AbortHandle) -> bool {
        let Some(state) = self.by_run.get_mut(&run_id) else {
            abort.abort();
            return false;
        };
        if state.generation != generation || state.abort.is_some() {
            abort.abort();
            return false;
        }
        state.abort = Some(abort);
        true
    }

    fn complete(&mut self, run_id: RunId, generation: u64) {
        if let Some(state) = self.by_run.get_mut(&run_id)
            && state.generation == generation
        {
            state.abort = None;
        }
    }

    fn cancel(&mut self, run_id: RunId) -> bool {
        let Some(state) = self.by_run.get_mut(&run_id) else {
            return false;
        };
        self.clock = self.clock.saturating_add(1);
        state.touch = self.clock;
        state.generation = state.generation.saturating_add(1).max(1);
        state.abort.take().is_some_and(|abort| {
            abort.abort();
            true
        })
    }

    fn active_count(&self) -> usize {
        self.by_run
            .values()
            .filter(|state| state.abort.is_some())
            .count()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopRuntimeCapacity {
    active_runs: usize,
    live_run_limit: usize,
    configured_max_live_runs: usize,
    preference_recovery_notice: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopRuntimeDiagnostics {
    runtime: RuntimeDiagnosticsSnapshot,
    active_git_review_jobs: usize,
    active_session_catalog_jobs: usize,
}

#[tauri::command]
async fn runtime_recover_ui(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<RuntimeHydrationSnapshot, String> {
    runtime
        .manager
        .recover_ui(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_automation_snapshot(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<DesktopAutomationSnapshot, String> {
    Ok(runtime.automation.snapshot().await)
}

#[tauri::command]
async fn runtime_automation_executions(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<Vec<AutomationExecutionSnapshot>, String> {
    Ok(runtime.automation.execution_snapshot().await)
}

#[tauri::command]
async fn runtime_save_automation_chain(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SaveAutomationChainRequest,
) -> Result<AutomationChain, String> {
    let chain = AutomationChain {
        id: request.id.unwrap_or_default(),
        name: request.name,
        prompts: request.prompts,
    };
    let saved = runtime
        .automation
        .store
        .lock()
        .await
        .upsert(chain)
        .map_err(|error| error.to_string())?;
    runtime.automation.signal_catalog_changed();
    Ok(saved)
}

#[tauri::command]
async fn runtime_delete_automation_chain(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: AutomationChainRequest,
) -> Result<bool, String> {
    let removed = runtime
        .automation
        .store
        .lock()
        .await
        .remove(request.id)
        .map_err(|error| error.to_string())?;
    if removed {
        runtime.automation.signal_catalog_changed();
    }
    Ok(removed)
}

#[tauri::command]
async fn runtime_start_automation(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: StartAutomationRequest,
) -> Result<AutomationExecutionId, String> {
    let chain = runtime
        .automation
        .store
        .lock()
        .await
        .get(request.chain_id)
        .cloned()
        .ok_or_else(|| format!("unknown automation chain {}", request.chain_id))?;
    let capacity = runtime.capacity_report().await?;
    if request.concurrency == 0 || request.concurrency > capacity.live_run_limit {
        return Err(format!(
            "automation worker concurrency must be between 1 and {}",
            capacity.live_run_limit
        ));
    }
    if !request.worktrees && request.concurrency != 1 {
        return Err(
            "parallel automation in one project requires Git worktrees; local-checkout chains are sequential"
                .to_owned(),
        );
    }
    let required_slots = request
        .concurrency
        .saturating_add(usize::from(request.supervisor));
    if required_slots > capacity.live_run_limit {
        return Err(format!(
            "{} worker{} plus the supervisor require {required_slots} live slots, but the current limit is {}",
            request.concurrency,
            if request.concurrency == 1 { "" } else { "s" },
            capacity.live_run_limit
        ));
    }
    let project = runtime.registered_project(request.project_id).await?;
    if project.verify_registered_location() != ProjectRegisteredLocation::Present {
        return Err(
            "selected project is detached or moved; relocate it before automation".to_owned(),
        );
    }
    let profile = runtime.launch_profile().await?;
    let base = if request.worktrees || request.supervisor {
        Some(
            inspect_worktree_base(
                project.canonical_root(),
                &profile.environment,
                runtime.limits,
            )
            .await
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };

    let id = AutomationExecutionId::new();
    let snapshot = AutomationExecutionSnapshot::new(
        id,
        &chain,
        request.project_id,
        request.concurrency,
        request.worktrees,
        request.supervisor,
        runtime.limits,
    );
    let cancel = runtime.automation.insert_execution(snapshot).await?;
    let context = AutomationRuntimeContext {
        manager: runtime.manager.clone(),
        limits: runtime.limits,
        launch_cleanup_gate: Arc::clone(&runtime.launch_cleanup_gate),
        worktrees: Arc::clone(&runtime.worktrees),
        coordinator: runtime.automation.clone(),
    };
    tauri::async_runtime::spawn(async move {
        let plan = AutomationExecutionPlan {
            execution_id: id,
            chain,
            project,
            environment: profile.environment,
            base,
            request,
        };
        run_automation_execution(context, plan, cancel).await;
    });
    Ok(id)
}

#[tauri::command]
async fn runtime_cancel_automation(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: AutomationExecutionRequest,
) -> Result<(), String> {
    runtime.automation.cancel(request.id).await
}

async fn run_automation_execution(
    context: AutomationRuntimeContext,
    plan: AutomationExecutionPlan,
    mut cancel: watch::Receiver<bool>,
) {
    let execution_id = plan.execution_id;
    let result = run_automation_execution_inner(&context, &plan, &mut cancel).await;
    if let Err(error) = result {
        let supervisor_run = {
            let executions = context.coordinator.executions.lock().await;
            executions
                .get(&execution_id)
                .and_then(|execution| execution.snapshot.supervisor_run_id)
        };
        let supervisor_cleanup_error = if let Some(run_id) = supervisor_run {
            terminate_automation_owned_run(&context.manager, run_id)
                .await
                .err()
                .map(|cleanup_error| (run_id, cleanup_error))
        } else {
            None
        };
        let _ = context
            .coordinator
            .mutate_execution(execution_id, |snapshot| {
                snapshot.supervisor_enabled = false;
                match &supervisor_cleanup_error {
                    Some((run_id, cleanup_error)) => {
                        snapshot.supervisor_run_id = Some(*run_id);
                        snapshot.supervisor_error = Some(format!(
                            "automation failed and supervisor cleanup also failed: {cleanup_error}"
                        ));
                    }
                    None => snapshot.supervisor_run_id = None,
                }
                if !snapshot.status.is_terminal() {
                    snapshot.status = AutomationExecutionStatus::Failed;
                    snapshot.error = Some(match &supervisor_cleanup_error {
                        Some((_, cleanup_error)) => {
                            format!("{error}; supervisor cleanup failed: {cleanup_error}")
                        }
                        None => error.clone(),
                    });
                    for step in &mut snapshot.steps {
                        if matches!(
                            step.status,
                            AutomationStepStatus::Queued | AutomationStepStatus::Starting
                        ) {
                            step.status = AutomationStepStatus::Failed;
                            step.error = Some(
                                "not started because the automation scheduler failed".to_owned(),
                            );
                        }
                    }
                }
            })
            .await;
    }
}

async fn run_automation_execution_inner(
    context: &AutomationRuntimeContext,
    plan: &AutomationExecutionPlan,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let execution_id = plan.execution_id;
    let chain = &plan.chain;
    let project = &plan.project;
    let environment = &plan.environment;
    let base = plan.base.as_ref();
    let request = &plan.request;
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| {
            snapshot.status = AutomationExecutionStatus::Running;
        })
        .await?;
    let mut state_changes = context.manager.subscribe_state_changes();
    let mut workers: HashMap<RunId, AutomationWorker> = HashMap::new();
    let mut next_step = 0usize;
    let mut supervisor_run: Option<RunId> = None;
    let mut supervisor_enabled = request.supervisor;
    let mut supervisor_cycles = 0usize;

    while next_step < chain.prompts.len() || !workers.is_empty() {
        if *cancel.borrow() {
            finish_automation_cancellation(context, execution_id, supervisor_run).await?;
            return Ok(());
        }

        if let Some(supervisor) = supervisor_run {
            let capacity = context
                .manager
                .capacity()
                .await
                .map_err(|error| error.to_string())?;
            if automation_supervisor_should_yield(
                capacity.active_runs,
                capacity.live_run_limit,
                workers.len(),
                next_step < chain.prompts.len(),
            ) {
                match terminate_automation_owned_run(&context.manager, supervisor).await {
                    Ok(()) => {
                        supervisor_run = None;
                        context
                            .coordinator
                            .mutate_execution(execution_id, |snapshot| {
                                snapshot.supervisor_run_id = None;
                            })
                            .await?;
                    }
                    Err(error) => {
                        supervisor_enabled = false;
                        context
                            .coordinator
                            .mutate_execution(execution_id, |snapshot| {
                                snapshot.supervisor_enabled = false;
                                snapshot.supervisor_error = Some(format!(
                                    "supervisor could not yield its live slot for queued worker work: {error}"
                                ));
                            })
                            .await?;
                    }
                }
            }
        }

        if supervisor_enabled && supervisor_run.is_none() {
            let capacity = context
                .manager
                .capacity()
                .await
                .map_err(|error| error.to_string())?;
            if automation_supervisor_can_start(
                capacity.active_runs,
                capacity.live_run_limit,
                workers.len(),
                next_step < chain.prompts.len(),
            ) {
                let supervisor_base = base
                    .ok_or_else(|| "supervisor requires a Git worktree base snapshot".to_owned())?;
                match launch_automation_run(
                    context,
                    AutomationRunLaunch {
                        execution_id,
                        project,
                        environment,
                        base: Some(supervisor_base),
                        label: "supervisor",
                        initial_task: None,
                        supervisor: true,
                    },
                    cancel,
                )
                .await
                {
                    Ok(AutomationLaunchAttempt::Deferred) => {}
                    Ok(AutomationLaunchAttempt::CancelledBeforeStart) => {
                        finish_automation_cancellation(context, execution_id, supervisor_run)
                            .await?;
                        return Ok(());
                    }
                    Ok(AutomationLaunchAttempt::Started {
                        run_id,
                        assistant_messages_at_start: _,
                    }) => {
                        supervisor_run = Some(run_id);
                        context
                            .coordinator
                            .mutate_execution(execution_id, |snapshot| {
                                snapshot.supervisor_run_id = Some(run_id);
                            })
                            .await?;
                    }
                    Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error }) => {
                        let cleanup =
                            terminate_automation_owned_run(&context.manager, run_id).await;
                        let cleanup_failed = cleanup.is_err();
                        supervisor_enabled = false;
                        context
                            .coordinator
                            .mutate_execution(execution_id, |snapshot| {
                                snapshot.supervisor_enabled = false;
                                snapshot.supervisor_run_id = cleanup.as_ref().err().map(|_| run_id);
                                snapshot.supervisor_error = Some(match cleanup.as_ref() {
                                    Ok(()) => error.clone(),
                                    Err(cleanup_error) => {
                                        format!(
                                            "{error}; supervisor cleanup failed: {cleanup_error}"
                                        )
                                    }
                                });
                            })
                            .await?;
                        if cleanup_failed {
                            supervisor_run = Some(run_id);
                        }
                    }
                    Err(error) => {
                        supervisor_enabled = false;
                        context
                            .coordinator
                            .mutate_execution(execution_id, |snapshot| {
                                snapshot.supervisor_enabled = false;
                                snapshot.supervisor_error = Some(error.clone());
                            })
                            .await?;
                    }
                }
            }
        }

        let hydration = context
            .manager
            .hydrate()
            .await
            .map_err(|error| error.to_string())?;
        let mut idle_workers = Vec::new();
        let mut terminal_workers = Vec::new();
        for (&run_id, worker) in &mut workers {
            let Some(run) = hydration.runs.iter().find(|run| run.run.id() == run_id) else {
                terminal_workers.push((run_id, "worker disappeared from runtime".to_owned()));
                continue;
            };
            if run.run.process_state().is_terminal() {
                terminal_workers.push((
                    run_id,
                    format!("worker process ended as {:?}", run.run.process_state()),
                ));
                continue;
            }
            if run.run.pending_ui_requests() > 0 {
                worker.turn_activity_observed = true;
                let step_index = worker.step_index;
                set_automation_step_status(
                    context,
                    execution_id,
                    step_index,
                    AutomationStepStatus::NeedsAttention,
                    None,
                )
                .await?;
                continue;
            }
            let queue = run.run.queue();
            let busy = run.run.activity_state() != pi_wizard_core::runtime::ActivityState::Idle
                || queue.steering > 0
                || queue.follow_up > 0
                || run.run.is_retry_waiting()
                || run.run.has_summarization_retry();
            if busy || run.run.process_state() != ProcessState::Ready {
                if busy {
                    worker.turn_activity_observed = true;
                }
                let step_index = worker.step_index;
                set_automation_step_status(
                    context,
                    execution_id,
                    step_index,
                    AutomationStepStatus::Working,
                    None,
                )
                .await?;
                continue;
            }
            let assistant_messages = session_assistant_messages(&context.manager, run_id).await?;
            if automation_worker_turn_complete(worker, assistant_messages) {
                idle_workers.push(run_id);
            }
        }

        for (run_id, error) in terminal_workers {
            if let Some(worker) = workers.remove(&run_id) {
                set_automation_step_status(
                    context,
                    execution_id,
                    worker.step_index,
                    AutomationStepStatus::Failed,
                    Some(error),
                )
                .await?;
            }
        }

        let mut retained_idle = std::collections::HashSet::new();
        if !idle_workers.is_empty()
            && supervisor_enabled
            && let Some(supervisor) = supervisor_run
        {
            if supervisor_cycles >= context.limits.max_supervisor_cycles_per_execution {
                supervisor_enabled = false;
                let cleanup = terminate_automation_owned_run(&context.manager, supervisor).await;
                let cleanup_failed = cleanup.is_err();
                context
                    .coordinator
                    .mutate_execution(execution_id, |snapshot| {
                        snapshot.supervisor_enabled = false;
                        snapshot.supervisor_run_id = cleanup.as_ref().err().map(|_| supervisor);
                        snapshot.supervisor_error = Some(match cleanup.as_ref() {
                            Ok(()) => format!(
                                "Supervisor stopped after the configured {}-cycle execution limit",
                                context.limits.max_supervisor_cycles_per_execution
                            ),
                            Err(cleanup_error) => format!(
                                "Supervisor reached the configured {}-cycle execution limit; cleanup failed: {cleanup_error}",
                                context.limits.max_supervisor_cycles_per_execution
                            ),
                        });
                    })
                    .await?;
                supervisor_run = cleanup_failed.then_some(supervisor);
            } else {
                supervisor_cycles += 1;
                context
                    .coordinator
                    .mutate_execution(execution_id, |snapshot| {
                        snapshot.supervisor_cycles = supervisor_cycles;
                    })
                    .await?;
                match run_supervisor_cycle(
                    context,
                    SupervisorCycleInput {
                        execution_id,
                        chain,
                        supervisor_run: supervisor,
                        hydration: &hydration,
                        workers: &mut workers,
                        idle_workers: &idle_workers,
                        cancel,
                    },
                )
                .await
                {
                    Ok(Some(retained)) => retained_idle = retained,
                    Ok(None) => {
                        finish_automation_cancellation(context, execution_id, Some(supervisor))
                            .await?;
                        return Ok(());
                    }
                    Err(error) => {
                        supervisor_enabled = false;
                        let cleanup =
                            terminate_automation_owned_run(&context.manager, supervisor).await;
                        let cleanup_failed = cleanup.is_err();
                        context
                            .coordinator
                            .mutate_execution(execution_id, |snapshot| {
                                snapshot.supervisor_enabled = false;
                                snapshot.supervisor_run_id =
                                    cleanup.as_ref().err().map(|_| supervisor);
                                snapshot.supervisor_error = Some(match cleanup.as_ref() {
                                    Ok(()) => error.clone(),
                                    Err(cleanup_error) => {
                                        format!(
                                            "{error}; supervisor cleanup failed: {cleanup_error}"
                                        )
                                    }
                                });
                            })
                            .await?;
                        supervisor_run = cleanup_failed.then_some(supervisor);
                    }
                }
            }
        }

        for run_id in idle_workers {
            if retained_idle.contains(&run_id) {
                continue;
            }
            let Some(worker) = workers.remove(&run_id) else {
                continue;
            };
            match context.manager.close_run(run_id).await {
                Ok(result) if !result.quarantined => {
                    set_automation_step_status(
                        context,
                        execution_id,
                        worker.step_index,
                        AutomationStepStatus::Completed,
                        None,
                    )
                    .await?;
                }
                Ok(_) => {
                    set_automation_step_status(
                        context,
                        execution_id,
                        worker.step_index,
                        AutomationStepStatus::Failed,
                        Some("worker process termination is uncertain".to_owned()),
                    )
                    .await?;
                }
                Err(error) => {
                    set_automation_step_status(
                        context,
                        execution_id,
                        worker.step_index,
                        AutomationStepStatus::Failed,
                        Some(format!("could not close completed worker: {error}")),
                    )
                    .await?;
                }
            }
        }

        while next_step < chain.prompts.len() && workers.len() < request.concurrency {
            let capacity = context
                .manager
                .capacity()
                .await
                .map_err(|error| error.to_string())?;
            if capacity.active_runs >= capacity.live_run_limit {
                break;
            }
            let step_index = next_step;
            set_automation_step_status(
                context,
                execution_id,
                step_index,
                AutomationStepStatus::Starting,
                None,
            )
            .await?;
            let worktree_base = if request.worktrees {
                Some(base.ok_or_else(|| "parallel worker is missing Git worktree base".to_owned())?)
            } else {
                None
            };
            let worker_label = format!("worker-{}", step_index + 1);
            match launch_automation_run(
                context,
                AutomationRunLaunch {
                    execution_id,
                    project,
                    environment,
                    base: worktree_base,
                    label: &worker_label,
                    initial_task: Some(chain.prompts[step_index].as_str()),
                    supervisor: false,
                },
                cancel,
            )
            .await
            {
                Ok(AutomationLaunchAttempt::Deferred) => {
                    set_automation_step_status(
                        context,
                        execution_id,
                        step_index,
                        AutomationStepStatus::Queued,
                        None,
                    )
                    .await?;
                    break;
                }
                Ok(AutomationLaunchAttempt::CancelledBeforeStart) => {
                    finish_automation_cancellation(context, execution_id, supervisor_run).await?;
                    return Ok(());
                }
                Ok(AutomationLaunchAttempt::Started {
                    run_id,
                    assistant_messages_at_start,
                }) => {
                    next_step += 1;
                    workers.insert(
                        run_id,
                        AutomationWorker {
                            step_index,
                            assistant_messages_at_start,
                            turn_activity_observed: false,
                        },
                    );
                    context
                        .coordinator
                        .mutate_execution(execution_id, |snapshot| {
                            let step = &mut snapshot.steps[step_index];
                            step.run_id = Some(run_id);
                            step.status = AutomationStepStatus::Working;
                        })
                        .await?;
                }
                Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error }) => {
                    next_step += 1;
                    let cleanup = terminate_automation_owned_run(&context.manager, run_id).await;
                    context
                        .coordinator
                        .mutate_execution(execution_id, |snapshot| {
                            let step = &mut snapshot.steps[step_index];
                            step.run_id = Some(run_id);
                            step.status = AutomationStepStatus::Failed;
                            step.error = Some(match cleanup.as_ref() {
                                Ok(()) => error.clone(),
                                Err(cleanup_error) => {
                                    format!("{error}; worker cleanup failed: {cleanup_error}")
                                }
                            });
                        })
                        .await?;
                }
                Err(error) => {
                    next_step += 1;
                    set_automation_step_status(
                        context,
                        execution_id,
                        step_index,
                        AutomationStepStatus::Failed,
                        Some(error),
                    )
                    .await?;
                }
            }
        }

        if next_step >= chain.prompts.len() && workers.is_empty() {
            break;
        }

        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    continue;
                }
            }
            changed = state_changes.recv() => {
                if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                    return Err("runtime state-change stream closed during automation".to_owned());
                }
            }
        }
    }

    let supervisor_cleanup_error = if let Some(run_id) = supervisor_run {
        terminate_automation_owned_run(&context.manager, run_id)
            .await
            .err()
            .map(|error| (run_id, error))
    } else {
        None
    };
    let mut status = {
        let executions = context.coordinator.executions.lock().await;
        let snapshot = &executions
            .get(&execution_id)
            .ok_or_else(|| format!("unknown automation execution {execution_id}"))?
            .snapshot;
        if snapshot
            .steps
            .iter()
            .any(|step| step.status == AutomationStepStatus::Failed)
        {
            AutomationExecutionStatus::CompletedWithErrors
        } else {
            AutomationExecutionStatus::Completed
        }
    };
    if supervisor_cleanup_error.is_some() && status == AutomationExecutionStatus::Completed {
        status = AutomationExecutionStatus::CompletedWithErrors;
    }
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| {
            snapshot.status = status;
            match &supervisor_cleanup_error {
                Some((run_id, error)) => {
                    snapshot.supervisor_run_id = Some(*run_id);
                    snapshot.error = Some(format!("supervisor cleanup failed: {error}"));
                }
                None => snapshot.supervisor_run_id = None,
            }
        })
        .await
}

async fn set_automation_step_status(
    context: &AutomationRuntimeContext,
    execution_id: AutomationExecutionId,
    step_index: usize,
    status: AutomationStepStatus,
    error: Option<String>,
) -> Result<(), String> {
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| {
            let step = &mut snapshot.steps[step_index];
            step.status = status;
            if error.is_some() {
                step.error = error.clone();
            }
        })
        .await
}

async fn finish_automation_cancellation(
    context: &AutomationRuntimeContext,
    execution_id: AutomationExecutionId,
    supervisor_run: Option<RunId>,
) -> Result<(), String> {
    let cleanup_error = if let Some(run_id) = supervisor_run {
        terminate_automation_owned_run(&context.manager, run_id)
            .await
            .err()
            .map(|error| (run_id, error))
    } else {
        None
    };
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| {
            snapshot.status = AutomationExecutionStatus::Cancelled;
            match &cleanup_error {
                Some((run_id, error)) => {
                    snapshot.supervisor_run_id = Some(*run_id);
                    snapshot.supervisor_error = Some(format!(
                        "chain cancelled, but supervisor cleanup failed: {error}"
                    ));
                }
                None => snapshot.supervisor_run_id = None,
            }
            for step in &mut snapshot.steps {
                if matches!(
                    step.status,
                    AutomationStepStatus::Queued | AutomationStepStatus::Starting
                ) {
                    step.status = AutomationStepStatus::Cancelled;
                }
            }
        })
        .await
}

enum AutomationLaunchAttempt {
    Deferred,
    CancelledBeforeStart,
    Started {
        run_id: RunId,
        assistant_messages_at_start: usize,
    },
    FailedAfterStart {
        run_id: RunId,
        error: String,
    },
}

async fn launch_automation_run(
    context: &AutomationRuntimeContext,
    launch_request: AutomationRunLaunch<'_>,
    cancel: &watch::Receiver<bool>,
) -> Result<AutomationLaunchAttempt, String> {
    let AutomationRunLaunch {
        execution_id,
        project,
        environment,
        base,
        label,
        initial_task,
        supervisor,
    } = launch_request;
    let _gate = context.launch_cleanup_gate.lock().await;
    if *cancel.borrow() {
        return Ok(AutomationLaunchAttempt::CancelledBeforeStart);
    }
    let capacity = context
        .manager
        .capacity()
        .await
        .map_err(|error| error.to_string())?;
    if capacity.active_runs >= capacity.live_run_limit {
        return Ok(AutomationLaunchAttempt::Deferred);
    }
    if base.is_none() {
        let hydration = context
            .manager
            .hydrate()
            .await
            .map_err(|error| error.to_string())?;
        if hydration.runs.iter().any(|run| {
            !run.run.process_state().is_terminal()
                && run.run.execution_root() == project.canonical_root()
        }) {
            return Ok(AutomationLaunchAttempt::Deferred);
        }
    }
    let (execution_root, worktree) = if let Some(base) = base {
        let plan = automation_worktree_plan(base, execution_id, label)?;
        let parent = plan.worktree_path.parent().ok_or_else(|| {
            format!(
                "automation worktree path has no parent: {}",
                plan.worktree_path.display()
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create automation worktree parent {}: {error}",
                parent.display()
            )
        })?;
        let recovery = {
            let mut registry = context.worktrees.lock().await;
            registry
                .begin_creation(project.id(), &plan)
                .map_err(|error| error.to_string())?
        };
        let created = match create_worktree(plan, environment, context.limits).await {
            Ok(created) => created,
            Err(error) => {
                if !error.may_have_mutated() {
                    let discard = {
                        let mut registry = context.worktrees.lock().await;
                        registry.discard_unmutated_plan(recovery.id)
                    };
                    if let Err(discard_error) = discard {
                        return Err(format!(
                            "{error}; Git mutation was not observed, but automation recovery intent {} could not be discarded: {discard_error}",
                            recovery.id
                        ));
                    }
                    return Err(error.to_string());
                }
                return Err(format!(
                    "{error}; automation worktree recovery transaction {} was retained",
                    recovery.id
                ));
            }
        };
        {
            let mut registry = context.worktrees.lock().await;
            registry
                .mark_created(recovery.id, created.clone())
                .map_err(|error| {
                    format!(
                        "automation worktree {} was created but recovery transaction {} could not record it: {error}",
                        created.worktree_root.display(),
                        recovery.id
                    )
                })?;
        }
        if *cancel.borrow() {
            return Ok(AutomationLaunchAttempt::CancelledBeforeStart);
        }
        (created.execution_root.clone(), Some(created.identity()))
    } else {
        if *cancel.borrow() {
            return Ok(AutomationLaunchAttempt::CancelledBeforeStart);
        }
        (project.canonical_root().to_path_buf(), None)
    };

    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        &execution_root,
        if supervisor {
            ProjectTrustPolicy::Ignore
        } else {
            ProjectTrustPolicy::Inherit
        },
    );
    if supervisor {
        launch_spec.context_files = ContextFilesPolicy::Disabled;
        launch_spec.extension_discovery = ExtensionDiscoveryPolicy::Disabled;
    }
    launch_spec.session = SessionLaunch::NewWithId(PiSessionId::new());
    let launch = launch_spec.resolve().map_err(|error| error.to_string())?;
    let run_id = context
        .manager
        .start_run(RunStartSpec {
            project_id: project.id(),
            execution_isolation: if worktree.is_some() {
                ExecutionIsolation::GitWorktree
            } else {
                ExecutionIsolation::LocalCheckout
            },
            worktree,
            launch,
            environment: environment.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    drop(_gate);
    let assistant_messages_at_start =
        match wait_automation_run_ready(&context.manager, context.limits, run_id).await {
            Ok(messages) => messages,
            Err(error) => {
                return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
            }
        };
    if let Some(task) = initial_task
        && let Err(error) = automation_submit_prompt(&context.manager, run_id, task).await
    {
        return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
    }
    Ok(AutomationLaunchAttempt::Started {
        run_id,
        assistant_messages_at_start,
    })
}

fn automation_supervisor_should_yield(
    active_runs: usize,
    live_run_limit: usize,
    active_workers: usize,
    pending_worker_steps: bool,
) -> bool {
    pending_worker_steps && active_workers == 0 && active_runs >= live_run_limit
}

fn automation_worker_turn_complete(worker: &AutomationWorker, assistant_messages: usize) -> bool {
    worker.turn_activity_observed || assistant_messages > worker.assistant_messages_at_start
}

fn automation_supervisor_can_start(
    active_runs: usize,
    live_run_limit: usize,
    active_workers: usize,
    pending_worker_steps: bool,
) -> bool {
    let available = live_run_limit.saturating_sub(active_runs);
    if available == 0 {
        return false;
    }
    if active_workers == 0 && pending_worker_steps {
        return available >= 2;
    }
    true
}

fn automation_execution_key(execution_id: AutomationExecutionId) -> String {
    execution_id
        .to_string()
        .chars()
        .filter(|character| *character != '-')
        .collect()
}

fn automation_worktree_plan(
    base: &WorktreeBaseSnapshot,
    execution_id: AutomationExecutionId,
    label: &str,
) -> Result<WorktreeCreatePlan, String> {
    let repository_name = base
        .repository_root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Git repository root has no usable directory name".to_owned())?;
    let parent = base
        .repository_root
        .parent()
        .ok_or_else(|| "Git repository root has no parent directory".to_owned())?;
    let execution_key = automation_execution_key(execution_id);
    let safe_label: String = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let leaf = format!("auto-{execution_key}-{safe_label}");
    Ok(WorktreeCreatePlan {
        base: base.clone(),
        branch: format!("pi-wizard/{leaf}"),
        worktree_path: parent
            .join(format!("{repository_name}-worktrees"))
            .join(leaf),
    })
}

async fn wait_automation_run_ready(
    manager: &RuntimeManagerHandle,
    limits: RuntimeLimits,
    run_id: RunId,
) -> Result<usize, String> {
    let mut state_changes = manager.subscribe_state_changes();
    let deadline = Duration::from_millis(limits.startup_rpc_deadline_ms.saturating_add(1_000));
    tokio::time::timeout(deadline, async {
        loop {
            let hydration = manager.hydrate().await.map_err(|error| error.to_string())?;
            let run = hydration
                .runs
                .iter()
                .find(|run| run.run.id() == run_id)
                .ok_or_else(|| format!("automation run {run_id} disappeared during startup"))?;
            if run.run.process_state().is_terminal() {
                return Err(format!(
                    "automation run {run_id} ended as {:?} during startup",
                    run.run.process_state()
                ));
            }
            if run.run.process_state() == ProcessState::Ready && !run.draft_restore_pending {
                return session_assistant_messages(manager, run_id).await;
            }
            match state_changes.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("runtime state stream closed during automation startup".to_owned());
                }
            }
        }
    })
    .await
    .map_err(|_| format!("timed out waiting for automation run {run_id} readiness"))?
}

async fn automation_submit_prompt(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
    prompt: &str,
) -> Result<(), String> {
    let completion = manager
        .request(
            run_id,
            RpcRequest::new(RpcCommand::Prompt {
                message: prompt.to_owned(),
                images: Vec::new(),
                streaming_behavior: None,
            }),
        )
        .await
        .map_err(|error| error.to_string())?;
    if completion.response.success {
        Ok(())
    } else {
        Err(completion
            .response
            .error
            .unwrap_or_else(|| "Pi rejected automation prompt".to_owned()))
    }
}

async fn session_assistant_messages(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
) -> Result<usize, String> {
    let completion = manager
        .request(run_id, RpcRequest::new(RpcCommand::GetSessionStats))
        .await
        .map_err(|error| error.to_string())?;
    if !completion.response.success {
        return Err(completion
            .response
            .error
            .unwrap_or_else(|| "Pi rejected get_session_stats".to_owned()));
    }
    let value = completion
        .response
        .data
        .as_ref()
        .and_then(|data| data.get("assistantMessages"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "Pi get_session_stats omitted assistantMessages".to_owned())?;
    usize::try_from(value).map_err(|_| "Pi assistant message count is out of range".to_owned())
}

async fn last_assistant_text(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
) -> Result<String, String> {
    let completion = manager
        .request(run_id, RpcRequest::new(RpcCommand::GetLastAssistantText))
        .await
        .map_err(|error| error.to_string())?;
    if !completion.response.success {
        return Err(completion
            .response
            .error
            .unwrap_or_else(|| "Pi rejected get_last_assistant_text".to_owned()));
    }
    completion
        .response
        .data
        .as_ref()
        .and_then(|data| data.get("text"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Pi returned no last assistant text".to_owned())
}

async fn terminate_automation_owned_run(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
) -> Result<(), String> {
    let hydration = manager.hydrate().await.map_err(|error| error.to_string())?;
    let Some(run) = hydration.runs.iter().find(|run| run.run.id() == run_id) else {
        return Ok(());
    };
    if run.run.process_state().is_terminal() {
        return Ok(());
    }
    if run.run.process_state() == ProcessState::Starting {
        let closed = manager
            .close_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if closed.quarantined || !closed.process_terminated {
            return Err(
                "automation-owned starting process could not be confirmed terminated".to_owned(),
            );
        }
        return Ok(());
    }
    if run.run.process_state() == ProcessState::Ready
        && run.run.activity_state() != pi_wizard_core::runtime::ActivityState::Idle
    {
        let stopped = manager
            .stop_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if stopped.quarantined {
            return Err("automation-owned Stop left process termination uncertain".to_owned());
        }
        if stopped.process_terminated {
            return Ok(());
        }
    }

    let hydration = manager.hydrate().await.map_err(|error| error.to_string())?;
    let Some(run) = hydration.runs.iter().find(|run| run.run.id() == run_id) else {
        return Ok(());
    };
    if run.run.process_state().is_terminal() {
        return Ok(());
    }
    if run.run.process_state() == ProcessState::Ready
        && run.run.activity_state() == pi_wizard_core::runtime::ActivityState::Idle
    {
        let closed = manager
            .close_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if closed.quarantined || !closed.process_terminated {
            return Err("automation-owned Close could not confirm process termination".to_owned());
        }
        return Ok(());
    }

    Err(format!(
        "automation-owned run {run_id} could not be terminated from process state {:?}",
        run.run.process_state()
    ))
}

async fn run_supervisor_cycle(
    context: &AutomationRuntimeContext,
    input: SupervisorCycleInput<'_>,
) -> Result<Option<std::collections::HashSet<RunId>>, String> {
    let SupervisorCycleInput {
        execution_id,
        chain,
        supervisor_run,
        hydration,
        workers,
        idle_workers,
        cancel,
    } = input;
    if *cancel.borrow() {
        return Ok(None);
    }
    let prompt = supervisor_prompt(
        context,
        chain,
        execution_id,
        hydration,
        workers,
        idle_workers,
    )
    .await?;
    let before = session_assistant_messages(&context.manager, supervisor_run).await?;
    let mut state_changes = context.manager.subscribe_state_changes();
    automation_submit_prompt(&context.manager, supervisor_run, &prompt).await?;
    let supervisor_wait = async {
        loop {
            tokio::select! {
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        return Ok::<bool, String>(false);
                    }
                }
                changed = state_changes.recv() => {
                    if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                        return Err("supervisor state stream closed".to_owned());
                    }
                }
            }
            let snapshot = context
                .manager
                .hydrate()
                .await
                .map_err(|error| error.to_string())?;
            let run = snapshot
                .runs
                .iter()
                .find(|run| run.run.id() == supervisor_run)
                .ok_or_else(|| "supervisor run disappeared".to_owned())?;
            if run.run.process_state().is_terminal() {
                return Err(format!(
                    "supervisor process ended as {:?}",
                    run.run.process_state()
                ));
            }
            if run.run.process_state() == ProcessState::Ready
                && run.run.activity_state() == pi_wizard_core::runtime::ActivityState::Idle
                && session_assistant_messages(&context.manager, supervisor_run).await? > before
            {
                return Ok(true);
            }
        }
    };
    let completed = tokio::time::timeout(
        Duration::from_millis(context.limits.automation_supervisor_turn_deadline_ms),
        supervisor_wait,
    )
    .await
    .map_err(|_| "supervisor turn exceeded its configured deadline".to_owned())??;
    if !completed {
        return Ok(None);
    }
    let text = last_assistant_text(&context.manager, supervisor_run).await?;
    if text.len() > context.limits.max_supervisor_context_bytes {
        return Err(format!(
            "supervisor response used {} bytes, exceeding limit {}",
            text.len(),
            context.limits.max_supervisor_context_bytes
        ));
    }
    let normalized = strip_json_fence(&text);
    let reply: SupervisorReply = serde_json::from_str(normalized)
        .map_err(|error| format!("supervisor returned invalid directive JSON: {error}"))?;
    if reply.directives.len() > context.limits.max_supervisor_directives_per_cycle {
        return Err(format!(
            "supervisor returned {} directives, exceeding limit {}",
            reply.directives.len(),
            context.limits.max_supervisor_directives_per_cycle
        ));
    }
    let idle_set: std::collections::HashSet<_> = idle_workers.iter().copied().collect();
    let mut retained_idle = std::collections::HashSet::new();
    let mut targeted = std::collections::HashSet::new();
    for directive in reply.directives {
        if !targeted.insert(directive.run_id) {
            return Err(format!(
                "supervisor returned multiple directives for worker {} in one cycle",
                directive.run_id
            ));
        }
        let worker = workers
            .get_mut(&directive.run_id)
            .ok_or_else(|| format!("supervisor targeted unknown worker {}", directive.run_id))?;
        let message = directive.message.trim();
        if message.is_empty() {
            return Err("supervisor directive message cannot be empty".to_owned());
        }
        if message.len() > context.limits.max_draft_bytes_per_session {
            return Err(format!(
                "supervisor directive uses {} bytes, exceeding prompt limit {}",
                message.len(),
                context.limits.max_draft_bytes_per_session
            ));
        }
        let current = context
            .manager
            .hydrate()
            .await
            .map_err(|error| error.to_string())?;
        let run = current
            .runs
            .iter()
            .find(|run| run.run.id() == directive.run_id)
            .ok_or_else(|| {
                format!(
                    "supervisor target {} is no longer hydrated",
                    directive.run_id
                )
            })?;
        let command = match directive.action {
            SupervisorAction::Send => {
                let queue = run.run.queue();
                if !idle_set.contains(&directive.run_id)
                    || run.run.process_state() != ProcessState::Ready
                    || run.run.activity_state() != pi_wizard_core::runtime::ActivityState::Idle
                    || queue.steering > 0
                    || queue.follow_up > 0
                    || run.run.pending_ui_requests() > 0
                    || run.run.is_retry_waiting()
                    || run.run.has_summarization_retry()
                {
                    return Err(format!(
                        "supervisor can send a new prompt only to a currently idle worker: {}",
                        directive.run_id
                    ));
                }
                worker.assistant_messages_at_start =
                    session_assistant_messages(&context.manager, directive.run_id).await?;
                worker.turn_activity_observed = false;
                retained_idle.insert(directive.run_id);
                RpcCommand::Prompt {
                    message: message.to_owned(),
                    images: Vec::new(),
                    streaming_behavior: None,
                }
            }
            SupervisorAction::Steer => {
                if run.run.process_state() != ProcessState::Ready
                    || run.run.activity_state() != pi_wizard_core::runtime::ActivityState::Working
                {
                    return Err(format!(
                        "supervisor can steer only a working worker: {}",
                        directive.run_id
                    ));
                }
                RpcCommand::Steer {
                    message: message.to_owned(),
                    images: Vec::new(),
                }
            }
            SupervisorAction::FollowUp => {
                if run.run.process_state() != ProcessState::Ready
                    || run.run.activity_state() != pi_wizard_core::runtime::ActivityState::Working
                {
                    return Err(format!(
                        "supervisor can queue follow-up only for a working worker: {}",
                        directive.run_id
                    ));
                }
                RpcCommand::FollowUp {
                    message: message.to_owned(),
                    images: Vec::new(),
                }
            }
        };
        let completion = context
            .manager
            .request(directive.run_id, RpcRequest::new(command))
            .await
            .map_err(|error| error.to_string())?;
        if !completion.response.success {
            return Err(completion
                .response
                .error
                .unwrap_or_else(|| "Pi rejected supervisor directive".to_owned()));
        }
        set_automation_step_status(
            context,
            execution_id,
            worker.step_index,
            AutomationStepStatus::Working,
            None,
        )
        .await?;
    }
    Ok(Some(retained_idle))
}

async fn supervisor_prompt(
    context: &AutomationRuntimeContext,
    chain: &AutomationChain,
    execution_id: AutomationExecutionId,
    hydration: &RuntimeHydrationSnapshot,
    workers: &HashMap<RunId, AutomationWorker>,
    idle_workers: &[RunId],
) -> Result<String, String> {
    let mut prompt = String::from(
        "You supervise a bounded Pi Wizard worker pool. Keep workers productive only when useful. Return JSON only in this exact shape: {\"directives\":[{\"runId\":\"UUID\",\"action\":\"send|steer|follow_up\",\"message\":\"...\"}]}. Use send only for idle workers. Use steer/follow_up only for working workers. An empty directives array means no intervention.\n",
    );
    prompt.push_str(&format!(
        "Execution: {execution_id}\nChain: {}\nWorkers:\n",
        chain.name
    ));
    let idle_set: std::collections::HashSet<_> = idle_workers.iter().copied().collect();
    for (run_id, worker) in workers {
        let Some(run) = hydration.runs.iter().find(|run| run.run.id() == *run_id) else {
            continue;
        };
        let status = match run.run.activity_state() {
            pi_wizard_core::runtime::ActivityState::Idle => "idle",
            pi_wizard_core::runtime::ActivityState::Working => "working",
            pi_wizard_core::runtime::ActivityState::Compacting => "compacting",
            pi_wizard_core::runtime::ActivityState::WaitingForInput => "needs_attention",
            pi_wizard_core::runtime::ActivityState::Aborting => "aborting",
        };
        let task = truncate_utf8_prefix(&chain.prompts[worker.step_index], 2_048);
        let result = if idle_set.contains(run_id) {
            last_assistant_text(&context.manager, *run_id)
                .await
                .ok()
                .map(|text| truncate_utf8_prefix(&text, 4_096).to_owned())
        } else {
            None
        };
        let line = match result {
            Some(result) => format!(
                "- runId={run_id} status={status} step={} task={:?} lastResult={:?}\n",
                worker.step_index + 1,
                task,
                result
            ),
            None => format!(
                "- runId={run_id} status={status} step={} task={:?}\n",
                worker.step_index + 1,
                task
            ),
        };
        if prompt.len().saturating_add(line.len()) > context.limits.max_supervisor_context_bytes {
            break;
        }
        prompt.push_str(&line);
    }
    if prompt.len() > context.limits.max_supervisor_context_bytes {
        return Err("supervisor instruction prefix exceeds configured context limit".to_owned());
    }
    Ok(prompt)
}

fn truncate_utf8_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn strip_json_fence(value: &str) -> &str {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix("```json") else {
        return trimmed;
    };
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

#[tauri::command]
async fn runtime_dismiss_terminal_run(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<(), String> {
    runtime
        .manager
        .dismiss_terminal_run(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_close(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<RuntimeCloseResult, String> {
    runtime
        .manager
        .close_run(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_session_tree(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<SessionTreeSnapshot, String> {
    runtime
        .manager
        .session_tree(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_compact_session(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<CompactionResult, String> {
    runtime
        .manager
        .compact_session(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_session_stats(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<SessionStats, String> {
    runtime
        .manager
        .session_stats(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[derive(Clone)]
struct DesktopLaunchProfile {
    environment: ResolvedLaunchEnvironment,
    version: PiVersion,
}

impl DesktopRuntime {
    #[cfg(test)]
    fn new() -> Result<Self, String> {
        Self::new_with_state_root(None)
    }

    fn new_with_state_root(state_root: Option<PathBuf>) -> Result<Self, String> {
        let limits = RuntimeLimits::default()
            .validate()
            .map_err(|error| error.to_string())?;
        let preferences = match state_root.as_ref() {
            Some(root) => {
                PreferencesStore::open(root, limits).map_err(|error| error.to_string())?
            }
            None => PreferencesStore::ephemeral(limits),
        };
        let initial_live_run_limit = preferences.live_run_limit();
        // `spawn_runtime_manager` requires an active Tokio runtime. Tauri owns
        // that runtime, so create the manager inside its async context even
        // though setup itself is synchronous.
        let manager = tauri::async_runtime::block_on(async {
            let manager = match state_root.as_ref() {
                Some(root) => spawn_runtime_manager_with_draft_persistence(limits, root),
                None => spawn_runtime_manager(limits),
            }?;
            if let Err(error) = manager.set_live_run_limit(initial_live_run_limit).await {
                let _ = manager.shutdown().await;
                return Err(error);
            }
            Ok(manager)
        })
        .map_err(|error| error.to_string())?;
        let projects = match state_root.as_ref() {
            Some(root) => ProjectRegistry::open(root, limits).map_err(|error| error.to_string())?,
            None => ProjectRegistry::ephemeral(limits),
        };
        let worktrees = match state_root.as_ref() {
            Some(root) => {
                WorktreeRegistry::open(root, limits).map_err(|error| error.to_string())?
            }
            None => WorktreeRegistry::ephemeral(limits),
        };
        let automation_store = match state_root.as_ref() {
            Some(root) => AutomationStore::open(root, limits).map_err(|error| error.to_string())?,
            None => AutomationStore::ephemeral(limits),
        };
        Ok(Self {
            manager,
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            launch_profile: Mutex::new(None),
            preferences: Mutex::new(preferences),
            projects: Mutex::new(projects),
            worktrees: Arc::new(Mutex::new(worktrees)),
            automation: AutomationCoordinator::new(automation_store, limits),
            git_review_jobs: Mutex::new(GitReviewJobRegistry::new(
                limits
                    .max_live_runs
                    .saturating_add(limits.max_retained_terminal_runs),
            )),
            session_catalog_jobs: AtomicUsize::new(0),
        })
    }

    async fn launch_profile(&self) -> Result<DesktopLaunchProfile, String> {
        let mut cache = self.launch_profile.lock().await;
        if let Some(profile) = cache.as_ref() {
            return Ok(profile.clone());
        }

        let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        let initial = resolve_launch_environment(LaunchEnvironmentInput {
            desktop_environment: desktop_environment.clone(),
            ..LaunchEnvironmentInput::default()
        });
        let environment = match initial {
            Ok(environment) => environment,
            Err(EnvironmentResolutionError::PiNotFoundInAnyEnvironment) => {
                let shell_probe_environment =
                    probe_login_shell_environment(&desktop_environment, self.limits)
                        .await
                        .map_err(|error| error.to_string())?;
                resolve_launch_environment(LaunchEnvironmentInput {
                    desktop_environment,
                    shell_probe_environment: Some(shell_probe_environment),
                    ..LaunchEnvironmentInput::default()
                })
                .map_err(|error| error.to_string())?
            }
            Err(error) => return Err(error.to_string()),
        };
        let version = probe_pi_version(&environment, self.limits)
            .await
            .map_err(|error| error.to_string())?;
        let profile = DesktopLaunchProfile {
            environment,
            version,
        };
        *cache = Some(profile.clone());
        Ok(profile)
    }

    async fn project_binding(&self, path: PathBuf) -> Result<ProjectBinding, String> {
        let mut projects = self.projects.lock().await;
        projects
            .resolve_or_register(path)
            .map_err(|error| error.to_string())
    }

    async fn registered_project(
        &self,
        project_id: pi_wizard_core::ProjectId,
    ) -> Result<ProjectBinding, String> {
        let projects = self.projects.lock().await;
        projects
            .get(project_id)
            .cloned()
            .ok_or_else(|| format!("worktree recovery references unknown project {project_id}"))
    }

    async fn set_persisted_live_run_limit(
        &self,
        limit: usize,
    ) -> Result<DesktopRuntimeCapacity, String> {
        let mut preferences = self.preferences.lock().await;
        let previous = preferences.live_run_limit();
        preferences
            .set_live_run_limit(limit)
            .map_err(|error| error.to_string())?;
        match self.manager.set_live_run_limit(limit).await {
            Ok(snapshot) => Ok(DesktopRuntimeCapacity {
                active_runs: snapshot.active_runs,
                live_run_limit: snapshot.live_run_limit,
                configured_max_live_runs: snapshot.configured_max_live_runs,
                preference_recovery_notice: preferences.recovery_notice().map(str::to_owned),
            }),
            Err(error) => {
                let rollback = preferences.set_live_run_limit(previous);
                Err(match rollback {
                    Ok(()) => error.to_string(),
                    Err(rollback_error) => format!(
                        "{error}; additionally failed to restore previous persisted live-run preference: {rollback_error}"
                    ),
                })
            }
        }
    }

    async fn capacity_report(&self) -> Result<DesktopRuntimeCapacity, String> {
        let snapshot = self
            .manager
            .capacity()
            .await
            .map_err(|error| error.to_string())?;
        let preferences = self.preferences.lock().await;
        Ok(DesktopRuntimeCapacity {
            active_runs: snapshot.active_runs,
            live_run_limit: snapshot.live_run_limit,
            configured_max_live_runs: snapshot.configured_max_live_runs,
            preference_recovery_notice: preferences.recovery_notice().map(str::to_owned),
        })
    }

    async fn begin_git_review(&self, run_id: RunId) -> Result<u64, String> {
        self.git_review_jobs.lock().await.begin(run_id)
    }

    async fn attach_git_review(&self, run_id: RunId, generation: u64, abort: AbortHandle) -> bool {
        self.git_review_jobs
            .lock()
            .await
            .attach(run_id, generation, abort)
    }

    async fn complete_git_review(&self, run_id: RunId, generation: u64) {
        self.git_review_jobs
            .lock()
            .await
            .complete(run_id, generation);
    }

    async fn cancel_git_review(&self, run_id: RunId) -> bool {
        self.git_review_jobs.lock().await.cancel(run_id)
    }
}

#[tauri::command]
async fn runtime_diagnostics(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<DesktopRuntimeDiagnostics, String> {
    let snapshot = runtime
        .manager
        .diagnostics()
        .await
        .map_err(|error| error.to_string())?;
    let active_git_review_jobs = runtime.git_review_jobs.lock().await.active_count();
    Ok(DesktopRuntimeDiagnostics {
        runtime: snapshot,
        active_git_review_jobs,
        active_session_catalog_jobs: runtime.session_catalog_jobs.load(Ordering::Acquire),
    })
}

#[tauri::command]
async fn runtime_cleanup_worktree_recovery(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: WorktreeRecoveryRequest,
) -> Result<WorktreeCleanupResult, String> {
    // Serialize all run starts and worktree cleanup so the active-run check
    // cannot race another desktop launch that claims this filesystem root.
    let _gate = runtime.launch_cleanup_gate.lock().await;
    let record = {
        let registry = runtime.worktrees.lock().await;
        registry
            .get(request.id)
            .cloned()
            .ok_or_else(|| format!("unknown worktree recovery {}", request.id))?
    };
    if record.created.is_none() {
        return Err(
            "worktree cleanup requires a previously verified created recovery; inspect it first"
                .to_owned(),
        );
    }

    let profile = runtime.launch_profile().await?;
    let probe = probe_worktree_recovery(&record.plan(), &profile.environment, runtime.limits)
        .await
        .map_err(|error| error.to_string())?;
    let WorktreeRecoveryProbe::Exact { created } = probe else {
        return Err(
            "worktree cleanup requires an exact repository/branch/path recovery match; inspect and resolve conflicting state first"
                .to_owned(),
        );
    };
    let snapshot = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    if snapshot.runs.iter().any(|run| {
        !run.run.process_state().is_terminal()
            && run.run.execution_root().starts_with(&created.worktree_root)
    }) {
        return Err(
            "worktree cleanup refused because a live Pi run still uses this worktree".to_owned(),
        );
    }

    let result = cleanup_pristine_worktree(&record.plan(), &profile.environment, runtime.limits)
        .await
        .map_err(|error| error.to_string())?;
    if matches!(result, WorktreeCleanupResult::Removed) {
        let proof = probe_worktree_recovery(&record.plan(), &profile.environment, runtime.limits)
            .await
            .map_err(|error| error.to_string())?;
        match proof {
            WorktreeRecoveryProbe::NotCreated => {
                let mut registry = runtime.worktrees.lock().await;
                registry
                    .discard_proven_absent(record.id, &WorktreeRecoveryProbe::NotCreated)
                    .map_err(|error| error.to_string())?;
            }
            WorktreeRecoveryProbe::Partial {
                branch_exists,
                path_exists,
                detail,
            } => {
                return Ok(WorktreeCleanupResult::Partial {
                    branch_exists,
                    path_exists,
                    detail,
                });
            }
            WorktreeRecoveryProbe::Exact { .. } => {
                return Ok(WorktreeCleanupResult::Partial {
                    branch_exists: true,
                    path_exists: true,
                    detail: "cleanup completed but the recorded Git resources became visible again; recovery record was retained"
                        .to_owned(),
                });
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PiEnvironmentProbeReport {
    environment: LaunchEnvironmentDiagnostics,
    version: PiVersion,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartProjectRequest {
    project_path: PathBuf,
    project_trust: ProjectTrustPolicy,
    #[serde(default)]
    context_files: ContextFilesPolicy,
    #[serde(default)]
    extension_discovery: ExtensionDiscoveryPolicy,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    thinking: Option<ThinkingLevel>,
    #[serde(default)]
    initial_task: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartProjectWorktreeRequest {
    project_path: PathBuf,
    project_trust: ProjectTrustPolicy,
    #[serde(default)]
    context_files: ContextFilesPolicy,
    #[serde(default)]
    extension_discovery: ExtensionDiscoveryPolicy,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    thinking: Option<ThinkingLevel>,
    base: WorktreeBaseSnapshot,
    branch: String,
    worktree_path: PathBuf,
    #[serde(default)]
    initial_task: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartRunResult {
    run_id: RunId,
    initial_task_submitted: bool,
    initial_task_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAutomationChainRequest {
    #[serde(default)]
    id: Option<AutomationChainId>,
    name: String,
    prompts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationChainRequest {
    id: AutomationChainId,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartAutomationRequest {
    chain_id: AutomationChainId,
    project_id: ProjectId,
    concurrency: usize,
    worktrees: bool,
    supervisor: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationExecutionRequest {
    id: AutomationExecutionId,
}

#[derive(Clone)]
struct AutomationRuntimeContext {
    manager: RuntimeManagerHandle,
    limits: RuntimeLimits,
    launch_cleanup_gate: Arc<Mutex<()>>,
    worktrees: Arc<Mutex<WorktreeRegistry>>,
    coordinator: AutomationCoordinator,
}

struct AutomationExecutionPlan {
    execution_id: AutomationExecutionId,
    chain: AutomationChain,
    project: ProjectBinding,
    environment: ResolvedLaunchEnvironment,
    base: Option<WorktreeBaseSnapshot>,
    request: StartAutomationRequest,
}

struct AutomationRunLaunch<'a> {
    execution_id: AutomationExecutionId,
    project: &'a ProjectBinding,
    environment: &'a ResolvedLaunchEnvironment,
    base: Option<&'a WorktreeBaseSnapshot>,
    label: &'a str,
    initial_task: Option<&'a str>,
    supervisor: bool,
}

struct SupervisorCycleInput<'a> {
    execution_id: AutomationExecutionId,
    chain: &'a AutomationChain,
    supervisor_run: RunId,
    hydration: &'a RuntimeHydrationSnapshot,
    workers: &'a mut HashMap<RunId, AutomationWorker>,
    idle_workers: &'a [RunId],
    cancel: &'a mut watch::Receiver<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SupervisorAction {
    Send,
    Steer,
    FollowUp,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorDirective {
    run_id: RunId,
    action: SupervisorAction,
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorReply {
    #[serde(default)]
    directives: Vec<SupervisorDirective>,
}

struct AutomationWorker {
    step_index: usize,
    assistant_messages_at_start: usize,
    turn_activity_observed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopProjectRecord {
    id: ProjectId,
    canonical_root: PathBuf,
    status: &'static str,
    detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelocateProjectRequest {
    id: ProjectId,
    new_root: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRequest {
    id: ProjectId,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PickDirectoryRequest {
    default_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeProjectWorktreeRequest {
    project_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeProjectResourcesRequest {
    project_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeRecoveryRequest {
    id: WorktreeId,
}

#[tauri::command]
async fn runtime_probe_project_resources(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ProbeProjectResourcesRequest,
) -> Result<ProjectResourcePreflight, String> {
    let project = runtime.project_binding(request.project_path).await?;
    let project_root = project.canonical_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        inspect_project_resources(&project_root).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRecoveredWorktreeRequest {
    id: WorktreeId,
    project_trust: ProjectTrustPolicy,
    #[serde(default)]
    context_files: ContextFilesPolicy,
    #[serde(default)]
    extension_discovery: ExtensionDiscoveryPolicy,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    thinking: Option<ThinkingLevel>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeRecoveryPage {
    records: Vec<WorktreeRecoveryRecord>,
    truncated: bool,
    recovery_notice: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeRecoveryInspection {
    record: Option<WorktreeRecoveryRecord>,
    probe: WorktreeRecoveryProbe,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListProjectSessionsRequest {
    project_path: PathBuf,
    query: Option<String>,
    cursor: Option<SessionCatalogCursor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeProjectLaunchOptionsRequest {
    project_path: PathBuf,
    project_trust: ProjectTrustPolicy,
    #[serde(default)]
    context_files: ContextFilesPolicy,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectLaunchOptions {
    current_model: Option<ModelSummary>,
    current_thinking_level: ThinkingLevel,
    models: Vec<ModelSummary>,
    thinking_levels: Vec<ThinkingLevel>,
    clear_queue_supported: bool,
}

#[derive(Clone, Debug)]
struct LaunchSelection {
    context_files: ContextFilesPolicy,
    extension_discovery: ExtensionDiscoveryPolicy,
    provider: Option<String>,
    model: Option<String>,
    thinking: Option<ThinkingLevel>,
}

impl LaunchSelection {
    fn validate(
        context_files: ContextFilesPolicy,
        extension_discovery: ExtensionDiscoveryPolicy,
        provider: Option<String>,
        model: Option<String>,
        thinking: Option<ThinkingLevel>,
    ) -> Result<Self, String> {
        let provider = provider
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let model = model
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if provider.is_some() != model.is_some() {
            return Err(
                "launch model selection must include both a provider and model id, or neither"
                    .to_owned(),
            );
        }
        Ok(Self {
            context_files,
            extension_discovery,
            provider,
            model,
            thinking,
        })
    }

    fn apply(&self, spec: &mut PiLaunchSpec) {
        spec.context_files = self.context_files;
        spec.extension_discovery = self.extension_discovery;
        spec.provider.clone_from(&self.provider);
        spec.model.clone_from(&self.model);
        spec.thinking = self.thinking;
    }
}

async fn probe_rpc_response(
    process: &mut SpawnedPiProcess,
    command: RpcCommand,
    deadline: Duration,
) -> Result<RpcResponse, String> {
    let request = RpcRequest::new(command);
    let request_id = request.id.as_str().to_owned();
    process
        .writer
        .send_request(&request)
        .await
        .map_err(|error| error.to_string())?;
    tokio::time::timeout(deadline, async {
        loop {
            let message = process
                .reader
                .next_message()
                .await
                .ok_or_else(|| {
                    "Pi launch-options probe closed stdout before responding".to_owned()
                })?
                .map_err(|error| error.to_string())?;
            if let InboundMessage::Response(response) = message
                && response.id.as_deref() == Some(request_id.as_str())
            {
                return Ok(response);
            }
        }
    })
    .await
    .map_err(|_| format!("Pi launch-options probe timed out waiting for {request_id}"))?
}

#[tauri::command]
async fn runtime_probe_project_launch_options(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ProbeProjectLaunchOptionsRequest,
) -> Result<ProjectLaunchOptions, String> {
    let profile = runtime.launch_profile().await?;
    let environment = profile.environment;
    let project = runtime.project_binding(request.project_path).await?;
    // Capability discovery is deliberately extension-free. A broken global or
    // project extension must not prevent the user from opening the launcher and
    // selecting the supported one-run `--no-extensions` recovery policy.
    let selection = LaunchSelection::validate(
        request.context_files,
        ExtensionDiscoveryPolicy::Disabled,
        request.provider,
        request.model,
        None,
    )?;
    let mut spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        project.canonical_root(),
        request.project_trust,
    );
    selection.apply(&mut spec);
    spec.session = SessionLaunch::Ephemeral;
    let resolved = spec.resolve().map_err(|error| error.to_string())?;
    let mut process = spawn_pi_process(&resolved, &environment, runtime.limits)
        .map_err(|error| error.to_string())?;
    let rpc_deadline = Duration::from_millis(runtime.limits.startup_rpc_deadline_ms);
    let result = async {
        let state = probe_rpc_response(&mut process, RpcCommand::GetState, rpc_deadline).await?;
        let models =
            probe_rpc_response(&mut process, RpcCommand::GetAvailableModels, rpc_deadline).await?;
        let thinking = probe_rpc_response(
            &mut process,
            RpcCommand::GetAvailableThinkingLevels,
            rpc_deadline,
        )
        .await?;
        let clear_queue =
            probe_rpc_response(&mut process, RpcCommand::ClearQueue, rpc_deadline).await?;
        let clear_queue_supported = if clear_queue.success {
            clear_queue
                .clear_queue_result(runtime.limits)
                .map_err(|error| error.to_string())?;
            true
        } else if clear_queue
            .error
            .as_deref()
            .is_some_and(|error| error.to_ascii_lowercase().contains("unknown command"))
        {
            false
        } else {
            return Err(format!(
                "Pi rejected clear_queue compatibility probe: {}",
                clear_queue.error.as_deref().unwrap_or("unknown error")
            ));
        };
        let state = state
            .state_snapshot(runtime.limits)
            .map_err(|error| error.to_string())?;
        Ok(ProjectLaunchOptions {
            current_model: state.model,
            current_thinking_level: state.thinking_level,
            models: models
                .available_models(runtime.limits)
                .map_err(|error| error.to_string())?,
            thinking_levels: thinking
                .available_thinking_levels(runtime.limits)
                .map_err(|error| error.to_string())?,
            clear_queue_supported,
        })
    }
    .await;
    let termination = process
        .control
        .terminate(Duration::from_millis(
            runtime.limits.stop_termination_deadline_ms,
        ))
        .await;
    match (result, termination) {
        (Ok(options), Ok(TerminationOutcome::Exited { .. })) => Ok(options),
        (Ok(_), Ok(TerminationOutcome::Unconfirmed { .. })) => Err(
            "Pi launch-options probe completed but its temporary process could not be confirmed terminated"
                .to_owned(),
        ),
        (Ok(_), Err(error)) => Err(format!(
            "Pi launch-options probe completed but cleanup failed: {error}"
        )),
        (Err(error), Ok(TerminationOutcome::Exited { .. })) => Err(error),
        (Err(error), Ok(TerminationOutcome::Unconfirmed { .. })) => Err(format!(
            "{error}; temporary Pi probe termination could not be confirmed"
        )),
        (Err(error), Err(cleanup)) => Err(format!("{error}; probe cleanup failed: {cleanup}")),
    }
}

#[tauri::command]
async fn runtime_list_worktree_recoveries(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<WorktreeRecoveryPage, String> {
    let registry = runtime.worktrees.lock().await;
    let mut records = registry.records();
    records.reverse();
    let truncated = records.len() > runtime.limits.max_worktree_recovery_page_entries;
    records.truncate(runtime.limits.max_worktree_recovery_page_entries);
    Ok(WorktreeRecoveryPage {
        records,
        truncated,
        recovery_notice: registry.recovery_notice().map(str::to_owned),
    })
}

#[tauri::command]
async fn runtime_reconcile_worktree_recovery(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: WorktreeRecoveryRequest,
) -> Result<WorktreeRecoveryInspection, String> {
    let record = {
        let registry = runtime.worktrees.lock().await;
        registry
            .get(request.id)
            .cloned()
            .ok_or_else(|| format!("unknown worktree recovery {}", request.id))?
    };
    let profile = runtime.launch_profile().await?;
    let probe = probe_worktree_recovery(&record.plan(), &profile.environment, runtime.limits)
        .await
        .map_err(|error| error.to_string())?;
    match &probe {
        WorktreeRecoveryProbe::NotCreated => {
            let mut registry = runtime.worktrees.lock().await;
            registry
                .discard_proven_absent(record.id, &probe)
                .map_err(|error| error.to_string())?;
            Ok(WorktreeRecoveryInspection {
                record: None,
                probe,
            })
        }
        WorktreeRecoveryProbe::Exact { created } => {
            let updated = {
                let mut registry = runtime.worktrees.lock().await;
                registry
                    .mark_created(record.id, created.clone())
                    .map_err(|error| error.to_string())?
            };
            Ok(WorktreeRecoveryInspection {
                record: Some(updated),
                probe,
            })
        }
        _ => Ok(WorktreeRecoveryInspection {
            record: Some(record),
            probe,
        }),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeProjectSessionRequest {
    project_path: PathBuf,
    project_trust: ProjectTrustPolicy,
    #[serde(default)]
    context_files: ContextFilesPolicy,
    #[serde(default)]
    extension_discovery: ExtensionDiscoveryPolicy,
    session_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrainRuntimeRequest {
    run_id: RunId,
    max_events: usize,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunControlRequest {
    run_id: RunId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewFileRequest {
    run_id: RunId,
    path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewFilePageRequest {
    run_id: RunId,
    path: PathBuf,
    cursor: Option<GitDiffCursor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditDraftRequest {
    run_id: RunId,
    text: String,
}

#[tauri::command]
async fn runtime_git_review_summary(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<GitReviewSummary, String> {
    let generation = runtime.begin_git_review(request.run_id).await?;
    let execution_root = match run_execution_root(&runtime, request.run_id).await {
        Ok(root) => root,
        Err(error) => {
            runtime
                .complete_git_review(request.run_id, generation)
                .await;
            return Err(error);
        }
    };
    let profile = match runtime.launch_profile().await {
        Ok(profile) => profile,
        Err(error) => {
            runtime
                .complete_git_review(request.run_id, generation)
                .await;
            return Err(error);
        }
    };
    let limits = runtime.limits;
    let task = tokio::spawn(async move {
        review_summary(&execution_root, &profile.environment, limits)
            .await
            .map_err(|error| error.to_string())
    });
    finish_git_review_task(&runtime, request.run_id, generation, task).await
}

#[tauri::command]
async fn runtime_git_review_file(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ReviewFileRequest,
) -> Result<GitFileDiff, String> {
    let generation = runtime.begin_git_review(request.run_id).await?;
    let execution_root = match run_execution_root(&runtime, request.run_id).await {
        Ok(root) => root,
        Err(error) => {
            runtime
                .complete_git_review(request.run_id, generation)
                .await;
            return Err(error);
        }
    };
    let profile = match runtime.launch_profile().await {
        Ok(profile) => profile,
        Err(error) => {
            runtime
                .complete_git_review(request.run_id, generation)
                .await;
            return Err(error);
        }
    };
    let path = request.path;
    let limits = runtime.limits;
    let task = tokio::spawn(async move {
        review_file_diff(&execution_root, &path, &profile.environment, limits)
            .await
            .map_err(|error| error.to_string())
    });
    finish_git_review_task(&runtime, request.run_id, generation, task).await
}

#[tauri::command]
async fn runtime_git_review_file_page(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ReviewFilePageRequest,
) -> Result<GitFileDiffPage, String> {
    let generation = runtime.begin_git_review(request.run_id).await?;
    let execution_root = match run_execution_root(&runtime, request.run_id).await {
        Ok(root) => root,
        Err(error) => {
            runtime
                .complete_git_review(request.run_id, generation)
                .await;
            return Err(error);
        }
    };
    let profile = match runtime.launch_profile().await {
        Ok(profile) => profile,
        Err(error) => {
            runtime
                .complete_git_review(request.run_id, generation)
                .await;
            return Err(error);
        }
    };
    let path = request.path;
    let cursor = request.cursor;
    let limits = runtime.limits;
    let task = tokio::spawn(async move {
        review_file_diff_page(
            &execution_root,
            &path,
            cursor.as_ref(),
            &profile.environment,
            limits,
        )
        .await
        .map_err(|error| error.to_string())
    });
    finish_git_review_task(&runtime, request.run_id, generation, task).await
}

#[tauri::command]
async fn runtime_cancel_git_review(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<bool, String> {
    Ok(runtime.cancel_git_review(request.run_id).await)
}

async fn finish_git_review_task<T>(
    runtime: &DesktopRuntime,
    run_id: RunId,
    generation: u64,
    task: JoinHandle<Result<T, String>>,
) -> Result<T, String>
where
    T: Send + 'static,
{
    if !runtime
        .attach_git_review(run_id, generation, task.abort_handle())
        .await
    {
        task.abort();
        let _ = task.await;
        return Err("Git review was superseded before execution".to_owned());
    }
    let result = match task.await {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => {
            Err("Git review was cancelled or superseded".to_owned())
        }
        Err(error) => Err(format!("Git review task failed: {error}")),
    };
    runtime.complete_git_review(run_id, generation).await;
    result
}

async fn run_execution_root(runtime: &DesktopRuntime, run_id: RunId) -> Result<PathBuf, String> {
    let snapshot = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    snapshot
        .runs
        .iter()
        .find(|run| run.run.id() == run_id)
        .map(|run| run.run.execution_root().clone())
        .ok_or_else(|| format!("unknown run {run_id}"))
}

#[cfg(windows)]
fn folder_opener(execution_root: &std::path::Path) -> (PathBuf, Vec<OsString>) {
    let executable = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("explorer.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("explorer.exe"));
    (executable, vec![execution_root.as_os_str().to_owned()])
}

#[cfg(target_os = "macos")]
fn folder_opener(execution_root: &std::path::Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from("open"),
        vec![execution_root.as_os_str().to_owned()],
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn folder_opener(execution_root: &std::path::Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from("xdg-open"),
        vec![execution_root.as_os_str().to_owned()],
    )
}

#[tauri::command]
async fn runtime_open_run_folder(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<(), String> {
    let execution_root = run_execution_root(&runtime, request.run_id).await?;
    if !execution_root.is_dir() {
        return Err(format!(
            "run execution folder no longer exists: {}",
            execution_root.display()
        ));
    }
    let (executable, args) = folder_opener(&execution_root);
    let mut child = tokio::process::Command::new(&executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not open run folder: {error}"))?;
    tauri::async_runtime::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachDraftImageRequest {
    run_id: RunId,
    file_name: String,
    mime_type: String,
    data: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveDraftImageRequest {
    run_id: RunId,
    image_id: DraftImageId,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeAttachmentLimits {
    max_attachments: usize,
    max_image_bytes: usize,
    max_aggregate_bytes: usize,
    max_name_bytes: usize,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitDraftRequest {
    run_id: RunId,
    action: ComposerAction,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetRunModelRequest {
    run_id: RunId,
    provider: String,
    model_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetRunThinkingRequest {
    run_id: RunId,
    level: ThinkingLevel,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAutoCompactionRequest {
    run_id: RunId,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAutoRetryRequest {
    run_id: RunId,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetLiveRunLimitRequest {
    limit: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSessionNameRequest {
    run_id: RunId,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForkSessionRequest {
    run_id: RunId,
    entry_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionHistoryRequest {
    run_id: RunId,
    cursor: Option<SessionHistoryCursor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
enum DesktopExtensionUiResponse {
    Value { id: String, value: String },
    Confirmation { id: String, confirmed: bool },
    Cancelled { id: String },
}

#[tauri::command]
async fn runtime_edit_draft(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: EditDraftRequest,
) -> Result<pi_wizard_core::draft::DraftSnapshot, String> {
    runtime
        .manager
        .edit_draft(request.run_id, request.text)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_set_auto_retry(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetAutoRetryRequest,
) -> Result<(), String> {
    runtime
        .manager
        .set_auto_retry(request.run_id, request.enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_set_auto_compaction(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetAutoCompactionRequest,
) -> Result<(), String> {
    runtime
        .manager
        .set_auto_compaction(request.run_id, request.enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_attach_draft_image(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: AttachDraftImageRequest,
) -> Result<pi_wizard_core::draft::DraftSnapshot, String> {
    runtime
        .manager
        .attach_draft_image(
            request.run_id,
            request.file_name,
            request.mime_type,
            request.data,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_remove_draft_image(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RemoveDraftImageRequest,
) -> Result<pi_wizard_core::draft::DraftSnapshot, String> {
    runtime
        .manager
        .remove_draft_image(request.run_id, request.image_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn runtime_attachment_limits(runtime: tauri::State<'_, DesktopRuntime>) -> RuntimeAttachmentLimits {
    RuntimeAttachmentLimits {
        max_attachments: runtime.limits.max_attachments_per_prompt,
        max_image_bytes: runtime.limits.max_attachment_bytes_per_image,
        max_aggregate_bytes: runtime.limits.max_attachment_bytes_per_prompt,
        max_name_bytes: runtime.limits.max_attachment_name_bytes,
    }
}

#[tauri::command]
async fn runtime_capacity(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<DesktopRuntimeCapacity, String> {
    runtime.capacity_report().await
}

#[tauri::command]
async fn runtime_set_live_run_limit(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetLiveRunLimitRequest,
) -> Result<DesktopRuntimeCapacity, String> {
    let _gate = runtime.launch_cleanup_gate.lock().await;
    runtime.set_persisted_live_run_limit(request.limit).await
}

#[tauri::command]
async fn runtime_fork_session(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ForkSessionRequest,
) -> Result<SessionReplacementResult, String> {
    runtime
        .manager
        .fork_session(request.run_id, request.entry_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_set_model(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetRunModelRequest,
) -> Result<(), String> {
    runtime
        .manager
        .set_model(request.run_id, request.provider, request.model_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_set_thinking_level(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetRunThinkingRequest,
) -> Result<(), String> {
    runtime
        .manager
        .set_thinking_level(request.run_id, request.level)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_set_session_name(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetSessionNameRequest,
) -> Result<(), String> {
    runtime
        .manager
        .set_session_name(request.run_id, request.name)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_clone_session(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<SessionReplacementResult, String> {
    runtime
        .manager
        .clone_session(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_submit_draft(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SubmitDraftRequest,
) -> Result<ComposerSubmitResult, String> {
    runtime
        .manager
        .submit_draft(request.run_id, request.action)
        .await
        .map_err(|error| error.to_string())
}

impl From<DesktopExtensionUiResponse> for ExtensionUiResponse {
    fn from(value: DesktopExtensionUiResponse) -> Self {
        match value {
            DesktopExtensionUiResponse::Value { id, value } => Self::Value { id, value },
            DesktopExtensionUiResponse::Confirmation { id, confirmed } => {
                Self::Confirmation { id, confirmed }
            }
            DesktopExtensionUiResponse::Cancelled { id } => Self::Cancelled { id },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondExtensionUiRequest {
    run_id: RunId,
    response: DesktopExtensionUiResponse,
}

#[tauri::command]
async fn runtime_hydrate(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<RuntimeHydrationSnapshot, String> {
    runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_respond_extension_ui(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RespondExtensionUiRequest,
) -> Result<(), String> {
    runtime
        .manager
        .respond_extension_ui(request.run_id, request.response.into())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_drain(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: DrainRuntimeRequest,
) -> Result<RuntimeUiDrain, String> {
    runtime
        .manager
        .drain_ui(request.run_id, request.max_events)
        .await
        .map_err(|error| error.to_string())
}

fn desktop_project_record(binding: &ProjectBinding) -> DesktopProjectRecord {
    let (status, detail) = match binding.verify_registered_location() {
        ProjectRegisteredLocation::Present => ("present", None),
        ProjectRegisteredLocation::Missing => (
            "missing",
            Some("registered project folder no longer exists at this path".to_owned()),
        ),
        ProjectRegisteredLocation::Changed { current } => (
            "changed",
            Some(format!("path now resolves to {}", current.display())),
        ),
        ProjectRegisteredLocation::Unverifiable { error } => ("unverifiable", Some(error)),
    };
    DesktopProjectRecord {
        id: binding.id(),
        canonical_root: binding.canonical_root().to_path_buf(),
        status,
        detail,
    }
}

#[tauri::command]
async fn runtime_list_projects(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<Vec<DesktopProjectRecord>, String> {
    let projects = runtime.projects.lock().await;
    Ok(projects
        .bindings()
        .iter()
        .map(desktop_project_record)
        .collect())
}

#[tauri::command]
async fn runtime_pick_directory(
    app: tauri::AppHandle,
    request: PickDirectoryRequest,
) -> Result<Option<PathBuf>, String> {
    let mut dialog = app.dialog().file().set_title("Choose folder");
    if let Some(default_path) = request.default_path {
        let start = if default_path.is_dir() {
            Some(default_path)
        } else {
            default_path.parent().map(|parent| parent.to_path_buf())
        };
        if let Some(start) = start.filter(|path| path.is_dir()) {
            dialog = dialog.set_directory(start);
        }
    }
    dialog
        .blocking_pick_folder()
        .map(|selected| selected.into_path().map_err(|error| error.to_string()))
        .transpose()
}

async fn ensure_project_has_no_live_run(
    runtime: &DesktopRuntime,
    project_id: ProjectId,
) -> Result<(), String> {
    let snapshot = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    if snapshot
        .runs
        .iter()
        .any(|run| run.run.project_id() == project_id && !run.run.process_state().is_terminal())
    {
        return Err("stop the project's live Pi runs before changing its registration".to_owned());
    }
    Ok(())
}

async fn ensure_project_has_no_worktree_recovery(
    runtime: &DesktopRuntime,
    project_id: ProjectId,
) -> Result<(), String> {
    if runtime
        .worktrees
        .lock()
        .await
        .records()
        .iter()
        .any(|record| record.project_id == project_id)
    {
        return Err(
            "this project still owns worktree recovery records; resolve or remove those worktrees first"
                .to_owned(),
        );
    }
    Ok(())
}

#[tauri::command]
async fn runtime_relocate_project(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RelocateProjectRequest,
) -> Result<DesktopProjectRecord, String> {
    ensure_project_has_no_live_run(&runtime, request.id).await?;
    ensure_project_has_no_worktree_recovery(&runtime, request.id).await?;
    let mut projects = runtime.projects.lock().await;
    let binding = projects
        .relocate_explicit(request.id, request.new_root)
        .map_err(|error| error.to_string())?;
    Ok(desktop_project_record(&binding))
}

#[tauri::command]
async fn runtime_remove_project(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ProjectRequest,
) -> Result<(), String> {
    ensure_project_has_no_live_run(&runtime, request.id).await?;
    ensure_project_has_no_worktree_recovery(&runtime, request.id).await?;
    runtime
        .projects
        .lock()
        .await
        .remove_explicit(request.id)
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn finish_started_run(
    runtime: &DesktopRuntime,
    run_id: RunId,
    initial_task: Option<String>,
) -> StartRunResult {
    let Some(task) = initial_task
        .map(|task| task.trim().to_owned())
        .filter(|task| !task.is_empty())
    else {
        return StartRunResult {
            run_id,
            initial_task_submitted: false,
            initial_task_error: None,
        };
    };

    let result = async {
        let mut signals = runtime.manager.subscribe();
        let deadline = Duration::from_millis(
            runtime
                .limits
                .startup_rpc_deadline_ms
                .saturating_add(runtime.limits.draft_flush_deadline_ms)
                .saturating_add(1_000),
        );
        tokio::time::timeout(deadline, async {
            loop {
                let snapshot = runtime
                    .manager
                    .hydrate()
                    .await
                    .map_err(|error| error.to_string())?;
                let run = snapshot
                    .runs
                    .iter()
                    .find(|run| run.run.id() == run_id)
                    .ok_or_else(|| {
                        format!("new run {run_id} disappeared before its initial task")
                    })?;
                if run.run.process_state().is_terminal() {
                    return Err(format!(
                        "new run ended as {:?} before its initial task could be sent",
                        run.run.process_state()
                    ));
                }
                if run.run.process_state() == ProcessState::Ready
                    && run.composer_availability == ComposerAvailability::Ready
                    && !run.draft_restore_pending
                {
                    runtime
                        .manager
                        .edit_draft(run_id, task.clone())
                        .await
                        .map_err(|error| error.to_string())?;
                    let submitted = runtime
                        .manager
                        .submit_draft(run_id, ComposerAction::Send)
                        .await
                        .map_err(|error| error.to_string())?;
                    if submitted.accepted {
                        return Ok(());
                    }
                    return Err(submitted
                        .error
                        .unwrap_or_else(|| "Pi rejected the initial task".to_owned()));
                }
                signals
                    .recv()
                    .await
                    .map_err(|error| format!("runtime signal stream closed: {error}"))?;
            }
        })
        .await
        .map_err(|_| "timed out waiting for the new run to accept its initial task".to_owned())?
    }
    .await;

    StartRunResult {
        run_id,
        initial_task_submitted: result.is_ok(),
        initial_task_error: result.err(),
    }
}

#[tauri::command]
async fn runtime_start_project(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: StartProjectRequest,
) -> Result<StartRunResult, String> {
    let _gate = runtime.launch_cleanup_gate.lock().await;
    let selection = LaunchSelection::validate(
        request.context_files,
        request.extension_discovery,
        request.provider,
        request.model,
        request.thinking,
    )?;
    let profile = runtime.launch_profile().await?;
    let environment = profile.environment;
    let project = runtime.project_binding(request.project_path).await?;
    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        project.canonical_root(),
        request.project_trust,
    );
    selection.apply(&mut launch_spec);
    launch_spec.session = SessionLaunch::NewWithId(PiSessionId::new());
    let run_id = start_resolved_project_run(
        &runtime,
        project,
        environment,
        launch_spec,
        ExecutionIsolation::LocalCheckout,
        None,
    )
    .await?;
    drop(_gate);
    Ok(finish_started_run(&runtime, run_id, request.initial_task).await)
}

#[tauri::command]
async fn runtime_probe_project_worktree(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ProbeProjectWorktreeRequest,
) -> Result<WorktreeBaseSnapshot, String> {
    let profile = runtime.launch_profile().await?;
    let project = runtime.project_binding(request.project_path).await?;
    inspect_worktree_base(
        project.canonical_root(),
        &profile.environment,
        runtime.limits,
    )
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_start_project_worktree(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: StartProjectWorktreeRequest,
) -> Result<StartRunResult, String> {
    let _gate = runtime.launch_cleanup_gate.lock().await;
    let selection = LaunchSelection::validate(
        request.context_files,
        request.extension_discovery,
        request.provider,
        request.model,
        request.thinking,
    )?;
    let profile = runtime.launch_profile().await?;
    let environment = profile.environment;
    let project = runtime.project_binding(request.project_path).await?;
    if request.base.project_root != project.canonical_root() {
        return Err(
            "worktree base does not belong to the selected project; inspect Git base again"
                .to_owned(),
        );
    }
    let plan = WorktreeCreatePlan {
        base: request.base,
        branch: request.branch,
        worktree_path: request.worktree_path,
    };
    let recovery = {
        let mut registry = runtime.worktrees.lock().await;
        registry
            .begin_creation(project.id(), &plan)
            .map_err(|error| error.to_string())?
    };
    let created = match create_worktree(plan, &environment, runtime.limits).await {
        Ok(created) => created,
        Err(error) => {
            if !error.may_have_mutated() {
                let discard = {
                    let mut registry = runtime.worktrees.lock().await;
                    registry.discard_unmutated_plan(recovery.id)
                };
                if let Err(discard_error) = discard {
                    return Err(format!(
                        "{error}; Git mutation was not observed, but recovery intent {} could not be discarded: {discard_error}",
                        recovery.id
                    ));
                }
                return Err(error.to_string());
            }
            return Err(format!(
                "{error}; recovery transaction {} was retained for explicit inspection",
                recovery.id
            ));
        }
    };
    {
        let mut registry = runtime.worktrees.lock().await;
        registry
            .mark_created(recovery.id, created.clone())
            .map_err(|error| {
                format!(
                    "Git worktree was created at {} but recovery transaction {} could not record the verified identity: {error}",
                    created.worktree_root.display(),
                    recovery.id
                )
            })?;
    }

    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        &created.execution_root,
        request.project_trust,
    );
    selection.apply(&mut launch_spec);
    launch_spec.session = SessionLaunch::NewWithId(PiSessionId::new());
    match start_resolved_project_run(
        &runtime,
        project,
        environment,
        launch_spec,
        ExecutionIsolation::GitWorktree,
        Some(created.identity()),
    )
    .await
    {
        Ok(run_id) => {
            drop(_gate);
            Ok(finish_started_run(&runtime, run_id, request.initial_task).await)
        }
        Err(error) => Err(format!(
            "{error}; Git worktree was created and retained at {} as recovery transaction {}",
            created.worktree_root.display(),
            recovery.id
        )),
    }
}

#[tauri::command]
async fn runtime_start_recovered_worktree(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: StartRecoveredWorktreeRequest,
) -> Result<RunId, String> {
    let _gate = runtime.launch_cleanup_gate.lock().await;
    let selection = LaunchSelection::validate(
        request.context_files,
        request.extension_discovery,
        request.provider,
        request.model,
        request.thinking,
    )?;
    let record = {
        let registry = runtime.worktrees.lock().await;
        registry
            .get(request.id)
            .cloned()
            .ok_or_else(|| format!("unknown worktree recovery {}", request.id))?
    };
    let profile = runtime.launch_profile().await?;
    let environment = profile.environment;
    let probe = probe_worktree_recovery(&record.plan(), &environment, runtime.limits)
        .await
        .map_err(|error| error.to_string())?;
    let WorktreeRecoveryProbe::Exact { created } = probe else {
        return Err(
            "worktree no longer matches its recorded repository/branch/path or no longer descends from the captured base; reconcile it before launching"
                .to_owned(),
        );
    };
    {
        let mut registry = runtime.worktrees.lock().await;
        registry
            .mark_created(record.id, created.clone())
            .map_err(|error| error.to_string())?;
    }
    let project = runtime.registered_project(record.project_id).await?;
    if project.verify_registered_location() != ProjectRegisteredLocation::Present {
        return Err(
            "the logical project registration is detached or moved; explicitly relocate it before launching this recovered worktree"
                .to_owned(),
        );
    }
    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        &created.execution_root,
        request.project_trust,
    );
    selection.apply(&mut launch_spec);
    launch_spec.session = SessionLaunch::NewWithId(PiSessionId::new());
    start_resolved_project_run(
        &runtime,
        project,
        environment,
        launch_spec,
        ExecutionIsolation::GitWorktree,
        Some(created.identity()),
    )
    .await
}

#[tauri::command]
async fn runtime_read_session_history(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ReadSessionHistoryRequest,
) -> Result<SessionHistoryPage, String> {
    let snapshot = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.run.id() == request.run_id)
        .ok_or_else(|| format!("unknown run {}", request.run_id))?;
    let session = run.run.session_state();
    let session_path = session
        .session_file
        .clone()
        .ok_or_else(|| "Pi has not exposed a persistent session file for this run".to_owned())?;
    let session_id = session
        .session_id
        .clone()
        .ok_or_else(|| "Pi has not exposed a session id for this run".to_owned())?;
    let canonical_session_path = session_path
        .canonicalize()
        .map_err(|error| format!("could not resolve Pi session file: {error}"))?;
    let project_root = run.run.execution_root().clone();
    let cursor = request.cursor;
    let limits = runtime.limits;
    let read_path = canonical_session_path.clone();
    let read_session_id = session_id.clone();
    let page = tauri::async_runtime::spawn_blocking(move || {
        read_session_history_page(
            &read_path,
            &project_root,
            &read_session_id,
            cursor.as_ref(),
            limits,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    // Disk work runs outside the manager actor. Re-check the exact session
    // binding after it completes so a concurrent clone/fork/switch cannot make
    // a stale page appear under the newly active session for this RunId.
    let current = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    let still_current = current
        .runs
        .iter()
        .find(|run| run.run.id() == request.run_id)
        .is_some_and(|run| {
            let current_session = run.run.session_state();
            current_session.session_id.as_deref() == Some(session_id.as_str())
                && current_session
                    .session_file
                    .as_ref()
                    .and_then(|path| path.canonicalize().ok())
                    .as_ref()
                    == Some(&canonical_session_path)
        });
    if !still_current {
        return Err(
            "Pi session changed while history was loading; retry against current session"
                .to_owned(),
        );
    }
    Ok(page)
}

async fn start_resolved_project_run(
    runtime: &DesktopRuntime,
    project: ProjectBinding,
    environment: ResolvedLaunchEnvironment,
    launch_spec: PiLaunchSpec,
    execution_isolation: ExecutionIsolation,
    worktree: Option<GitWorktreeIdentity>,
) -> Result<RunId, String> {
    let launch = launch_spec.resolve().map_err(|error| error.to_string())?;
    runtime
        .manager
        .start_run(RunStartSpec {
            project_id: project.id(),
            execution_isolation,
            worktree,
            launch,
            environment,
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn runtime_list_project_sessions(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ListProjectSessionsRequest,
) -> Result<SessionCatalogPage, String> {
    let profile = runtime.launch_profile().await?;
    let project = runtime.project_binding(request.project_path).await?;
    let project_root = project.canonical_root().to_path_buf();
    let environment = profile.environment.variables().clone();
    let limits = runtime.limits;
    let query = request.query;
    let cursor = request.cursor;
    let _job = ActiveJobGuard::new(&runtime.session_catalog_jobs);
    tauri::async_runtime::spawn_blocking(move || {
        list_project_sessions(
            &project_root,
            &environment,
            query.as_deref(),
            cursor.as_ref(),
            limits,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn runtime_resume_project_session(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ResumeProjectSessionRequest,
) -> Result<RunId, String> {
    let _gate = runtime.launch_cleanup_gate.lock().await;
    let profile = runtime.launch_profile().await?;
    let environment = profile.environment;
    let project = runtime.project_binding(request.project_path).await?;
    let project_root = project.canonical_root().to_path_buf();
    let session_path = request.session_path;
    let limits = runtime.limits;
    let validated = tauri::async_runtime::spawn_blocking(move || {
        validate_project_session(&project_root, &session_path, limits)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    let active = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    if active.runs.iter().any(|run| {
        !run.run.process_state().is_terminal()
            && run
                .run
                .session_state()
                .session_file
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .is_some_and(|path| path == validated.path)
    }) {
        return Err("that Pi session is already open in a live run".to_owned());
    }

    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        project.canonical_root(),
        request.project_trust,
    );
    launch_spec.context_files = request.context_files;
    launch_spec.extension_discovery = request.extension_discovery;
    launch_spec.session = SessionLaunch::Resume(validated.path);
    start_resolved_project_run(
        &runtime,
        project,
        environment,
        launch_spec,
        ExecutionIsolation::LocalCheckout,
        None,
    )
    .await
}

#[tauri::command]
async fn runtime_stop(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<RuntimeStopResult, String> {
    runtime
        .manager
        .stop_run(request.run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn probe_pi_environment(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<PiEnvironmentProbeReport, String> {
    let profile = runtime.launch_profile().await?;
    Ok(PiEnvironmentProbeReport {
        environment: profile.environment.diagnostics().clone(),
        version: profile.version,
    })
}

fn forward_runtime_signals(app: tauri::AppHandle, manager: RuntimeManagerHandle) {
    let mut signals = manager.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match signals.recv().await {
                Ok(signal @ RuntimeManagerSignal::RunDirty { .. }) => {
                    let _ = app.emit(RUNTIME_DIRTY_EVENT, signal);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // A wake-up is advisory; authoritative recovery is a fresh
                    // versioned hydration snapshot, never replaying raw Pi traffic.
                    let _ = app.emit(RUNTIME_REHYDRATE_EVENT, ());
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

fn forward_automation_signals(app: tauri::AppHandle, automation: AutomationCoordinator) {
    let mut signals = automation.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match signals.recv().await {
                Ok(signal) => {
                    let _ = app.emit(AUTOMATION_CHANGED_EVENT, signal);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // The signal is invalidation only. A lagged renderer widens
                    // to the full Automation snapshot when it next receives
                    // the event, rather than replaying intermediate states.
                    let _ = app.emit(AUTOMATION_CHANGED_EVENT, AutomationChangedSignal::Catalog);
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

/// Desktop composition root. Tauri commands adapt the Tauri-independent core;
/// they do not own Pi protocol or run semantics.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state_root = app
                .path()
                .app_data_dir()
                .map_err(|error| io::Error::other(error.to_string()))?
                .join("runtime-state");
            let runtime =
                DesktopRuntime::new_with_state_root(Some(state_root)).map_err(io::Error::other)?;
            let manager = runtime.manager.clone();
            let automation = runtime.automation.clone();
            app.manage(runtime);
            forward_runtime_signals(app.handle().clone(), manager);
            forward_automation_signals(app.handle().clone(), automation);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            runtime_hydrate,
            runtime_recover_ui,
            runtime_drain,
            runtime_edit_draft,
            runtime_attach_draft_image,
            runtime_remove_draft_image,
            runtime_attachment_limits,
            runtime_capacity,
            runtime_diagnostics,
            runtime_automation_snapshot,
            runtime_automation_executions,
            runtime_save_automation_chain,
            runtime_delete_automation_chain,
            runtime_start_automation,
            runtime_cancel_automation,
            runtime_set_live_run_limit,
            runtime_submit_draft,
            runtime_set_model,
            runtime_set_thinking_level,
            runtime_set_auto_compaction,
            runtime_set_auto_retry,
            runtime_session_stats,
            runtime_session_tree,
            runtime_compact_session,
            runtime_set_session_name,
            runtime_clone_session,
            runtime_fork_session,
            runtime_pick_directory,
            runtime_list_projects,
            runtime_relocate_project,
            runtime_remove_project,
            runtime_probe_project_resources,
            runtime_probe_project_launch_options,
            runtime_start_project,
            runtime_probe_project_worktree,
            runtime_start_project_worktree,
            runtime_list_worktree_recoveries,
            runtime_reconcile_worktree_recovery,
            runtime_cleanup_worktree_recovery,
            runtime_start_recovered_worktree,
            runtime_list_project_sessions,
            runtime_resume_project_session,
            runtime_read_session_history,
            runtime_git_review_summary,
            runtime_git_review_file,
            runtime_git_review_file_page,
            runtime_cancel_git_review,
            runtime_stop,
            runtime_close,
            runtime_dismiss_terminal_run,
            runtime_open_run_folder,
            runtime_respond_extension_ui,
            probe_pi_environment
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Pi Wizard desktop host");

    let shutdown_started = Arc::new(AtomicBool::new(false));
    let shutdown_complete = Arc::new(AtomicBool::new(false));
    app.run(move |app_handle, event| {
        let tauri::RunEvent::ExitRequested { api, code, .. } = event else {
            return;
        };
        if shutdown_complete.load(Ordering::Acquire) {
            return;
        }
        api.prevent_exit();
        if shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }

        let manager = app_handle.state::<DesktopRuntime>().manager.clone();
        let app_handle = app_handle.clone();
        let shutdown_complete = Arc::clone(&shutdown_complete);
        tauri::async_runtime::spawn(async move {
            // The core shutdown path is deadline-bounded per owned child and
            // quarantines uncertainty. App exit does not bypass that ownership.
            let _ = manager.shutdown().await;
            shutdown_complete.store(true, Ordering::Release);
            app_handle.exit(code.unwrap_or(0));
        });
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::*;
    use pi_wizard_core::runtime::RUNTIME_HYDRATION_SCHEMA_VERSION;
    use serde_json::json;

    struct AutomationFakePiFixture {
        root: PathBuf,
        fake_pi: PathBuf,
    }

    impl AutomationFakePiFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pi-wizard-automation-integration-{}",
                AutomationExecutionId::new()
            ));
            fs::create_dir_all(&root).expect("create automation integration fixture");
            let script = root.join("automation-fake-pi.js");
            fs::write(&script, AUTOMATION_FAKE_PI_JS).expect("write automation fake Pi");

            #[cfg(windows)]
            let fake_pi = {
                let path = root.join("pi.cmd");
                fs::write(
                    &path,
                    "@echo off\r\nnode \"%~dp0automation-fake-pi.js\" %*\r\n",
                )
                .expect("write automation fake Pi wrapper");
                path
            };

            #[cfg(not(windows))]
            let fake_pi = {
                use std::os::unix::fs::PermissionsExt;
                let path = root.join("pi");
                fs::write(
                    &path,
                    "#!/bin/sh\nexec node \"$(dirname \"$0\")/automation-fake-pi.js\" \"$@\"\n",
                )
                .expect("write automation fake Pi wrapper");
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
            .expect("resolve automation fake Pi environment")
        }

        fn initialize_git_repository(&self) -> ResolvedLaunchEnvironment {
            let environment = self.environment();
            let git = environment
                .git_executable()
                .expect("Git is required for automation worktree integration");
            let run_git = |args: &[&str]| {
                let status = Command::new(git)
                    .current_dir(&self.root)
                    .args(args)
                    .status()
                    .expect("run Git fixture command");
                assert!(status.success(), "Git fixture command failed: {args:?}");
            };
            run_git(&["init"]);
            run_git(&["config", "user.email", "pi-wizard-tests@example.invalid"]);
            run_git(&["config", "user.name", "Pi Wizard Tests"]);
            run_git(&["config", "core.autocrlf", "false"]);
            fs::write(self.root.join("seed.txt"), "automation worktree fixture\n")
                .expect("write Git seed file");
            run_git(&["add", "."]);
            run_git(&["commit", "-m", "automation fixture base"]);
            environment
        }

        fn worktree_parent(&self) -> PathBuf {
            let repository_name = self
                .root
                .file_name()
                .expect("fixture repository name")
                .to_string_lossy();
            self.root
                .parent()
                .expect("fixture repository parent")
                .join(format!("{repository_name}-worktrees"))
        }
    }

    impl Drop for AutomationFakePiFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const AUTOMATION_FAKE_PI_JS: &str = r#"
const fs = require("fs");
let buffer = "";
let working = false;
let assistantMessages = 0;
let lastAssistantText = "";
let supervisorTurns = 0;
const sessionId = `automation-${process.pid}`;
const supervisorProcess = process.argv.includes("--no-context-files") && process.argv.includes("--no-extensions");

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

function handle(request) {
  switch (request.type) {
    case "get_state":
      respond(request, {
        model: {
          provider: "fake",
          id: "fake-model",
          name: "Fake Model",
          input: ["text"],
        },
        thinkingLevel: "medium",
        isStreaming: working,
        isCompacting: false,
        steeringMode: "all",
        followUpMode: "one-at-a-time",
        sessionFile: null,
        sessionId,
        sessionName: null,
        autoCompactionEnabled: true,
        messageCount: assistantMessages,
        pendingMessageCount: 0,
      });
      break;
    case "get_available_models":
      respond(request, {
        models: [{provider: "fake", id: "fake-model", name: "Fake Model", input: ["text"]}],
      });
      break;
    case "get_available_thinking_levels":
      respond(request, {levels: ["off", "medium", "high"]});
      break;
    case "get_commands":
      respond(request, {commands: []});
      break;
    case "get_session_stats":
      respond(request, {assistantMessages});
      break;
    case "get_last_assistant_text":
      respond(request, {text: lastAssistantText});
      break;
    case "prompt":
      if (request.message === "reject this step") {
        reject(request, "fixture prompt rejection");
        break;
      }
      respond(request);
      working = true;
      emit({type: "agent_start"});
      if (supervisorProcess) {
        const match = String(request.message).match(/runId=([0-9a-f-]{36}) status=idle/);
        if (supervisorTurns === 0 && match) {
          lastAssistantText = JSON.stringify({directives: [{runId: match[1], action: "send", message: "supervised continuation"}]});
        } else {
          lastAssistantText = JSON.stringify({directives: []});
        }
        supervisorTurns += 1;
      } else {
        fs.appendFileSync("automation-worker-prompts.log", String(request.message) + "\n");
        lastAssistantText = `done: ${request.message}`;
      }
      setTimeout(() => {
        assistantMessages += 1;
        working = false;
        emit({type: "agent_settled"});
      }, String(request.message).startsWith("slow ") ? 250 : 5);
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

    #[test]
    fn git_review_job_registry_supersedes_and_aborts_previous_run_job() {
        tauri::async_runtime::block_on(async {
            let run_id = RunId::new();
            let mut registry = GitReviewJobRegistry::new(2);
            let first_generation = registry.begin(run_id).expect("first review generation");
            let first = tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
            });
            assert!(registry.attach(run_id, first_generation, first.abort_handle()));

            let second_generation = registry.begin(run_id).expect("replacement generation");
            assert!(second_generation > first_generation);
            let first_result = first.await;
            assert!(first_result.is_err_and(|error| error.is_cancelled()));

            let second = tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
            });
            assert!(registry.attach(run_id, second_generation, second.abort_handle()));
            assert!(registry.cancel(run_id));
            assert!(second.await.is_err_and(|error| error.is_cancelled()));
            assert!(!registry.cancel(run_id));
        });
    }

    #[test]
    fn active_job_guard_counts_only_the_lifetime_of_explicit_catalog_work() {
        let counter = AtomicUsize::new(0);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        {
            let _first = ActiveJobGuard::new(&counter);
            assert_eq!(counter.load(Ordering::Acquire), 1);
            {
                let _second = ActiveJobGuard::new(&counter);
                assert_eq!(counter.load(Ordering::Acquire), 2);
            }
            assert_eq!(counter.load(Ordering::Acquire), 1);
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn git_review_registry_reports_only_attached_active_jobs() {
        tauri::async_runtime::block_on(async {
            let run_id = RunId::new();
            let mut registry = GitReviewJobRegistry::new(2);
            let generation = registry.begin(run_id).expect("review generation");
            assert_eq!(registry.active_count(), 0);
            let task = tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
            });
            assert!(registry.attach(run_id, generation, task.abort_handle()));
            assert_eq!(registry.active_count(), 1);
            registry.complete(run_id, generation);
            assert_eq!(registry.active_count(), 0);
            task.abort();
            let _ = task.await;
        });
    }

    #[test]
    fn finite_automation_runs_every_step_and_continues_after_one_fake_pi_rejection() {
        tauri::async_runtime::block_on(async {
            let fixture = AutomationFakePiFixture::new();
            let limits = RuntimeLimits {
                max_live_runs: 2,
                startup_rpc_deadline_ms: 2_000,
                ..RuntimeLimits::default()
            };
            let manager = spawn_runtime_manager(limits).expect("automation runtime manager");
            let coordinator =
                AutomationCoordinator::new(AutomationStore::ephemeral(limits), limits);
            let project =
                ProjectBinding::register(&fixture.root).expect("register fixture project");
            let project_id = project.id();
            let chain = AutomationChain {
                id: AutomationChainId::new(),
                name: "three-step integration".to_owned(),
                prompts: vec![
                    "first step".to_owned(),
                    "reject this step".to_owned(),
                    "third step".to_owned(),
                ],
            };
            let execution_id = AutomationExecutionId::new();
            let snapshot = AutomationExecutionSnapshot::new(
                execution_id,
                &chain,
                project.id(),
                1,
                false,
                false,
                limits,
            );
            let cancel = coordinator
                .insert_execution(snapshot)
                .await
                .expect("insert automation execution");
            let context = AutomationRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
                coordinator: coordinator.clone(),
            };
            let plan = AutomationExecutionPlan {
                execution_id,
                chain: chain.clone(),
                project,
                environment: fixture.environment(),
                base: None,
                request: StartAutomationRequest {
                    chain_id: chain.id,
                    project_id,
                    concurrency: 1,
                    worktrees: false,
                    supervisor: false,
                },
            };

            tokio::time::timeout(
                Duration::from_secs(10),
                run_automation_execution(context, plan, cancel),
            )
            .await
            .expect("finite automation execution deadline");

            let executions = coordinator.execution_snapshot().await;
            assert_eq!(executions.len(), 1);
            let execution = &executions[0];
            assert_eq!(
                execution.status,
                AutomationExecutionStatus::CompletedWithErrors,
                "execution error: {:?}; steps: {:?}",
                execution.error,
                execution.steps
            );
            assert_eq!(execution.steps.len(), 3);
            assert_eq!(
                execution.steps[0].status,
                AutomationStepStatus::Completed,
                "steps: {:?}",
                execution.steps
            );
            assert_eq!(
                execution.steps[1].status,
                AutomationStepStatus::Failed,
                "steps: {:?}",
                execution.steps
            );
            assert_eq!(
                execution.steps[2].status,
                AutomationStepStatus::Completed,
                "steps: {:?}",
                execution.steps
            );
            assert!(
                execution.steps[1]
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("fixture prompt rejection"))
            );

            let run_ids: Vec<_> = execution
                .steps
                .iter()
                .map(|step| step.run_id.expect("every attempted step has a run id"))
                .collect();
            assert_ne!(run_ids[0], run_ids[1]);
            assert_ne!(run_ids[0], run_ids[2]);
            assert_ne!(run_ids[1], run_ids[2]);
            assert_eq!(
                manager
                    .capacity()
                    .await
                    .expect("final capacity")
                    .active_runs,
                0
            );
            assert!(
                manager
                    .hydrate()
                    .await
                    .expect("final automation hydration")
                    .runs
                    .iter()
                    .all(|run| run.run.process_state().is_terminal())
            );
            manager
                .shutdown()
                .await
                .expect("shutdown automation runtime");
        });
    }

    #[test]
    fn parallel_automation_uses_two_live_slots_and_unique_real_git_worktrees() {
        tauri::async_runtime::block_on(async {
            let fixture = AutomationFakePiFixture::new();
            let environment = fixture.initialize_git_repository();
            let limits = RuntimeLimits {
                max_live_runs: 4,
                startup_rpc_deadline_ms: 2_000,
                ..RuntimeLimits::default()
            };
            let manager = spawn_runtime_manager(limits).expect("parallel automation manager");
            let coordinator =
                AutomationCoordinator::new(AutomationStore::ephemeral(limits), limits);
            let worktrees = Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits)));
            let project = ProjectBinding::register(&fixture.root).expect("register Git project");
            let project_id = project.id();
            let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
                .await
                .expect("inspect automation worktree base");
            assert!(!base.dirty);
            let chain = AutomationChain {
                id: AutomationChainId::new(),
                name: "parallel integration".to_owned(),
                prompts: (1..=4)
                    .map(|index| format!("slow worker {index}"))
                    .collect(),
            };
            let execution_id = AutomationExecutionId::new();
            let snapshot = AutomationExecutionSnapshot::new(
                execution_id,
                &chain,
                project_id,
                2,
                true,
                false,
                limits,
            );
            let cancel = coordinator
                .insert_execution(snapshot)
                .await
                .expect("insert parallel automation execution");
            let context = AutomationRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                worktrees: Arc::clone(&worktrees),
                coordinator: coordinator.clone(),
            };
            let plan = AutomationExecutionPlan {
                execution_id,
                chain: chain.clone(),
                project,
                environment,
                base: Some(base),
                request: StartAutomationRequest {
                    chain_id: chain.id,
                    project_id,
                    concurrency: 2,
                    worktrees: true,
                    supervisor: false,
                },
            };
            let mut state_changes = manager.subscribe_state_changes();
            let execution_task = tokio::spawn(run_automation_execution(context, plan, cancel));

            if tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if manager
                        .capacity()
                        .await
                        .expect("parallel capacity")
                        .active_runs
                        >= 2
                    {
                        break;
                    }
                    match state_changes.recv().await {
                        Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => {
                            panic!("runtime state stream closed before two workers were live")
                        }
                    }
                }
            })
            .await
            .is_err()
            {
                let snapshot = coordinator.execution_snapshot().await;
                let capacity = manager.capacity().await.expect("diagnostic capacity");
                let records = worktrees.lock().await.records();
                panic!(
                    "parallel automation did not reach two live workers; task_finished={}; capacity={capacity:?}; executions={snapshot:?}; worktrees={records:?}",
                    execution_task.is_finished()
                );
            }

            tokio::time::timeout(Duration::from_secs(20), execution_task)
                .await
                .expect("parallel automation execution deadline")
                .expect("parallel automation task");

            let executions = coordinator.execution_snapshot().await;
            assert_eq!(executions.len(), 1);
            let execution = &executions[0];
            assert_eq!(execution.status, AutomationExecutionStatus::Completed);
            assert!(
                execution
                    .steps
                    .iter()
                    .all(|step| step.status == AutomationStepStatus::Completed)
            );
            assert_eq!(
                manager
                    .capacity()
                    .await
                    .expect("final capacity")
                    .active_runs,
                0
            );

            let registry = worktrees.lock().await;
            let records = registry.records();
            assert_eq!(records.len(), 4);
            let mut branches: Vec<_> = records.iter().map(|record| record.plan().branch).collect();
            branches.sort();
            branches.dedup();
            assert_eq!(branches.len(), 4);
            let mut roots: Vec<_> = records
                .iter()
                .filter_map(|record| {
                    record
                        .created
                        .as_ref()
                        .map(|created| created.execution_root.clone())
                })
                .collect();
            roots.sort();
            roots.dedup();
            assert_eq!(roots.len(), 4);
            assert!(roots.iter().all(|root| root != &fixture.root));
            drop(registry);

            manager
                .shutdown()
                .await
                .expect("shutdown parallel automation");
            let worktree_parent = fixture.worktree_parent();
            if worktree_parent.exists() {
                fs::remove_dir_all(worktree_parent).expect("remove automation worktree fixtures");
            }
        });
    }

    #[test]
    fn supervised_automation_sends_one_real_directive_back_into_worker_session() {
        tauri::async_runtime::block_on(async {
            let fixture = AutomationFakePiFixture::new();
            let environment = fixture.initialize_git_repository();
            let limits = RuntimeLimits {
                max_live_runs: 3,
                startup_rpc_deadline_ms: 2_000,
                ..RuntimeLimits::default()
            };
            let manager = spawn_runtime_manager(limits).expect("supervised automation manager");
            let coordinator =
                AutomationCoordinator::new(AutomationStore::ephemeral(limits), limits);
            let worktrees = Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits)));
            let project =
                ProjectBinding::register(&fixture.root).expect("register supervised project");
            let project_id = project.id();
            let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
                .await
                .expect("inspect supervised worktree base");
            assert!(!base.dirty);
            let chain = AutomationChain {
                id: AutomationChainId::new(),
                name: "supervisor integration".to_owned(),
                prompts: vec!["worker initial task".to_owned()],
            };
            let execution_id = AutomationExecutionId::new();
            let snapshot = AutomationExecutionSnapshot::new(
                execution_id,
                &chain,
                project_id,
                1,
                true,
                true,
                limits,
            );
            let cancel = coordinator
                .insert_execution(snapshot)
                .await
                .expect("insert supervised execution");
            let context = AutomationRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                worktrees: Arc::clone(&worktrees),
                coordinator: coordinator.clone(),
            };
            let plan = AutomationExecutionPlan {
                execution_id,
                chain: chain.clone(),
                project,
                environment,
                base: Some(base),
                request: StartAutomationRequest {
                    chain_id: chain.id,
                    project_id,
                    concurrency: 1,
                    worktrees: true,
                    supervisor: true,
                },
            };

            tokio::time::timeout(
                Duration::from_secs(20),
                run_automation_execution(context, plan, cancel),
            )
            .await
            .expect("supervised automation execution deadline");

            let executions = coordinator.execution_snapshot().await;
            assert_eq!(executions.len(), 1);
            let execution = &executions[0];
            assert_eq!(
                execution.status,
                AutomationExecutionStatus::Completed,
                "execution error: {:?}; supervisor error: {:?}; steps: {:?}",
                execution.error,
                execution.supervisor_error,
                execution.steps
            );
            assert!(execution.supervisor_error.is_none());
            assert!(execution.supervisor_cycles >= 1);
            assert_eq!(execution.steps.len(), 1);
            assert_eq!(execution.steps[0].status, AutomationStepStatus::Completed);
            assert_eq!(
                manager
                    .capacity()
                    .await
                    .expect("final capacity")
                    .active_runs,
                0
            );

            let records = worktrees.lock().await.records();
            assert_eq!(records.len(), 2);
            let worker = records
                .iter()
                .find(|record| record.branch.ends_with("worker-1"))
                .expect("worker recovery record");
            let supervisor = records
                .iter()
                .find(|record| record.branch.ends_with("supervisor"))
                .expect("supervisor recovery record");
            let worker_root = worker
                .created
                .as_ref()
                .expect("created worker worktree")
                .execution_root
                .clone();
            let supervisor_root = supervisor
                .created
                .as_ref()
                .expect("created supervisor worktree")
                .execution_root
                .clone();
            assert_ne!(worker_root, supervisor_root);
            let worker_prompts =
                fs::read_to_string(worker_root.join("automation-worker-prompts.log"))
                    .expect("read worker prompt audit");
            let prompt_lines: Vec<_> = worker_prompts.lines().collect();
            assert_eq!(
                prompt_lines,
                ["worker initial task", "supervised continuation"],
                "the supervisor directive must reach the existing worker as a second prompt"
            );
            assert!(
                !supervisor_root
                    .join("automation-worker-prompts.log")
                    .exists()
            );

            manager
                .shutdown()
                .await
                .expect("shutdown supervised automation");
            let worktree_parent = fixture.worktree_parent();
            if worktree_parent.exists() {
                fs::remove_dir_all(worktree_parent).expect("remove supervised worktree fixtures");
            }
        });
    }

    #[test]
    fn automation_request_wire_shapes_keep_exact_chain_project_and_policy() {
        let chain_id = AutomationChainId::new();
        let project_id = ProjectId::new();
        let save: SaveAutomationChainRequest = serde_json::from_value(json!({
            "id": chain_id,
            "name": "review loop",
            "prompts": ["inspect", "fix"]
        }))
        .expect("deserialize automation chain save");
        assert_eq!(save.id, Some(chain_id));
        assert_eq!(save.name, "review loop");
        assert_eq!(save.prompts, ["inspect", "fix"]);

        let start: StartAutomationRequest = serde_json::from_value(json!({
            "chainId": chain_id,
            "projectId": project_id,
            "concurrency": 6,
            "worktrees": true,
            "supervisor": true
        }))
        .expect("deserialize automation start");
        assert_eq!(start.chain_id, chain_id);
        assert_eq!(start.project_id, project_id);
        assert_eq!(start.concurrency, 6);
        assert!(start.worktrees);
        assert!(start.supervisor);
    }

    #[test]
    fn automation_change_signal_wire_shape_distinguishes_catalog_from_execution_updates() {
        assert_eq!(
            serde_json::to_value(AutomationChangedSignal::Catalog).expect("catalog signal"),
            json!("catalog")
        );
        assert_eq!(
            serde_json::to_value(AutomationChangedSignal::Executions).expect("execution signal"),
            json!("executions")
        );
    }

    #[test]
    fn automation_coordinator_suppresses_noop_execution_updates_and_cancel_keeps_workers() {
        tauri::async_runtime::block_on(async {
            let limits = RuntimeLimits::default();
            let coordinator =
                AutomationCoordinator::new(AutomationStore::ephemeral(limits), limits);
            let chain = AutomationChain {
                id: AutomationChainId::new(),
                name: "bounded chain".to_owned(),
                prompts: vec!["first".to_owned(), "second".to_owned()],
            };
            let execution_id = AutomationExecutionId::new();
            let snapshot = AutomationExecutionSnapshot::new(
                execution_id,
                &chain,
                ProjectId::new(),
                2,
                true,
                false,
                limits,
            );
            let mut signals = coordinator.subscribe();
            let cancel = coordinator
                .insert_execution(snapshot)
                .await
                .expect("insert execution");
            assert_eq!(
                signals.recv().await.expect("insert signal"),
                AutomationChangedSignal::Executions
            );

            coordinator
                .mutate_execution(execution_id, |snapshot| {
                    snapshot.status = AutomationExecutionStatus::Starting;
                })
                .await
                .expect("noop update");
            assert!(
                tokio::time::timeout(Duration::from_millis(25), signals.recv())
                    .await
                    .is_err(),
                "an identical execution projection must not wake the renderer"
            );

            coordinator
                .mutate_execution(execution_id, |snapshot| {
                    snapshot.status = AutomationExecutionStatus::Running;
                    snapshot.steps[0].status = AutomationStepStatus::Working;
                })
                .await
                .expect("working update");
            assert_eq!(
                signals.recv().await.expect("working signal"),
                AutomationChangedSignal::Executions
            );

            coordinator
                .cancel(execution_id)
                .await
                .expect("cancel execution");
            assert!(*cancel.borrow(), "cancel watch must be raised");
            let current = coordinator.execution_snapshot().await;
            assert_eq!(current.len(), 1);
            assert_eq!(current[0].status, AutomationExecutionStatus::Cancelled);
            assert_eq!(current[0].steps[0].status, AutomationStepStatus::Working);
            assert_eq!(current[0].steps[1].status, AutomationStepStatus::Cancelled);
        });
    }

    #[test]
    fn automation_supervisor_admission_never_consumes_the_only_worker_progress_slot() {
        assert!(!automation_supervisor_can_start(7, 8, 0, true));
        assert!(automation_supervisor_can_start(6, 8, 0, true));
        assert!(automation_supervisor_can_start(7, 8, 1, true));
        assert!(automation_supervisor_can_start(7, 8, 1, false));
        assert!(!automation_supervisor_can_start(8, 8, 1, false));

        assert!(automation_supervisor_should_yield(8, 8, 0, true));
        assert!(!automation_supervisor_should_yield(7, 8, 0, true));
        assert!(!automation_supervisor_should_yield(8, 8, 1, true));
        assert!(!automation_supervisor_should_yield(8, 8, 0, false));
    }

    #[test]
    fn automation_worker_completion_accepts_real_activity_or_a_new_assistant_message() {
        let mut worker = AutomationWorker {
            step_index: 0,
            assistant_messages_at_start: 3,
            turn_activity_observed: false,
        };
        assert!(!automation_worker_turn_complete(&worker, 3));
        assert!(automation_worker_turn_complete(&worker, 4));
        worker.turn_activity_observed = true;
        assert!(automation_worker_turn_complete(&worker, 3));
    }

    #[test]
    fn automation_worktree_plan_is_unique_and_stays_beside_repository() {
        let root = PathBuf::from(r"C:\projects\sample");
        let base = WorktreeBaseSnapshot {
            repository_root: root.clone(),
            project_root: root.clone(),
            project_relative_path: PathBuf::new(),
            source_branch: Some("main".to_owned()),
            base_commit: "0123456789abcdef".to_owned(),
            dirty: false,
        };
        let execution_id = AutomationExecutionId::new();
        let worker =
            automation_worktree_plan(&base, execution_id, "worker-1").expect("worker plan");
        let supervisor =
            automation_worktree_plan(&base, execution_id, "supervisor").expect("supervisor plan");
        let execution_key = automation_execution_key(execution_id);
        assert_ne!(worker.branch, supervisor.branch);
        assert_ne!(worker.worktree_path, supervisor.worktree_path);
        assert!(worker.branch.starts_with("pi-wizard/auto-"));
        assert!(worker.branch.contains(&execution_key));
        assert!(
            worker
                .worktree_path
                .to_string_lossy()
                .contains(&execution_key)
        );
        assert_eq!(
            worker.worktree_path.parent(),
            Some(Path::new(r"C:\projects\sample-worktrees"))
        );
        assert_eq!(worker.base, base);
    }

    #[test]
    fn automation_worktree_plan_uses_full_execution_identity_not_uuidv7_time_prefix() {
        let first: AutomationExecutionId =
            serde_json::from_value(json!("018f1234-0000-7000-8000-000000000001"))
                .expect("first execution id");
        let second: AutomationExecutionId =
            serde_json::from_value(json!("018f1234-0000-7000-8000-000000000002"))
                .expect("second execution id");
        assert_eq!(&first.to_string()[..8], &second.to_string()[..8]);

        let root = PathBuf::from(r"C:\projects\sample");
        let base = WorktreeBaseSnapshot {
            repository_root: root.clone(),
            project_root: root,
            project_relative_path: PathBuf::new(),
            source_branch: Some("main".to_owned()),
            base_commit: "0123456789abcdef".to_owned(),
            dirty: false,
        };
        let first_plan = automation_worktree_plan(&base, first, "worker-1").expect("first plan");
        let second_plan = automation_worktree_plan(&base, second, "worker-1").expect("second plan");
        assert_ne!(first_plan.branch, second_plan.branch);
        assert_ne!(first_plan.worktree_path, second_plan.worktree_path);
        assert!(first_plan.branch.contains(&automation_execution_key(first)));
        assert!(
            second_plan
                .branch
                .contains(&automation_execution_key(second))
        );
    }

    #[test]
    fn supervisor_json_fence_normalization_preserves_strict_directive_shape() {
        let run_id = RunId::new();
        let text = format!(
            "```json\n{{\"directives\":[{{\"runId\":\"{run_id}\",\"action\":\"send\",\"message\":\"continue\"}}]}}\n```"
        );
        let reply: SupervisorReply =
            serde_json::from_str(strip_json_fence(&text)).expect("parse supervisor reply");
        assert_eq!(reply.directives.len(), 1);
        assert_eq!(reply.directives[0].run_id, run_id);
        assert!(matches!(reply.directives[0].action, SupervisorAction::Send));
        assert_eq!(reply.directives[0].message, "continue");
    }

    #[test]
    fn project_resource_probe_wire_shape_keeps_exact_project_path() {
        let request: ProbeProjectResourcesRequest = serde_json::from_value(json!({
            "projectPath": r"C:\projects\pi-wizard"
        }))
        .expect("deserialize project-resource preflight");
        assert_eq!(
            request.project_path,
            PathBuf::from(r"C:\projects\pi-wizard")
        );
    }

    #[test]
    fn git_review_job_registry_is_bounded_and_only_evicts_idle_owners() {
        tauri::async_runtime::block_on(async {
            let first_run = RunId::new();
            let second_run = RunId::new();
            let third_run = RunId::new();
            let mut registry = GitReviewJobRegistry::new(2);

            let first_generation = registry.begin(first_run).expect("first owner");
            let active = tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
            });
            assert!(registry.attach(first_run, first_generation, active.abort_handle()));

            let second_generation = registry.begin(second_run).expect("second owner");
            registry.complete(second_run, second_generation);
            assert!(
                registry.begin(third_run).is_ok(),
                "idle owner can be evicted"
            );
            assert_eq!(registry.by_run.len(), 2);
            assert!(registry.by_run.contains_key(&first_run));
            assert!(registry.by_run.contains_key(&third_run));

            active.abort();
            let _ = active.await;
        });
    }

    #[test]
    fn fresh_desktop_runtime_hydrates_through_runtime_manager() {
        let runtime = DesktopRuntime::new().expect("desktop runtime");
        let snapshot = tauri::async_runtime::block_on(runtime.manager.hydrate()).expect("hydrate");
        assert_eq!(snapshot.schema_version, RUNTIME_HYDRATION_SCHEMA_VERSION);
        assert_eq!(snapshot.runtime_revision, 0);
        assert!(snapshot.runs.is_empty());
        tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("shutdown manager");
    }

    #[test]
    #[ignore = "startup measurement fixture; exercised by full verification"]
    fn cold_and_warm_desktop_state_startup_stay_within_bounded_release_budget() {
        const STARTUP_BUDGET: Duration = Duration::from_secs(5);
        let root = std::env::temp_dir().join(format!("pi-wizard-startup-scale-{}", RunId::new()));
        let project = root.join("project");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("project fixture");

        let cold_started = Instant::now();
        let cold = DesktopRuntime::new_with_state_root(Some(state.clone())).expect("cold runtime");
        let cold_elapsed = cold_started.elapsed();
        tauri::async_runtime::block_on(async {
            cold.project_binding(project.clone())
                .await
                .expect("persist project");
            cold.set_persisted_live_run_limit(3)
                .await
                .expect("persist preference");
            cold.manager.shutdown().await.expect("cold shutdown");
        });

        let warm_started = Instant::now();
        let warm = DesktopRuntime::new_with_state_root(Some(state.clone())).expect("warm runtime");
        let warm_elapsed = warm_started.elapsed();
        let snapshot =
            tauri::async_runtime::block_on(warm.manager.hydrate()).expect("warm hydrate");
        assert!(snapshot.runs.is_empty());
        tauri::async_runtime::block_on(warm.manager.shutdown()).expect("warm shutdown");

        eprintln!(
            "desktop startup measurement: cold={cold_elapsed:?}, warm={warm_elapsed:?}, budget={STARTUP_BUDGET:?}"
        );
        assert!(
            cold_elapsed < STARTUP_BUDGET,
            "cold app-owned state startup exceeded {STARTUP_BUDGET:?}: {cold_elapsed:?}"
        );
        assert!(
            warm_elapsed < STARTUP_BUDGET,
            "warm app-owned state startup exceeded {STARTUP_BUDGET:?}: {warm_elapsed:?}"
        );
        fs::remove_dir_all(root).expect("startup fixture cleanup");
    }

    #[test]
    fn repeated_canonical_project_launches_share_one_in_memory_project_identity() {
        let root = std::env::temp_dir().join(format!("pi-wizard-desktop-project-{}", RunId::new()));
        fs::create_dir_all(&root).expect("create project fixture");
        let runtime = DesktopRuntime::new().expect("desktop runtime");
        let (first, second) = tauri::async_runtime::block_on(async {
            let first = runtime
                .project_binding(root.clone())
                .await
                .expect("first binding");
            let second = runtime
                .project_binding(root.clone())
                .await
                .expect("second binding");
            (first, second)
        });
        assert_eq!(first.id(), second.id());
        assert_eq!(first.canonical_root(), second.canonical_root());
        tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("shutdown manager");
        fs::remove_dir_all(root).expect("remove project fixture");
    }

    #[test]
    fn persistent_desktop_runtime_restores_saved_live_run_admission_limit() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-desktop-preferences-restart-{}",
            RunId::new()
        ));
        let state = root.join("state");
        fs::create_dir_all(&state).expect("create state fixture");

        {
            let runtime =
                DesktopRuntime::new_with_state_root(Some(state.clone())).expect("first runtime");
            let saved = tauri::async_runtime::block_on(runtime.set_persisted_live_run_limit(3))
                .expect("persist live-run limit");
            assert_eq!(saved.live_run_limit, 3);
            tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("first shutdown");
        }

        let reopened =
            DesktopRuntime::new_with_state_root(Some(state)).expect("reopened desktop runtime");
        let capacity =
            tauri::async_runtime::block_on(reopened.manager.capacity()).expect("capacity");
        assert_eq!(capacity.live_run_limit, 3);
        assert_eq!(
            capacity.configured_max_live_runs,
            RuntimeLimits::default().max_live_runs
        );
        tauri::async_runtime::block_on(reopened.manager.shutdown()).expect("second shutdown");
        fs::remove_dir_all(root).expect("remove preferences fixture");
    }

    #[test]
    fn corrupt_preferences_are_quarantined_without_hiding_project_or_worktree_state() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-desktop-preferences-isolation-{}",
            RunId::new()
        ));
        let project = root.join("project");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("create project fixture");

        let (project_id, recovery_id) = {
            let runtime =
                DesktopRuntime::new_with_state_root(Some(state.clone())).expect("first runtime");
            let binding = tauri::async_runtime::block_on(runtime.project_binding(project.clone()))
                .expect("project binding");
            let recovery = tauri::async_runtime::block_on(async {
                let mut worktrees = runtime.worktrees.lock().await;
                worktrees
                    .begin_creation(
                        binding.id(),
                        &WorktreeCreatePlan {
                            base: WorktreeBaseSnapshot {
                                repository_root: root.canonicalize().expect("repository root"),
                                project_root: binding.canonical_root().to_path_buf(),
                                project_relative_path: PathBuf::from("project"),
                                source_branch: Some("main".to_owned()),
                                base_commit: "abc123".to_owned(),
                                dirty: false,
                            },
                            branch: "agent/preferences-isolation".to_owned(),
                            worktree_path: root.join("task-worktree"),
                        },
                    )
                    .expect("persist recovery")
            });
            tauri::async_runtime::block_on(runtime.set_persisted_live_run_limit(3))
                .expect("create preferences file");
            tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("first shutdown");
            (binding.id(), recovery.id)
        };

        fs::write(state.join("preferences.json"), b"{corrupt")
            .expect("corrupt only preferences domain");
        let reopened = DesktopRuntime::new_with_state_root(Some(state.clone()))
            .expect("safe startup with corrupt preferences");
        let (project_retained, recovery_retained, report) = tauri::async_runtime::block_on(async {
            let project_retained = reopened.projects.lock().await.get(project_id).is_some();
            let recovery_retained = reopened.worktrees.lock().await.get(recovery_id).is_some();
            let report = reopened.capacity_report().await.expect("capacity report");
            (project_retained, recovery_retained, report)
        });
        assert!(project_retained);
        assert!(recovery_retained);
        assert_eq!(
            report.live_run_limit,
            RuntimeLimits::default().max_live_runs
        );
        assert!(report.preference_recovery_notice.is_some());
        assert!(!state.join("preferences.json").exists());
        assert!(state.join("preferences-quarantine").is_dir());
        tauri::async_runtime::block_on(reopened.manager.shutdown()).expect("second shutdown");
        fs::remove_dir_all(root).expect("remove isolation fixture");
    }

    #[test]
    fn failed_preference_persistence_does_not_change_manager_admission_limit() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-desktop-preferences-write-failure-{}",
            RunId::new()
        ));
        let state = root.join("state");
        fs::create_dir_all(&state).expect("create state fixture");
        let runtime =
            DesktopRuntime::new_with_state_root(Some(state.clone())).expect("desktop runtime");
        fs::create_dir_all(state.join("preferences.json")).expect("block atomic preference write");

        assert!(tauri::async_runtime::block_on(runtime.set_persisted_live_run_limit(2)).is_err());
        let capacity =
            tauri::async_runtime::block_on(runtime.manager.capacity()).expect("manager capacity");
        assert_eq!(
            capacity.live_run_limit,
            RuntimeLimits::default().max_live_runs
        );
        let stored_limit = tauri::async_runtime::block_on(async {
            runtime.preferences.lock().await.live_run_limit()
        });
        assert_eq!(stored_limit, RuntimeLimits::default().max_live_runs);
        tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("shutdown manager");
        fs::remove_dir_all(root).expect("remove write-failure fixture");
    }

    #[test]
    fn persistent_desktop_runtime_reopens_unfinished_worktree_recovery_intent() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-desktop-worktree-recovery-{}",
            WorktreeId::new()
        ));
        let project = root.join("project");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("create project fixture");

        let recovery_id = {
            let runtime =
                DesktopRuntime::new_with_state_root(Some(state.clone())).expect("first runtime");
            let binding = tauri::async_runtime::block_on(runtime.project_binding(project.clone()))
                .expect("project binding");
            let base = WorktreeBaseSnapshot {
                repository_root: root.canonicalize().expect("repository root"),
                project_root: binding.canonical_root().to_path_buf(),
                project_relative_path: PathBuf::from("project"),
                source_branch: Some("main".to_owned()),
                base_commit: "abc123".to_owned(),
                dirty: false,
            };
            let recovery = tauri::async_runtime::block_on(async {
                let mut registry = runtime.worktrees.lock().await;
                registry
                    .begin_creation(
                        binding.id(),
                        &WorktreeCreatePlan {
                            base,
                            branch: "agent/recover".to_owned(),
                            worktree_path: root.join("task-worktree"),
                        },
                    )
                    .expect("persist recovery intent")
            });
            tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("first shutdown");
            recovery.id
        };

        let reopened =
            DesktopRuntime::new_with_state_root(Some(state)).expect("reopened desktop runtime");
        let retained = tauri::async_runtime::block_on(async {
            reopened.worktrees.lock().await.get(recovery_id).cloned()
        })
        .expect("recovery survives reopen");
        assert_eq!(retained.id, recovery_id);
        assert!(retained.created.is_none());
        assert_eq!(retained.branch, "agent/recover");
        tauri::async_runtime::block_on(reopened.manager.shutdown()).expect("second shutdown");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn persistent_desktop_runtime_reuses_project_identity_after_restart() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-desktop-project-restart-{}",
            RunId::new()
        ));
        let project = root.join("project");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("create project fixture");

        let first_id = {
            let runtime =
                DesktopRuntime::new_with_state_root(Some(state.clone())).expect("first runtime");
            let binding = tauri::async_runtime::block_on(runtime.project_binding(project.clone()))
                .expect("first binding");
            tauri::async_runtime::block_on(runtime.manager.shutdown()).expect("first shutdown");
            binding.id()
        };

        let second =
            DesktopRuntime::new_with_state_root(Some(state)).expect("second desktop runtime");
        let second_binding =
            tauri::async_runtime::block_on(second.project_binding(project.clone()))
                .expect("reopened binding");
        assert_eq!(second_binding.id(), first_id);
        tauri::async_runtime::block_on(second.manager.shutdown()).expect("second shutdown");
        fs::remove_dir_all(root).expect("remove project fixture");
    }

    #[test]
    fn extension_dialog_response_wire_shape_is_camel_case_and_exactly_typed() {
        let confirmation: DesktopExtensionUiResponse = serde_json::from_value(json!({
            "kind": "confirmation",
            "id": "dialog-1",
            "confirmed": true
        }))
        .expect("deserialize confirmation response");
        assert!(matches!(
            ExtensionUiResponse::from(confirmation),
            ExtensionUiResponse::Confirmation { id, confirmed }
                if id == "dialog-1" && confirmed
        ));

        let value: DesktopExtensionUiResponse = serde_json::from_value(json!({
            "kind": "value",
            "id": "dialog-2",
            "value": "selected"
        }))
        .expect("deserialize value response");
        assert!(matches!(
            ExtensionUiResponse::from(value),
            ExtensionUiResponse::Value { id, value }
                if id == "dialog-2" && value == "selected"
        ));
    }

    #[test]
    fn composer_action_wire_shape_uses_current_camel_case_contract() {
        let request: SubmitDraftRequest = serde_json::from_value(json!({
            "runId": RunId::new(),
            "action": "followUp"
        }))
        .expect("deserialize follow-up composer action");
        assert_eq!(request.action, ComposerAction::FollowUp);
    }

    #[test]
    fn automatic_compaction_wire_shape_is_explicit_boolean_state() {
        let run_id = RunId::new();
        let request: SetAutoCompactionRequest = serde_json::from_value(json!({
            "runId": run_id,
            "enabled": false
        }))
        .expect("deserialize automatic compaction request");
        assert_eq!(request.run_id, run_id);
        assert!(!request.enabled);
    }

    #[test]
    fn automatic_retry_wire_shape_is_an_explicit_pi_command_not_cached_state() {
        let run_id = RunId::new();
        let request: SetAutoRetryRequest = serde_json::from_value(json!({
            "runId": run_id,
            "enabled": true
        }))
        .expect("deserialize automatic retry request");
        assert_eq!(request.run_id, run_id);
        assert!(request.enabled);
    }

    #[test]
    fn live_run_limit_wire_shape_is_one_explicit_integer() {
        let request: SetLiveRunLimitRequest = serde_json::from_value(json!({
            "limit": 4
        }))
        .expect("deserialize live run limit request");
        assert_eq!(request.limit, 4);
    }

    #[test]
    fn desktop_capacity_wire_shape_includes_preference_recovery_notice() {
        assert_eq!(
            serde_json::to_value(DesktopRuntimeCapacity {
                active_runs: 2,
                live_run_limit: 3,
                configured_max_live_runs: 8,
                preference_recovery_notice: Some("saved preference was invalid".to_owned()),
            })
            .expect("capacity wire shape"),
            json!({
                "activeRuns": 2,
                "liveRunLimit": 3,
                "configuredMaxLiveRuns": 8,
                "preferenceRecoveryNotice": "saved preference was invalid"
            })
        );
    }

    #[test]
    fn folder_opener_passes_execution_root_as_one_exact_argument() {
        let root = PathBuf::from(r"C:\projects\pi wizard\task");
        let (executable, args) = folder_opener(&root);
        assert_eq!(args, vec![root.as_os_str().to_owned()]);
        #[cfg(windows)]
        assert_eq!(
            executable.file_name().and_then(|name| name.to_str()),
            Some("explorer.exe")
        );
        #[cfg(target_os = "macos")]
        assert_eq!(executable, PathBuf::from("open"));
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(executable, PathBuf::from("xdg-open"));
    }

    #[test]
    fn runtime_close_result_wire_shape_is_explicit() {
        assert_eq!(
            serde_json::to_value(RuntimeCloseResult {
                process_terminated: true,
                quarantined: false,
            })
            .expect("close result wire shape"),
            json!({
                "processTerminated": true,
                "quarantined": false
            })
        );
    }

    #[test]
    fn project_start_wire_shape_preserves_explicit_pi_trust_choice() {
        let request: StartProjectRequest = serde_json::from_value(json!({
            "projectPath": "project-fixture",
            "projectTrust": "ignore",
            "contextFiles": "disabled",
            "extensionDiscovery": "disabled",
            "provider": "openai",
            "model": "gpt-5.6",
            "thinking": "high",
            "initialTask": "fix the thing"
        }))
        .expect("deserialize start project request");
        assert_eq!(request.project_path, PathBuf::from("project-fixture"));
        assert_eq!(request.project_trust, ProjectTrustPolicy::Ignore);
        assert_eq!(request.context_files, ContextFilesPolicy::Disabled);
        assert_eq!(
            request.extension_discovery,
            ExtensionDiscoveryPolicy::Disabled
        );
        assert_eq!(request.provider.as_deref(), Some("openai"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(request.thinking, Some(ThinkingLevel::High));
        assert_eq!(request.initial_task.as_deref(), Some("fix the thing"));
    }

    #[test]
    fn launch_selection_requires_an_exact_provider_model_pair_and_applies_before_spawn() {
        assert!(
            LaunchSelection::validate(
                ContextFilesPolicy::Inherit,
                ExtensionDiscoveryPolicy::Inherit,
                Some("openai".to_owned()),
                None,
                Some(ThinkingLevel::High),
            )
            .is_err()
        );

        let selection = LaunchSelection::validate(
            ContextFilesPolicy::Disabled,
            ExtensionDiscoveryPolicy::Disabled,
            Some(" openai ".to_owned()),
            Some(" gpt-5.6 ".to_owned()),
            Some(ThinkingLevel::Xhigh),
        )
        .expect("valid launch selection");
        let root =
            std::env::temp_dir().join(format!("pi-wizard-launch-selection-{}", RunId::new()));
        fs::create_dir_all(&root).expect("launch root");
        let mut spec = PiLaunchSpec::new("pi", &root, ProjectTrustPolicy::Inherit);
        selection.apply(&mut spec);
        assert_eq!(spec.context_files, ContextFilesPolicy::Disabled);
        assert_eq!(spec.extension_discovery, ExtensionDiscoveryPolicy::Disabled);
        assert_eq!(spec.provider.as_deref(), Some("openai"));
        assert_eq!(spec.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(spec.thinking, Some(ThinkingLevel::Xhigh));
        fs::remove_dir_all(root).expect("remove launch root");
    }

    #[test]
    fn launch_options_probe_wire_shape_keeps_context_and_optional_model_identity() {
        let request: ProbeProjectLaunchOptionsRequest = serde_json::from_value(json!({
            "projectPath": "project-fixture",
            "projectTrust": "approve",
            "contextFiles": "disabled",
            "provider": "opencode-go",
            "model": "gpt-5.6-luna"
        }))
        .expect("deserialize launch options probe");
        assert_eq!(request.project_path, PathBuf::from("project-fixture"));
        assert_eq!(request.project_trust, ProjectTrustPolicy::Approve);
        assert_eq!(request.context_files, ContextFilesPolicy::Disabled);
        assert_eq!(request.provider.as_deref(), Some("opencode-go"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6-luna"));
    }

    #[test]
    fn launch_options_wire_shape_exposes_clear_queue_compatibility_without_version_guessing() {
        assert_eq!(
            serde_json::to_value(ProjectLaunchOptions {
                current_model: None,
                current_thinking_level: ThinkingLevel::Medium,
                models: Vec::new(),
                thinking_levels: vec![ThinkingLevel::Off, ThinkingLevel::Medium],
                clear_queue_supported: false,
            })
            .expect("serialize launch options"),
            json!({
                "currentModel": null,
                "currentThinkingLevel": "medium",
                "models": [],
                "thinkingLevels": ["off", "medium"],
                "clearQueueSupported": false
            })
        );
    }

    #[test]
    fn start_run_result_wire_shape_keeps_run_identity_and_initial_task_failure() {
        let run_id = RunId::new();
        assert_eq!(
            serde_json::to_value(StartRunResult {
                run_id,
                initial_task_submitted: false,
                initial_task_error: Some("Pi did not accept the task".to_owned()),
            })
            .expect("serialize start result"),
            json!({
                "runId": run_id,
                "initialTaskSubmitted": false,
                "initialTaskError": "Pi did not accept the task"
            })
        );
    }

    #[test]
    fn project_record_wire_shape_exposes_detached_state_without_path_guessing() {
        let id = ProjectId::new();
        assert_eq!(
            serde_json::to_value(DesktopProjectRecord {
                id,
                canonical_root: PathBuf::from(r"C:\missing-project"),
                status: "missing",
                detail: Some("registered project folder no longer exists at this path".to_owned()),
            })
            .expect("serialize project record"),
            json!({
                "id": id,
                "canonicalRoot": r"C:\missing-project",
                "status": "missing",
                "detail": "registered project folder no longer exists at this path"
            })
        );
    }

    #[test]
    fn worktree_start_wire_shape_preserves_exact_base_and_explicit_target() {
        let request: StartProjectWorktreeRequest = serde_json::from_value(json!({
            "projectPath": "project-fixture",
            "projectTrust": "approve",
            "contextFiles": "disabled",
            "extensionDiscovery": "disabled",
            "provider": "openai",
            "model": "gpt-5.6",
            "thinking": "medium",
            "base": {
                "repositoryRoot": "repo",
                "projectRoot": "repo/project",
                "projectRelativePath": "project",
                "sourceBranch": "feature/base",
                "baseCommit": "abc123",
                "dirty": true
            },
            "branch": "agent/task",
            "worktreePath": "worktrees/agent-task",
            "initialTask": "implement the task"
        }))
        .expect("deserialize worktree start request");
        assert_eq!(request.project_trust, ProjectTrustPolicy::Approve);
        assert_eq!(request.context_files, ContextFilesPolicy::Disabled);
        assert_eq!(
            request.extension_discovery,
            ExtensionDiscoveryPolicy::Disabled
        );
        assert_eq!(request.provider.as_deref(), Some("openai"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(request.thinking, Some(ThinkingLevel::Medium));
        assert_eq!(request.base.source_branch.as_deref(), Some("feature/base"));
        assert_eq!(request.base.base_commit, "abc123");
        assert!(request.base.dirty);
        assert_eq!(request.branch, "agent/task");
        assert_eq!(request.worktree_path, PathBuf::from("worktrees/agent-task"));
        assert_eq!(request.initial_task.as_deref(), Some("implement the task"));
    }

    #[test]
    fn directory_picker_wire_shape_keeps_optional_default_path() {
        let request: PickDirectoryRequest = serde_json::from_value(json!({
            "defaultPath": r"C:\projects\pi-wizard"
        }))
        .expect("deserialize directory picker request");
        assert_eq!(
            request.default_path,
            Some(PathBuf::from(r"C:\projects\pi-wizard"))
        );
    }

    #[test]
    fn review_file_wire_shape_keeps_run_identity_and_project_relative_path() {
        let run_id = RunId::new();
        let request: ReviewFileRequest = serde_json::from_value(json!({
            "runId": run_id,
            "path": "src/lib.rs"
        }))
        .expect("deserialize review file request");
        assert_eq!(request.run_id, run_id);
        assert_eq!(request.path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn review_file_page_wire_shape_keeps_cursor_hash_and_offset() {
        let run_id = RunId::new();
        let request: ReviewFilePageRequest = serde_json::from_value(json!({
            "runId": run_id,
            "path": "src/lib.rs",
            "cursor": {
                "path": "src/lib.rs",
                "offset": 4096,
                "prefixSha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }
        }))
        .expect("deserialize paged review request");
        assert_eq!(request.run_id, run_id);
        assert_eq!(request.path, PathBuf::from("src/lib.rs"));
        let cursor = request.cursor.expect("cursor");
        assert_eq!(cursor.path, PathBuf::from("src/lib.rs"));
        assert_eq!(cursor.offset, 4096);
    }

    #[test]
    fn recovered_worktree_start_wire_shape_keeps_recovery_id_and_trust_policy() {
        let id = WorktreeId::new();
        let request: StartRecoveredWorktreeRequest = serde_json::from_value(json!({
            "id": id,
            "projectTrust": "inherit",
            "contextFiles": "disabled",
            "extensionDiscovery": "disabled",
            "provider": "opencode-go",
            "model": "gpt-5.6-luna",
            "thinking": "xhigh"
        }))
        .expect("deserialize recovered worktree request");
        assert_eq!(request.id, id);
        assert_eq!(request.project_trust, ProjectTrustPolicy::Inherit);
        assert_eq!(request.context_files, ContextFilesPolicy::Disabled);
        assert_eq!(
            request.extension_discovery,
            ExtensionDiscoveryPolicy::Disabled
        );
        assert_eq!(request.provider.as_deref(), Some("opencode-go"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(request.thinking, Some(ThinkingLevel::Xhigh));
    }

    #[test]
    fn resume_session_wire_shape_preserves_independent_launch_policies() {
        let request: ResumeProjectSessionRequest = serde_json::from_value(json!({
            "projectPath": "project-fixture",
            "projectTrust": "inherit",
            "contextFiles": "disabled",
            "extensionDiscovery": "disabled",
            "sessionPath": "sessions/resume.jsonl"
        }))
        .expect("deserialize resume request");
        assert_eq!(request.project_trust, ProjectTrustPolicy::Inherit);
        assert_eq!(request.context_files, ContextFilesPolicy::Disabled);
        assert_eq!(
            request.extension_discovery,
            ExtensionDiscoveryPolicy::Disabled
        );
        assert_eq!(request.session_path, PathBuf::from("sessions/resume.jsonl"));
    }

    #[test]
    fn session_catalog_cursor_wire_shape_is_explicit_and_opaque_to_tauri() {
        let request: ListProjectSessionsRequest = serde_json::from_value(json!({
            "projectPath": "project-fixture",
            "query": "older task",
            "cursor": {
                "modifiedUnixMs": 1234,
                "path": "sessions/older.jsonl",
                "scopeSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "snapshotSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }
        }))
        .expect("deserialize session catalog cursor");
        let cursor = request.cursor.expect("cursor");
        assert_eq!(cursor.modified_unix_ms, 1234);
        assert_eq!(cursor.path, PathBuf::from("sessions/older.jsonl"));
        assert_eq!(cursor.scope_sha256.len(), 64);
        assert_eq!(cursor.snapshot_sha256.len(), 64);
    }

    #[test]
    fn worktree_cleanup_wire_shape_keeps_exact_recovery_identity() {
        let id = WorktreeId::new();
        let request: WorktreeRecoveryRequest = serde_json::from_value(json!({ "id": id }))
            .expect("deserialize cleanup recovery request");
        assert_eq!(request.id, id);
    }
}
