use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

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
use pi_wizard_core::model_catalog::ModelCatalogStore;
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
use pi_wizard_core::{DraftImageId, PiSessionId, ProjectId, RunId, RuntimeLimits, WorktreeId};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{Mutex, broadcast};
use tokio::task::{AbortHandle, JoinHandle};

use crate::services::automation::{AutomationChangedSignal, AutomationCoordinator};
use crate::services::supervision::SupervisionCoordinator;

const RUNTIME_DIRTY_EVENT: &str = "runtime://dirty";
const RUNTIME_REHYDRATE_EVENT: &str = "runtime://rehydrate";
const AUTOMATION_CHANGED_EVENT: &str = "automation://changed";
const SUPERVISION_CHANGED_EVENT: &str = "supervision://changed";
const PORTABLE_STATE_DIRECTORY: &str = "pi-wizard-data";
const PORTABLE_STATE_MIGRATION_DIRECTORY: &str = "pi-wizard-data.migrating";

fn portable_state_root(executable: &Path) -> Result<PathBuf, io::Error> {
    let executable_dir = executable
        .parent()
        .ok_or_else(|| io::Error::other("Pi Wizard executable has no parent directory"))?;
    let build_profile = executable_dir.file_name().and_then(|name| name.to_str());
    let target_dir = executable_dir.parent();
    let project_root = target_dir.and_then(Path::parent);
    if matches!(build_profile, Some("debug" | "release"))
        && target_dir
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("target")
        && project_root.is_some_and(|root| {
            root.join("Cargo.toml").is_file() && root.join("src-tauri").is_dir()
        })
    {
        return Ok(project_root
            .expect("checked project root")
            .join(PORTABLE_STATE_DIRECTORY));
    }
    Ok(executable_dir.join(PORTABLE_STATE_DIRECTORY))
}

fn copy_state_tree(source: &Path, destination: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_state_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        } else {
            return Err(io::Error::other(format!(
                "portable state migration refuses non-file entry {}",
                source_path.display()
            )));
        }
    }
    Ok(())
}

fn prepare_portable_state_root(
    executable: &Path,
    legacy_state_root: Option<&Path>,
) -> Result<PathBuf, io::Error> {
    let state_root = portable_state_root(executable)?;
    if state_root.exists() {
        fs::create_dir_all(&state_root)?;
        return Ok(state_root);
    }

    let old_executable_sibling = executable
        .parent()
        .map(|parent| parent.join(PORTABLE_STATE_DIRECTORY));
    let migration_source = old_executable_sibling
        .as_deref()
        .filter(|path| *path != state_root && path.is_dir())
        .or_else(|| legacy_state_root.filter(|path| path.is_dir()));
    let Some(migration_source) = migration_source else {
        fs::create_dir_all(&state_root)?;
        return Ok(state_root);
    };

    if fs::rename(migration_source, &state_root).is_ok() {
        return Ok(state_root);
    }

    let parent = state_root
        .parent()
        .ok_or_else(|| io::Error::other("portable state root has no parent directory"))?;
    let migration_root = parent.join(PORTABLE_STATE_MIGRATION_DIRECTORY);
    if migration_root.exists() {
        fs::remove_dir_all(&migration_root)?;
    }
    if let Err(error) = copy_state_tree(migration_source, &migration_root) {
        let _ = fs::remove_dir_all(&migration_root);
        return Err(error);
    }
    fs::rename(&migration_root, &state_root)?;
    let _ = fs::remove_dir_all(migration_source);
    Ok(state_root)
}

