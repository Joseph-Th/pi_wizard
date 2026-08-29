use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use pi_wizard_core::environment::ResolvedLaunchEnvironment;
use pi_wizard_core::launch::{
    ContextFilesPolicy, ExtensionDiscoveryPolicy, PiLaunchSpec, ProjectTrustPolicy, SessionLaunch,
};
use pi_wizard_core::project::ProjectBinding;
use pi_wizard_core::rpc::{RpcCommand, RpcRequest};
use pi_wizard_core::runtime::{
    ActivityState, ExecutionIsolation, ProcessState, RunHydrationSnapshot, RunStartSpec,
    RuntimeHydrationSnapshot, RuntimeManagerHandle,
};
use pi_wizard_core::supervision::{SupervisionSnapshot, SupervisionStatus};
use pi_wizard_core::worktree::{WorktreeBaseSnapshot, WorktreeCreatePlan, create_worktree};
use pi_wizard_core::worktree_registry::WorktreeRegistry;
use pi_wizard_core::{PiSessionId, ProjectId, RunId, RuntimeLimits, SupervisionId};
use serde::Deserialize;
use tokio::sync::{Mutex, broadcast, watch};

use crate::LaunchSelection;
use crate::services::internal_run::terminate_internal_run;
use crate::services::pi_session::{
    last_assistant_text, session_assistant_messages, submit_text_prompt,
};

#[derive(Clone)]
pub(crate) struct SupervisionCoordinator {
    sessions: Arc<Mutex<HashMap<SupervisionId, ActiveSupervision>>>,
    changed: broadcast::Sender<()>,
    limits: RuntimeLimits,
}

struct ActiveSupervision {
    snapshot: SupervisionSnapshot,
    stop: watch::Sender<bool>,
}

impl SupervisionCoordinator {
    pub(crate) fn new(limits: RuntimeLimits) -> Self {
        let (changed, _) = broadcast::channel(limits.max_runtime_command_queue);
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            changed,
            limits,
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changed.subscribe()
    }

    fn signal_changed(&self) {
        let _ = self.changed.send(());
    }

    pub(crate) async fn snapshots(&self) -> Vec<SupervisionSnapshot> {
        let mut snapshots: Vec<_> = self
            .sessions
            .lock()
            .await
            .values()
            .map(|session| session.snapshot.clone())
            .collect();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.id.to_string()));
        snapshots
    }

    pub(crate) async fn insert(
        &self,
        snapshot: SupervisionSnapshot,
    ) -> Result<watch::Receiver<bool>, String> {
        let mut sessions = self.sessions.lock().await;
        if sessions.values().any(|session| {
            session.snapshot.project_id == snapshot.project_id
                && !session.snapshot.status.is_terminal()
        }) {
            return Err(
                "this project already has an active supervision session; stop it before starting another"
                    .to_owned(),
            );
        }
        let capacity = self
            .limits
            .max_live_runs
            .saturating_add(self.limits.max_retained_terminal_runs);
        if sessions.len() >= capacity {
            let evict = sessions
                .iter()
                .filter(|(_, session)| session.snapshot.status.is_terminal())
                .min_by_key(|(id, _)| id.to_string())
                .map(|(id, _)| *id)
                .ok_or_else(|| {
                    format!(
                        "supervision history capacity {capacity} is occupied by active sessions"
                    )
                })?;
            sessions.remove(&evict);
        }
        let (stop, receiver) = watch::channel(false);
        sessions.insert(snapshot.id, ActiveSupervision { snapshot, stop });
        drop(sessions);
        self.signal_changed();
        Ok(receiver)
    }

    async fn mutate(
        &self,
        id: SupervisionId,
        mutate: impl FnOnce(&mut SupervisionSnapshot),
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(&id)
            .ok_or_else(|| format!("unknown supervision session {id}"))?;
        let before = session.snapshot.clone();
        mutate(&mut session.snapshot);
        let changed = session.snapshot != before;
        drop(sessions);
        if changed {
            self.signal_changed();
        }
        Ok(())
    }

    pub(crate) async fn request_stop(&self, id: SupervisionId) -> Result<(), String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(&id)
            .ok_or_else(|| format!("unknown supervision session {id}"))?;
        if session.snapshot.status.is_terminal() {
            return Ok(());
        }
        session
            .stop
            .send(true)
            .map_err(|_| format!("supervision session {id} is no longer running"))
    }
}

