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
use pi_wizard_core::rpc::{AssistantStopReason, RpcCommand, RpcRequest};
use pi_wizard_core::runtime::{
    ExecutionIsolation, ProcessState, RunRecord, RunStartSpec, RuntimeManagerHandle,
};
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
    pub(crate) coordinator: AutomationCoordinator,
}

pub(crate) struct AutomationExecutionPlan {
    pub(crate) execution_id: AutomationExecutionId,
    pub(crate) chain: AutomationChain,
    pub(crate) project: ProjectBinding,
    pub(crate) environment: ResolvedLaunchEnvironment,
    pub(crate) selection: LaunchSelection,
}

struct AutomationWorker {
    step_index: usize,
    assistant_generation_at_start: u64,
    settled_generation_at_start: u64,
}

enum AutomationLaunchAttempt {
    Deferred,
    CancelledBeforeStart,
    Started {
        run_id: RunId,
        assistant_generation_at_start: u64,
        settled_generation_at_start: u64,
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
        let mut settled_workers = Vec::new();
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
            if run.run.agent_settled_generation() > worker.settled_generation_at_start {
                settled_workers
                    .push((run_id, automation_worker_completion_error(worker, &run.run)));
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

        for (run_id, completion_error) in settled_workers {
            let Some(worker) = workers.remove(&run_id) else {
                continue;
            };
            match context.manager.close_run(run_id).await {
                Ok(result) if !result.quarantined => {
                    set_automation_step_status(
                        context,
                        execution_id,
                        worker.step_index,
                        if completion_error.is_some() {
                            AutomationStepStatus::Failed
                        } else {
                            AutomationStepStatus::Completed
                        },
                        completion_error,
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

        if next_step < plan.chain.prompts.len() && workers.is_empty() {
            let step_index = next_step;
            set_automation_step_status(
                context,
                execution_id,
                step_index,
                AutomationStepStatus::Starting,
                None,
            )
            .await?;
            match launch_automation_worker(
                context,
                &plan.project,
                &plan.environment,
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
                }
                Ok(AutomationLaunchAttempt::CancelledBeforeStart) => {
                    finish_automation_cancellation(context, execution_id).await?;
                    return Ok(());
                }
                Ok(AutomationLaunchAttempt::Started {
                    run_id,
                    assistant_generation_at_start,
                    settled_generation_at_start,
                }) => {
                    next_step += 1;
                    workers.insert(
                        run_id,
                        AutomationWorker {
                            step_index,
                            assistant_generation_at_start,
                            settled_generation_at_start,
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

async fn launch_automation_worker(
    context: &AutomationRuntimeContext,
    project: &ProjectBinding,
    environment: &ResolvedLaunchEnvironment,
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

    let mut launch_spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        project.canonical_root(),
        ProjectTrustPolicy::Inherit,
    );
    selection.apply(&mut launch_spec);
    launch_spec.session = SessionLaunch::NewWithId(PiSessionId::new());
    let launch = launch_spec.resolve().map_err(|error| error.to_string())?;
    let run_id = context
        .manager
        .start_run(RunStartSpec {
            project_id: project.id(),
            execution_isolation: ExecutionIsolation::LocalCheckout,
            worktree: None,
            launch,
            environment: environment.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    drop(_gate);
    let (assistant_generation_at_start, settled_generation_at_start) =
        match wait_automation_run_ready(&context.manager, context.limits, run_id).await {
            Ok(generations) => generations,
            Err(error) => {
                return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
            }
        };
    let extension_command = match automation_prompt_is_extension_command(
        &context.manager,
        context.limits,
        run_id,
        initial_task,
    )
    .await
    {
        Ok(extension_command) => extension_command,
        Err(error) => {
            return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
        }
    };
    if extension_command {
        let command = slash_command_name(initial_task).unwrap_or("<unknown>");
        return Ok(AutomationLaunchAttempt::FailedAfterStart {
            run_id,
            error: format!(
                "Pi extension command /{command} executes outside a normal model-turn settlement boundary and cannot be used as a Prompt chain step"
            ),
        });
    }
    if let Err(error) = submit_text_prompt(&context.manager, run_id, initial_task).await {
        return Ok(AutomationLaunchAttempt::FailedAfterStart { run_id, error });
    }
    Ok(AutomationLaunchAttempt::Started {
        run_id,
        assistant_generation_at_start,
        settled_generation_at_start,
    })
}

fn slash_command_name(text: &str) -> Option<&str> {
    let command = text.trim_start().strip_prefix('/')?;
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    (end > 0).then_some(&command[..end])
}

async fn automation_prompt_is_extension_command(
    manager: &RuntimeManagerHandle,
    limits: RuntimeLimits,
    run_id: RunId,
    prompt: &str,
) -> Result<bool, String> {
    let Some(name) = slash_command_name(prompt) else {
        return Ok(false);
    };
    let completion = manager
        .request(run_id, RpcRequest::new(RpcCommand::GetCommands))
        .await
        .map_err(|error| error.to_string())?;
    if !completion.response.success {
        return Err(completion.response.error.unwrap_or_else(|| {
            "Pi rejected command discovery before Prompt chain submission".to_owned()
        }));
    }
    let commands = completion
        .response
        .available_commands(limits)
        .map_err(|error| error.to_string())?;
    Ok(commands
        .iter()
        .any(|command| command.source == "extension" && command.name == name))
}

fn automation_worker_completion_error(
    worker: &AutomationWorker,
    run: &RunRecord,
) -> Option<String> {
    if run.assistant_message_generation() <= worker.assistant_generation_at_start {
        return Some("Pi settled the prompt without producing an assistant result".to_owned());
    }
    match run.last_assistant_stop_reason() {
        Some(AssistantStopReason::Stop) => None,
        Some(AssistantStopReason::ToolUse) => Some(
            "Pi settled after a tool-use assistant message without a final assistant result"
                .to_owned(),
        ),
        Some(AssistantStopReason::Length) => {
            Some("Pi stopped because the assistant output length limit was reached".to_owned())
        }
        Some(AssistantStopReason::Error) => Some(
            run.last_assistant_error()
                .unwrap_or("Pi assistant turn ended with an error")
                .to_owned(),
        ),
        Some(AssistantStopReason::Aborted) => Some("Pi assistant turn was aborted".to_owned()),
        Some(AssistantStopReason::Unknown(reason)) => Some(format!(
            "Pi assistant turn ended with unknown stop reason {reason:?}"
        )),
        None => Some("Pi settled the prompt without a recorded assistant outcome".to_owned()),
    }
}

async fn wait_automation_run_ready(
    manager: &RuntimeManagerHandle,
    limits: RuntimeLimits,
    run_id: RunId,
) -> Result<(u64, u64), String> {
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
                return Ok((
                    run.run.assistant_message_generation(),
                    run.run.agent_settled_generation(),
                ));
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
    use std::collections::HashSet;
    use std::fs;

    #[test]
    fn worker_completion_uses_final_assistant_outcome_after_settlement() {
        let worker = AutomationWorker {
            step_index: 0,
            assistant_generation_at_start: 3,
            settled_generation_at_start: 7,
        };
        let mut store = pi_wizard_core::runtime::RuntimeStore::new(RuntimeLimits::default());
        let run_id = RunId::new();
        store
            .register(
                RunRecord::starting(
                    run_id,
                    pi_wizard_core::ProjectId::new(),
                    std::path::PathBuf::from("project"),
                    ExecutionIsolation::LocalCheckout,
                    ProjectTrustPolicy::Inherit,
                )
                .expect("run"),
            )
            .expect("register");
        store
            .apply(run_id, pi_wizard_core::runtime::RunMutation::ProcessReady)
            .expect("ready");
        for _ in 0..4 {
            store
                .apply(
                    run_id,
                    pi_wizard_core::runtime::RunMutation::AssistantMessageCompleted {
                        stop_reason: AssistantStopReason::ToolUse,
                        error_message: None,
                    },
                )
                .expect("assistant message");
        }
        assert_eq!(
            automation_worker_completion_error(&worker, store.get(run_id).expect("run")),
            Some(
                "Pi settled after a tool-use assistant message without a final assistant result"
                    .to_owned()
            )
        );
        store
            .apply(
                run_id,
                pi_wizard_core::runtime::RunMutation::AssistantMessageCompleted {
                    stop_reason: AssistantStopReason::Error,
                    error_message: Some("rate limited".to_owned()),
                },
            )
            .expect("error result");
        assert_eq!(
            automation_worker_completion_error(&worker, store.get(run_id).expect("run")),
            Some("rate limited".to_owned())
        );
    }

    #[tokio::test]
    async fn queued_chain_waits_for_local_execution_root_release() {
        let fixture = WorkflowFakePiFixture::new("automation-root-deferral");
        let limits = RuntimeLimits {
            max_live_runs: 2,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("automation runtime manager");
        let coordinator = AutomationCoordinator::new(AutomationStore::ephemeral(limits), limits);
        let project = ProjectBinding::register(&fixture.root).expect("register fixture project");

        let mut blocker_launch = PiLaunchSpec::new(
            fixture.environment().pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        blocker_launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let blocker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: blocker_launch.resolve().expect("resolve blocker launch"),
                environment: fixture.environment(),
            })
            .await
            .expect("start blocker");
        wait_automation_run_ready(&manager, limits, blocker)
            .await
            .expect("blocker ready");

        let chain = AutomationChain {
            id: pi_wizard_core::AutomationChainId::new(),
            name: "queued local chain".to_owned(),
            prompts: vec!["after blocker".to_owned()],
        };
        let execution_id = AutomationExecutionId::new();
        let snapshot = AutomationExecutionSnapshot::new(execution_id, &chain, project.id(), limits);
        let cancel = coordinator
            .insert_execution(snapshot)
            .await
            .expect("insert queued execution");
        let mut changed = coordinator.subscribe();
        let task = tokio::spawn(run_automation_execution(
            AutomationRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                coordinator: coordinator.clone(),
            },
            AutomationExecutionPlan {
                execution_id,
                chain,
                project,
                environment: fixture.environment(),
                selection: LaunchSelection {
                    context_files: pi_wizard_core::launch::ContextFilesPolicy::Inherit,
                    extension_discovery: pi_wizard_core::launch::ExtensionDiscoveryPolicy::Inherit,
                    provider: None,
                    model: None,
                    thinking: None,
                },
            },
            cancel,
        ));

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let executions = coordinator.execution_snapshot().await;
                let execution = executions
                    .iter()
                    .find(|execution| execution.id == execution_id)
                    .expect("queued execution snapshot");
                if execution.status == AutomationExecutionStatus::Running
                    && execution.steps[0].status == AutomationStepStatus::Queued
                    && execution.steps[0].run_id.is_none()
                {
                    break;
                }
                match changed.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("automation change stream closed while waiting for deferral")
                    }
                }
            }
        })
        .await
        .expect("queued execution observation deadline");

        terminate_internal_run(&manager, blocker)
            .await
            .expect("release blocker execution root");
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("queued chain completion deadline")
            .expect("queued chain task join");

        let execution = coordinator
            .execution_snapshot()
            .await
            .into_iter()
            .find(|execution| execution.id == execution_id)
            .expect("completed queued execution");
        assert_eq!(execution.status, AutomationExecutionStatus::Completed);
        assert_eq!(execution.steps[0].status, AutomationStepStatus::Completed);
        assert!(execution.steps[0].run_id.is_some());
        manager
            .shutdown()
            .await
            .expect("shutdown queued automation manager");
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
            name: "six-step integration".to_owned(),
            prompts: vec![
                "tool loop step".to_owned(),
                "tool-only settle step".to_owned(),
                "provider error step".to_owned(),
                "/fixture-extension inspect".to_owned(),
                "reject this step".to_owned(),
                "third step".to_owned(),
            ],
        };
        let execution_id = AutomationExecutionId::new();
        let snapshot = AutomationExecutionSnapshot::new(execution_id, &chain, project.id(), limits);
        let cancel = coordinator
            .insert_execution(snapshot)
            .await
            .expect("insert automation execution");
        let context = AutomationRuntimeContext {
            manager: manager.clone(),
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
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
            Duration::from_secs(20),
            run_automation_execution(
                context,
                AutomationExecutionPlan {
                    execution_id,
                    chain,
                    project,
                    environment: fixture.environment(),
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
        assert_eq!(execution.steps.len(), 6);
        assert_eq!(execution.steps[0].status, AutomationStepStatus::Completed);
        assert_eq!(execution.steps[1].status, AutomationStepStatus::Failed);
        assert_eq!(execution.steps[2].status, AutomationStepStatus::Failed);
        assert_eq!(execution.steps[3].status, AutomationStepStatus::Failed);
        assert_eq!(execution.steps[4].status, AutomationStepStatus::Failed);
        assert_eq!(execution.steps[5].status, AutomationStepStatus::Completed);
        assert!(
            execution.steps[1]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("without a final assistant result"))
        );
        assert!(
            execution.steps[2]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("fixture provider rate limit"))
        );
        assert!(
            execution.steps[3]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("extension command /fixture-extension"))
        );
        assert!(
            execution.steps[4]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("fixture prompt rejection"))
        );
        let run_ids: Vec<_> = execution
            .steps
            .iter()
            .map(|step| step.run_id.expect("attempted step run id"))
            .collect();
        assert_eq!(run_ids.iter().copied().collect::<HashSet<_>>().len(), 6);
        let session_ids = fs::read_to_string(fixture.root.join("workflow-worker-sessions.log"))
            .expect("read worker session identities");
        let session_ids: Vec<_> = session_ids.lines().collect();
        assert_eq!(session_ids.len(), 6);
        assert_eq!(session_ids.iter().copied().collect::<HashSet<_>>().len(), 6);
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
