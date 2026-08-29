use std::sync::Arc;

use pi_wizard_core::launch::{ContextFilesPolicy, ExtensionDiscoveryPolicy};
use pi_wizard_core::project::ProjectRegisteredLocation;
use pi_wizard_core::rpc::ThinkingLevel;
use pi_wizard_core::supervision::SupervisionSnapshot;
use pi_wizard_core::worktree::inspect_worktree_base;
use pi_wizard_core::{ProjectId, SupervisionId};
use serde::Deserialize;

use crate::services::supervision::{SupervisionPlan, SupervisionRuntimeContext, run_supervision};
use crate::{DesktopRuntime, LaunchSelection};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartSupervisionRequest {
    pub(crate) project_id: ProjectId,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) thinking: Option<ThinkingLevel>,
    pub(crate) max_cycles: usize,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupervisionRequest {
    pub(crate) id: SupervisionId,
}

#[tauri::command]
pub(crate) async fn runtime_supervision_snapshot(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<Vec<SupervisionSnapshot>, String> {
    Ok(runtime.supervision.snapshots().await)
}

#[tauri::command]
pub(crate) async fn runtime_start_supervision(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: StartSupervisionRequest,
) -> Result<SupervisionId, String> {
    if request.max_cycles == 0 || request.max_cycles > runtime.limits.max_supervision_cycles {
        return Err(format!(
            "supervision cycles must be between 1 and {}",
            runtime.limits.max_supervision_cycles
        ));
    }
    let capacity = runtime.capacity_report().await?;
    if capacity.active_runs >= capacity.live_run_limit {
        return Err("no live-run slot is available for supervision".to_owned());
    }
    let selection = LaunchSelection::validate(
        ContextFilesPolicy::Disabled,
        ExtensionDiscoveryPolicy::Disabled,
        request.provider.clone(),
        request.model.clone(),
        request.thinking,
    )?;
    let project = runtime.registered_project(request.project_id).await?;
    if project.verify_registered_location() != ProjectRegisteredLocation::Present {
        return Err(
            "selected project is detached or moved; relocate it before supervision".to_owned(),
        );
    }
    let profile = runtime.launch_profile().await?;
    let base = inspect_worktree_base(
        project.canonical_root(),
        &profile.environment,
        runtime.limits,
    )
    .await
    .map_err(|error| error.to_string())?;
    let id = SupervisionId::new();
    let snapshot = SupervisionSnapshot::new(
        id,
        request.project_id,
        selection.provider.clone(),
        selection.model.clone(),
        selection.thinking,
        request.max_cycles,
    );
    let stop = runtime.supervision.insert(snapshot).await?;
    let context = SupervisionRuntimeContext {
        manager: runtime.manager.clone(),
        limits: runtime.limits,
        launch_cleanup_gate: Arc::clone(&runtime.launch_cleanup_gate),
        worktrees: Arc::clone(&runtime.worktrees),
        coordinator: runtime.supervision.clone(),
    };
    tauri::async_runtime::spawn(async move {
        run_supervision(
            context,
            SupervisionPlan {
                id,
                project,
                environment: profile.environment,
                base,
                selection,
                max_cycles: request.max_cycles,
            },
            stop,
        )
        .await;
    });
    Ok(id)
}

#[tauri::command]
pub(crate) async fn runtime_stop_supervision(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SupervisionRequest,
) -> Result<(), String> {
    runtime.supervision.request_stop(request.id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervision_start_wire_shape_is_project_scoped_and_model_selectable() {
        let project_id = ProjectId::new();
        let request: StartSupervisionRequest = serde_json::from_value(serde_json::json!({
            "projectId": project_id,
            "provider": "openai",
            "model": "gpt-5.6",
            "thinking": "high",
            "maxCycles": 12
        }))
        .expect("deserialize supervision request");
        assert_eq!(request.project_id, project_id);
        assert_eq!(request.provider.as_deref(), Some("openai"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(request.thinking, Some(ThinkingLevel::High));
        assert_eq!(request.max_cycles, 12);
    }
}
