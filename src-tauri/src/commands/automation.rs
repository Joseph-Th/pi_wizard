use std::sync::Arc;

use pi_wizard_core::automation::{AutomationChain, AutomationExecutionSnapshot};
use pi_wizard_core::launch::{ContextFilesPolicy, ExtensionDiscoveryPolicy};
use pi_wizard_core::project::ProjectRegisteredLocation;
use pi_wizard_core::rpc::ThinkingLevel;
use pi_wizard_core::{AutomationChainId, AutomationExecutionId, ProjectId};
use serde::Deserialize;

use crate::services::automation::{
    AutomationExecutionPlan, AutomationRuntimeContext, DesktopAutomationSnapshot,
    run_automation_execution,
};
use crate::{DesktopRuntime, LaunchSelection};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveAutomationChainRequest {
    #[serde(default)]
    pub(crate) id: Option<AutomationChainId>,
    pub(crate) name: String,
    pub(crate) prompts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationChainRequest {
    pub(crate) id: AutomationChainId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartAutomationRequest {
    pub(crate) chain_id: AutomationChainId,
    pub(crate) project_id: ProjectId,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) thinking: Option<ThinkingLevel>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationExecutionRequest {
    pub(crate) id: AutomationExecutionId,
}

#[tauri::command]
pub(crate) async fn runtime_automation_snapshot(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<DesktopAutomationSnapshot, String> {
    Ok(runtime.automation.snapshot().await)
}

#[tauri::command]
pub(crate) async fn runtime_automation_executions(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<Vec<AutomationExecutionSnapshot>, String> {
    Ok(runtime.automation.execution_snapshot().await)
}

#[tauri::command]
pub(crate) async fn runtime_save_automation_chain(
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
pub(crate) async fn runtime_delete_automation_chain(
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
pub(crate) async fn runtime_start_automation(
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
    let selection = LaunchSelection::validate(
        ContextFilesPolicy::Inherit,
        ExtensionDiscoveryPolicy::Inherit,
        request.provider.clone(),
        request.model.clone(),
        request.thinking,
    )?;
    let project = runtime.registered_project(request.project_id).await?;
    if project.verify_registered_location() != ProjectRegisteredLocation::Present {
        return Err(
            "selected project is detached or moved; relocate it before automation".to_owned(),
        );
    }
    let profile = runtime.launch_profile().await?;

    let id = AutomationExecutionId::new();
    let snapshot = AutomationExecutionSnapshot::new(id, &chain, request.project_id, runtime.limits);
    let cancel = runtime.automation.insert_execution(snapshot).await?;
    let context = AutomationRuntimeContext {
        manager: runtime.manager.clone(),
        limits: runtime.limits,
        launch_cleanup_gate: Arc::clone(&runtime.launch_cleanup_gate),
        coordinator: runtime.automation.clone(),
    };
    tauri::async_runtime::spawn(async move {
        run_automation_execution(
            context,
            AutomationExecutionPlan {
                execution_id: id,
                chain,
                project,
                environment: profile.environment,
                selection,
            },
            cancel,
        )
        .await;
    });
    Ok(id)
}

#[tauri::command]
pub(crate) async fn runtime_cancel_automation(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: AutomationExecutionRequest,
) -> Result<(), String> {
    runtime.automation.cancel(request.id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_start_wire_shape_is_sequential_and_model_selectable() {
        let chain_id = AutomationChainId::new();
        let project_id = ProjectId::new();
        let request: StartAutomationRequest = serde_json::from_value(serde_json::json!({
            "chainId": chain_id,
            "projectId": project_id,
            "provider": "opencode-go",
            "model": "gpt-5.6-luna",
            "thinking": "xhigh"
        }))
        .expect("deserialize independent automation request");
        assert_eq!(request.chain_id, chain_id);
        assert_eq!(request.project_id, project_id);
        assert_eq!(request.provider.as_deref(), Some("opencode-go"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(request.thinking, Some(ThinkingLevel::Xhigh));
    }

    #[test]
    fn obsolete_parallel_and_supervisor_fields_are_ignored() {
        let request: StartAutomationRequest = serde_json::from_value(serde_json::json!({
            "chainId": AutomationChainId::new(),
            "projectId": ProjectId::new(),
            "concurrency": 8,
            "worktrees": true,
            "supervisor": true
        }))
        .expect("serde ignores obsolete extra fields");
        assert!(request.provider.is_none());
        assert!(request.model.is_none());
    }
}
