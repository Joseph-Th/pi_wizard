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
    pub project_id: ProjectId,
    pub supervisor_run_id: Option<RunId>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub cycles: usize,
    pub max_cycles: usize,
    pub watched_runs: usize,
    pub status: SupervisionStatus,
    pub error: Option<String>,
}

impl SupervisionSnapshot {
    #[must_use]
    pub fn new(
        id: SupervisionId,
        project_id: ProjectId,
        provider: Option<String>,
        model: Option<String>,
        thinking: Option<ThinkingLevel>,
        max_cycles: usize,
    ) -> Self {
        Self {
            id,
            project_id,
            supervisor_run_id: None,
            provider,
            model,
            thinking,
            cycles: 0,
            max_cycles,
            watched_runs: 0,
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
        let snapshot = SupervisionSnapshot::new(
            SupervisionId::new(),
            ProjectId::new(),
            Some("provider".to_owned()),
            Some("model".to_owned()),
            Some(ThinkingLevel::High),
            12,
        );
        assert_eq!(snapshot.status, SupervisionStatus::Starting);
        assert_eq!(snapshot.max_cycles, 12);
        assert!(snapshot.supervisor_run_id.is_none());
    }
}
