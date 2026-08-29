use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::RunId;
use crate::draft::DraftSnapshot;
use crate::rpc::ExtensionDialogRequest;

use super::{
    ComposerAvailability, ExtensionUiSnapshot, LiveProjectionSnapshot, RunCapabilities, RunRecord,
    RunRpcController, RuntimeStore, SessionSyncState,
};

pub const RUNTIME_HYDRATION_SCHEMA_VERSION: u32 = 10;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHydrationSnapshot {
    pub schema_version: u32,
    pub runtime_revision: u64,
    pub runs: Vec<RunHydrationSnapshot>,
}

impl RuntimeHydrationSnapshot {
    #[must_use]
    pub fn build(
        store: &RuntimeStore,
        controllers: &HashMap<RunId, RunRpcController>,
        now: Instant,
    ) -> Self {
        let mut runs: Vec<_> = store
            .records()
            .map(|run| RunHydrationSnapshot {
                run: run.clone(),
                draft: None,
                composer_availability: run.composer_availability(),
                composer_submission_pending: false,
                draft_restore_pending: false,
                rpc: controllers
                    .get(&run.id())
                    .map(|controller| controller.hydration_snapshot(now)),
            })
            .collect();
        runs.sort_by_key(|run| run.run.id().to_string());
        Self {
            schema_version: RUNTIME_HYDRATION_SCHEMA_VERSION,
            runtime_revision: store.revision(),
            runs,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunHydrationSnapshot {
    pub run: RunRecord,
    pub draft: Option<DraftSnapshot>,
    pub composer_availability: ComposerAvailability,
    pub composer_submission_pending: bool,
    pub draft_restore_pending: bool,
    pub rpc: Option<RunRpcHydrationSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRpcHydrationSnapshot {
    pub run_id: RunId,
    pub capabilities: RunCapabilities,
    pub session_sync: SessionSyncState,
    pub live: LiveProjectionSnapshot,
    pub extension_ui: ExtensionUiSnapshot,
    pub pending_dialogs: Vec<PendingExtensionDialogSnapshot>,
    pub compaction: Option<RunCompactionSnapshot>,
    pub retry: Option<RunRetrySnapshot>,
    pub summarization_retry: Option<RunSummarizationRetrySnapshot>,
    pub last_extension_error: Option<RunExtensionErrorSnapshot>,
    pub stream_stalled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCompactionSnapshot {
    pub reason: String,
    pub reason_truncated: bool,
    pub finished: bool,
    pub aborted: bool,
    pub will_retry: bool,
    pub error_message: Option<String>,
    pub error_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRetrySnapshot {
    pub attempt: usize,
    pub max_attempts: usize,
    pub delay_ms: u64,
    pub error_message: String,
    pub error_truncated: bool,
    pub waiting: bool,
    pub finished: bool,
    pub success: Option<bool>,
    pub final_error: Option<String>,
    pub final_error_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummarizationRetrySnapshot {
    pub attempt: usize,
    pub max_attempts: usize,
    pub delay_ms: u64,
    pub error_message: String,
    pub error_truncated: bool,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub finished: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunExtensionErrorSnapshot {
    pub extension_path: String,
    pub event: String,
    pub error: String,
    pub detail_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingExtensionDialogSnapshot {
    pub request: ExtensionDialogRequest,
    pub remaining_timeout_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::ProjectId;
    use crate::RuntimeLimits;
    use crate::launch::ProjectTrustPolicy;
    use crate::runtime::{ExecutionIsolation, RunMutation};

    #[test]
    fn hydration_is_schema_versioned_revisioned_and_deterministically_ordered() {
        let limits = RuntimeLimits::default();
        let mut store = RuntimeStore::new(limits);
        let first = RunId::new();
        let second = RunId::new();
        for id in [second, first] {
            store
                .register(
                    RunRecord::starting(
                        id,
                        ProjectId::new(),
                        PathBuf::from(format!("project-{id}")),
                        ExecutionIsolation::LocalCheckout,
                        ProjectTrustPolicy::Ignore,
                    )
                    .expect("local run"),
                )
                .expect("register");
        }
        store
            .apply(first, RunMutation::ProcessReady)
            .expect("ready first");
        let mut controllers = HashMap::new();
        controllers.insert(first, RunRpcController::new(first, limits));

        let snapshot = RuntimeHydrationSnapshot::build(&store, &controllers, Instant::now());
        assert_eq!(snapshot.schema_version, RUNTIME_HYDRATION_SCHEMA_VERSION);
        assert_eq!(snapshot.runtime_revision, 3);
        assert_eq!(snapshot.runs.len(), 2);
        assert!(snapshot.runs.iter().any(|run| run.rpc.is_some()));
        let ids: Vec<_> = snapshot
            .runs
            .iter()
            .map(|run| run.run.id().to_string())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);

        let once = serde_json::to_vec(&snapshot).expect("serialize");
        let twice = serde_json::to_vec(&snapshot).expect("serialize again");
        assert_eq!(once, twice);
    }
}
