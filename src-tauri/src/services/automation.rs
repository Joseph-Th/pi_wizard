use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pi_wizard_core::automation::{
    AutomationCatalogSnapshot, AutomationChain, AutomationExecutionSnapshot,
    AutomationExecutionStatus, AutomationStepStatus, AutomationStore,
};
use pi_wizard_core::environment::ResolvedLaunchEnvironment;
use pi_wizard_core::launch::{PiLaunchSpec, ProjectTrustPolicy, SessionLaunch};
use pi_wizard_core::project::ProjectBinding;
use pi_wizard_core::runtime::{
    ExecutionIsolation, ProcessState, RunStartSpec, RuntimeManagerHandle,
};
use pi_wizard_core::worktree::{WorktreeBaseSnapshot, WorktreeCreatePlan, create_worktree};
use pi_wizard_core::worktree_registry::WorktreeRegistry;
use pi_wizard_core::{AutomationExecutionId, PiSessionId, RunId, RuntimeLimits};
use serde::Serialize;
use tokio::sync::{Mutex, broadcast, watch};

use crate::LaunchSelection;
use crate::services::internal_run::terminate_internal_run;
use crate::services::pi_session::submit_text_prompt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomationChangedSignal {
    Catalog,
    Executions,
}

#[derive(Clone)]
pub(crate) struct AutomationCoordinator {
    pub(crate) store: Arc<Mutex<AutomationStore>>,
    executions: Arc<Mutex<HashMap<AutomationExecutionId, ActiveAutomationExecution>>>,
    changed: broadcast::Sender<AutomationChangedSignal>,
    limits: RuntimeLimits,
}