pub(crate) struct DesktopRuntime {
    pub(crate) manager: RuntimeManagerHandle,
    pub(crate) limits: RuntimeLimits,
    pub(crate) launch_cleanup_gate: Arc<Mutex<()>>,
    launch_profile: Mutex<Option<DesktopLaunchProfile>>,
    pub(crate) preferences: Mutex<PreferencesStore>,
    projects: Mutex<ProjectRegistry>,
    pub(crate) worktrees: Arc<Mutex<WorktreeRegistry>>,
    pub(crate) automation: AutomationCoordinator,
    pub(crate) supervision: SupervisionCoordinator,
    pub(crate) models: Arc<Mutex<ModelCatalogStore>>,
    git_review_jobs: Mutex<GitReviewJobRegistry>,
    session_catalog_jobs: AtomicUsize,
}

struct ActiveJobGuard<'a> {
    counter: &'a AtomicUsize,
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
pub(crate) struct DesktopRuntimeCapacity {
    pub(crate) active_runs: usize,
    pub(crate) live_run_limit: usize,
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
pub(crate) struct DesktopLaunchProfile {
    pub(crate) environment: ResolvedLaunchEnvironment,
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
        let model_store = match state_root.as_ref() {
            Some(root) => {
                ModelCatalogStore::open(root, limits).map_err(|error| error.to_string())?
            }
            None => ModelCatalogStore::ephemeral(limits),
        };
        Ok(Self {
            manager,
            limits,
            launch_cleanup_gate: Arc::new(Mutex::new(())),
            launch_profile: Mutex::new(None),
            preferences: Mutex::new(preferences),
            projects: Mutex::new(projects),
            worktrees: Arc::new(Mutex::new(worktrees)),
            automation: AutomationCoordinator::new(limits),
            supervision: SupervisionCoordinator::new(limits),
            models: Arc::new(Mutex::new(model_store)),
            git_review_jobs: Mutex::new(GitReviewJobRegistry::new(
                limits
                    .max_live_runs
                    .saturating_add(limits.max_retained_terminal_runs),
            )),
            session_catalog_jobs: AtomicUsize::new(0),
        })
    }