#[derive(Clone)]
pub(crate) struct SupervisionRuntimeContext {
    pub(crate) manager: RuntimeManagerHandle,
    pub(crate) limits: RuntimeLimits,
    pub(crate) launch_cleanup_gate: Arc<Mutex<()>>,
    pub(crate) worktrees: Arc<Mutex<WorktreeRegistry>>,
    pub(crate) coordinator: SupervisionCoordinator,
}

pub(crate) struct SupervisionPlan {
    pub(crate) id: SupervisionId,
    pub(crate) project: ProjectBinding,
    pub(crate) environment: ResolvedLaunchEnvironment,
    pub(crate) base: WorktreeBaseSnapshot,
    pub(crate) selection: LaunchSelection,
    pub(crate) max_cycles: usize,
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

pub(crate) async fn run_supervision(
    context: SupervisionRuntimeContext,
    plan: SupervisionPlan,
    mut stop: watch::Receiver<bool>,
) {
    let id = plan.id;
    let result = run_supervision_inner(&context, &plan, &mut stop).await;
    if let Err(error) = result {
        let supervisor_run = {
            let sessions = context.coordinator.sessions.lock().await;
            sessions
                .get(&id)
                .and_then(|session| session.snapshot.supervisor_run_id)
        };
        let cleanup_error = if let Some(run_id) = supervisor_run {
            terminate_internal_run(&context.manager, run_id).await.err()
        } else {
            None
        };
        let _ = context
            .coordinator
            .mutate(id, |snapshot| {
                snapshot.status = SupervisionStatus::Failed;
                snapshot.supervisor_run_id = if cleanup_error.is_some() {
                    supervisor_run
                } else {
                    None
                };
                snapshot.error = Some(match &cleanup_error {
                    Some(cleanup) => format!("{error}; supervisor cleanup failed: {cleanup}"),
                    None => error.clone(),
                });
            })
            .await;
    }
}

async fn run_supervision_inner(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let supervisor_run = launch_supervisor(context, plan, stop).await?;
    context
        .coordinator
        .mutate(plan.id, |snapshot| {
            snapshot.supervisor_run_id = Some(supervisor_run);
            snapshot.status = SupervisionStatus::Running;
        })
        .await?;

    let mut state_changes = context.manager.subscribe_state_changes();
    let mut baselines: HashMap<RunId, usize> = HashMap::new();

    loop {
        if *stop.borrow() {
            terminate_internal_run(&context.manager, supervisor_run).await?;
            context
                .coordinator
                .mutate(plan.id, |snapshot| {
                    snapshot.supervisor_run_id = None;
                    snapshot.status = SupervisionStatus::Stopped;
                })
                .await?;
            return Ok(());
        }

        let hydration = context
            .manager
            .hydrate()
            .await
            .map_err(|error| error.to_string())?;
        let eligible = eligible_runs(&hydration, plan.project.id(), supervisor_run);
        baselines.retain(|run_id, _| eligible.contains(run_id));
        let mut settled = HashSet::new();
        for run_id in &eligible {
            let Some(run) = hydration.runs.iter().find(|run| run.run.id() == *run_id) else {
                continue;
            };
            let assistant_messages = session_assistant_messages(&context.manager, *run_id).await?;
            let baseline = baselines.entry(*run_id).or_insert(assistant_messages);
            if run_is_idle_actionable(run) && assistant_messages > *baseline {
                *baseline = assistant_messages;
                settled.insert(*run_id);
            }
        }
        context
            .coordinator
            .mutate(plan.id, |snapshot| snapshot.watched_runs = eligible.len())
            .await?;

        if !settled.is_empty() {
            let cycles = {
                let sessions = context.coordinator.sessions.lock().await;
                sessions
                    .get(&plan.id)
                    .ok_or_else(|| format!("unknown supervision session {}", plan.id))?
                    .snapshot
                    .cycles
            };
            if cycles >= plan.max_cycles {
                terminate_internal_run(&context.manager, supervisor_run).await?;
                context
                    .coordinator
                    .mutate(plan.id, |snapshot| {
                        snapshot.supervisor_run_id = None;
                        snapshot.status = SupervisionStatus::Completed;
                    })
                    .await?;
                return Ok(());
            }
            context
                .coordinator
                .mutate(plan.id, |snapshot| {
                    snapshot.cycles = snapshot.cycles.saturating_add(1)
                })
                .await?;
            run_supervisor_cycle(
                context,
                plan,
                supervisor_run,
                &hydration,
                &eligible,
                &settled,
                &mut baselines,
                stop,
            )
            .await?;
        }

        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    continue;
                }
            }
            changed = state_changes.recv() => {
                if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                    return Err("runtime state-change stream closed during supervision".to_owned());
                }
            }
        }
    }
}

