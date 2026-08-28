use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bounded::BoundedText;
use crate::launch::ProjectTrustPolicy;
use crate::rpc::{QueueMode, ThinkingLevel};
use crate::worktree::GitWorktreeIdentity;
use crate::{ProjectId, RunId, RuntimeLimits};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionIsolation {
    LocalCheckout,
    GitWorktree,
}

/// Lifecycle of the owned operating-system process only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Starting,
    Ready,
    Stopping,
    Exited,
    Failed,
    /// App has revoked the writer/control path because process termination could
    /// not be confirmed. The OS process may still exist and must not be reused.
    Quarantined,
}

impl ProcessState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Failed | Self::Quarantined)
    }
}

/// Derived agent activity. This is deliberately separate from process lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Idle,
    Working,
    WaitingForInput,
    Aborting,
    Compacting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposerAvailability {
    Ready,
    AgentWorking,
    BlockedByCompaction,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub steering: usize,
    pub follow_up: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunModelState {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
    pub supports_images: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSessionState {
    pub model: Option<RunModelState>,
    pub thinking_level: Option<ThinkingLevel>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub session_file: Option<PathBuf>,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub auto_compaction_enabled: Option<bool>,
    pub message_count: Option<usize>,
    pub pending_message_count: Option<usize>,
}

/// One authoritative `get_state` observation from the current Pi child.
///
/// This is intentionally renderer-independent. It lets initial hydration and
/// reconnect/recovery reconcile the app's live projection with Pi without
/// treating cached UI state as authoritative.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunStateObservation {
    pub model: Option<RunModelState>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureKind {
    Spawn,
    Protocol,
    UnexpectedExit,
    Stop,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFailure {
    pub kind: RunFailureKind,
    pub detail: String,
    pub detail_truncated: bool,
}

impl RunFailure {
    #[must_use]
    pub fn bounded(kind: RunFailureKind, detail: &str, max_detail_bytes: usize) -> Self {
        let mut bounded = BoundedText::new(max_detail_bytes);
        bounded.replace(detail);
        Self {
            kind,
            detail: bounded.as_str().to_owned(),
            detail_truncated: bounded.dropped_bytes() > 0,
        }
    }

    #[must_use]
    pub fn from_runtime_limits(kind: RunFailureKind, detail: &str, limits: RuntimeLimits) -> Self {
        Self::bounded(kind, detail, limits.max_failure_detail_bytes)
    }
}

/// Authoritative live state for one app-owned Pi process.
///
/// Fields whose mutation can violate lifecycle invariants are private. Callers
/// apply [`RunMutation`] through [`RuntimeStore`] instead of editing snapshots.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    id: RunId,
    project_id: ProjectId,
    execution_root: PathBuf,
    execution_isolation: ExecutionIsolation,
    worktree: Option<GitWorktreeIdentity>,
    project_trust: ProjectTrustPolicy,
    process: ProcessState,
    agent_working: bool,
    compacting: bool,
    abort_requested: bool,
    pending_ui_requests: BTreeSet<String>,
    queue: QueueState,
    session: RunSessionState,
    exit_code: Option<i32>,
    failure: Option<RunFailure>,
    revision: u64,
}

impl RunRecord {
    pub fn starting(
        id: RunId,
        project_id: ProjectId,
        execution_root: PathBuf,
        execution_isolation: ExecutionIsolation,
        project_trust: ProjectTrustPolicy,
    ) -> Result<Self, RuntimeError> {
        if execution_isolation == ExecutionIsolation::GitWorktree {
            return Err(RuntimeError::MissingWorktreeIdentity);
        }
        Ok(Self::starting_unchecked(
            id,
            project_id,
            execution_root,
            execution_isolation,
            project_trust,
            None,
        ))
    }

    fn starting_unchecked(
        id: RunId,
        project_id: ProjectId,
        execution_root: PathBuf,
        execution_isolation: ExecutionIsolation,
        project_trust: ProjectTrustPolicy,
        worktree: Option<GitWorktreeIdentity>,
    ) -> Self {
        Self {
            id,
            project_id,
            execution_root,
            execution_isolation,
            worktree,
            project_trust,
            process: ProcessState::Starting,
            agent_working: false,
            compacting: false,
            abort_requested: false,
            pending_ui_requests: BTreeSet::new(),
            queue: QueueState::default(),
            session: RunSessionState::default(),
            exit_code: None,
            failure: None,
            revision: 0,
        }
    }

    pub fn starting_with_worktree(
        id: RunId,
        project_id: ProjectId,
        execution_root: PathBuf,
        execution_isolation: ExecutionIsolation,
        project_trust: ProjectTrustPolicy,
        worktree: Option<GitWorktreeIdentity>,
    ) -> Result<Self, RuntimeError> {
        match (execution_isolation, worktree.as_ref()) {
            (ExecutionIsolation::LocalCheckout, Some(_)) => {
                return Err(RuntimeError::UnexpectedWorktreeIdentity);
            }
            (ExecutionIsolation::GitWorktree, None) => {
                return Err(RuntimeError::MissingWorktreeIdentity);
            }
            (ExecutionIsolation::GitWorktree, Some(identity))
                if !path_is_within(&execution_root, &identity.worktree_root) =>
            {
                return Err(RuntimeError::WorktreeExecutionRootMismatch {
                    execution_root,
                    worktree_root: identity.worktree_root.clone(),
                });
            }
            _ => {}
        }

        let record = Self::starting_unchecked(
            id,
            project_id,
            execution_root,
            execution_isolation,
            project_trust,
            worktree,
        );
        Ok(record)
    }

    #[must_use]
    pub const fn id(&self) -> RunId {
        self.id
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    #[must_use]
    pub fn execution_root(&self) -> &PathBuf {
        &self.execution_root
    }

    #[must_use]
    pub const fn worktree_identity(&self) -> Option<&GitWorktreeIdentity> {
        self.worktree.as_ref()
    }

    #[must_use]
    pub const fn process_state(&self) -> ProcessState {
        self.process
    }

    #[must_use]
    pub fn activity_state(&self) -> ActivityState {
        if self.abort_requested {
            ActivityState::Aborting
        } else if self.compacting {
            ActivityState::Compacting
        } else if !self.pending_ui_requests.is_empty() {
            ActivityState::WaitingForInput
        } else if self.agent_working {
            ActivityState::Working
        } else {
            ActivityState::Idle
        }
    }

    #[must_use]
    pub const fn is_compacting(&self) -> bool {
        self.compacting
    }

    #[must_use]
    pub fn composer_availability(&self) -> ComposerAvailability {
        if self.process != ProcessState::Ready {
            ComposerAvailability::Unavailable
        } else if self.compacting {
            ComposerAvailability::BlockedByCompaction
        } else if self.agent_working {
            ComposerAvailability::AgentWorking
        } else {
            ComposerAvailability::Ready
        }
    }

    #[must_use]
    pub const fn queue(&self) -> QueueState {
        self.queue
    }

    #[must_use]
    pub const fn session_state(&self) -> &RunSessionState {
        &self.session
    }

    pub(crate) fn retained_runtime_state_bytes_with_name(&self, name: Option<&str>) -> usize {
        self.session
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
                self.session
                    .session_file
                    .as_ref()
                    .map_or(0, |path| path.as_os_str().to_string_lossy().len()),
            )
            .saturating_add(self.session.session_id.as_ref().map_or(0, String::len))
            .saturating_add(name.map_or(0, str::len))
    }

    #[must_use]
    pub fn pending_ui_requests(&self) -> usize {
        self.pending_ui_requests.len()
    }

    #[must_use]
    pub fn has_pending_ui_request(&self, request_id: &str) -> bool {
        self.pending_ui_requests.contains(request_id)
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    fn apply(
        &mut self,
        mutation: RunMutation,
        max_pending_ui_requests: usize,
    ) -> Result<(), RuntimeError> {
        match mutation {
            RunMutation::ProcessReady => {
                self.require_process("process_ready", &[ProcessState::Starting])?;
                self.process = ProcessState::Ready;
            }
            RunMutation::AgentStarted => {
                self.require_process("agent_started", &[ProcessState::Ready])?;
                self.agent_working = true;
                self.abort_requested = false;
            }
            RunMutation::CompactionStarted => {
                self.require_process("compaction_started", &[ProcessState::Ready])?;
                self.compacting = true;
            }
            RunMutation::CompactionEnded => {
                self.require_process("compaction_ended", &[ProcessState::Ready])?;
                self.compacting = false;
            }
            RunMutation::AgentSettled => {
                self.require_process("agent_settled", &[ProcessState::Ready])?;
                self.agent_working = false;
                self.abort_requested = false;
            }
            RunMutation::UiRequestOpened { request_id } => {
                self.require_process("ui_request_opened", &[ProcessState::Ready])?;
                if self.pending_ui_requests.contains(&request_id) {
                    return Err(RuntimeError::DuplicateUiRequest { request_id });
                }
                if self.pending_ui_requests.len() >= max_pending_ui_requests {
                    return Err(RuntimeError::PendingUiLimit {
                        limit: max_pending_ui_requests,
                    });
                }
                self.pending_ui_requests.insert(request_id);
            }
            RunMutation::UiRequestClosed { request_id } => {
                self.require_process("ui_request_closed", &[ProcessState::Ready])?;
                if !self.pending_ui_requests.remove(&request_id) {
                    return Err(RuntimeError::UnknownUiRequest { request_id });
                }
            }
            RunMutation::AbortRequested => {
                self.require_process("abort_requested", &[ProcessState::Ready])?;
                if !self.agent_working {
                    return Err(RuntimeError::AbortWhileIdle);
                }
                self.abort_requested = true;
            }
            RunMutation::QueueChanged(queue) => {
                self.require_process("queue_changed", &[ProcessState::Ready])?;
                self.queue = queue;
            }
            RunMutation::StateObserved(observation) => {
                self.require_process(
                    "state_observed",
                    &[ProcessState::Starting, ProcessState::Ready],
                )?;
                self.agent_working = observation.is_streaming;
                self.compacting = observation.is_compacting;
                if !observation.is_streaming {
                    self.abort_requested = false;
                }
                self.session = RunSessionState {
                    model: observation.model,
                    thinking_level: Some(observation.thinking_level),
                    steering_mode: Some(observation.steering_mode),
                    follow_up_mode: Some(observation.follow_up_mode),
                    session_file: observation.session_file,
                    session_id: Some(observation.session_id),
                    session_name: observation.session_name,
                    auto_compaction_enabled: Some(observation.auto_compaction_enabled),
                    message_count: Some(observation.message_count),
                    pending_message_count: Some(observation.pending_message_count),
                };
            }
            RunMutation::ThinkingLevelChanged(level) => {
                self.require_process("thinking_level_changed", &[ProcessState::Ready])?;
                self.session.thinking_level = Some(level);
            }
            RunMutation::SessionNameChanged(name) => {
                self.require_process("session_name_changed", &[ProcessState::Ready])?;
                self.session.session_name = name;
            }
            RunMutation::BeginStop => {
                self.require_process("begin_stop", &[ProcessState::Starting, ProcessState::Ready])?;
                self.process = ProcessState::Stopping;
                self.abort_requested = self.agent_working;
            }
            RunMutation::ProcessExited { code } => {
                self.require_nonterminal("process_exited")?;
                self.process = ProcessState::Exited;
                self.exit_code = code;
                self.clear_hot_activity();
            }
            RunMutation::ProcessFailed { failure } => {
                self.require_nonterminal("process_failed")?;
                self.process = ProcessState::Failed;
                self.failure = Some(failure);
                self.clear_hot_activity();
            }
            RunMutation::ProcessQuarantined { failure } => {
                self.require_process("process_quarantined", &[ProcessState::Stopping])?;
                self.process = ProcessState::Quarantined;
                self.failure = Some(failure);
                self.clear_hot_activity();
            }
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    fn clear_hot_activity(&mut self) {
        self.agent_working = false;
        self.compacting = false;
        self.abort_requested = false;
        self.pending_ui_requests.clear();
        self.queue = QueueState::default();
    }

    fn require_nonterminal(&self, mutation: &'static str) -> Result<(), RuntimeError> {
        if self.process.is_terminal() {
            return Err(RuntimeError::InvalidProcessState {
                mutation,
                actual: self.process,
            });
        }
        Ok(())
    }

    fn require_process(
        &self,
        mutation: &'static str,
        allowed: &[ProcessState],
    ) -> Result<(), RuntimeError> {
        if allowed.contains(&self.process) {
            Ok(())
        } else {
            Err(RuntimeError::InvalidProcessState {
                mutation,
                actual: self.process,
            })
        }
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(path), Ok(root)) => path.starts_with(root),
        _ => path.starts_with(root),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunMutation {
    ProcessReady,
    AgentStarted,
    AgentSettled,
    CompactionStarted,
    CompactionEnded,
    UiRequestOpened { request_id: String },
    UiRequestClosed { request_id: String },
    AbortRequested,
    QueueChanged(QueueState),
    StateObserved(RunStateObservation),
    ThinkingLevelChanged(ThinkingLevel),
    SessionNameChanged(Option<String>),
    BeginStop,
    ProcessExited { code: Option<i32> },
    ProcessFailed { failure: RunFailure },
    ProcessQuarantined { failure: RunFailure },
}

#[derive(Debug)]
pub struct RuntimeStore {
    runs: HashMap<RunId, RunRecord>,
    max_pending_ui_requests_per_run: usize,
    revision: u64,
}

impl RuntimeStore {
    #[must_use]
    pub fn new(limits: RuntimeLimits) -> Self {
        Self {
            runs: HashMap::new(),
            max_pending_ui_requests_per_run: limits.max_pending_ui_requests_per_run,
            revision: 0,
        }
    }

    pub fn register(&mut self, record: RunRecord) -> Result<(), RuntimeError> {
        let id = record.id();
        if self.runs.contains_key(&id) {
            return Err(RuntimeError::DuplicateRun { run_id: id });
        }
        self.runs.insert(id, record);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn apply(&mut self, run_id: RunId, mutation: RunMutation) -> Result<(), RuntimeError> {
        let record = self
            .runs
            .get_mut(&run_id)
            .ok_or(RuntimeError::UnknownRun { run_id })?;
        record.apply(mutation, self.max_pending_ui_requests_per_run)?;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, run_id: RunId) -> Option<&RunRecord> {
        self.runs.get(&run_id)
    }

    pub fn records(&self) -> impl Iterator<Item = &RunRecord> {
        self.runs.values()
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn remove_terminal(&mut self, run_id: RunId) -> Result<RunRecord, RuntimeError> {
        let record = self
            .runs
            .get(&run_id)
            .ok_or(RuntimeError::UnknownRun { run_id })?;
        if !record.process_state().is_terminal() {
            return Err(RuntimeError::RemoveLiveRun { run_id });
        }
        let removed = self
            .runs
            .remove(&run_id)
            .ok_or(RuntimeError::UnknownRun { run_id })?;
        self.revision = self.revision.saturating_add(1);
        Ok(removed)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RuntimeError {
    #[error("run {run_id} is already registered")]
    DuplicateRun { run_id: RunId },
    #[error("run {run_id} is not registered")]
    UnknownRun { run_id: RunId },
    #[error("mutation {mutation} is invalid while process is {actual:?}")]
    InvalidProcessState {
        mutation: &'static str,
        actual: ProcessState,
    },
    #[error("pending extension UI request limit {limit} reached")]
    PendingUiLimit { limit: usize },
    #[error("extension UI request {request_id} is already pending")]
    DuplicateUiRequest { request_id: String },
    #[error("extension UI request {request_id} is not pending")]
    UnknownUiRequest { request_id: String },
    #[error("cannot request abort while the agent is idle")]
    AbortWhileIdle,
    #[error("cannot remove live run {run_id}")]
    RemoveLiveRun { run_id: RunId },
    #[error("local-checkout run cannot retain Git worktree identity")]
    UnexpectedWorktreeIdentity,
    #[error("Git-worktree run requires immutable worktree identity")]
    MissingWorktreeIdentity,
    #[error("run execution root {execution_root} is not inside worktree root {worktree_root}")]
    WorktreeExecutionRootMismatch {
        execution_root: PathBuf,
        worktree_root: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: RunId) -> RunRecord {
        RunRecord::starting(
            id,
            ProjectId::new(),
            PathBuf::from("project"),
            ExecutionIsolation::LocalCheckout,
            ProjectTrustPolicy::Ignore,
        )
        .expect("local run")
    }

    #[test]
    fn compaction_blocks_all_composer_submission_without_faking_process_stop() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store
            .apply(id, RunMutation::CompactionStarted)
            .expect("compaction starts");

        let run = store.get(id).expect("run");
        assert_eq!(run.process_state(), ProcessState::Ready);
        assert_eq!(run.activity_state(), ActivityState::Compacting);
        assert_eq!(
            run.composer_availability(),
            ComposerAvailability::BlockedByCompaction
        );

        store
            .apply(id, RunMutation::CompactionEnded)
            .expect("compaction ends");
        assert_eq!(
            store.get(id).expect("run").composer_availability(),
            ComposerAvailability::Ready
        );
    }

    #[test]
    fn get_state_observation_rehydrates_live_activity_and_session_identity() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store
            .apply(
                id,
                RunMutation::StateObserved(RunStateObservation {
                    model: Some(RunModelState {
                        provider: "openai".to_owned(),
                        id: "gpt-5.6".to_owned(),
                        name: Some("GPT-5.6".to_owned()),
                        supports_images: Some(true),
                    }),
                    thinking_level: ThinkingLevel::Xhigh,
                    is_streaming: true,
                    is_compacting: false,
                    steering_mode: QueueMode::OneAtATime,
                    follow_up_mode: QueueMode::All,
                    session_file: Some(PathBuf::from("session.jsonl")),
                    session_id: "session-1".to_owned(),
                    session_name: Some("feature".to_owned()),
                    auto_compaction_enabled: true,
                    message_count: 12,
                    pending_message_count: 2,
                }),
            )
            .expect("observe state");

        let run = store.get(id).expect("run");
        assert_eq!(run.activity_state(), ActivityState::Working);
        assert_eq!(
            run.session_state().model,
            Some(RunModelState {
                provider: "openai".to_owned(),
                id: "gpt-5.6".to_owned(),
                name: Some("GPT-5.6".to_owned()),
                supports_images: Some(true),
            })
        );
        assert_eq!(
            run.session_state().thinking_level,
            Some(ThinkingLevel::Xhigh)
        );
        assert_eq!(run.session_state().session_id.as_deref(), Some("session-1"));
        assert_eq!(run.session_state().pending_message_count, Some(2));

        store
            .apply(
                id,
                RunMutation::StateObserved(RunStateObservation {
                    model: None,
                    thinking_level: ThinkingLevel::Medium,
                    is_streaming: false,
                    is_compacting: true,
                    steering_mode: QueueMode::All,
                    follow_up_mode: QueueMode::OneAtATime,
                    session_file: None,
                    session_id: "session-1".to_owned(),
                    session_name: None,
                    auto_compaction_enabled: false,
                    message_count: 13,
                    pending_message_count: 0,
                }),
            )
            .expect("reconcile state");
        assert_eq!(
            store.get(id).expect("run").activity_state(),
            ActivityState::Compacting
        );
    }

    #[test]
    fn startup_state_observation_populates_identity_without_prematurely_marking_process_ready() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register starting run");
        store
            .apply(
                id,
                RunMutation::StateObserved(RunStateObservation {
                    model: None,
                    thinking_level: ThinkingLevel::Medium,
                    is_streaming: false,
                    is_compacting: false,
                    steering_mode: QueueMode::All,
                    follow_up_mode: QueueMode::OneAtATime,
                    session_file: None,
                    session_id: "startup-session".to_owned(),
                    session_name: None,
                    auto_compaction_enabled: true,
                    message_count: 0,
                    pending_message_count: 0,
                }),
            )
            .expect("startup state observation");

        let starting = store.get(id).expect("run");
        assert_eq!(starting.process_state(), ProcessState::Starting);
        assert_eq!(
            starting.session_state().session_id.as_deref(),
            Some("startup-session")
        );
        assert_eq!(
            starting.composer_availability(),
            ComposerAvailability::Unavailable
        );

        store
            .apply(id, RunMutation::ProcessReady)
            .expect("handshake completes readiness");
        assert_eq!(
            store.get(id).expect("run").process_state(),
            ProcessState::Ready
        );
    }

    #[test]
    fn session_metadata_events_update_only_their_owned_fields() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store
            .apply(id, RunMutation::ThinkingLevelChanged(ThinkingLevel::High))
            .expect("thinking change");
        store
            .apply(
                id,
                RunMutation::SessionNameChanged(Some("renamed".to_owned())),
            )
            .expect("name change");

        let session = store.get(id).expect("run").session_state();
        assert_eq!(session.thinking_level, Some(ThinkingLevel::High));
        assert_eq!(session.session_name.as_deref(), Some("renamed"));
        assert!(session.model.is_none());
    }

    #[test]
    fn logical_project_identity_is_independent_from_execution_root() {
        let run_id = RunId::new();
        let project_id = ProjectId::new();
        let identity = GitWorktreeIdentity {
            repository_root: PathBuf::from("repo"),
            worktree_root: PathBuf::from("project-worktree"),
            branch: "agent/feature".to_owned(),
            base_commit: "abc123".to_owned(),
        };
        let run = RunRecord::starting_with_worktree(
            run_id,
            project_id,
            PathBuf::from("project-worktree"),
            ExecutionIsolation::GitWorktree,
            ProjectTrustPolicy::Inherit,
            Some(identity.clone()),
        )
        .expect("worktree run");

        assert_eq!(run.id(), run_id);
        assert_eq!(run.project_id(), project_id);
        assert_eq!(run.execution_root(), &PathBuf::from("project-worktree"));
        assert_eq!(run.worktree_identity(), Some(&identity));
    }

    #[test]
    fn worktree_identity_is_required_and_must_contain_execution_root() {
        let id = RunId::new();
        assert_eq!(
            RunRecord::starting(
                id,
                ProjectId::new(),
                PathBuf::from("worktree"),
                ExecutionIsolation::GitWorktree,
                ProjectTrustPolicy::Inherit,
            ),
            Err(RuntimeError::MissingWorktreeIdentity)
        );

        let identity = GitWorktreeIdentity {
            repository_root: PathBuf::from("repo"),
            worktree_root: PathBuf::from("other-worktree"),
            branch: "agent/feature".to_owned(),
            base_commit: "abc123".to_owned(),
        };
        assert!(matches!(
            RunRecord::starting_with_worktree(
                id,
                ProjectId::new(),
                PathBuf::from("worktree/project"),
                ExecutionIsolation::GitWorktree,
                ProjectTrustPolicy::Inherit,
                Some(identity),
            ),
            Err(RuntimeError::WorktreeExecutionRootMismatch { .. })
        ));
    }

    #[test]
    fn agent_settled_returns_to_idle_without_ending_process() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store.apply(id, RunMutation::AgentStarted).expect("start");
        store.apply(id, RunMutation::AgentSettled).expect("settle");

        let run = store.get(id).expect("run remains registered");
        assert_eq!(run.process_state(), ProcessState::Ready);
        assert_eq!(run.activity_state(), ActivityState::Idle);

        store
            .apply(id, RunMutation::AgentStarted)
            .expect("same process can run another prompt");
        assert_eq!(
            store.get(id).expect("run").activity_state(),
            ActivityState::Working
        );
    }

    #[test]
    fn abort_requires_active_agent_work() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");

        assert_eq!(
            store.apply(id, RunMutation::AbortRequested),
            Err(RuntimeError::AbortWhileIdle)
        );
    }

    #[test]
    fn pending_ui_requests_are_bounded_and_drive_attention_state() {
        let id = RunId::new();
        let limits = RuntimeLimits {
            max_pending_ui_requests_per_run: 1,
            ..RuntimeLimits::default()
        };
        let mut store = RuntimeStore::new(limits);
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store
            .apply(
                id,
                RunMutation::UiRequestOpened {
                    request_id: "request-a".to_owned(),
                },
            )
            .expect("first request");

        assert_eq!(
            store.get(id).expect("run").activity_state(),
            ActivityState::WaitingForInput
        );
        assert_eq!(
            store.apply(
                id,
                RunMutation::UiRequestOpened {
                    request_id: "request-b".to_owned(),
                },
            ),
            Err(RuntimeError::PendingUiLimit { limit: 1 })
        );
        assert!(
            store
                .get(id)
                .expect("run")
                .has_pending_ui_request("request-a")
        );
    }

    #[test]
    fn ui_request_identity_prevents_cross_resolution() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store
            .apply(
                id,
                RunMutation::UiRequestOpened {
                    request_id: "request-a".to_owned(),
                },
            )
            .expect("open request");

        assert_eq!(
            store.apply(
                id,
                RunMutation::UiRequestClosed {
                    request_id: "request-b".to_owned(),
                },
            ),
            Err(RuntimeError::UnknownUiRequest {
                request_id: "request-b".to_owned(),
            })
        );
        assert!(
            store
                .get(id)
                .expect("run")
                .has_pending_ui_request("request-a")
        );
    }

    #[test]
    fn terminal_process_rejects_further_activity_mutation() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store
            .apply(id, RunMutation::ProcessExited { code: Some(0) })
            .expect("exit");

        assert_eq!(
            store.apply(id, RunMutation::AgentStarted),
            Err(RuntimeError::InvalidProcessState {
                mutation: "agent_started",
                actual: ProcessState::Exited,
            })
        );
    }

    #[test]
    fn live_runs_cannot_be_removed_from_authoritative_store() {
        let id = RunId::new();
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store.register(record(id)).expect("register");

        assert_eq!(
            store.remove_terminal(id),
            Err(RuntimeError::RemoveLiveRun { run_id: id })
        );
    }

    #[test]
    fn uncertain_stop_is_quarantined_and_cannot_accept_more_activity() {
        let id = RunId::new();
        let limits = RuntimeLimits::default();
        let mut store = RuntimeStore::new(limits);
        store.register(record(id)).expect("register");
        store.apply(id, RunMutation::ProcessReady).expect("ready");
        store.apply(id, RunMutation::BeginStop).expect("begin stop");
        store
            .apply(
                id,
                RunMutation::ProcessQuarantined {
                    failure: RunFailure::from_runtime_limits(
                        RunFailureKind::Stop,
                        "termination deadline expired",
                        limits,
                    ),
                },
            )
            .expect("quarantine");

        assert_eq!(
            store.get(id).expect("run").process_state(),
            ProcessState::Quarantined
        );
        assert_eq!(
            store.apply(id, RunMutation::AgentStarted),
            Err(RuntimeError::InvalidProcessState {
                mutation: "agent_started",
                actual: ProcessState::Quarantined,
            })
        );
        store
            .remove_terminal(id)
            .expect("quarantined run is removable");
    }
}