    pub(crate) async fn launch_profile(&self) -> Result<DesktopLaunchProfile, String> {
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
            Err(
                EnvironmentResolutionError::PiNotFoundInAnyEnvironment
                | EnvironmentResolutionError::WindowsCommandWrapperUnavailable { .. },
            ) => {
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
        // Environment resolution is the launch-readiness boundary. `pi --version`
        // is diagnostic metadata only and must never prevent model discovery or
        // a real Pi launch when the resolved invocation itself is usable.
        let profile = DesktopLaunchProfile { environment };
        *cache = Some(profile.clone());
        Ok(profile)
    }

    async fn project_binding(&self, path: PathBuf) -> Result<ProjectBinding, String> {
        let mut projects = self.projects.lock().await;
        projects
            .resolve_or_register(path)
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn registered_project(
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

    pub(crate) async fn capacity_report(&self) -> Result<DesktopRuntimeCapacity, String> {
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

mod desktop_commands;
pub(crate) use desktop_commands::LaunchSelection;
use desktop_commands::{RunControlRequest, WorktreeRecoveryRequest};

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

fn forward_supervision_signals(app: tauri::AppHandle, supervision: SupervisionCoordinator) {
    let mut signals = supervision.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match signals.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = app.emit(SUPERVISION_CHANGED_EVENT, ());
                }
                Err(broadcast::error::RecvError::Closed) => return,
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
    let _process_containment = crate::platform::ProcessContainment::establish()
        .unwrap_or_else(|error| panic!("failed to establish desktop process containment: {error}"));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let legacy_state_root = app
                .path()
                .app_data_dir()
                .map_err(|error| io::Error::other(error.to_string()))?
                .join("runtime-state");
            let executable = std::env::current_exe()?;
            let state_root = prepare_portable_state_root(&executable, Some(&legacy_state_root))?;
            let runtime =
                DesktopRuntime::new_with_state_root(Some(state_root)).map_err(io::Error::other)?;
            let manager = runtime.manager.clone();
            let automation = runtime.automation.clone();
            let supervision = runtime.supervision.clone();
            app.manage(runtime);
            forward_runtime_signals(app.handle().clone(), manager);
            forward_automation_signals(app.handle().clone(), automation);
            forward_supervision_signals(app.handle().clone(), supervision);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_commands::runtime_backend_ready,
            desktop_commands::runtime_hydrate,
            runtime_recover_ui,
            desktop_commands::runtime_drain,
            desktop_commands::runtime_edit_draft,
            desktop_commands::runtime_attach_draft_image,
            desktop_commands::runtime_remove_draft_image,
            desktop_commands::runtime_attachment_limits,
            desktop_commands::runtime_capacity,
            runtime_diagnostics,
            crate::commands::automation::runtime_automation_snapshot,
            crate::commands::automation::runtime_automation_executions,
            crate::commands::automation::runtime_save_automation_chain,
            crate::commands::automation::runtime_delete_automation_chain,
            crate::commands::automation::runtime_start_automation,
            crate::commands::automation::runtime_cancel_automation,
            crate::commands::supervision::runtime_supervision_snapshot,
            crate::commands::supervision::runtime_start_supervision,
            crate::commands::supervision::runtime_stop_supervision,
            crate::commands::models::runtime_model_catalog,
            crate::commands::models::runtime_model_preferences,
            crate::commands::models::runtime_set_new_run_model_preference,
            crate::commands::models::runtime_set_model_favorite,
            crate::commands::models::runtime_save_custom_model,
            crate::commands::models::runtime_delete_custom_model,
            desktop_commands::runtime_set_live_run_limit,
            desktop_commands::runtime_submit_draft,
            desktop_commands::runtime_set_model,
            desktop_commands::runtime_set_thinking_level,
            desktop_commands::runtime_set_auto_compaction,
            desktop_commands::runtime_set_auto_retry,
            desktop_commands::runtime_run_bash,
            desktop_commands::runtime_abort_bash,
            runtime_session_stats,
            runtime_session_tree,
            runtime_compact_session,
            desktop_commands::runtime_set_session_name,
            desktop_commands::runtime_export_session_html,
            desktop_commands::runtime_clone_session,
            desktop_commands::runtime_fork_session,
            desktop_commands::runtime_pick_directory,
            desktop_commands::runtime_list_projects,
            desktop_commands::runtime_relocate_project,
            desktop_commands::runtime_remove_project,
            desktop_commands::runtime_probe_project_resources,
            desktop_commands::runtime_probe_project_models,
            desktop_commands::runtime_probe_project_launch_options,
            desktop_commands::runtime_start_project,
            desktop_commands::runtime_probe_project_worktree,
            desktop_commands::runtime_start_project_worktree,
            desktop_commands::runtime_list_worktree_recoveries,
            desktop_commands::runtime_reconcile_worktree_recovery,
            runtime_cleanup_worktree_recovery,
            desktop_commands::runtime_start_recovered_worktree,
            desktop_commands::runtime_list_project_sessions,
            desktop_commands::runtime_resume_project_session,
            desktop_commands::runtime_read_session_history,
            desktop_commands::runtime_git_review_summary,
            desktop_commands::runtime_git_review_file,
            desktop_commands::runtime_git_review_file_page,
            desktop_commands::runtime_cancel_git_review,
            desktop_commands::runtime_stop,
            runtime_close,
            runtime_dismiss_terminal_run,
            desktop_commands::runtime_open_run_folder,
            desktop_commands::runtime_respond_extension_ui,
            desktop_commands::probe_pi_environment
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
mod portable_state_tests {
    use super::*;

    #[test]
    fn portable_state_root_is_sibling_of_the_executable() {
        let executable = PathBuf::from(r"C:\apps\pi-wizard\pi-wizard-desktop.exe");
        assert_eq!(
            portable_state_root(&executable).expect("portable state root"),
            PathBuf::from(r"C:\apps\pi-wizard\pi-wizard-data")
        );
    }

    #[test]
    fn repository_build_state_root_lives_outside_target_output() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-portable-repository-root-{}",
            RunId::new()
        ));
        fs::create_dir_all(root.join("src-tauri")).expect("src-tauri fixture");
        fs::create_dir_all(root.join("target").join("release")).expect("release fixture");
        fs::write(root.join("Cargo.toml"), b"[workspace]\n").expect("workspace manifest");
        let executable = root
            .join("target")
            .join("release")
            .join("pi-wizard-desktop.exe");
        assert_eq!(
            portable_state_root(&executable).expect("repository portable root"),
            root.join(PORTABLE_STATE_DIRECTORY)
        );
        fs::remove_dir_all(root).expect("repository root fixture cleanup");
    }

    #[test]
    fn repository_build_migrates_old_target_state_before_legacy_app_data() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-portable-target-migration-{}",
            RunId::new()
        ));
        let release = root.join("target").join("release");
        let executable = release.join("pi-wizard-desktop.exe");
        let old_target_state = release.join(PORTABLE_STATE_DIRECTORY);
        let legacy = root.join("legacy-runtime-state");
        fs::create_dir_all(root.join("src-tauri")).expect("src-tauri fixture");
        fs::create_dir_all(&old_target_state).expect("old target state");
        fs::create_dir_all(&legacy).expect("legacy state");
        fs::write(root.join("Cargo.toml"), b"[workspace]\n").expect("workspace manifest");
        fs::write(
            old_target_state.join("model-profiles.json"),
            b"newer-target",
        )
        .expect("old target models");
        fs::write(legacy.join("model-profiles.json"), b"older-app-data").expect("legacy models");