fn eligible_runs(
    hydration: &RuntimeHydrationSnapshot,
    project_id: ProjectId,
    supervisor_run: RunId,
) -> HashSet<RunId> {
    hydration
        .runs
        .iter()
        .filter(|run| {
            run.run.id() != supervisor_run
                && run.run.project_id() == project_id
                && !run.run.process_state().is_terminal()
        })
        .map(|run| run.run.id())
        .collect()
}

fn run_is_idle_actionable(run: &RunHydrationSnapshot) -> bool {
    let queue = run.run.queue();
    run.run.process_state() == ProcessState::Ready
        && run.run.activity_state() == ActivityState::Idle
        && queue.steering == 0
        && queue.follow_up == 0
        && run.run.pending_ui_requests() == 0
        && !run.run.is_retry_waiting()
        && !run.run.has_summarization_retry()
}

async fn launch_supervisor(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    stop: &watch::Receiver<bool>,
) -> Result<RunId, String> {
    let _gate = context.launch_cleanup_gate.lock().await;
    if *stop.borrow() {
        return Err("supervision was stopped before its supervisor could start".to_owned());
    }
    let capacity = context
        .manager
        .capacity()
        .await
        .map_err(|error| error.to_string())?;
    if capacity.active_runs >= capacity.live_run_limit {
        return Err("no live-run slot is available for supervision".to_owned());
    }

    let worktree_plan = supervision_worktree_plan(&plan.base, plan.id)?;
    let parent = worktree_plan.worktree_path.parent().ok_or_else(|| {
        format!(
            "supervision worktree path has no parent: {}",
            worktree_plan.worktree_path.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create supervision worktree parent {}: {error}",
            parent.display()
        )
    })?;
    let recovery = {
        let mut registry = context.worktrees.lock().await;
        registry
            .begin_creation(plan.project.id(), &worktree_plan)
            .map_err(|error| error.to_string())?
    };
    let created = match create_worktree(worktree_plan, &plan.environment, context.limits).await {
        Ok(created) => created,
        Err(error) => {
            if !error.may_have_mutated() {
                let mut registry = context.worktrees.lock().await;
                registry
                    .discard_unmutated_plan(recovery.id)
                    .map_err(|discard| {
                        format!("{error}; could not discard recovery intent: {discard}")
                    })?;
            }
            return Err(error.to_string());
        }
    };
    {
        let mut registry = context.worktrees.lock().await;
        registry
            .mark_created(recovery.id, created.clone())
            .map_err(|error| error.to_string())?;
    }
    if *stop.borrow() {
        return Err("supervision was stopped before Pi spawn".to_owned());
    }

    let mut spec = PiLaunchSpec::new(
        plan.environment.pi_executable().to_path_buf(),
        &created.execution_root,
        ProjectTrustPolicy::Ignore,
    );
    spec.context_files = ContextFilesPolicy::Disabled;
    spec.extension_discovery = ExtensionDiscoveryPolicy::Disabled;
    plan.selection.apply(&mut spec);
    spec.session = SessionLaunch::NewWithId(PiSessionId::new());
    let launch = spec.resolve().map_err(|error| error.to_string())?;
    let run_id = context
        .manager
        .start_run(RunStartSpec {
            project_id: plan.project.id(),
            execution_isolation: ExecutionIsolation::GitWorktree,
            worktree: Some(created.identity()),
            launch,
            environment: plan.environment.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    drop(_gate);
    wait_supervisor_ready(&context.manager, context.limits, run_id).await?;
    Ok(run_id)
}

fn supervision_worktree_plan(
    base: &WorktreeBaseSnapshot,
    id: SupervisionId,
) -> Result<WorktreeCreatePlan, String> {
    let repository_name = base
        .repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Git repository root has no usable directory name".to_owned())?;
    let parent = base
        .repository_root
        .parent()
        .ok_or_else(|| "Git repository root has no parent directory".to_owned())?;
    let key: String = id
        .to_string()
        .chars()
        .filter(|character| *character != '-')
        .collect();
    let leaf = format!("supervision-{key}");
    Ok(WorktreeCreatePlan {
        base: base.clone(),
        branch: format!("pi-wizard/{leaf}"),
        worktree_path: parent
            .join(format!("{repository_name}-worktrees"))
            .join(leaf),
    })
}

async fn wait_supervisor_ready(
    manager: &RuntimeManagerHandle,
    limits: RuntimeLimits,
    run_id: RunId,
) -> Result<(), String> {
    let mut state_changes = manager.subscribe_state_changes();
    tokio::time::timeout(
        Duration::from_millis(limits.startup_rpc_deadline_ms.saturating_add(1_000)),
        async {
            loop {
                let hydration = manager.hydrate().await.map_err(|error| error.to_string())?;
                let run = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == run_id)
                    .ok_or_else(|| "supervisor disappeared during startup".to_owned())?;
                if run.run.process_state().is_terminal() {
                    return Err(format!(
                        "supervisor ended as {:?} during startup",
                        run.run.process_state()
                    ));
                }
                if run.run.process_state() == ProcessState::Ready && !run.draft_restore_pending {
                    return Ok(());
                }
                match state_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(
                            "runtime state stream closed during supervisor startup".to_owned()
                        );
                    }
                }
            }
        },
    )
    .await
    .map_err(|_| "timed out waiting for supervisor readiness".to_owned())?
}

