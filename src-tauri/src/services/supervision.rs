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
use crate::services::pi_session::{last_assistant_text, submit_text_prompt};

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
            !session.snapshot.status.is_terminal()
                && session
                    .snapshot
                    .project_ids
                    .iter()
                    .any(|project_id| snapshot.project_ids.contains(project_id))
        }) {
            return Err(
                "one or more selected projects already have active supervision; stop the overlapping session before starting another"
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
    pub(crate) project_ids: HashSet<ProjectId>,
    pub(crate) environment: ResolvedLaunchEnvironment,
    pub(crate) base: WorktreeBaseSnapshot,
    pub(crate) selection: LaunchSelection,
    pub(crate) prompt_templates: Vec<String>,
    pub(crate) max_cycles: Option<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SupervisorAction {
    Send,
    Steer,
    FollowUp,
    Stop,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorDirective {
    run_id: RunId,
    action: SupervisorAction,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorReply {
    #[serde(default)]
    directives: Vec<SupervisorDirective>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedRunVersion {
    session_replacement_generation: u64,
    session_id: Option<String>,
    assistant_message_generation: u64,
}

enum SupervisorLaunchOutcome {
    Ready(RunId),
    Stopped(Option<RunId>),
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
    let supervisor_run = match launch_supervisor(context, plan, stop).await? {
        SupervisorLaunchOutcome::Ready(run_id) => run_id,
        SupervisorLaunchOutcome::Stopped(run_id) => {
            if let Some(run_id) = run_id {
                terminate_internal_run(&context.manager, run_id).await?;
            }
            context
                .coordinator
                .mutate(plan.id, |snapshot| {
                    snapshot.supervisor_run_id = None;
                    snapshot.status = SupervisionStatus::Stopped;
                })
                .await?;
            return Ok(());
        }
    };
    context
        .coordinator
        .mutate(plan.id, |snapshot| {
            snapshot.supervisor_run_id = Some(supervisor_run);
            snapshot.status = SupervisionStatus::Running;
        })
        .await?;

    let mut considered_idle: HashMap<RunId, ObservedRunVersion> = HashMap::new();

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

        // Subscribe before taking the authoritative snapshot. Any semantic
        // change after this point is either already reflected by hydration or
        // remains queued for the wait below. A completed supervisor decision
        // starts the next loop with a fresh receiver so read-only RPC wakeups
        // generated while building/applying that decision are absorbed by the
        // next snapshot instead of replayed one by one.
        let mut state_changes = context.manager.subscribe_state_changes();
        let hydration = context
            .manager
            .hydrate()
            .await
            .map_err(|error| error.to_string())?;
        let eligible = eligible_runs(&hydration, &plan.project_ids, supervisor_run);
        considered_idle.retain(|run_id, _| eligible.contains(run_id));
        let mut settled = HashSet::new();
        let mut observed = HashMap::with_capacity(eligible.len());
        for run_id in &eligible {
            let Some(run) = hydration.runs.iter().find(|run| run.run.id() == *run_id) else {
                continue;
            };
            let generation = ObservedRunVersion {
                session_replacement_generation: run.run.session_replacement_generation(),
                session_id: run.run.session_state().session_id.clone(),
                assistant_message_generation: run.run.assistant_message_generation(),
            };
            observed.insert(*run_id, generation.clone());
            if !run_is_idle_actionable(run) {
                continue;
            }
            if considered_idle.get(run_id) != Some(&generation) {
                considered_idle.insert(*run_id, generation);
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
            if plan.max_cycles.is_some_and(|maximum| cycles >= maximum) {
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
            let deferred_idle = run_supervisor_cycle(
                context,
                plan,
                supervisor_run,
                &hydration,
                &eligible,
                &settled,
                &observed,
                stop,
            )
            .await?;
            for run_id in deferred_idle {
                considered_idle.remove(&run_id);
            }
            if *stop.borrow() {
                continue;
            }
            let completed_cycles = cycles.saturating_add(1);
            context
                .coordinator
                .mutate(plan.id, |snapshot| snapshot.cycles = completed_cycles)
                .await?;
            if plan
                .max_cycles
                .is_some_and(|maximum| completed_cycles >= maximum)
            {
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
            continue;
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
    project_ids: &HashSet<ProjectId>,
    supervisor_run: RunId,
) -> HashSet<RunId> {
    hydration
        .runs
        .iter()
        .filter(|run| {
            run.run.id() != supervisor_run
                && project_ids.contains(&run.run.project_id())
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
        && !run_has_active_direct_bash(run)
}

fn run_has_active_direct_bash(run: &RunHydrationSnapshot) -> bool {
    run.rpc
        .as_ref()
        .is_some_and(|rpc| !rpc.live.direct_bash.is_empty())
}

async fn launch_supervisor(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    stop: &mut watch::Receiver<bool>,
) -> Result<SupervisorLaunchOutcome, String> {
    let _gate = context.launch_cleanup_gate.lock().await;
    if *stop.borrow() {
        return Ok(SupervisorLaunchOutcome::Stopped(None));
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
        return Ok(SupervisorLaunchOutcome::Stopped(None));
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
    if let Err(error) = context
        .coordinator
        .mutate(plan.id, |snapshot| {
            snapshot.supervisor_run_id = Some(run_id)
        })
        .await
    {
        drop(_gate);
        let cleanup = terminate_internal_run(&context.manager, run_id).await;
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup_error) => {
                format!("{error}; untracked supervisor cleanup failed: {cleanup_error}")
            }
        });
    }
    drop(_gate);
    if wait_supervisor_ready_or_stopped(&context.manager, context.limits, run_id, stop).await? {
        Ok(SupervisorLaunchOutcome::Ready(run_id))
    } else {
        Ok(SupervisorLaunchOutcome::Stopped(Some(run_id)))
    }
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

async fn wait_supervisor_ready_or_stopped(
    manager: &RuntimeManagerHandle,
    limits: RuntimeLimits,
    run_id: RunId,
    stop: &mut watch::Receiver<bool>,
) -> Result<bool, String> {
    if *stop.borrow() {
        return Ok(false);
    }
    tokio::select! {
        result = wait_supervisor_ready(manager, limits, run_id) => result.map(|()| true),
        changed = stop.changed() => {
            if changed.is_err() || *stop.borrow() {
                Ok(false)
            } else {
                Err("supervision stop signal changed without requesting Stop".to_owned())
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_supervisor_cycle(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    supervisor_run: RunId,
    hydration: &RuntimeHydrationSnapshot,
    eligible: &HashSet<RunId>,
    settled: &HashSet<RunId>,
    observed: &HashMap<RunId, ObservedRunVersion>,
    stop: &mut watch::Receiver<bool>,
) -> Result<HashSet<RunId>, String> {
    let prompt = supervisor_prompt(context, plan, hydration, eligible, settled).await?;
    if *stop.borrow() {
        return Ok(HashSet::new());
    }
    let mut state_changes = context.manager.subscribe_state_changes();
    let before = context
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?
        .runs
        .iter()
        .find(|run| run.run.id() == supervisor_run)
        .ok_or_else(|| "supervisor run disappeared before its turn".to_owned())?
        .run
        .assistant_message_generation();
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
                && run.run.assistant_message_generation() > before
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
        return Ok(HashSet::new());
    }
    if *stop.borrow() {
        return Ok(HashSet::new());
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
    let mut decision_summary = Vec::new();
    let mut deferred_idle = HashSet::new();
    for directive in reply.directives {
        if *stop.borrow() {
            return Ok(HashSet::new());
        }
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
        let current = context
            .manager
            .hydrate()
            .await
            .map_err(|error| error.to_string())?;
        let Some(run) = current
            .runs
            .iter()
            .find(|run| run.run.id() == directive.run_id)
        else {
            decision_summary.push(format!(
                "Skipped run {}: it is no longer live",
                short_run_id(directive.run_id)
            ));
            continue;
        };
        if !plan.project_ids.contains(&run.run.project_id()) {
            return Err(format!(
                "supervisor target {} is no longer eligible",
                directive.run_id
            ));
        }
        let observed_version = observed.get(&directive.run_id).ok_or_else(|| {
            format!(
                "supervisor target {} has no observation token for this cycle",
                directive.run_id
            )
        })?;
        if run.run.session_replacement_generation()
            != observed_version.session_replacement_generation
            || run.run.session_state().session_id != observed_version.session_id
        {
            decision_summary.push(format!(
                "Skipped run {}: it changed Pi sessions during the supervisor decision",
                short_run_id(directive.run_id)
            ));
            continue;
        }
        if matches!(
            directive.action,
            SupervisorAction::Send | SupervisorAction::Stop
        ) && run.run.assistant_message_generation()
            != observed_version.assistant_message_generation
        {
            decision_summary.push(format!(
                "Skipped run {}: it produced a newer assistant result during the supervisor decision",
                short_run_id(directive.run_id)
            ));
            continue;
        }
        if run.run.process_state().is_terminal() {
            decision_summary.push(format!(
                "Skipped run {}: it already finished",
                short_run_id(directive.run_id)
            ));
            continue;
        }
        if run_has_active_direct_bash(run) {
            decision_summary.push(format!(
                "Skipped run {}: a user direct command owns the execution root",
                short_run_id(directive.run_id)
            ));
            if settled.contains(&directive.run_id) {
                deferred_idle.insert(directive.run_id);
            }
            continue;
        }
        if matches!(directive.action, SupervisorAction::Stop) {
            terminate_supervised_run(&context.manager, directive.run_id).await?;
            decision_summary.push(format!("Stopped run {}", short_run_id(directive.run_id)));
            continue;
        }
        let message = directive
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .ok_or_else(|| {
                "supervisor send/steer/follow_up directive requires a message".to_owned()
            })?;
        if message.len() > context.limits.max_draft_bytes_per_session {
            return Err(format!(
                "supervisor directive uses {} bytes, exceeding prompt limit {}",
                message.len(),
                context.limits.max_draft_bytes_per_session
            ));
        }
        if let Some(reason) = directive_state_skip_reason(run, directive.action) {
            decision_summary.push(format!(
                "Skipped run {}: {reason}",
                short_run_id(directive.run_id)
            ));
            continue;
        }
        let command = match directive.action {
            SupervisorAction::Send => RpcCommand::Prompt {
                message: message.to_owned(),
                images: Vec::new(),
                streaming_behavior: None,
            },
            SupervisorAction::Steer => RpcCommand::Steer {
                message: message.to_owned(),
                images: Vec::new(),
            },
            SupervisorAction::FollowUp => RpcCommand::FollowUp {
                message: message.to_owned(),
                images: Vec::new(),
            },
            SupervisorAction::Stop => {
                unreachable!("stop directives are handled before message validation")
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
        let action = match directive.action {
            SupervisorAction::Send => "Sent next task to",
            SupervisorAction::Steer => "Steered",
            SupervisorAction::FollowUp => "Queued follow-up for",
            SupervisorAction::Stop => unreachable!("stop handled before Pi RPC"),
        };
        decision_summary.push(format!(
            "{action} run {}: {}",
            short_run_id(directive.run_id),
            truncate_utf8_prefix(message, 240)
        ));
    }
    let summary = if decision_summary.is_empty() {
        format!(
            "No intervention for {} newly idle run{}",
            settled.len(),
            if settled.len() == 1 { "" } else { "s" }
        )
    } else {
        decision_summary.join(" · ")
    };
    let summary =
        truncate_utf8_prefix(&summary, context.limits.max_failure_detail_bytes).to_owned();
    context
        .coordinator
        .mutate(plan.id, |snapshot| snapshot.last_decision = Some(summary))
        .await?;
    Ok(deferred_idle)
}

fn directive_state_skip_reason(
    run: &RunHydrationSnapshot,
    action: SupervisorAction,
) -> Option<&'static str> {
    match action {
        SupervisorAction::Send if !run_is_idle_actionable(run) => Some("it is no longer idle"),
        SupervisorAction::Steer | SupervisorAction::FollowUp
            if run.run.process_state() != ProcessState::Ready
                || run.run.activity_state() != ActivityState::Working =>
        {
            Some("it is no longer working")
        }
        _ => None,
    }
}

fn short_run_id(run_id: RunId) -> String {
    run_id.to_string().chars().take(8).collect()
}

async fn terminate_supervised_run(
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
    if run.run.process_state() != ProcessState::Ready
        || run.run.activity_state() != ActivityState::Idle
    {
        let stopped = manager
            .stop_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if stopped.quarantined {
            return Err(format!(
                "supervisor Stop left run {run_id} with uncertain process termination"
            ));
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
        && run.run.activity_state() == ActivityState::Idle
    {
        let closed = manager
            .close_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if closed.quarantined || !closed.process_terminated {
            return Err(format!(
                "supervisor could not confirm process termination for run {run_id}"
            ));
        }
        return Ok(());
    }
    Err(format!(
        "supervisor cannot stop run {run_id} from process state {:?}",
        run.run.process_state()
    ))
}

async fn supervisor_prompt(
    context: &SupervisionRuntimeContext,
    plan: &SupervisionPlan,
    hydration: &RuntimeHydrationSnapshot,
    eligible: &HashSet<RunId>,
    settled: &HashSet<RunId>,
) -> Result<String, String> {
    let mut prompt = String::from(
        "You supervise live Pi Wizard coding runs across the selected projects. Your default job is to keep newly idle runs productively moving through the next logical engineering task. When a run has just finished, inspect its lastResult and normally send a concrete next task that advances that project without simply repeating completed work. If the result reports a problem, either send a bounded task that can resolve it or stop the run when continuing autonomously is unsafe or unproductive. Return JSON only in this exact shape: {\"directives\":[{\"runId\":\"UUID\",\"action\":\"send|steer|follow_up|stop\",\"message\":\"...\"}]}. The message is required for send/steer/follow_up and may be omitted for stop. Use send only for idle runs. Use steer/follow_up only for working runs. Use stop only when the run should be terminated rather than continued. An empty directives array means deliberate no intervention.\n",
    );
    prompt.push_str("Newly idle runs requiring a decision this cycle are marked decisionRequired=true. Prioritize those runs over unrelated active work.\nRuns:\n");
    for run_id in ordered_supervision_run_ids(eligible, settled) {
        let Some(run) = hydration.runs.iter().find(|run| run.run.id() == run_id) else {
            continue;
        };
        let status = if run_has_active_direct_bash(run) {
            "command_running"
        } else {
            match run.run.activity_state() {
                ActivityState::Idle => "idle",
                ActivityState::Working => "working",
                ActivityState::Compacting => "compacting",
                ActivityState::WaitingForInput => "needs_attention",
                ActivityState::Aborting => "aborting",
            }
        };
        let session = run.run.session_state();
        let model = session
            .model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider, model.id))
            .unwrap_or_else(|| "pi-default".to_owned());
        let decision_required = settled.contains(&run_id);
        let result = if decision_required {
            last_assistant_text(&context.manager, run_id)
                .await
                .ok()
                .map(|text| truncate_utf8_prefix(&text, 4_096).to_owned())
        } else {
            None
        };
        let line = match result {
            Some(result) => format!(
                "- runId={run_id} projectId={} decisionRequired={decision_required} status={status} model={model:?} root={:?} lastResult={result:?}\n",
                run.run.project_id(),
                run.run.execution_root()
            ),
            None => format!(
                "- runId={run_id} projectId={} decisionRequired={decision_required} status={status} model={model:?} root={:?}\n",
                run.run.project_id(),
                run.run.execution_root()
            ),
        };
        if prompt.len().saturating_add(line.len()) > context.limits.max_supervisor_context_bytes {
            break;
        }
        prompt.push_str(&line);
    }
    if !plan.prompt_templates.is_empty() {
        let heading = "Reusable playbook prompts. Treat these as candidate work themes, adapt them to each project's current state, and choose the next logical one rather than blindly replaying them:\n";
        if prompt.len().saturating_add(heading.len()) <= context.limits.max_supervisor_context_bytes
        {
            prompt.push_str(heading);
            for template in &plan.prompt_templates {
                let line = format!("- {:?}\n", truncate_utf8_prefix(template, 2_048));
                if prompt.len().saturating_add(line.len())
                    > context.limits.max_supervisor_context_bytes
                {
                    break;
                }
                prompt.push_str(&line);
            }
        }
    }
    if prompt.len() > context.limits.max_supervisor_context_bytes {
        return Err("supervisor instruction prefix exceeds configured context limit".to_owned());
    }
    Ok(prompt)
}

fn ordered_supervision_run_ids(eligible: &HashSet<RunId>, settled: &HashSet<RunId>) -> Vec<RunId> {
    let mut run_ids: Vec<_> = eligible.iter().copied().collect();
    run_ids.sort_by_key(|run_id| (!settled.contains(run_id), run_id.to_string()));
    run_ids
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

    fn assert_no_session_stats_probes(root: &std::path::Path) {
        if !root.exists() {
            return;
        }
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("read workflow fixture directory") {
                let path = entry.expect("read workflow fixture entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.file_name().and_then(|name| name.to_str())
                    == Some("workflow-session-stats.log")
                {
                    panic!(
                        "orchestration must not poll Pi get_session_stats; found {}",
                        path.display()
                    );
                }
            }
        }
    }

    async fn wait_for_supervisor_turn_started(
        coordinator: &SupervisionCoordinator,
        manager: &RuntimeManagerHandle,
        id: SupervisionId,
    ) -> RunId {
        let mut supervision_changes = coordinator.subscribe();
        let mut runtime_changes = manager.subscribe_state_changes();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let snapshots = coordinator.snapshots().await;
                let supervisor_run = snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .and_then(|snapshot| snapshot.supervisor_run_id);
                if let Some(supervisor_run) = supervisor_run {
                    let hydration = manager.hydrate().await.expect("supervisor-turn hydration");
                    if hydration.runs.iter().any(|run| {
                        run.run.id() == supervisor_run
                            && run.run.activity_state() == ActivityState::Working
                    }) {
                        return supervisor_run;
                    }
                }
                tokio::select! {
                    changed = supervision_changes.recv() => {
                        if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                            panic!("supervision stream closed before supervisor turn started");
                        }
                    }
                    changed = runtime_changes.recv() => {
                        if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                            panic!("runtime stream closed before supervisor turn started");
                        }
                    }
                }
            }
        })
        .await
        .expect("supervisor turn start deadline")
    }

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
        assert_eq!(reply.directives[0].message.as_deref(), Some("continue"));

        let stop_text =
            format!("{{\"directives\":[{{\"runId\":\"{run_id}\",\"action\":\"stop\"}}]}}");
        let stop_reply: SupervisorReply =
            serde_json::from_str(&stop_text).expect("parse supervisor stop reply");
        assert!(matches!(
            stop_reply.directives[0].action,
            SupervisorAction::Stop
        ));
        assert!(stop_reply.directives[0].message.is_none());
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

    #[test]
    fn supervisor_prompt_order_prioritizes_runs_that_require_a_decision() {
        let first_settled = RunId::new();
        let unrelated = RunId::new();
        let second_settled = RunId::new();
        let eligible = HashSet::from([first_settled, unrelated, second_settled]);
        let settled = HashSet::from([first_settled, second_settled]);

        let ordered = ordered_supervision_run_ids(&eligible, &settled);
        assert_eq!(ordered.len(), 3);
        assert!(settled.contains(&ordered[0]));
        assert!(settled.contains(&ordered[1]));
        assert_eq!(ordered[2], unrelated);
    }

    #[tokio::test]
    async fn supervisor_state_revalidation_classifies_stale_actions_without_failing_session() {
        let fixture = WorkflowFakePiFixture::new("supervision-state-race");
        let environment = fixture.environment();
        let limits = RuntimeLimits {
            max_live_runs: 2,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("supervision race runtime manager");
        let project = ProjectBinding::register(&fixture.root).expect("register race project");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch.resolve().expect("resolve race worker launch"),
                environment,
            })
            .await
            .expect("start race worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("race worker ready");

        let hydration = manager.hydrate().await.expect("idle race hydration");
        let idle = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == worker)
            .expect("idle race worker");
        assert_eq!(
            directive_state_skip_reason(idle, SupervisorAction::Send),
            None
        );
        assert_eq!(
            directive_state_skip_reason(idle, SupervisorAction::Steer),
            Some("it is no longer working")
        );

        let mut changes = manager.subscribe_state_changes();
        submit_text_prompt(&manager, worker, "slow race task")
            .await
            .expect("start slow race task");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let hydration = manager.hydrate().await.expect("working race hydration");
                let working = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == worker)
                    .expect("working race worker");
                if working.run.activity_state() == ActivityState::Working {
                    assert_eq!(
                        directive_state_skip_reason(working, SupervisorAction::Send),
                        Some("it is no longer idle")
                    );
                    assert_eq!(
                        directive_state_skip_reason(working, SupervisorAction::Steer),
                        None
                    );
                    break;
                }
                match changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("runtime state stream closed before race worker became active")
                    }
                }
            }
        })
        .await
        .expect("race worker working deadline");

        manager.shutdown().await.expect("shutdown race manager");
    }

    #[tokio::test]
    async fn supervision_skips_send_when_user_takes_over_idle_run_during_supervisor_turn() {
        let fixture = WorkflowFakePiFixture::new("supervision-send-race");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("send-race runtime manager");
        let project = ProjectBinding::register(&fixture.root).expect("register send-race project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect send-race Git base");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch.resolve().expect("resolve send-race worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start send-race worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("send-race worker ready");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                Some(1),
            ))
            .await
            .expect("insert send-race supervision");
        let mut supervision_changes = coordinator.subscribe();
        let mut runtime_changes = manager.subscribe_state_changes();
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
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection,
                prompt_templates: vec!["RACE_TEST_DELAY".to_owned()],
                max_cycles: Some(1),
            },
            stop,
        ));

        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let snapshots = coordinator.snapshots().await;
                let supervisor_run = snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .and_then(|snapshot| snapshot.supervisor_run_id);
                if let Some(supervisor_run) = supervisor_run {
                    let hydration = manager
                        .hydrate()
                        .await
                        .expect("send-race supervisor hydration");
                    if hydration.runs.iter().any(|run| {
                        run.run.id() == supervisor_run
                            && run.run.activity_state() == ActivityState::Working
                    }) {
                        break;
                    }
                }
                tokio::select! {
                    changed = supervision_changes.recv() => {
                        if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                            panic!("supervision stream closed before race decision started");
                        }
                    }
                    changed = runtime_changes.recv() => {
                        if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                            panic!("runtime stream closed before race decision started");
                        }
                    }
                }
            }
        })
        .await
        .expect("supervisor race-decision start deadline");

        submit_text_prompt(&manager, worker, "race manual takeover")
            .await
            .expect("user takes over race worker");

        tokio::time::timeout(Duration::from_secs(12), supervision_task)
            .await
            .expect("send-race supervision deadline")
            .expect("send-race supervision join");

        let snapshots = coordinator.snapshots().await;
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .expect("send-race terminal snapshot");
        assert_eq!(
            snapshot.status,
            SupervisionStatus::Completed,
            "{snapshot:?}"
        );
        assert!(snapshot.error.is_none(), "{snapshot:?}");
        let decision = snapshot
            .last_decision
            .as_deref()
            .expect("send-race last decision");
        assert!(decision.contains("Skipped run"), "{decision}");
        assert!(decision.contains("no longer idle"), "{decision}");

        let prompts = fs::read_to_string(fixture.root.join("workflow-worker-prompts.log"))
            .expect("read send-race worker prompt audit");
        assert_eq!(
            prompts.lines().collect::<Vec<_>>(),
            ["race manual takeover"],
            "the stale supervisor send must not overwrite or duplicate the user's takeover"
        );

        manager
            .shutdown()
            .await
            .expect("shutdown send-race manager");
    }

    #[tokio::test]
    async fn supervision_defers_autonomous_stop_while_direct_bash_owns_execution_root() {
        let fixture = WorkflowFakePiFixture::new("supervision-bash-race");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("Bash-race runtime manager");
        let project = ProjectBinding::register(&fixture.root).expect("register Bash-race project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect Bash-race Git base");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch.resolve().expect("resolve Bash-race worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start Bash-race worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("Bash-race worker ready");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                None,
            ))
            .await
            .expect("insert Bash-race supervision");
        let mut supervision_changes = coordinator.subscribe();
        let mut runtime_changes = manager.subscribe_state_changes();
        let supervision_task = tokio::spawn(run_supervision(
            SupervisionRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
                coordinator: coordinator.clone(),
            },
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: vec!["RACE_TEST_DELAY STOP_DURING_BASH_RACE".to_owned()],
                max_cycles: None,
            },
            stop,
        ));

        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let supervisor_run = coordinator
                    .snapshots()
                    .await
                    .into_iter()
                    .find(|snapshot| snapshot.id == id)
                    .and_then(|snapshot| snapshot.supervisor_run_id);
                if let Some(supervisor_run) = supervisor_run {
                    let hydration = manager
                        .hydrate()
                        .await
                        .expect("Bash-race supervisor hydration");
                    if hydration.runs.iter().any(|run| {
                        run.run.id() == supervisor_run
                            && run.run.activity_state() == ActivityState::Working
                    }) {
                        break;
                    }
                }
                tokio::select! {
                    changed = supervision_changes.recv() => {
                        if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                            panic!("supervision stream closed before Bash-race decision started");
                        }
                    }
                    changed = runtime_changes.recv() => {
                        if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                            panic!("runtime stream closed before Bash-race decision started");
                        }
                    }
                }
            }
        })
        .await
        .expect("Bash-race supervisor decision start deadline");

        let bash_manager = manager.clone();
        let bash_task = tokio::spawn(async move {
            bash_manager
                .request(
                    worker,
                    RpcRequest::new(RpcCommand::Bash {
                        command: "slow-bash".to_owned(),
                        exclude_from_context: Some(true),
                    }),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let hydration = manager.hydrate().await.expect("Bash-race worker hydration");
                let run = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == worker)
                    .expect("Bash-race worker");
                if run
                    .rpc
                    .as_ref()
                    .is_some_and(|rpc| !rpc.live.direct_bash.is_empty())
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Bash-race direct command start deadline");

        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let snapshot = coordinator
                    .snapshots()
                    .await
                    .into_iter()
                    .find(|snapshot| snapshot.id == id)
                    .expect("Bash-race supervision snapshot");
                if snapshot.cycles == 1
                    && snapshot.last_decision.as_deref().is_some_and(|decision| {
                        decision.contains("a user direct command owns the execution root")
                    })
                {
                    break;
                }
                match supervision_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("supervision stream closed before Bash-race deferral")
                    }
                }
            }
        })
        .await
        .expect("Bash-race deferral deadline");
        assert!(
            fs::read_to_string(fixture.root.join("workflow-worker-prompts.log"))
                .unwrap_or_default()
                .is_empty(),
            "supervision must not inject model work while direct Bash owns the checkout"
        );
        let during_bash = manager
            .hydrate()
            .await
            .expect("Bash-race retained worker hydration");
        let worker_during_bash = during_bash
            .runs
            .iter()
            .find(|run| run.run.id() == worker)
            .expect("Bash-race worker remains registered");
        assert!(
            !worker_during_bash.run.process_state().is_terminal(),
            "an autonomous Stop decision must not terminate a worker while user Bash owns its execution root"
        );

        let bash = bash_task
            .await
            .expect("Bash-race command task join")
            .expect("Bash-race command completion");
        assert!(bash.response.success);

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = coordinator
                    .snapshots()
                    .await
                    .into_iter()
                    .find(|snapshot| snapshot.id == id)
                    .expect("Bash-race post-command snapshot");
                if snapshot.cycles >= 2
                    && snapshot.last_decision.as_deref().is_some_and(|decision| {
                        decision.contains("No intervention for 1 newly idle run")
                    })
                {
                    break;
                }
                match supervision_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("supervision stream closed before Bash-race reconsideration")
                    }
                }
            }
        })
        .await
        .expect("Bash-race reconsideration deadline");

        coordinator
            .request_stop(id)
            .await
            .expect("stop Bash-race supervision");
        tokio::time::timeout(Duration::from_secs(8), supervision_task)
            .await
            .expect("Bash-race stop deadline")
            .expect("Bash-race supervision join");
        let stopped = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("stopped Bash-race snapshot");
        assert_eq!(stopped.status, SupervisionStatus::Stopped, "{stopped:?}");

        manager
            .shutdown()
            .await
            .expect("shutdown Bash-race manager");
    }

    #[tokio::test]
    async fn supervision_skips_stale_send_when_target_switches_sessions_during_decision() {
        let fixture = WorkflowFakePiFixture::new("supervision-session-race");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("session-race runtime manager");
        let project =
            ProjectBinding::register(&fixture.root).expect("register session-race project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect session-race Git base");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch
                    .resolve()
                    .expect("resolve session-race worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start session-race worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("session-race worker ready");
        let original_session = manager
            .hydrate()
            .await
            .expect("original session hydration")
            .runs
            .iter()
            .find(|run| run.run.id() == worker)
            .and_then(|run| run.run.session_state().session_id.clone())
            .expect("original worker session id");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                Some(1),
            ))
            .await
            .expect("insert session-race supervision");
        let context = SupervisionRuntimeContext {
            manager: manager.clone(),
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
            coordinator: coordinator.clone(),
        };
        let supervision_task = tokio::spawn(run_supervision(
            context,
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: vec!["RACE_TEST_DELAY".to_owned()],
                max_cycles: Some(1),
            },
            stop,
        ));

        wait_for_supervisor_turn_started(&coordinator, &manager, id).await;
        let mut runtime_changes = manager.subscribe_state_changes();
        let replacement = manager
            .request(
                worker,
                RpcRequest::new(RpcCommand::SwitchSession {
                    session_path: std::path::PathBuf::from("switched-session.jsonl"),
                }),
            )
            .await
            .expect("switch worker session while supervisor decides");
        assert!(replacement.response.success);
        let reconciliation_gap = manager
            .hydrate()
            .await
            .expect("session-reconciliation gap hydration");
        let gap_run = reconciliation_gap
            .runs
            .iter()
            .find(|run| run.run.id() == worker)
            .expect("worker remains registered during session reconciliation");
        assert_eq!(
            gap_run.run.session_state().session_id.as_deref(),
            Some(original_session.as_str()),
            "the fixture must keep the old session ID visible until delayed get_state reconciliation"
        );
        assert_eq!(
            gap_run.run.session_replacement_generation(),
            1,
            "accepted replacement must invalidate stale autonomous decisions before session ID reconciliation"
        );

        tokio::time::timeout(Duration::from_secs(12), supervision_task)
            .await
            .expect("session-race supervision deadline")
            .expect("session-race supervision join");
        let snapshot = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("session-race terminal snapshot");
        assert_eq!(
            snapshot.status,
            SupervisionStatus::Completed,
            "{snapshot:?}"
        );
        let decision = snapshot
            .last_decision
            .as_deref()
            .expect("session-race last decision");
        assert!(decision.contains("changed Pi sessions"), "{decision}");
        assert!(
            !fixture.root.join("workflow-worker-prompts.log").exists(),
            "a decision based on the previous Pi session must not send into the replacement session"
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let hydration = manager
                    .hydrate()
                    .await
                    .expect("eventual switched-session hydration");
                let current = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == worker)
                    .and_then(|run| run.run.session_state().session_id.as_deref());
                if current.is_some_and(|session_id| session_id != original_session) {
                    break;
                }
                match runtime_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("runtime stream closed before delayed session reconciliation")
                    }
                }
            }
        })
        .await
        .expect("delayed session reconciliation deadline");

        manager
            .shutdown()
            .await
            .expect("shutdown session-race manager");
    }

    #[tokio::test]
    async fn supervision_skips_stale_send_after_newer_manual_result_already_settled() {
        let fixture = WorkflowFakePiFixture::new("supervision-newer-result-race");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("newer-result runtime manager");
        let project =
            ProjectBinding::register(&fixture.root).expect("register newer-result project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect newer-result Git base");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch
                    .resolve()
                    .expect("resolve newer-result worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start newer-result worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("newer-result worker ready");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                Some(1),
            ))
            .await
            .expect("insert newer-result supervision");
        let context = SupervisionRuntimeContext {
            manager: manager.clone(),
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
            coordinator: coordinator.clone(),
        };
        let supervision_task = tokio::spawn(run_supervision(
            context,
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: vec!["RACE_TEST_DELAY".to_owned()],
                max_cycles: Some(1),
            },
            stop,
        ));

        wait_for_supervisor_turn_started(&coordinator, &manager, id).await;
        let mut runtime_changes = manager.subscribe_state_changes();
        submit_text_prompt(&manager, worker, "slow manual takeover")
            .await
            .expect("start manual takeover that will settle before supervisor reply");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let hydration = manager
                    .hydrate()
                    .await
                    .expect("newer-result worker hydration");
                let run = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == worker)
                    .expect("newer-result worker remains live");
                if run_is_idle_actionable(run) && run.run.assistant_message_generation() >= 1 {
                    break;
                }
                match runtime_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("runtime stream closed before manual takeover settled")
                    }
                }
            }
        })
        .await
        .expect("manual takeover settlement deadline");

        tokio::time::timeout(Duration::from_secs(12), supervision_task)
            .await
            .expect("newer-result supervision deadline")
            .expect("newer-result supervision join");
        let snapshot = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("newer-result terminal snapshot");
        assert_eq!(
            snapshot.status,
            SupervisionStatus::Completed,
            "{snapshot:?}"
        );
        let decision = snapshot
            .last_decision
            .as_deref()
            .expect("newer-result last decision");
        assert!(decision.contains("newer assistant result"), "{decision}");

        let prompts = fs::read_to_string(fixture.root.join("workflow-worker-prompts.log"))
            .expect("read newer-result worker prompt audit");
        assert_eq!(
            prompts.lines().collect::<Vec<_>>(),
            ["slow manual takeover"],
            "a supervisor decision based on the older idle generation must not append another task"
        );

        manager
            .shutdown()
            .await
            .expect("shutdown newer-result manager");
    }

    #[tokio::test]
    async fn stopping_supervision_during_supervisor_turn_prevents_worker_directives() {
        let fixture = WorkflowFakePiFixture::new("supervision-user-stop-race");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("user-stop-race runtime manager");
        let project =
            ProjectBinding::register(&fixture.root).expect("register user-stop-race project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect user-stop-race Git base");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch
                    .resolve()
                    .expect("resolve user-stop-race worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start user-stop-race worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("user-stop-race worker ready");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                None,
            ))
            .await
            .expect("insert user-stop-race supervision");
        let context = SupervisionRuntimeContext {
            manager: manager.clone(),
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
            coordinator: coordinator.clone(),
        };
        let supervision_task = tokio::spawn(run_supervision(
            context,
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: vec!["RACE_TEST_DELAY".to_owned()],
                max_cycles: None,
            },
            stop,
        ));

        wait_for_supervisor_turn_started(&coordinator, &manager, id).await;
        coordinator
            .request_stop(id)
            .await
            .expect("request supervision stop while supervisor is working");
        tokio::time::timeout(Duration::from_secs(12), supervision_task)
            .await
            .expect("user-stop-race supervision deadline")
            .expect("user-stop-race supervision join");
        let snapshot = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("user-stop-race terminal snapshot");
        assert_eq!(snapshot.status, SupervisionStatus::Stopped, "{snapshot:?}");
        assert_eq!(
            snapshot.cycles, 0,
            "a cancelled supervisor turn must not be counted as a completed decision"
        );
        assert!(
            !fixture.root.join("workflow-worker-prompts.log").exists(),
            "explicit supervision Stop must prevent pending autonomous worker directives"
        );

        manager
            .shutdown()
            .await
            .expect("shutdown user-stop-race manager");
    }

    #[tokio::test]
    async fn supervisor_stop_path_terminates_an_idle_target_run() {
        let fixture = WorkflowFakePiFixture::new("supervision-stop-target");
        let environment = fixture.environment();
        let limits = RuntimeLimits {
            max_live_runs: 2,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("supervision stop runtime manager");
        let project = ProjectBinding::register(&fixture.root).expect("register stop project");
        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch.resolve().expect("resolve stop worker launch"),
                environment,
            })
            .await
            .expect("start stop worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("stop worker ready");

        terminate_supervised_run(&manager, worker)
            .await
            .expect("supervisor stop target");
        let hydration = manager.hydrate().await.expect("stop target hydration");
        let stopped = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == worker)
            .expect("stopped worker remains registered");
        assert!(stopped.run.process_state().is_terminal());

        manager.shutdown().await.expect("shutdown stop manager");
    }

    #[tokio::test]
    async fn supervision_directs_idle_runs_across_multiple_projects_and_leaves_them_alive() {
        let fixture = WorkflowFakePiFixture::new("supervision-integration");
        let second_root = fixture.root.join("second-project");
        fs::create_dir_all(&second_root).expect("create second supervised project");
        fs::write(second_root.join("seed.txt"), "second supervised project\n")
            .expect("write second project seed");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 4,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("supervision runtime manager");
        let first_project =
            ProjectBinding::register(&fixture.root).expect("register supervision project");
        let second_project =
            ProjectBinding::register(&second_root).expect("register second supervision project");
        let base = inspect_worktree_base(first_project.canonical_root(), &environment, limits)
            .await
            .expect("inspect supervision Git base");

        let mut first_worker_launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            first_project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        first_worker_launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let first_worker = manager
            .start_run(RunStartSpec {
                project_id: first_project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: first_worker_launch
                    .resolve()
                    .expect("resolve first worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start first ordinary worker run");
        let mut second_worker_launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            second_project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        second_worker_launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let second_worker = manager
            .start_run(RunStartSpec {
                project_id: second_project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: second_worker_launch
                    .resolve()
                    .expect("resolve second worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start second ordinary worker run");
        wait_supervisor_ready(&manager, limits, first_worker)
            .await
            .expect("first ordinary worker ready");
        wait_supervisor_ready(&manager, limits, second_worker)
            .await
            .expect("second ordinary worker ready");

        submit_text_prompt(&manager, first_worker, "first initial task")
            .await
            .expect("submit first worker prompt");
        submit_text_prompt(&manager, second_worker, "second initial task")
            .await
            .expect("submit second worker prompt");
        let mut worker_changes = manager.subscribe_state_changes();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let hydration = manager
                    .hydrate()
                    .await
                    .expect("worker completion hydration");
                let first = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == first_worker)
                    .expect("first ordinary worker");
                let second = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == second_worker)
                    .expect("second ordinary worker");
                if run_is_idle_actionable(first)
                    && run_is_idle_actionable(second)
                    && first.run.assistant_message_generation() >= 1
                    && second.run.assistant_message_generation() >= 1
                {
                    break;
                }
                match worker_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("runtime state stream closed before worker completion")
                    }
                }
            }
        })
        .await
        .expect("worker initial task completion deadline");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![first_project.id(), second_project.id()],
                first_project.id(),
                None,
                None,
                None,
                Some(1),
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
                project: first_project.clone(),
                project_ids: HashSet::from([first_project.id(), second_project.id()]),
                environment: environment.clone(),
                base,
                selection,
                prompt_templates: vec!["Improve testing after audits complete".to_owned()],
                max_cycles: Some(1),
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
                if snapshot.status == SupervisionStatus::Running && snapshot.watched_runs == 2 {
                    break;
                }
                match changed.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("supervision change stream closed before worker observation")
                    }
                }
            }
        })
        .await
        .expect("supervision observation deadline");

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
        let decision = snapshot
            .last_decision
            .as_deref()
            .expect("last supervisor decision");
        assert!(decision.contains("Sent next task to run"), "{decision}");
        assert!(decision.contains("supervised continuation"), "{decision}");

        let hydration = manager
            .hydrate()
            .await
            .expect("worker hydration after supervision");
        let first = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == first_worker)
            .expect("first worker remains registered");
        let second = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == second_worker)
            .expect("second worker remains registered");
        assert_eq!(first.run.process_state(), ProcessState::Ready);
        assert_eq!(second.run.process_state(), ProcessState::Ready);

        let first_prompts = fs::read_to_string(fixture.root.join("workflow-worker-prompts.log"))
            .expect("read first worker prompt audit");
        let second_prompts = fs::read_to_string(second_root.join("workflow-worker-prompts.log"))
            .expect("read second worker prompt audit");
        assert_eq!(
            first_prompts.lines().collect::<Vec<_>>(),
            ["first initial task", "supervised continuation"],
            "supervisor must continue the idle run in the first project"
        );
        assert_eq!(
            second_prompts.lines().collect::<Vec<_>>(),
            ["second initial task", "supervised continuation"],
            "supervisor must continue the idle run in the second project"
        );
        assert_no_session_stats_probes(&fixture.root);
        assert_no_session_stats_probes(&fixture.worktree_parent());

        manager
            .shutdown()
            .await
            .expect("shutdown supervision manager");
    }

    #[tokio::test]
    async fn continuous_supervision_reacts_to_one_new_result_then_noop_does_not_self_wake() {
        let fixture = WorkflowFakePiFixture::new("supervision-continuous-no-poll");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 3,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("continuous supervision manager");
        let project = ProjectBinding::register(&fixture.root).expect("register continuous project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect continuous supervision Git base");

        let mut launch = PiLaunchSpec::new(
            environment.pi_executable().to_path_buf(),
            project.canonical_root(),
            ProjectTrustPolicy::Inherit,
        );
        launch.session = SessionLaunch::NewWithId(PiSessionId::new());
        let worker = manager
            .start_run(RunStartSpec {
                project_id: project.id(),
                execution_isolation: ExecutionIsolation::LocalCheckout,
                worktree: None,
                launch: launch.resolve().expect("resolve continuous worker launch"),
                environment: environment.clone(),
            })
            .await
            .expect("start continuous worker");
        wait_supervisor_ready(&manager, limits, worker)
            .await
            .expect("continuous worker ready");
        submit_text_prompt(&manager, worker, "continuous initial task")
            .await
            .expect("submit continuous initial task");

        let mut runtime_changes = manager.subscribe_state_changes();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let hydration = manager
                    .hydrate()
                    .await
                    .expect("continuous worker hydration");
                let run = hydration
                    .runs
                    .iter()
                    .find(|run| run.run.id() == worker)
                    .expect("continuous worker");
                if run_is_idle_actionable(run) && run.run.assistant_message_generation() >= 1 {
                    break;
                }
                match runtime_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("runtime state stream closed before continuous worker settled")
                    }
                }
            }
        })
        .await
        .expect("continuous worker initial settlement deadline");

        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                None,
            ))
            .await
            .expect("insert continuous supervision");
        let mut supervision_changes = coordinator.subscribe();
        let context = SupervisionRuntimeContext {
            manager: manager.clone(),
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
            coordinator: coordinator.clone(),
        };
        let supervision_task = tokio::spawn(run_supervision(
            context,
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: Vec::new(),
                max_cycles: None,
            },
            stop,
        ));

        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let snapshots = coordinator.snapshots().await;
                let snapshot = snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .expect("continuous supervision snapshot");
                if snapshot.cycles == 2
                    && snapshot.last_decision.as_deref().is_some_and(|decision| {
                        decision.contains("No intervention for 1 newly idle run")
                    })
                {
                    break;
                }
                match supervision_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("supervision stream closed before deliberate no-op cycle")
                    }
                }
            }
        })
        .await
        .expect("continuous supervision second-cycle deadline");

        let state_probe = manager
            .request(worker, RpcRequest::new(RpcCommand::GetState))
            .await
            .expect("inject unrelated runtime wake");
        assert!(state_probe.response.success);
        tokio::time::sleep(Duration::from_millis(150)).await;
        let after_wake = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("continuous snapshot after unrelated wake");
        assert_eq!(
            after_wake.cycles, 2,
            "unchanged assistant generation must not retrigger a no-op supervision cycle"
        );
        assert_eq!(after_wake.status, SupervisionStatus::Running);

        coordinator
            .request_stop(id)
            .await
            .expect("stop continuous supervision");
        tokio::time::timeout(Duration::from_secs(8), supervision_task)
            .await
            .expect("continuous supervision stop deadline")
            .expect("continuous supervision task join");
        let stopped = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("stopped continuous supervision snapshot");
        assert_eq!(stopped.status, SupervisionStatus::Stopped, "{stopped:?}");
        assert_eq!(stopped.cycles, 2);

        let prompts = fs::read_to_string(fixture.root.join("workflow-worker-prompts.log"))
            .expect("read continuous worker prompts");
        assert_eq!(
            prompts.lines().collect::<Vec<_>>(),
            ["continuous initial task", "supervised continuation"],
            "the deliberate no-op cycle must not inject another worker prompt"
        );
        assert_no_session_stats_probes(&fixture.root);
        assert_no_session_stats_probes(&fixture.worktree_parent());

        manager
            .shutdown()
            .await
            .expect("shutdown continuous supervision manager");
    }

    #[tokio::test]
    async fn stop_requested_before_supervisor_launch_is_stopped_not_failed() {
        let fixture = WorkflowFakePiFixture::new("supervision-stop-before-launch");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 2,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("prelaunch-stop runtime manager");
        let project =
            ProjectBinding::register(&fixture.root).expect("register prelaunch-stop project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect prelaunch-stop Git base");
        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                None,
            ))
            .await
            .expect("insert prelaunch-stop supervision");
        coordinator
            .request_stop(id)
            .await
            .expect("request stop before supervisor launch");

        run_supervision(
            SupervisionRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
                coordinator: coordinator.clone(),
            },
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: Vec::new(),
                max_cycles: None,
            },
            stop,
        )
        .await;

        let snapshot = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("prelaunch-stop snapshot");
        assert_eq!(snapshot.status, SupervisionStatus::Stopped, "{snapshot:?}");
        assert!(snapshot.supervisor_run_id.is_none());
        assert!(snapshot.error.is_none());
        assert_eq!(
            manager
                .capacity()
                .await
                .expect("prelaunch-stop capacity")
                .active_runs,
            0
        );

        manager
            .shutdown()
            .await
            .expect("shutdown prelaunch-stop manager");
    }

    #[tokio::test]
    async fn stop_during_supervisor_readiness_terminates_exact_starting_run() {
        let fixture = WorkflowFakePiFixture::new("supervision-stop-during-startup");
        fs::write(
            fixture.root.join("workflow-delay-supervisor-startup"),
            "delay supervisor get_state\n",
        )
        .expect("write delayed supervisor startup marker");
        let environment = fixture.initialize_git_repository();
        let limits = RuntimeLimits {
            max_live_runs: 2,
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let manager = spawn_runtime_manager(limits).expect("startup-stop runtime manager");
        let project =
            ProjectBinding::register(&fixture.root).expect("register startup-stop project");
        let base = inspect_worktree_base(project.canonical_root(), &environment, limits)
            .await
            .expect("inspect startup-stop Git base");
        let coordinator = SupervisionCoordinator::new(limits);
        let id = SupervisionId::new();
        let stop = coordinator
            .insert(SupervisionSnapshot::new(
                id,
                vec![project.id()],
                project.id(),
                None,
                None,
                None,
                None,
            ))
            .await
            .expect("insert startup-stop supervision");
        let mut changed = coordinator.subscribe();
        let mut runtime_changes = manager.subscribe_state_changes();
        let supervision_task = tokio::spawn(run_supervision(
            SupervisionRuntimeContext {
                manager: manager.clone(),
                limits,
                launch_cleanup_gate: Arc::new(Mutex::new(())),
                worktrees: Arc::new(Mutex::new(WorktreeRegistry::ephemeral(limits))),
                coordinator: coordinator.clone(),
            },
            SupervisionPlan {
                id,
                project: project.clone(),
                project_ids: HashSet::from([project.id()]),
                environment,
                base,
                selection: LaunchSelection {
                    context_files: ContextFilesPolicy::Disabled,
                    extension_discovery: ExtensionDiscoveryPolicy::Disabled,
                    provider: None,
                    model: None,
                    thinking: None,
                },
                prompt_templates: Vec::new(),
                max_cycles: None,
            },
            stop,
        ));

        let supervisor_run = tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let hydration = manager
                    .hydrate()
                    .await
                    .expect("startup-stop hydration while launching");
                if let Some(run) = hydration
                    .runs
                    .iter()
                    .find(|run| !run.run.process_state().is_terminal())
                {
                    return run.run.id();
                }
                match runtime_changes.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("runtime stream closed before supervisor child was registered")
                    }
                }
            }
        })
        .await
        .expect("supervisor child creation deadline");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = coordinator
                    .snapshots()
                    .await
                    .into_iter()
                    .find(|snapshot| snapshot.id == id)
                    .expect("startup-stop supervision snapshot");
                if snapshot.supervisor_run_id == Some(supervisor_run) {
                    break;
                }
                match changed.recv().await {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("supervision stream closed before starting RunId was recorded")
                    }
                }
            }
        })
        .await
        .expect("starting supervisor ownership-record deadline");

        let hydration = manager
            .hydrate()
            .await
            .expect("startup-stop starting hydration");
        let starting = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == supervisor_run)
            .expect("starting supervisor run");
        assert_eq!(starting.run.process_state(), ProcessState::Starting);

        coordinator
            .request_stop(id)
            .await
            .expect("request stop during supervisor readiness");
        tokio::time::timeout(Duration::from_secs(3), supervision_task)
            .await
            .expect("startup-stop supervision deadline")
            .expect("startup-stop supervision join");

        let snapshot = coordinator
            .snapshots()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("terminal startup-stop snapshot");
        assert_eq!(snapshot.status, SupervisionStatus::Stopped, "{snapshot:?}");
        assert!(snapshot.supervisor_run_id.is_none());
        assert!(snapshot.error.is_none());

        let hydration = manager
            .hydrate()
            .await
            .expect("startup-stop terminal hydration");
        let terminated = hydration
            .runs
            .iter()
            .find(|run| run.run.id() == supervisor_run)
            .expect("terminated supervisor run remains retained");
        assert!(
            terminated.run.process_state().is_terminal(),
            "{terminated:?}"
        );
        assert_eq!(
            manager
                .capacity()
                .await
                .expect("startup-stop final capacity")
                .active_runs,
            0
        );

        manager
            .shutdown()
            .await
            .expect("shutdown startup-stop manager");
    }
}