struct ActiveAutomationExecution {
    snapshot: AutomationExecutionSnapshot,
    cancel: watch::Sender<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopAutomationSnapshot {
    pub(crate) catalog: AutomationCatalogSnapshot,
    pub(crate) executions: Vec<AutomationExecutionSnapshot>,
}

impl AutomationCoordinator {
    pub(crate) fn new(store: AutomationStore, limits: RuntimeLimits) -> Self {
        let (changed, _) = broadcast::channel(limits.max_runtime_command_queue);
        Self {
            store: Arc::new(Mutex::new(store)),
            executions: Arc::new(Mutex::new(HashMap::new())),
            changed,
            limits,
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AutomationChangedSignal> {
        self.changed.subscribe()
    }

    pub(crate) fn signal_catalog_changed(&self) {
        let _ = self.changed.send(AutomationChangedSignal::Catalog);
    }

    fn signal_executions_changed(&self) {
        let _ = self.changed.send(AutomationChangedSignal::Executions);
    }

    pub(crate) async fn snapshot(&self) -> DesktopAutomationSnapshot {
        let catalog = self.store.lock().await.snapshot();
        DesktopAutomationSnapshot {
            catalog,
            executions: self.execution_snapshot().await,
        }
    }

    pub(crate) async fn execution_snapshot(&self) -> Vec<AutomationExecutionSnapshot> {
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

    pub(crate) async fn insert_execution(
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

    pub(crate) async fn cancel(&self, id: AutomationExecutionId) -> Result<(), String> {
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

#[derive(Clone)]
pub(crate) struct AutomationRuntimeContext {
    pub(crate) manager: RuntimeManagerHandle,
    pub(crate) limits: RuntimeLimits,
    pub(crate) launch_cleanup_gate: Arc<Mutex<()>>,
    pub(crate) worktrees: Arc<Mutex<WorktreeRegistry>>,
    pub(crate) coordinator: AutomationCoordinator,
}

pub(crate) struct AutomationExecutionPlan {
    pub(crate) execution_id: AutomationExecutionId,
    pub(crate) chain: AutomationChain,
    pub(crate) project: ProjectBinding,
    pub(crate) environment: ResolvedLaunchEnvironment,
    pub(crate) base: Option<WorktreeBaseSnapshot>,
    pub(crate) concurrency: usize,
    pub(crate) worktrees: bool,
    pub(crate) selection: LaunchSelection,
}

struct AutomationWorker {
    step_index: usize,
    assistant_generation_at_start: u64,
    turn_activity_observed: bool,
}

enum AutomationLaunchAttempt {
    Deferred,
    CancelledBeforeStart,
    Started {
        run_id: RunId,
        assistant_generation_at_start: u64,
    },
    FailedAfterStart {
        run_id: RunId,
        error: String,
    },
}

pub(crate) async fn run_automation_execution(
    context: AutomationRuntimeContext,
    plan: AutomationExecutionPlan,
    mut cancel: watch::Receiver<bool>,
) {
    let execution_id = plan.execution_id;
    if let Err(error) = run_automation_execution_inner(&context, &plan, &mut cancel).await {
        let _ = context
            .coordinator
            .mutate_execution(execution_id, |snapshot| {
                if !snapshot.status.is_terminal() {
                    snapshot.status = AutomationExecutionStatus::Failed;
                    snapshot.error = Some(error.clone());
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
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| {
            snapshot.status = AutomationExecutionStatus::Running;
        })
        .await?;
    let mut state_changes = context.manager.subscribe_state_changes();
    let mut workers: HashMap<RunId, AutomationWorker> = HashMap::new();
    let mut next_step = 0usize;

    while next_step < plan.chain.prompts.len() || !workers.is_empty() {
        if *cancel.borrow() {
            finish_automation_cancellation(context, execution_id).await?;
            return Ok(());
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
                set_automation_step_status(
                    context,
                    execution_id,
                    worker.step_index,
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
                set_automation_step_status(
                    context,
                    execution_id,
                    worker.step_index,
                    AutomationStepStatus::Working,
                    None,
                )
                .await?;
                continue;
            }
            if automation_worker_turn_complete(worker, run.run.assistant_message_generation()) {
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

        for run_id in idle_workers {
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

        while next_step < plan.chain.prompts.len() && workers.len() < plan.concurrency {
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
            let worktree_base = if plan.worktrees {
                Some(
                    plan.base
                        .as_ref()
                        .ok_or_else(|| "parallel worker is missing Git worktree base".to_owned())?,
                )
            } else {
                None
            };
            let worker_label = format!("worker-{}", step_index + 1);
            match launch_automation_worker(
                context,
                &plan.project,
                &plan.environment,
                worktree_base,
                execution_id,
                &worker_label,
                &plan.selection,
                plan.chain.prompts[step_index].as_str(),
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
                    finish_automation_cancellation(context, execution_id).await?;
                    return Ok(());
                }
                Ok(AutomationLaunchAttempt::Started {
                    run_id,
                    assistant_generation_at_start,
                }) => {
                    next_step += 1;
                    workers.insert(
                        run_id,
                        AutomationWorker {
                            step_index,
                            assistant_generation_at_start,
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
                    let cleanup = terminate_internal_run(&context.manager, run_id).await;
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

        if next_step >= plan.chain.prompts.len() && workers.is_empty() {
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

    let status = {
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
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| snapshot.status = status)
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
) -> Result<(), String> {
    context
        .coordinator
        .mutate_execution(execution_id, |snapshot| {
            snapshot.status = AutomationExecutionStatus::Cancelled;
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

#[allow(clippy::too_many_arguments)]
async fn launch_automation_worker(
    context: &AutomationRuntimeContext,
    project: &ProjectBinding,
    environment: &ResolvedLaunchEnvironment,
    base: Option<&WorktreeBaseSnapshot>,
    execution_id: AutomationExecutionId,
    label: &str,
    selection: &LaunchSelection,
    initial_task: &str,
    cancel: &watch::Receiver<bool>,
) -> Result<AutomationLaunchAttempt, String> {
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
        (project.canonical_root().to_path_buf(), None)
    };

    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        &execution_root,
        ProjectTrustPolicy::Inherit,
    );
    selection.apply(&mut launch_spec);
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
    let assistant_generation_at_start =
        match wait_automation_run_ready(&context.manager, context.limits, run_id).await {
            Ok(generation) => generation,
            Err(error) => {
                return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
            }
        };
    if let Err(error) = submit_text_prompt(&context.manager, run_id, initial_task).await {
        return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
    }
    Ok(AutomationLaunchAttempt::Started {
        run_id,
        assistant_generation_at_start,
    })
}

fn automation_worker_turn_complete(worker: &AutomationWorker, assistant_generation: u64) -> bool {
    worker.turn_activity_observed || assistant_generation > worker.assistant_generation_at_start
}

fn automation_execution_key(execution_id: AutomationExecutionId) -> String {
    execution_id
        .to_string()
        .chars()
        .filter(|character| *character != '-')
        .collect()
}

pub(crate) fn automation_worktree_plan(
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
) -> Result<u64, String> {
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
                return Ok(run.run.assistant_message_generation());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LaunchSelection;
    use crate::services::test_support::WorkflowFakePiFixture;
    use pi_wizard_core::runtime::spawn_runtime_manager;

    #[test]
    fn worker_completion_accepts_real_activity_or_a_new_assistant_message() {
        let mut worker = AutomationWorker {
            step_index: 0,
            assistant_generation_at_start: 3,
            turn_activity_observed: false,
        };
        assert!(!automation_worker_turn_complete(&worker, 3));
        assert!(automation_worker_turn_complete(&worker, 4));
        worker.turn_activity_observed = true;
        assert!(automation_worker_turn_complete(&worker, 3));
    }

    #[tokio::test]
    async fn finite_automation_runs_each_prompt_and_continues_after_one_rejection() {
        let fixture = WorkflowFakePiFixture::new("automation-integration");
        let limits = RuntimeLimits {
            max_live_runs: 2,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("automation runtime manager");
        let coordinator = AutomationCoordinator::new(AutomationStore::ephemeral(limits), limits);
        let project = ProjectBinding::register(&fixture.root).expect("register fixture project");
        let chain = AutomationChain {
            id: pi_wizard_core::AutomationChainId::new(),
            name: "three-step integration".to_owned(),
            prompts: vec![
                "first step".to_owned(),
                "reject this step".to_owned(),
                "third step".to_owned(),
            ],
        };
        let execution_id = AutomationExecutionId::new();
        let snapshot =
            AutomationExecutionSnapshot::new(execution_id, &chain, project.id(), 1, false, limits);
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
        let selection = LaunchSelection {
            context_files: pi_wizard_core::launch::ContextFilesPolicy::Inherit,
            extension_discovery: pi_wizard_core::launch::ExtensionDiscoveryPolicy::Inherit,
            provider: None,
            model: None,
            thinking: None,
        };

        tokio::time::timeout(
            Duration::from_secs(12),
            run_automation_execution(
                context,
                AutomationExecutionPlan {
                    execution_id,
                    chain,
                    project,
                    environment: fixture.environment(),
                    base: None,
                    concurrency: 1,
                    worktrees: false,
                    selection,
                },
                cancel,
            ),
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
        assert_eq!(execution.steps[0].status, AutomationStepStatus::Completed);
        assert_eq!(execution.steps[1].status, AutomationStepStatus::Failed);
        assert_eq!(execution.steps[2].status, AutomationStepStatus::Completed);
        assert!(
            execution.steps[1]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("fixture prompt rejection"))
        );
        let run_ids: Vec<_> = execution
            .steps
            .iter()
            .map(|step| step.run_id.expect("attempted step run id"))
            .collect();
        assert_ne!(run_ids[0], run_ids[1]);
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
            !fixture.root.join("workflow-session-stats.log").exists(),
            "automation completion must not poll Pi get_session_stats"
        );
        manager
            .shutdown()
            .await
            .expect("shutdown automation manager");
    }
}
