use std::sync::Arc;

use pi_wizard_core::automation::{AutomationChain, AutomationExecutionSnapshot};
use pi_wizard_core::launch::{ContextFilesPolicy, ExtensionDiscoveryPolicy};
use pi_wizard_core::project::ProjectRegisteredLocation;
use pi_wizard_core::rpc::ThinkingLevel;
use pi_wizard_core::worktree::inspect_worktree_base;
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
    pub(crate) concurrency: usize,
    pub(crate) worktrees: bool,
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
    let base = if request.worktrees {
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
    let concurrency = request.concurrency;
    let worktrees = request.worktrees;
    tauri::async_runtime::spawn(async move {
        run_automation_execution(
            context,
            AutomationExecutionPlan {
                execution_id: id,
                chain,
                project,
                environment: profile.environment,
                base,
                concurrency,
                worktrees,
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
    fn automation_start_wire_shape_has_worker_model_but_no_supervisor_switch() {
        let chain_id = AutomationChainId::new();
        let project_id = ProjectId::new();
        let request: StartAutomationRequest = serde_json::from_value(serde_json::json!({
            "chainId": chain_id,
            "projectId": project_id,
            "concurrency": 6,
            "worktrees": true,
            "provider": "opencode-go",
            "model": "gpt-5.6-luna",
            "thinking": "xhigh"
        }))
        .expect("deserialize independent automation request");
        assert_eq!(request.chain_id, chain_id);
        assert_eq!(request.project_id, project_id);
        assert_eq!(request.concurrency, 6);
        assert!(request.worktrees);
        assert_eq!(request.provider.as_deref(), Some("opencode-go"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(request.thinking, Some(ThinkingLevel::Xhigh));
    }

    #[test]
    fn old_coupled_supervisor_field_is_ignored_not_reintroduced_into_automation_state() {
        let request: StartAutomationRequest = serde_json::from_value(serde_json::json!({
            "chainId": AutomationChainId::new(),
            "projectId": ProjectId::new(),
            "concurrency": 1,
            "worktrees": false,
            "supervisor": true
        }))
        .expect("serde ignores obsolete extra field");
        assert_eq!(request.concurrency, 1);
        assert!(!request.worktrees);
    }
}