#[allow(clippy::too_many_arguments)]
async fn run_supervisor_cycle(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    supervisor_run: RunId,
    hydration: &RuntimeHydrationSnapshot,
    eligible: &HashSet<RunId>,
    settled: &HashSet<RunId>,
    baselines: &mut HashMap<RunId, usize>,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let prompt = supervisor_prompt(context, plan, hydration, eligible, settled).await?;
    let before = session_assistant_messages(&context.manager, supervisor_run).await?;
    let mut state_changes = context.manager.subscribe_state_changes();
    submit_text_prompt(&context.manager, supervisor_run, &prompt).await?;
    let wait = async {
        loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return Ok::<bool, String>(false);
                    }
                }
                changed = state_changes.recv() => {
                    if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                        return Err("supervisor state stream closed".to_owned());
                    }
                }
            }
            let current = context
                .manager
                .hydrate()
                .await
                .map_err(|error| error.to_string())?;
            let run = current
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
                && run.run.activity_state() == ActivityState::Idle
                && session_assistant_messages(&context.manager, supervisor_run).await? > before
            {
                return Ok(true);
            }
        }
    };
    let completed = tokio::time::timeout(
        Duration::from_millis(context.limits.supervision_turn_deadline_ms),
        wait,
    )
    .await
    .map_err(|_| "supervisor turn exceeded its configured deadline".to_owned())??;
    if !completed {
        return Ok(());
    }

    let text = last_assistant_text(&context.manager, supervisor_run).await?;
    if text.len() > context.limits.max_supervisor_context_bytes {
        return Err(format!(
            "supervisor response used {} bytes, exceeding limit {}",
            text.len(),
            context.limits.max_supervisor_context_bytes
        ));
    }
    let reply: SupervisorReply = serde_json::from_str(strip_json_fence(&text))
        .map_err(|error| format!("supervisor returned invalid directive JSON: {error}"))?;
    if reply.directives.len() > context.limits.max_supervisor_directives_per_cycle {
        return Err(format!(
            "supervisor returned {} directives, exceeding limit {}",
            reply.directives.len(),
            context.limits.max_supervisor_directives_per_cycle
        ));
    }

    let mut targeted = HashSet::new();
    for directive in reply.directives {
        if directive.run_id == supervisor_run || !eligible.contains(&directive.run_id) {
            return Err(format!(
                "supervisor targeted unknown or ineligible run {}",
                directive.run_id
            ));
        }
        if !targeted.insert(directive.run_id) {
            return Err(format!(
                "supervisor returned multiple directives for run {} in one cycle",
                directive.run_id
            ));
        }
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
            .ok_or_else(|| format!("supervisor target {} disappeared", directive.run_id))?;
        if run.run.project_id() != plan.project.id() || run.run.process_state().is_terminal() {
            return Err(format!(
                "supervisor target {} is no longer eligible",
                directive.run_id
            ));
        }
        let command = match directive.action {
            SupervisorAction::Send => {
                if !run_is_idle_actionable(run) {
                    return Err(format!(
                        "supervisor can send only to a currently idle run: {}",
                        directive.run_id
                    ));
                }
                baselines.insert(
                    directive.run_id,
                    session_assistant_messages(&context.manager, directive.run_id).await?,
                );
                RpcCommand::Prompt {
                    message: message.to_owned(),
                    images: Vec::new(),
                    streaming_behavior: None,
                }
            }
            SupervisorAction::Steer => {
                if run.run.process_state() != ProcessState::Ready
                    || run.run.activity_state() != ActivityState::Working
                {
                    return Err(format!(
                        "supervisor can steer only a working run: {}",
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
                    || run.run.activity_state() != ActivityState::Working
                {
                    return Err(format!(
                        "supervisor can queue follow-up only for a working run: {}",
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
    }
    Ok(())
}

async fn supervisor_prompt(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    hydration: &RuntimeHydrationSnapshot,
    eligible: &HashSet<RunId>,
    settled: &HashSet<RunId>,
) -> Result<String, String> {
    let mut prompt = String::from(
        "You supervise live Pi Wizard runs for one project. Keep them productive only when useful. Return JSON only in this exact shape: {\"directives\":[{\"runId\":\"UUID\",\"action\":\"send|steer|follow_up\",\"message\":\"...\"}]}. Use send only for idle runs. Use steer/follow_up only for working runs. An empty directives array means no intervention.\nRuns:\n",
    );
    for run_id in eligible {
        let Some(run) = hydration.runs.iter().find(|run| run.run.id() == *run_id) else {
            continue;
        };
        let status = match run.run.activity_state() {
            ActivityState::Idle => "idle",
            ActivityState::Working => "working",
            ActivityState::Compacting => "compacting",
            ActivityState::WaitingForInput => "needs_attention",
            ActivityState::Aborting => "aborting",
        };
        let session = run.run.session_state();
        let model = session
            .model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider, model.id))
            .unwrap_or_else(|| "pi-default".to_owned());
        let result = if settled.contains(run_id) {
            last_assistant_text(&context.manager, *run_id)
                .await
                .ok()
                .map(|text| truncate_utf8_prefix(&text, 4_096).to_owned())
        } else {
            None
        };
        let line = match result {
            Some(result) => format!(
                "- runId={run_id} status={status} model={model:?} root={:?} lastResult={result:?}\n",
                run.run.execution_root()
            ),
            None => format!(
                "- runId={run_id} status={status} model={model:?} root={:?}\n",
                run.run.execution_root()
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
    let _ = plan;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LaunchSelection;
    use crate::services::pi_session::submit_text_prompt;
    use crate::services::test_support::WorkflowFakePiFixture;
    use pi_wizard_core::launch::{PiLaunchSpec, ProjectTrustPolicy, SessionLaunch};
    use pi_wizard_core::runtime::{ExecutionIsolation, RunStartSpec, spawn_runtime_manager};
    use pi_wizard_core::worktree::inspect_worktree_base;
    use pi_wizard_core::{PiSessionId, SupervisionId};
    use std::fs;

    #[test]
    fn strict_supervisor_json_accepts_only_narrow_directive_shape() {
        let run_id = RunId::new();
        let text = format!(
            "```json\n{{\"directives\":[{{\"runId\":\"{run_id}\",\"action\":\"send\",\"message\":\"continue\"}}]}}\n```"
        );
        let reply: SupervisorReply =
            serde_json::from_str(strip_json_fence(&text)).expect("parse supervisor reply");
        assert_eq!(reply.directives.len(), 1);
        assert_eq!(reply.directives[0].run_id, run_id);
        assert!(matches!(reply.directives[0].action, SupervisorAction::Send));
    }

    #[test]
    fn supervision_worktree_identity_uses_full_session_uuid() {
        let root = std::path::PathBuf::from(r"C:\projects\sample");
        let base = WorktreeBaseSnapshot {
            repository_root: root.clone(),
            project_root: root,
            project_relative_path: std::path::PathBuf::new(),
            source_branch: Some("main".to_owned()),
            base_commit: "0123456789abcdef".to_owned(),
            dirty: false,
        };
        let id = SupervisionId::new();
        let plan = supervision_worktree_plan(&base, id).expect("plan");
        let key: String = id
            .to_string()
            .chars()
            .filter(|character| *character != '-')
            .collect();
        assert!(plan.branch.contains(&key));
        assert!(plan.worktree_path.to_string_lossy().contains(&key));
    }

    #[tokio::test]
    async fn supervision_independently_directs_a_normal_project_run_and_leaves_it_alive() {
        let fixture = WorkflowFakePiFixture::new("supervision-integration");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("supervision runtime manager");
        let project =
            ProjectBinding::register(&fixture.root).expect("register supervision project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect supervision Git base");

        let mut worker_launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        worker_launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker_run = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: worker_launch.resolve().expect("resolve worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start ordinary worker run");
        wait_supervisor_ready(&manager, limits, worker_run)
            .await
            .expect("ordinary worker ready");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                project.id(),
                None,
                None,
                None,
                1,
            ))
            .await
            .expect("insert supervision session");
        let mut changed = coordinator.subscribe();
        let context = SupervisionRuntimeContext {
            manager: manager.clone(),
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
            coordinator: coordinator.clone(),
        };
        let selection = LaunchSelection {
            context_files: ContextFilesPolicy::Disabled,
            extension_discovery: ExtensionDiscoveryPolicy::Disabled,
            provider: None,
            model: None,
            thinking: None,
        };
        let supervision_task = tokio::spawn(run_supervision(
            context,
            SupervisionPlan {
                id,
                project: project.clone(),
                environment: environment.clone(),
                base,
                selection,
                max_cycles: 1,
            },
            stop,
        ));

        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let snapshots = coordinator.snapshots().await;
                let snapshot = snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .expect("supervision snapshot");
                if snapshot.status == SupervisionStatus::Running && snapshot.watched_runs == 1 {
                    break;
                }
                match changed.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("supervision change stream closed before worker baseline")
                    }
                }
            }
        })
        .await
        .expect("supervision baseline deadline");

        submit_text_prompt(&manager, worker_run, "worker initial task")
            .await
            .expect("submit ordinary worker prompt");

        tokio::time::timeout(Duration::from_secs(15), supervision_task)
            .await
            .expect("supervision execution deadline")
            .expect("supervision task join");

        let snapshots = coordinator.snapshots().await;
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .expect("terminal supervision snapshot");
        assert_eq!(
            snapshot.status,
            SupervisionStatus::Completed,
            "{snapshot:?}"
        );
        assert_eq!(snapshot.cycles, 1);
        assert!(snapshot.supervisor_run_id.is_none());
        assert!(snapshot.error.is_none());

        let hydration = manager
            .hydrate()
            .await
            .expect("worker hydration after supervision");
        let worker = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == worker_run)
            .expect("ordinary worker remains registered");
        assert_eq!(worker.run.process_state(), ProcessState::Ready);

        let worker_prompts = fs::read_to_string(fixture.root.join("workflow-worker-prompts.log"))
            .expect("read worker prompt audit");
        assert_eq!(
            worker_prompts.lines().collect::<Vec<_>>(),
            ["worker initial task", "supervised continuation"],
            "independent supervisor must send a second prompt into the existing normal run"
        );

        manager
            .shutdown()
            .await
            .expect("shutdown supervision manager");
    }
}
