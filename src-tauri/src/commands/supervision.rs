use std::collections::HashSet;
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
    #[serde(default)]
    pub(crate) project_ids: Vec<ProjectId>,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) thinking: Option<ThinkingLevel>,
    #[serde(default)]
    pub(crate) prompt_templates: Vec<String>,
    #[serde(default)]
    pub(crate) max_cycles: Option<usize>,
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
    if request.project_ids.is_empty() {
        return Err("select at least one project for supervision".to_owned());
    }
    if request.project_ids.len() > runtime.limits.max_project_registry_entries {
        return Err(format!(
            "supervision selected {} projects, exceeding project limit {}",
            request.project_ids.len(),
            runtime.limits.max_project_registry_entries
        ));
    }
    let mut project_ids = Vec::with_capacity(request.project_ids.len());
    let mut project_id_set = HashSet::with_capacity(request.project_ids.len());
    for project_id in request.project_ids.iter().copied() {
        if project_id_set.insert(project_id) {
            project_ids.push(project_id);
        }
    }
    if let Some(max_cycles) = request.max_cycles
        && (max_cycles == 0 || max_cycles > runtime.limits.max_supervision_cycles)
    {
        return Err(format!(
            "finite supervision cycles must be between 1 and {}",
            runtime.limits.max_supervision_cycles
        ));
    }
    if request.prompt_templates.len() > runtime.limits.max_automation_steps_per_chain {
        return Err(format!(
            "supervision playbook has {} prompts, exceeding limit {}",
            request.prompt_templates.len(),
            runtime.limits.max_automation_steps_per_chain
        ));
    }
    let mut template_bytes = 0usize;
    let mut prompt_templates = Vec::with_capacity(request.prompt_templates.len());
    for (index, template) in request.prompt_templates.iter().enumerate() {
        let template = template.trim();
        if template.is_empty() {
            return Err(format!(
                "supervision playbook prompt {} is empty",
                index + 1
            ));
        }
        if template.len() > runtime.limits.max_draft_bytes_per_session {
            return Err(format!(
                "supervision playbook prompt {} uses {} bytes, exceeding prompt limit {}",
                index + 1,
                template.len(),
                runtime.limits.max_draft_bytes_per_session
            ));
        }
        template_bytes = template_bytes.saturating_add(template.len());
        if template_bytes > runtime.limits.max_supervisor_context_bytes {
            return Err(format!(
                "supervision playbook uses more than {} bytes",
                runtime.limits.max_supervisor_context_bytes
            ));
        }
        prompt_templates.push(template.to_owned());
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
    let mut projects = Vec::with_capacity(project_ids.len());
    for project_id in &project_ids {
        let project = runtime.registered_project(*project_id).await?;
        if project.verify_registered_location() != ProjectRegisteredLocation::Present {
            return Err(format!(
                "selected project {project_id} is detached or moved; relocate it before supervision"
            ));
        }
        projects.push(project);
    }
    let profile = runtime.launch_profile().await?;
    let mut host = None;
    let mut host_errors = Vec::new();
    for project in &projects {
        match inspect_worktree_base(
            project.canonical_root(),
            &profile.environment,
            runtime.limits,
        )
        .await
        {
            Ok(base) => {
                host = Some((project.clone(), base));
                break;
            }
            Err(error) => {
                host_errors.push(format!("{}: {error}", project.canonical_root().display()))
            }
        }
    }
    let (project, base) = host.ok_or_else(|| {
        format!(
            "supervision requires at least one selected Git project to host its supervisor worktree: {}",
            host_errors.join("; ")
        )
    })?;
    let id = SupervisionId::new();
    let snapshot = SupervisionSnapshot::new(
        id,
        project_ids.clone(),
        project.id(),
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
                project_ids: project_id_set,
                environment: profile.environment,
                base,
                selection,
                prompt_templates,
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
    fn supervision_start_wire_shape_is_multi_project_continuous_and_model_selectable() {
        let first_project = ProjectId::new();
        let second_project = ProjectId::new();
        let request: StartSupervisionRequest = serde_json::from_value(serde_json::json!({
            "projectIds": [first_project, second_project],
            "provider": "openai",
            "model": "gpt-5.6",
            "thinking": "high",
            "promptTemplates": ["Audit the project", "Improve testing"],
            "maxCycles": null
        }))
        .expect("deserialize supervision request");
        assert_eq!(request.project_ids, vec![first_project, second_project]);
        assert_eq!(request.provider.as_deref(), Some("openai"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(request.thinking, Some(ThinkingLevel::High));
        assert_eq!(request.prompt_templates.len(), 2);
        assert_eq!(request.max_cycles, None);
    }
}
