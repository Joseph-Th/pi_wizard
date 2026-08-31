use std::sync::Arc;

use pi_wizard_core::automation::{
    AutomationCatalogSnapshot, AutomationChain, AutomationExecutionSnapshot, AutomationStore,
};
use pi_wizard_core::launch::{ContextFilesPolicy, ExtensionDiscoveryPolicy};
use pi_wizard_core::project::{ProjectBinding, ProjectRegisteredLocation};
use pi_wizard_core::rpc::ThinkingLevel;
use pi_wizard_core::{AutomationChainId, AutomationExecutionId, ProjectId};
use serde::Deserialize;

use crate::services::automation::{
    AutomationExecutionPlan, AutomationRuntimeContext, DesktopAutomationSnapshot,
    run_automation_execution,
};
use crate::{DesktopRuntime, LaunchSelection};

pub(crate) const PROJECT_AUTOMATION_DIRECTORY: &str = ".pi-wizard";

fn project_automation_root(project: &ProjectBinding) -> std::path::PathBuf {
    project.canonical_root().join(PROJECT_AUTOMATION_DIRECTORY)
}

fn open_project_automation_store(
    project: &ProjectBinding,
    limits: pi_wizard_core::RuntimeLimits,
) -> Result<AutomationStore, String> {
    AutomationStore::open(project_automation_root(project), limits)
        .map_err(|error| error.to_string())
}

async fn resolve_project_for_automation(
    runtime: &DesktopRuntime,
    project_id: ProjectId,
) -> Result<ProjectBinding, String> {
    let project = runtime.registered_project(project_id).await?;
    if project.verify_registered_location() != ProjectRegisteredLocation::Present {
        return Err(
            "selected project is detached or moved; relocate it before automation".to_owned(),
        );
    }
    Ok(project)
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectAutomationRequest {
    #[serde(default)]
    pub(crate) project_id: Option<ProjectId>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveAutomationChainRequest {
    pub(crate) project_id: ProjectId,
    #[serde(default)]
    pub(crate) id: Option<AutomationChainId>,
    pub(crate) name: String,
    pub(crate) prompts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationChainRequest {
    pub(crate) project_id: ProjectId,
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
    request: Option<ProjectAutomationRequest>,
) -> Result<DesktopAutomationSnapshot, String> {
    let project_id = request.and_then(|request| request.project_id);
    let catalog = if let Some(project_id) = project_id {
        let project = resolve_project_for_automation(&runtime, project_id).await?;
        let _catalog_gate = runtime.automation.catalog_gate.lock().await;
        open_project_automation_store(&project, runtime.limits)?.snapshot()
    } else {
        AutomationCatalogSnapshot::default()
    };
    Ok(runtime.automation.snapshot(project_id, catalog).await)
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
    let project = resolve_project_for_automation(&runtime, request.project_id).await?;
    let chain = AutomationChain {
        id: request.id.unwrap_or_default(),
        name: request.name,
        prompts: request.prompts,
    };
    let _catalog_gate = runtime.automation.catalog_gate.lock().await;
    let mut store = open_project_automation_store(&project, runtime.limits)?;
    let saved = store.upsert(chain).map_err(|error| error.to_string())?;
    runtime.automation.signal_catalog_changed();
    Ok(saved)
}

#[tauri::command]
pub(crate) async fn runtime_delete_automation_chain(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: AutomationChainRequest,
) -> Result<bool, String> {
    let project = resolve_project_for_automation(&runtime, request.project_id).await?;
    let _catalog_gate = runtime.automation.catalog_gate.lock().await;
    let mut store = open_project_automation_store(&project, runtime.limits)?;
    let removed = store
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
    let project = resolve_project_for_automation(&runtime, request.project_id).await?;
    let chain = {
        let _catalog_gate = runtime.automation.catalog_gate.lock().await;
        open_project_automation_store(&project, runtime.limits)?
            .get(request.chain_id)
            .cloned()
            .ok_or_else(|| format!("unknown automation chain {}", request.chain_id))?
    };
    let selection = LaunchSelection::validate(
        ContextFilesPolicy::Inherit,
        ExtensionDiscoveryPolicy::Inherit,
        request.provider.clone(),
        request.model.clone(),
        request.thinking,
    )?;
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
    use std::fs;

    fn project_fixture(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-project-prompt-chains-{name}-{}",
            AutomationExecutionId::new()
        ));
        fs::create_dir_all(&root).expect("create project prompt-chain fixture");
        root
    }

    #[test]
    fn project_prompt_chain_catalog_is_saved_inside_selected_directory_and_isolated() {
        let first_root = project_fixture("first");
        let second_root = project_fixture("second");
        let first = ProjectBinding::register(&first_root).expect("register first project");
        let second = ProjectBinding::register(&second_root).expect("register second project");
        let limits = pi_wizard_core::RuntimeLimits::default();
        let local_directory = first.canonical_root().join(PROJECT_AUTOMATION_DIRECTORY);

        let mut first_store = open_project_automation_store(&first, limits).expect("open first");
        assert!(
            !local_directory.exists(),
            "reading an empty project catalog must not create project state"
        );
        let chain = AutomationChain {
            id: AutomationChainId::new(),
            name: "Project-local chain".to_owned(),
            prompts: vec!["work only in this project".to_owned()],
        };
        let saved = first_store.upsert(chain).expect("save project-local chain");
        assert!(local_directory.join("prompt-chains.json").is_file());

        let reopened = open_project_automation_store(&first, limits).expect("reopen first");
        assert_eq!(reopened.get(saved.id), Some(&saved));
        let other = open_project_automation_store(&second, limits).expect("open second");
        assert!(other.snapshot().chains.is_empty());
        assert!(
            !second
                .canonical_root()
                .join(PROJECT_AUTOMATION_DIRECTORY)
                .exists()
        );

        fs::remove_dir_all(first_root).expect("cleanup first project");
        fs::remove_dir_all(second_root).expect("cleanup second project");
    }

    #[test]
    fn automation_catalog_commands_require_project_identity() {
        let project_id = ProjectId::new();
        let save: SaveAutomationChainRequest = serde_json::from_value(serde_json::json!({
            "projectId": project_id,
            "name": "local",
            "prompts": ["one"]
        }))
        .expect("save request project identity");
        assert_eq!(save.project_id, project_id);

        let snapshot: ProjectAutomationRequest = serde_json::from_value(serde_json::json!({
            "projectId": project_id
        }))
        .expect("snapshot request project identity");
        assert_eq!(snapshot.project_id, Some(project_id));

        let remove: AutomationChainRequest = serde_json::from_value(serde_json::json!({
            "projectId": project_id,
            "id": AutomationChainId::new()
        }))
        .expect("delete request project identity");
        assert_eq!(remove.project_id, project_id);
    }

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
