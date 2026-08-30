use serde::{Deserialize, Serialize};

use crate::rpc::ThinkingLevel;
use crate::{ProjectId, RunId, SupervisionId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisionStatus {
    Starting,
    Running,
    Completed,
    Stopped,
    Failed,
}

impl SupervisionStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisionSnapshot {
    pub id: SupervisionId,
    pub project_ids: Vec<ProjectId>,
    pub host_project_id: ProjectId,
    pub supervisor_run_id: Option<RunId>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub cycles: usize,
    pub watched_runs: usize,
    pub last_decision: Option<String>,
    pub status: SupervisionStatus,
    pub error: Option<String>,
}

impl SupervisionSnapshot {
    #[must_use]
    pub fn new(
        id: SupervisionId,
        mut project_ids: Vec<ProjectId>,
        host_project_id: ProjectId,
        provider: Option<String>,
        model: Option<String>,
        thinking: Option<ThinkingLevel>,
    ) -> Self {
        project_ids.sort_by_key(ToString::to_string);
        project_ids.dedup();
        Self {
            id,
            project_ids,
            host_project_id,
            supervisor_run_id: None,
            provider,
            model,
            thinking,
            cycles: 0,
            watched_runs: 0,
            last_decision: None,
            status: SupervisionStatus::Starting,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervision_state_is_independent_from_automation_identity() {
        let first = ProjectId::new();
        let second = ProjectId::new();
        let snapshot = SupervisionSnapshot::new(
            SupervisionId::new(),
            vec![first, second],
            first,
            Some("provider".to_owned()),
            Some("model".to_owned()),
            Some(ThinkingLevel::High),
        );
        assert_eq!(snapshot.status, SupervisionStatus::Starting);
        assert_eq!(snapshot.project_ids.len(), 2);
        assert_eq!(snapshot.host_project_id, first);
        assert!(snapshot.last_decision.is_none());
        assert!(snapshot.supervisor_run_id.is_none());
    }
}
