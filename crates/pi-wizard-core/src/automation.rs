use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AutomationChainId, AutomationExecutionId, ProjectId, RunId, RuntimeLimits};

pub const AUTOMATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationChain {
    pub id: AutomationChainId,
    pub name: String,
    pub prompts: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCatalogSnapshot {
    pub chains: Vec<AutomationChain>,
    pub recovery_notice: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationExecutionStatus {
    Starting,
    Running,
    Completed,
    CompletedWithErrors,
    Cancelled,
    Failed,
}

impl AutomationExecutionStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::CompletedWithErrors | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStepStatus {
    Queued,
    Starting,
    Working,
    NeedsAttention,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationStepSnapshot {
    pub index: usize,
    pub prompt_preview: String,
    pub prompt_truncated: bool,
    pub run_id: Option<RunId>,
    pub status: AutomationStepStatus,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationExecutionSnapshot {
    pub id: AutomationExecutionId,
    pub chain_id: AutomationChainId,
    pub chain_name: String,
    pub project_id: ProjectId,
    pub concurrency: usize,
    pub worktrees: bool,
    pub error: Option<String>,
    pub status: AutomationExecutionStatus,
    pub steps: Vec<AutomationStepSnapshot>,
}

impl AutomationExecutionSnapshot {
    #[must_use]
    pub fn new(
        id: AutomationExecutionId,
        chain: &AutomationChain,
        project_id: ProjectId,
        concurrency: usize,
        worktrees: bool,
        limits: RuntimeLimits,
    ) -> Self {
        Self {
            id,
            chain_id: chain.id,
            chain_name: chain.name.clone(),
            project_id,
            concurrency,
            worktrees,
            error: None,
            status: AutomationExecutionStatus::Starting,
            steps: chain
                .prompts
                .iter()
                .enumerate()
                .map(|(index, prompt)| {
                    let (prompt_preview, prompt_truncated) =
                        bounded_utf8_preview(prompt, limits.max_automation_prompt_preview_bytes);
                    AutomationStepSnapshot {
                        index,
                        prompt_preview,
                        prompt_truncated,
                        run_id: None,
                        status: AutomationStepStatus::Queued,
                        error: None,
                    }
                })
                .collect(),
        }
    }
}

fn bounded_utf8_preview(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAutomationCatalog {
    schema_version: u32,
    chains: Vec<AutomationChain>,
}

/// Recoverable, bounded app-owned prompt-chain definitions.
///
/// This store owns only reusable automation definitions. Pi sessions and live
/// workflow state remain owned by their existing runtime/session authorities.
#[derive(Debug)]
pub struct AutomationStore {
    path: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
    limits: RuntimeLimits,
    chains: HashMap<AutomationChainId, AutomationChain>,
    recovery_notice: Option<String>,
}

impl AutomationStore {
    #[must_use]
    pub fn ephemeral(limits: RuntimeLimits) -> Self {
        Self {
            path: None,
            quarantine_dir: None,
            limits,
            chains: HashMap::new(),
            recovery_notice: None,
        }
    }

    pub fn open(root: impl AsRef<Path>, limits: RuntimeLimits) -> Result<Self, AutomationError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|source| AutomationError::CreateDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let mut store = Self {
            path: Some(root.join("automation-chains.json")),
            quarantine_dir: Some(root.join("automation-quarantine")),
            limits,
            chains: HashMap::new(),
            recovery_notice: None,
        };
        match store.load_from_disk() {
            Ok(()) => Ok(store),
            Err(AutomationError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(store)
            }
            Err(error) if error.is_recoverable_corruption() => {
                let notice = error.to_string();
                store.quarantine_corrupt_file(error)?;
                store.chains.clear();
                store.recovery_notice = Some(notice);
                Ok(store)
            }
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn get(&self, id: AutomationChainId) -> Option<&AutomationChain> {
        self.chains.get(&id)
    }

    #[must_use]
    pub fn snapshot(&self) -> AutomationCatalogSnapshot {
        let mut chains: Vec<_> = self.chains.values().cloned().collect();
        chains.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
        });
        AutomationCatalogSnapshot {
            chains,
            recovery_notice: self.recovery_notice.clone(),
        }
    }

    pub fn upsert(
        &mut self,
        mut chain: AutomationChain,
    ) -> Result<AutomationChain, AutomationError> {
        chain.name = chain.name.trim().to_owned();
        self.validate_chain(&chain)?;
        if !self.chains.contains_key(&chain.id)
            && self.chains.len() >= self.limits.max_automation_chains
        {
            return Err(AutomationError::ChainLimit {
                limit: self.limits.max_automation_chains,
            });
        }
        let mut candidate = self.chains.clone();
        candidate.insert(chain.id, chain.clone());
        self.persist(&candidate)?;
        self.chains = candidate;
        self.recovery_notice = None;
        Ok(chain)
    }

    pub fn remove(&mut self, id: AutomationChainId) -> Result<bool, AutomationError> {
        if !self.chains.contains_key(&id) {
            return Ok(false);
        }
        let mut candidate = self.chains.clone();
        candidate.remove(&id);
        self.persist(&candidate)?;
        self.chains = candidate;
        self.recovery_notice = None;
        Ok(true)
    }

    fn validate_chain(&self, chain: &AutomationChain) -> Result<(), AutomationError> {
        if chain.name.is_empty() {
            return Err(AutomationError::EmptyName);
        }
        if chain.name.len() > self.limits.max_automation_name_bytes {
            return Err(AutomationError::NameTooLarge {
                actual: chain.name.len(),
                limit: self.limits.max_automation_name_bytes,
            });
        }
        if chain.prompts.is_empty() {
            return Err(AutomationError::EmptyChain);
        }
        if chain.prompts.len() > self.limits.max_automation_steps_per_chain {
            return Err(AutomationError::StepLimit {
                actual: chain.prompts.len(),
                limit: self.limits.max_automation_steps_per_chain,
            });
        }
        for (index, prompt) in chain.prompts.iter().enumerate() {
            if prompt.trim().is_empty() {
                return Err(AutomationError::EmptyPrompt { index });
            }
            if prompt.len() > self.limits.max_draft_bytes_per_session {
                return Err(AutomationError::PromptTooLarge {
                    index,
                    actual: prompt.len(),
                    limit: self.limits.max_draft_bytes_per_session,
                });
            }
        }
        Ok(())
    }

    fn load_from_disk(&mut self) -> Result<(), AutomationError> {
        let path = self.path.as_ref().expect("persistent automation path");
        let metadata = fs::metadata(path).map_err(|source| AutomationError::Read {
            path: path.clone(),
            source,
        })?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > self.limits.max_automation_state_bytes {
            return Err(AutomationError::ByteLimit {
                attempted: size,
                limit: self.limits.max_automation_state_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|source| AutomationError::Read {
            path: path.clone(),
            source,
        })?;
        let persisted: PersistedAutomationCatalog =
            serde_json::from_slice(&bytes).map_err(AutomationError::Decode)?;
        if persisted.schema_version != AUTOMATION_SCHEMA_VERSION {
            return Err(AutomationError::UnsupportedSchema {
                actual: persisted.schema_version,
                supported: AUTOMATION_SCHEMA_VERSION,
            });
        }
        if persisted.chains.len() > self.limits.max_automation_chains {
            return Err(AutomationError::ChainLimit {
                limit: self.limits.max_automation_chains,
            });
        }
        let mut seen = HashSet::with_capacity(persisted.chains.len());
        let mut chains = HashMap::with_capacity(persisted.chains.len());
        for mut chain in persisted.chains {
            chain.name = chain.name.trim().to_owned();
            self.validate_chain(&chain)?;
            if !seen.insert(chain.id) {
                return Err(AutomationError::DuplicateChain { id: chain.id });
            }
            chains.insert(chain.id, chain);
        }
        self.chains = chains;
        Ok(())
    }

    fn persist(
        &self,
        candidate: &HashMap<AutomationChainId, AutomationChain>,
    ) -> Result<(), AutomationError> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let mut chains: Vec<_> = candidate.values().cloned().collect();
        chains.sort_by_key(|chain| chain.id.to_string());
        let encoded = serde_json::to_vec(&PersistedAutomationCatalog {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            chains,
        })
        .map_err(AutomationError::Encode)?;
        if encoded.len() > self.limits.max_automation_state_bytes {
            return Err(AutomationError::ByteLimit {
                attempted: encoded.len(),
                limit: self.limits.max_automation_state_bytes,
            });
        }
        let mut file = AtomicWriteFile::options().open(path).map_err(|source| {
            AutomationError::OpenAtomic {
                path: path.clone(),
                source,
            }
        })?;
        file.write_all(&encoded)
            .map_err(|source| AutomationError::Write {
                path: path.clone(),
                source,
            })?;
        file.commit().map_err(|source| AutomationError::Commit {
            path: path.clone(),
            source,
        })
    }

    fn quarantine_corrupt_file(&self, cause: AutomationError) -> Result<(), AutomationError> {
        let path = self.path.as_ref().expect("persistent automation path");
        let quarantine_dir = self
            .quarantine_dir
            .as_ref()
            .expect("persistent automation quarantine directory");
        if let Err(source) = fs::create_dir_all(quarantine_dir) {
            return Err(AutomationError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            });
        }
        let quarantine_path = quarantine_dir.join(format!(
            "{}-automation-chains.json",
            AutomationExecutionId::new()
        ));
        fs::rename(path, quarantine_path).map_err(|source| AutomationError::QuarantineFailed {
            path: path.clone(),
            cause: Box::new(cause),
            source,
        })
    }
}

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error("automation chain name cannot be empty")]
    EmptyName,
    #[error("automation chain name uses {actual} bytes, exceeding limit {limit}")]
    NameTooLarge { actual: usize, limit: usize },
    #[error("automation chain must contain at least one prompt")]
    EmptyChain,
    #[error("automation chain has {actual} prompts, exceeding limit {limit}")]
    StepLimit { actual: usize, limit: usize },
    #[error("automation prompt {index} cannot be empty")]
    EmptyPrompt { index: usize },
    #[error("automation prompt {index} uses {actual} bytes, exceeding limit {limit}")]
    PromptTooLarge {
        index: usize,
        actual: usize,
        limit: usize,
    },
    #[error("automation chain limit {limit} reached")]
    ChainLimit { limit: usize },
    #[error("automation definitions use {attempted} bytes, exceeding limit {limit}")]
    ByteLimit { attempted: usize, limit: usize },
    #[error("automation definitions use schema {actual}; supported schema is {supported}")]
    UnsupportedSchema { actual: u32, supported: u32 },
    #[error("automation definitions contain duplicate chain id {id}")]
    DuplicateChain { id: AutomationChainId },
    #[error("could not create automation state directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not read automation definitions {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not decode automation definitions: {0}")]
    Decode(serde_json::Error),
    #[error("could not encode automation definitions: {0}")]
    Encode(serde_json::Error),
    #[error("could not open atomic automation file {path}: {source}")]
    OpenAtomic {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write automation definitions {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not commit automation definitions {path}: {source}")]
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid automation definitions {path} could not be quarantined: {cause}; {source}")]
    QuarantineFailed {
        path: PathBuf,
        cause: Box<AutomationError>,
        source: std::io::Error,
    },
}

impl AutomationError {
    fn is_recoverable_corruption(&self) -> bool {
        matches!(
            self,
            Self::EmptyName
                | Self::NameTooLarge { .. }
                | Self::EmptyChain
                | Self::StepLimit { .. }
                | Self::EmptyPrompt { .. }
                | Self::PromptTooLarge { .. }
                | Self::ChainLimit { .. }
                | Self::ByteLimit { .. }
                | Self::UnsupportedSchema { .. }
                | Self::DuplicateChain { .. }
                | Self::Decode(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-automation-{name}-{}",
            AutomationExecutionId::new()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn chain(name: &str, prompts: &[&str]) -> AutomationChain {
        AutomationChain {
            id: AutomationChainId::new(),
            name: name.to_owned(),
            prompts: prompts.iter().map(|prompt| (*prompt).to_owned()).collect(),
        }
    }

    #[test]
    fn chain_round_trips_through_atomic_catalog() {
        let root = fixture("round-trip");
        let limits = RuntimeLimits::default();
        let mut store = AutomationStore::open(&root, limits).expect("open");
        let saved = store
            .upsert(chain(" Review loop ", &[" first task ", "second task"]))
            .expect("save");
        assert_eq!(saved.name, "Review loop");
        assert_eq!(saved.prompts, [" first task ", "second task"]);
        drop(store);

        let reopened = AutomationStore::open(&root, limits).expect("reopen");
        assert_eq!(reopened.get(saved.id), Some(&saved));
        assert!(reopened.recovery_notice.is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn chain_limits_fail_without_mutating_existing_catalog() {
        let limits = RuntimeLimits {
            max_automation_steps_per_chain: 1,
            ..RuntimeLimits::default()
        };
        let mut store = AutomationStore::ephemeral(limits);
        let saved = store.upsert(chain("one", &["keep"])).expect("save");
        let replacement = AutomationChain {
            id: saved.id,
            name: "invalid".to_owned(),
            prompts: vec!["one".to_owned(), "two".to_owned()],
        };
        assert!(matches!(
            store.upsert(replacement),
            Err(AutomationError::StepLimit {
                actual: 2,
                limit: 1
            })
        ));
        assert_eq!(store.get(saved.id), Some(&saved));
    }

    #[test]
    fn corrupt_catalog_is_quarantined_independently() {
        let root = fixture("quarantine");
        fs::write(root.join("automation-chains.json"), b"{broken").expect("corrupt file");
        let recovered = AutomationStore::open(&root, RuntimeLimits::default()).expect("recover");
        assert!(recovered.snapshot().chains.is_empty());
        assert!(recovered.snapshot().recovery_notice.is_some());
        assert!(!root.join("automation-chains.json").exists());
        assert!(root.join("automation-quarantine").is_dir());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn execution_snapshot_is_bounded_to_chain_steps() {
        let chain = chain("batch", &["one", "two"]);
        let snapshot = AutomationExecutionSnapshot::new(
            AutomationExecutionId::new(),
            &chain,
            ProjectId::new(),
            2,
            true,
            RuntimeLimits::default(),
        );
        assert_eq!(snapshot.steps.len(), 2);
        assert!(snapshot.steps.iter().all(|step| step.run_id.is_none()));
        assert_eq!(snapshot.status, AutomationExecutionStatus::Starting);
    }

    #[test]
    fn execution_snapshot_keeps_only_a_utf8_safe_prompt_preview() {
        let limits = RuntimeLimits {
            max_automation_prompt_preview_bytes: 5,
            ..RuntimeLimits::default()
        };
        let chain = chain("preview", &["abc😀def"]);
        let snapshot = AutomationExecutionSnapshot::new(
            AutomationExecutionId::new(),
            &chain,
            ProjectId::new(),
            1,
            false,
            limits,
        );
        assert_eq!(snapshot.steps[0].prompt_preview, "abc");
        assert!(snapshot.steps[0].prompt_truncated);
    }
}
