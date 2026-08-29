use super::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PiEnvironmentProbeReport {
    environment: LaunchEnvironmentDiagnostics,
    version: PiVersion,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProbeProjectModelsRequest {
    project_path: PathBuf,
    project_trust: ProjectTrustPolicy,
    #[serde(default)]
    context_files: ContextFilesPolicy,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectModelCatalog {
    models: Vec<ModelSummary>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StartProjectRequest {
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
pub(super) struct StartProjectWorktreeRequest {
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
pub(super) struct StartRunResult {
    run_id: RunId,
    initial_task_submitted: bool,
    initial_task_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopProjectRecord {
    id: ProjectId,
    canonical_root: PathBuf,
    status: &'static str,
    detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RelocateProjectRequest {
    id: ProjectId,
    new_root: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectRequest {
    id: ProjectId,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PickDirectoryRequest {
    default_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProbeProjectWorktreeRequest {
    project_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProbeProjectResourcesRequest {
    project_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorktreeRecoveryRequest {
    pub(super) id: WorktreeId,
}

#[tauri::command]
pub(super) async fn runtime_probe_project_resources(
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
pub(super) struct StartRecoveredWorktreeRequest {
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
pub(super) struct WorktreeRecoveryPage {
    records: Vec<WorktreeRecoveryRecord>,
    truncated: bool,
    recovery_notice: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorktreeRecoveryInspection {
    record: Option<WorktreeRecoveryRecord>,
    probe: WorktreeRecoveryProbe,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListProjectSessionsRequest {
    project_path: PathBuf,
    query: Option<String>,
    cursor: Option<SessionCatalogCursor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProbeProjectLaunchOptionsRequest {
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
pub(super) struct ProjectLaunchOptions {
    current_model: Option<ModelSummary>,
    current_thinking_level: ThinkingLevel,
    thinking_levels: Vec<ThinkingLevel>,
    clear_queue_supported: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct LaunchSelection {
    pub(crate) context_files: ContextFilesPolicy,
    pub(crate) extension_discovery: ExtensionDiscoveryPolicy,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) thinking: Option<ThinkingLevel>,
}

impl LaunchSelection {
    pub(crate) fn validate(
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

    pub(crate) fn apply(&self, spec: &mut PiLaunchSpec) {
        spec.context_files = self.context_files;
        spec.extension_discovery = self.extension_discovery;
        spec.provider.clone_from(&self.provider);
        spec.model.clone_from(&self.model);
        spec.thinking = self.thinking;
    }
}

pub(super) async fn probe_rpc_response(
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

async fn probe_project_models(
    environment: &ResolvedLaunchEnvironment,
    project: &ProjectBinding,
    project_trust: ProjectTrustPolicy,
    context_files: ContextFilesPolicy,
    limits: RuntimeLimits,
) -> Result<ProjectModelCatalog, String> {
    let selection = LaunchSelection::validate(
        context_files,
        ExtensionDiscoveryPolicy::Disabled,
        None,
        None,
        None,
    )?;
    let mut spec = PiLaunchSpec::new(
        environment.pi_executable().to_path_buf(),
        project.canonical_root(),
        project_trust,
    );
    selection.apply(&mut spec);
    spec.session = SessionLaunch::Ephemeral;
    let resolved = spec.resolve().map_err(|error| error.to_string())?;
    let mut process =
        spawn_pi_process(&resolved, environment, limits).map_err(|error| error.to_string())?;
    let rpc_deadline = Duration::from_millis(limits.startup_rpc_deadline_ms);
    let result = probe_rpc_response(&mut process, RpcCommand::GetAvailableModels, rpc_deadline)
        .await
        .and_then(|response| {
            response
                .available_models(limits)
                .map(|models| ProjectModelCatalog { models })
                .map_err(|error| error.to_string())
        });
    let termination = process
        .control
        .terminate(Duration::from_millis(limits.stop_termination_deadline_ms))
        .await;
    match (result, termination) {
        (Ok(catalog), Ok(TerminationOutcome::Exited { .. })) => Ok(catalog),
        (Ok(_), Ok(TerminationOutcome::Unconfirmed { .. })) => Err(
            "Pi model probe completed but its temporary process could not be confirmed terminated"
                .to_owned(),
        ),
        (Ok(_), Err(error)) => Err(format!(
            "Pi model probe completed but cleanup failed: {error}"
        )),
        (Err(error), Ok(TerminationOutcome::Exited { .. })) => Err(error),
        (Err(error), Ok(TerminationOutcome::Unconfirmed { .. })) => Err(format!(
            "{error}; temporary Pi model probe termination could not be confirmed"
        )),
        (Err(error), Err(cleanup)) => {
            Err(format!("{error}; model probe cleanup failed: {cleanup}"))
        }
    }
}

#[tauri::command]
pub(super) async fn runtime_probe_project_models(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: ProbeProjectModelsRequest,
) -> Result<ProjectModelCatalog, String> {
    let profile = runtime.launch_profile().await?;
    let project = runtime.project_binding(request.project_path).await?;
    probe_project_models(
        &profile.environment,
        &project,
        request.project_trust,
        request.context_files,
        runtime.limits,
    )
    .await
}

#[tauri::command]
pub(super) async fn runtime_probe_project_launch_options(
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
pub(super) async fn runtime_list_worktree_recoveries(
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
pub(super) async fn runtime_reconcile_worktree_recovery(
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
pub(super) struct ResumeProjectSessionRequest {
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
pub(super) struct DrainRuntimeRequest {
    run_id: RunId,
    max_events: usize,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunControlRequest {
    pub(super) run_id: RunId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewFileRequest {
    run_id: RunId,
    path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewFilePageRequest {
    run_id: RunId,
    path: PathBuf,
    cursor: Option<GitDiffCursor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EditDraftRequest {
    run_id: RunId,
    text: String,
}

#[tauri::command]
pub(super) async fn runtime_git_review_summary(
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
pub(super) async fn runtime_git_review_file(
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
pub(super) async fn runtime_git_review_file_page(
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
pub(super) async fn runtime_cancel_git_review(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: RunControlRequest,
) -> Result<bool, String> {
    Ok(runtime.cancel_git_review(request.run_id).await)
}

pub(super) async fn finish_git_review_task<T>(
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

pub(super) async fn run_execution_root(
    runtime: &DesktopRuntime,
    run_id: RunId,
) -> Result<PathBuf, String> {
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
pub(super) fn folder_opener(execution_root: &std::path::Path) -> (PathBuf, Vec<OsString>) {
    let executable = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("explorer.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("explorer.exe"));
    (executable, vec![execution_root.as_os_str().to_owned()])
}

#[cfg(target_os = "macos")]
pub(super) fn folder_opener(execution_root: &std::path::Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from("open"),
        vec![execution_root.as_os_str().to_owned()],
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn folder_opener(execution_root: &std::path::Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from("xdg-open"),
        vec![execution_root.as_os_str().to_owned()],
    )
}

#[tauri::command]
pub(super) async fn runtime_open_run_folder(
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
pub(super) struct AttachDraftImageRequest {
    run_id: RunId,
    file_name: String,
    mime_type: String,
    data: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoveDraftImageRequest {
    run_id: RunId,
    image_id: DraftImageId,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeAttachmentLimits {
    max_attachments: usize,
    max_image_bytes: usize,
    max_aggregate_bytes: usize,
    max_name_bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopStartupSnapshot {
    runtime: RuntimeHydrationSnapshot,
    capacity: DesktopRuntimeCapacity,
    attachment_limits: RuntimeAttachmentLimits,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubmitDraftRequest {
    run_id: RunId,
    action: ComposerAction,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetRunModelRequest {
    run_id: RunId,
    provider: String,
    model_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetRunThinkingRequest {
    run_id: RunId,
    level: ThinkingLevel,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetAutoCompactionRequest {
    run_id: RunId,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetAutoRetryRequest {
    run_id: RunId,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetLiveRunLimitRequest {
    limit: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSessionNameRequest {
    run_id: RunId,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ForkSessionRequest {
    run_id: RunId,
    entry_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadSessionHistoryRequest {
    run_id: RunId,
    cursor: Option<SessionHistoryCursor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub(super) enum DesktopExtensionUiResponse {
    Value { id: String, value: String },
    Confirmation { id: String, confirmed: bool },
    Cancelled { id: String },
}

#[tauri::command]
pub(super) async fn runtime_edit_draft(
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
pub(super) async fn runtime_set_auto_retry(
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
pub(super) async fn runtime_set_auto_compaction(
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
pub(super) async fn runtime_attach_draft_image(
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
pub(super) async fn runtime_remove_draft_image(
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
pub(super) fn runtime_attachment_limits(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> RuntimeAttachmentLimits {
    RuntimeAttachmentLimits {
        max_attachments: runtime.limits.max_attachments_per_prompt,
        max_image_bytes: runtime.limits.max_attachment_bytes_per_image,
        max_aggregate_bytes: runtime.limits.max_attachment_bytes_per_prompt,
        max_name_bytes: runtime.limits.max_attachment_name_bytes,
    }
}

async fn desktop_startup_snapshot(
    runtime: &DesktopRuntime,
) -> Result<DesktopStartupSnapshot, String> {
    let hydration = runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())?;
    let capacity = runtime.capacity_report().await?;
    Ok(DesktopStartupSnapshot {
        runtime: hydration,
        capacity,
        attachment_limits: RuntimeAttachmentLimits {
            max_attachments: runtime.limits.max_attachments_per_prompt,
            max_image_bytes: runtime.limits.max_attachment_bytes_per_image,
            max_aggregate_bytes: runtime.limits.max_attachment_bytes_per_prompt,
            max_name_bytes: runtime.limits.max_attachment_name_bytes,
        },
    })
}

#[tauri::command]
pub(super) async fn runtime_backend_ready(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<DesktopStartupSnapshot, String> {
    desktop_startup_snapshot(&runtime).await
}

#[tauri::command]
pub(super) async fn runtime_capacity(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<DesktopRuntimeCapacity, String> {
    runtime.capacity_report().await
}

#[tauri::command]
pub(super) async fn runtime_set_live_run_limit(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SetLiveRunLimitRequest,
) -> Result<DesktopRuntimeCapacity, String> {
    let _gate = runtime.launch_cleanup_gate.lock().await;
    runtime.set_persisted_live_run_limit(request.limit).await
}

#[tauri::command]
pub(super) async fn runtime_fork_session(
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
pub(super) async fn runtime_set_model(
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
pub(super) async fn runtime_set_thinking_level(
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
pub(super) async fn runtime_set_session_name(
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
pub(super) async fn runtime_clone_session(
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
pub(super) async fn runtime_submit_draft(
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
pub(super) struct RespondExtensionUiRequest {
    run_id: RunId,
    response: DesktopExtensionUiResponse,
}

#[tauri::command]
pub(super) async fn runtime_hydrate(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<RuntimeHydrationSnapshot, String> {
    runtime
        .manager
        .hydrate()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) async fn runtime_respond_extension_ui(
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
pub(super) async fn runtime_drain(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: DrainRuntimeRequest,
) -> Result<RuntimeUiDrain, String> {
    runtime
        .manager
        .drain_ui(request.run_id, request.max_events)
        .await
        .map_err(|error| error.to_string())
}

pub(super) fn desktop_project_record(binding: &ProjectBinding) -> DesktopProjectRecord {
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
pub(super) async fn runtime_list_projects(
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
pub(super) async fn runtime_pick_directory(
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

pub(super) async fn ensure_project_has_no_live_run(
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

pub(super) async fn ensure_project_has_no_worktree_recovery(
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
pub(super) async fn runtime_relocate_project(
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
pub(super) async fn runtime_remove_project(
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

pub(super) async fn finish_started_run(
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
pub(super) async fn runtime_start_project(
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
pub(super) async fn runtime_probe_project_worktree(
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
pub(super) async fn runtime_start_project_worktree(
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
pub(super) async fn runtime_start_recovered_worktree(
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
pub(super) async fn runtime_read_session_history(
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

pub(super) async fn start_resolved_project_run(
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
pub(super) async fn runtime_list_project_sessions(
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
pub(super) async fn runtime_resume_project_session(
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
pub(super) async fn runtime_stop(
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
pub(super) async fn probe_pi_environment(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<PiEnvironmentProbeReport, String> {
    let profile = runtime.launch_profile().await?;
    Ok(PiEnvironmentProbeReport {
        environment: profile.environment.diagnostics().clone(),
        version: profile.version,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::services::test_support::WorkflowFakePiFixture;
    use pi_wizard_core::runtime::RUNTIME_HYDRATION_SCHEMA_VERSION;
    use serde_json::json;

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
    fn fresh_desktop_startup_snapshot_proves_runtime_manager_hydration() {
        let runtime = DesktopRuntime::new().expect("desktop runtime");
        let snapshot =
            tauri::async_runtime::block_on(desktop_startup_snapshot(&runtime)).expect("startup");
        assert_eq!(
            snapshot.runtime.schema_version,
            RUNTIME_HYDRATION_SCHEMA_VERSION
        );
        assert_eq!(snapshot.runtime.runtime_revision, 0);
        assert!(snapshot.runtime.runs.is_empty());
        assert_eq!(snapshot.capacity.active_runs, 0);
        assert_eq!(
            snapshot.attachment_limits.max_attachments,
            runtime.limits.max_attachments_per_prompt
        );
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
                thinking_levels: vec![ThinkingLevel::Off, ThinkingLevel::Medium],
                clear_queue_supported: false,
            })
            .expect("serialize launch options"),
            json!({
                "currentModel": null,
                "currentThinkingLevel": "medium",
                "thinkingLevels": ["off", "medium"],
                "clearQueueSupported": false
            })
        );
    }

    #[tokio::test]
    async fn project_model_probe_returns_multiple_pi_models_independently() {
        let fixture = WorkflowFakePiFixture::new("model-probe-integration");
        let environment = fixture.environment();
        let project = ProjectBinding::register(&fixture.root).expect("register fixture project");
        let limits = RuntimeLimits {
            startup_rpc_deadline_ms: 2_000,
            ..RuntimeLimits::default()
        };
        let catalog = probe_project_models(
            &environment,
            &project,
            ProjectTrustPolicy::Inherit,
            ContextFilesPolicy::Inherit,
            limits,
        )
        .await
        .expect("probe Pi models without launch-options dependencies");
        assert_eq!(catalog.models.len(), 3);
        assert_eq!(catalog.models[0].id, "fake-model");
        assert_eq!(catalog.models[1].id, "alternate-model");
        assert_eq!(catalog.models[2].provider, "other-provider");
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