        let portable =
            prepare_portable_state_root(&executable, Some(&legacy)).expect("migrate target state");
        assert_eq!(portable, root.join(PORTABLE_STATE_DIRECTORY));
        assert_eq!(
            fs::read(portable.join("model-profiles.json")).expect("migrated models"),
            b"newer-target"
        );
        assert!(
            legacy.is_dir(),
            "unused older AppData source must not overwrite target state"
        );
        fs::remove_dir_all(root).expect("target migration fixture cleanup");
    }

    #[test]
    fn portable_state_migration_preserves_nested_legacy_state_and_never_overwrites_existing_root() {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-portable-state-migration-{}",
            RunId::new()
        ));
        let executable_dir = root.join("app");
        let executable = executable_dir.join("pi-wizard-desktop.exe");
        let legacy = root.join("legacy-runtime-state");
        fs::create_dir_all(legacy.join("drafts")).expect("legacy drafts");
        fs::create_dir_all(&executable_dir).expect("executable directory");
        fs::write(legacy.join("model-profiles.json"), b"models").expect("legacy models");
        fs::write(legacy.join("drafts").join("one.json"), b"draft").expect("legacy draft");

        let portable = prepare_portable_state_root(&executable, Some(&legacy))
            .expect("migrate legacy portable state");
        assert_eq!(portable, executable_dir.join(PORTABLE_STATE_DIRECTORY));
        assert_eq!(
            fs::read(portable.join("model-profiles.json")).expect("migrated models"),
            b"models"
        );
        assert_eq!(
            fs::read(portable.join("drafts").join("one.json")).expect("migrated draft"),
            b"draft"
        );

        fs::write(portable.join("preferences.json"), b"portable-wins")
            .expect("portable preference");
        fs::create_dir_all(&legacy).expect("second legacy root");
        fs::write(legacy.join("preferences.json"), b"legacy-loses")
            .expect("second legacy preference");
        let reopened =
            prepare_portable_state_root(&executable, Some(&legacy)).expect("reuse portable root");
        assert_eq!(reopened, portable);
        assert_eq!(
            fs::read(reopened.join("preferences.json")).expect("portable preference remains"),
            b"portable-wins"
        );
        fs::remove_dir_all(root).expect("portable migration fixture cleanup");
    }
}
