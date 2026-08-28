use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::worktree::{
    CreatedWorktree, WorktreeBaseSnapshot, WorktreeCreatePlan, WorktreeRecoveryProbe,
};
use crate::{ProjectId, RuntimeLimits, WorktreeId};

pub const WORKTREE_REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeRecoveryState {
    Planned,
    Created,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRecoveryRecord {
    pub id: WorktreeId,
    pub project_id: ProjectId,
    pub base: WorktreeBaseSnapshot,
    pub branch: String,
    pub requested_path: PathBuf,
    pub created: Option<CreatedWorktree>,
}

impl WorktreeRecoveryRecord {
    #[must_use]
    pub const fn state(&self) -> WorktreeRecoveryState {
        if self.created.is_some() {
            WorktreeRecoveryState::Created
        } else {
            WorktreeRecoveryState::Planned
        }
    }

    #[must_use]
    pub fn plan(&self) -> WorktreeCreatePlan {
        WorktreeCreatePlan {
            base: self.base.clone(),
            branch: self.branch.clone(),
            worktree_path: self.requested_path.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedWorktreeRegistry {
    schema_version: u32,
    records: Vec<WorktreeRecoveryRecord>,
}

/// Recoverable journal for app-initiated Git worktree creation.
///
/// An intent is durably recorded before the first mutating Git command. If the
/// app crashes after `git worktree add` but before Pi starts, the planned
/// record is still enough to inspect the exact branch/path/base on restart.
/// The registry never deletes Git resources itself.
#[derive(Debug)]
pub struct WorktreeRegistry {
    registry_path: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
    limits: RuntimeLimits,
    by_id: HashMap<WorktreeId, WorktreeRecoveryRecord>,
    recovery_notice: Option<String>,
}

impl WorktreeRegistry {
    #[must_use]
    pub fn ephemeral(limits: RuntimeLimits) -> Self {
        Self {
            registry_path: None,
            quarantine_dir: None,
            limits,
            by_id: HashMap::new(),
            recovery_notice: None,
        }
    }

    pub fn open(
        root: impl AsRef<Path>,
        limits: RuntimeLimits,
    ) -> Result<Self, WorktreeRegistryError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|source| WorktreeRegistryError::CreateDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let registry_path = root.join("worktrees.json");
        let quarantine_dir = root.join("worktree-registry-quarantine");
        let mut registry = Self {
            registry_path: Some(registry_path.clone()),
            quarantine_dir: Some(quarantine_dir),
            limits,
            by_id: HashMap::new(),
            recovery_notice: None,
        };
        match registry.load_from_disk() {
            Ok(()) => Ok(registry),
            Err(WorktreeRegistryError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(registry)
            }
            Err(error) if error.is_recoverable_corruption() => {
                let notice = error.to_string();
                registry.quarantine_corrupt_registry(error)?;
                registry.by_id.clear();
                registry.recovery_notice = Some(notice);
                Ok(registry)
            }
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn recovery_notice(&self) -> Option<&str> {
        self.recovery_notice.as_deref()
    }

    #[must_use]
    pub fn get(&self, id: WorktreeId) -> Option<&WorktreeRecoveryRecord> {
        self.by_id.get(&id)
    }

    #[must_use]
    pub fn records(&self) -> Vec<WorktreeRecoveryRecord> {
        let mut records: Vec<_> = self.by_id.values().cloned().collect();
        records.sort_by_key(|record| record.id.to_string());
        records
    }

    pub fn begin_creation(
        &mut self,
        project_id: ProjectId,
        plan: &WorktreeCreatePlan,
    ) -> Result<WorktreeRecoveryRecord, WorktreeRegistryError> {
        if self.by_id.len() >= self.limits.max_worktree_registry_entries {
            return Err(WorktreeRegistryError::EntryLimit {
                attempted: self.by_id.len().saturating_add(1),
                limit: self.limits.max_worktree_registry_entries,
            });
        }
        if let Some(existing) = self.by_id.values().find(|record| {
            record.requested_path == plan.worktree_path
                || (record.base.repository_root == plan.base.repository_root
                    && record.branch == plan.branch)
        }) {
            return Err(WorktreeRegistryError::RecoveryConflict {
                existing_id: existing.id,
            });
        }
        let record = WorktreeRecoveryRecord {
            id: WorktreeId::new(),
            project_id,
            base: plan.base.clone(),
            branch: plan.branch.clone(),
            requested_path: plan.worktree_path.clone(),
            created: None,
        };
        validate_record(&record, self.limits)?;
        let mut next = self.records();
        next.push(record.clone());
        self.persist_entries(&next)?;
        self.by_id.insert(record.id, record.clone());
        Ok(record)
    }

    pub fn mark_created(
        &mut self,
        id: WorktreeId,
        created: CreatedWorktree,
    ) -> Result<WorktreeRecoveryRecord, WorktreeRegistryError> {
        let current = self
            .by_id
            .get(&id)
            .cloned()
            .ok_or(WorktreeRegistryError::UnknownRecovery { id })?;
        validate_created_matches_plan(&current, &created)?;
        let mut updated = current;
        updated.created = Some(created);
        validate_record(&updated, self.limits)?;
        let mut next = self.records();
        let entry = next
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(WorktreeRegistryError::IndexInconsistent { id })?;
        *entry = updated.clone();
        self.persist_entries(&next)?;
        self.by_id.insert(id, updated.clone());
        Ok(updated)
    }

    /// Remove only an app-owned intent that the caller has independently
    /// proven did not mutate Git. Created records cannot be discarded here.
    pub fn discard_unmutated_plan(&mut self, id: WorktreeId) -> Result<(), WorktreeRegistryError> {
        let current = self
            .by_id
            .get(&id)
            .ok_or(WorktreeRegistryError::UnknownRecovery { id })?;
        if current.created.is_some() {
            return Err(WorktreeRegistryError::CannotDiscardCreated { id });
        }
        let mut next = self.records();
        next.retain(|record| record.id != id);
        self.persist_entries(&next)?;
        self.by_id.remove(&id);
        Ok(())
    }

    /// Retires a journal record only when a fresh recovery probe proves that
    /// neither the captured branch nor the requested worktree path remains.
    pub fn discard_proven_absent(
        &mut self,
        id: WorktreeId,
        proof: &WorktreeRecoveryProbe,
    ) -> Result<(), WorktreeRegistryError> {
        if !matches!(proof, WorktreeRecoveryProbe::NotCreated) {
            return Err(WorktreeRegistryError::RecoveryStillPresent { id });
        }
        if !self.by_id.contains_key(&id) {
            return Err(WorktreeRegistryError::UnknownRecovery { id });
        }
        let mut next = self.records();
        next.retain(|record| record.id != id);
        self.persist_entries(&next)?;
        self.by_id.remove(&id);
        Ok(())
    }

    fn load_from_disk(&mut self) -> Result<(), WorktreeRegistryError> {
        let path = self
            .registry_path
            .as_ref()
            .expect("persistent worktree registry path");
        let metadata = fs::metadata(path).map_err(|source| WorktreeRegistryError::Read {
            path: path.clone(),
            source,
        })?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > self.limits.max_worktree_registry_bytes {
            return Err(WorktreeRegistryError::ByteLimit {
                attempted: size,
                limit: self.limits.max_worktree_registry_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|source| WorktreeRegistryError::Read {
            path: path.clone(),
            source,
        })?;
        let persisted: PersistedWorktreeRegistry =
            serde_json::from_slice(&bytes).map_err(WorktreeRegistryError::Decode)?;
        if persisted.schema_version != WORKTREE_REGISTRY_SCHEMA_VERSION {
            return Err(WorktreeRegistryError::UnsupportedSchema {
                actual: persisted.schema_version,
                supported: WORKTREE_REGISTRY_SCHEMA_VERSION,
            });
        }
        if persisted.records.len() > self.limits.max_worktree_registry_entries {
            return Err(WorktreeRegistryError::EntryLimit {
                attempted: persisted.records.len(),
                limit: self.limits.max_worktree_registry_entries,
            });
        }
        let mut ids = HashSet::with_capacity(persisted.records.len());
        let mut paths = HashSet::with_capacity(persisted.records.len());
        let mut branches = HashSet::with_capacity(persisted.records.len());
        let mut by_id = HashMap::with_capacity(persisted.records.len());
        for record in persisted.records {
            validate_record(&record, self.limits)?;
            if !ids.insert(record.id) {
                return Err(WorktreeRegistryError::DuplicateRecoveryId { id: record.id });
            }
            if !paths.insert(record.requested_path.clone()) {
                return Err(WorktreeRegistryError::DuplicateRequestedPath {
                    path: record.requested_path,
                });
            }
            let branch_key = (record.base.repository_root.clone(), record.branch.clone());
            if !branches.insert(branch_key) {
                return Err(WorktreeRegistryError::DuplicateBranch {
                    repository: record.base.repository_root,
                    branch: record.branch,
                });
            }
            by_id.insert(record.id, record);
        }
        self.by_id = by_id;
        Ok(())
    }

    fn persist_entries(
        &self,
        records: &[WorktreeRecoveryRecord],
    ) -> Result<(), WorktreeRegistryError> {
        let Some(path) = self.registry_path.as_ref() else {
            return Ok(());
        };
        let encoded = serde_json::to_vec(&PersistedWorktreeRegistry {
            schema_version: WORKTREE_REGISTRY_SCHEMA_VERSION,
            records: records.to_vec(),
        })
        .map_err(WorktreeRegistryError::Encode)?;
        if encoded.len() > self.limits.max_worktree_registry_bytes {
            return Err(WorktreeRegistryError::ByteLimit {
                attempted: encoded.len(),
                limit: self.limits.max_worktree_registry_bytes,
            });
        }
        let mut file = AtomicWriteFile::options().open(path).map_err(|source| {
            WorktreeRegistryError::OpenAtomic {
                path: path.clone(),
                source,
            }
        })?;
        file.write_all(&encoded)
            .map_err(|source| WorktreeRegistryError::Write {
                path: path.clone(),
                source,
            })?;
        file.commit()
            .map_err(|source| WorktreeRegistryError::Commit {
                path: path.clone(),
                source,
            })
    }

    fn quarantine_corrupt_registry(
        &self,
        cause: WorktreeRegistryError,
    ) -> Result<(), WorktreeRegistryError> {
        let path = self
            .registry_path
            .as_ref()
            .expect("persistent worktree registry path");
        let quarantine_dir = self
            .quarantine_dir
            .as_ref()
            .expect("persistent worktree quarantine directory");
        if let Err(source) = fs::create_dir_all(quarantine_dir) {
            return Err(WorktreeRegistryError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            });
        }
        let quarantine_path = quarantine_dir.join(format!("{}-worktrees.json", WorktreeId::new()));
        fs::rename(path, quarantine_path).map_err(|source| {
            WorktreeRegistryError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            }
        })
    }
}

fn validate_record(
    record: &WorktreeRecoveryRecord,
    limits: RuntimeLimits,
) -> Result<(), WorktreeRegistryError> {
    for path in [
        &record.base.repository_root,
        &record.base.project_root,
        &record.base.project_relative_path,
        &record.requested_path,
    ] {
        validate_path(path, limits)?;
    }
    validate_ref(&record.base.base_commit, limits)?;
    if let Some(branch) = record.base.source_branch.as_deref() {
        validate_ref(branch, limits)?;
    }
    validate_ref(&record.branch, limits)?;
    if let Some(created) = record.created.as_ref() {
        validate_created_matches_plan(record, created)?;
        for path in [
            &created.repository_root,
            &created.worktree_root,
            &created.execution_root,
        ] {
            validate_path(path, limits)?;
        }
        validate_ref(&created.branch, limits)?;
        validate_ref(&created.base_commit, limits)?;
    }
    Ok(())
}

fn validate_created_matches_plan(
    record: &WorktreeRecoveryRecord,
    created: &CreatedWorktree,
) -> Result<(), WorktreeRegistryError> {
    let requested_matches = record.requested_path.canonicalize().map_or_else(
        |_| comparable_path(&record.requested_path) == comparable_path(&created.worktree_root),
        |path| comparable_path(&path) == comparable_path(&created.worktree_root),
    );
    if created.repository_root != record.base.repository_root
        || created.branch != record.branch
        || created.base_commit != record.base.base_commit
        || !requested_matches
        || !comparable_path(&created.execution_root)
            .starts_with(comparable_path(&created.worktree_root))
    {
        return Err(WorktreeRegistryError::CreatedIdentityMismatch { id: record.id });
    }
    Ok(())
}

#[cfg(windows)]
fn comparable_path(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    let normalized = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        text.into_owned()
    };
    PathBuf::from(normalized.to_lowercase())
}

#[cfg(not(windows))]
fn comparable_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn validate_path(path: &Path, limits: RuntimeLimits) -> Result<(), WorktreeRegistryError> {
    let actual = path.as_os_str().to_string_lossy().len();
    if actual > limits.max_worktree_path_bytes {
        return Err(WorktreeRegistryError::PathTooLong {
            path: path.to_path_buf(),
            actual,
            limit: limits.max_worktree_path_bytes,
        });
    }
    Ok(())
}

fn validate_ref(value: &str, limits: RuntimeLimits) -> Result<(), WorktreeRegistryError> {
    if value.is_empty() || value.len() > limits.max_git_ref_bytes {
        return Err(WorktreeRegistryError::GitRefTooLong {
            actual: value.len(),
            limit: limits.max_git_ref_bytes,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum WorktreeRegistryError {
    #[error("worktree recovery registry contains {attempted} entries, exceeding limit {limit}")]
    EntryLimit { attempted: usize, limit: usize },
    #[error("worktree recovery registry uses {attempted} bytes, exceeding limit {limit}")]
    ByteLimit { attempted: usize, limit: usize },
    #[error("worktree recovery registry uses schema {actual}; supported schema is {supported}")]
    UnsupportedSchema { actual: u32, supported: u32 },
    #[error("worktree recovery registry contains duplicate id {id}")]
    DuplicateRecoveryId { id: WorktreeId },
    #[error("worktree recovery registry contains duplicate requested path {path}")]
    DuplicateRequestedPath { path: PathBuf },
    #[error("worktree recovery registry contains duplicate branch {branch} in {repository}")]
    DuplicateBranch { repository: PathBuf, branch: String },
    #[error("a worktree recovery transaction already owns this branch/path as {existing_id}")]
    RecoveryConflict { existing_id: WorktreeId },
    #[error("worktree recovery transaction {id} is unknown")]
    UnknownRecovery { id: WorktreeId },
    #[error("worktree recovery registry index is inconsistent for {id}")]
    IndexInconsistent { id: WorktreeId },
    #[error("created worktree identity does not match recovery plan {id}")]
    CreatedIdentityMismatch { id: WorktreeId },
    #[error("created worktree recovery {id} cannot be discarded as an unmutated plan")]
    CannotDiscardCreated { id: WorktreeId },
    #[error("worktree recovery {id} still has Git state and cannot be retired")]
    RecoveryStillPresent { id: WorktreeId },
    #[error("Git ref or object identity is {actual} bytes; limit is {limit}")]
    GitRefTooLong { actual: usize, limit: usize },
    #[error("path {path} is {actual} bytes; worktree path limit is {limit}")]
    PathTooLong {
        path: PathBuf,
        actual: usize,
        limit: usize,
    },
    #[error("could not create worktree-registry directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not read worktree recovery registry {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not decode worktree recovery registry: {0}")]
    Decode(serde_json::Error),
    #[error("could not encode worktree recovery registry: {0}")]
    Encode(serde_json::Error),
    #[error("could not open atomic worktree recovery registry {path}: {source}")]
    OpenAtomic {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write worktree recovery registry {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not commit worktree recovery registry {path}: {source}")]
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "invalid worktree recovery registry {path} could not be quarantined: {cause}; {source}"
    )]
    QuarantineFailed {
        path: PathBuf,
        cause: Box<WorktreeRegistryError>,
        source: std::io::Error,
    },
}

impl WorktreeRegistryError {
    fn is_recoverable_corruption(&self) -> bool {
        matches!(
            self,
            Self::EntryLimit { .. }
                | Self::ByteLimit { .. }
                | Self::UnsupportedSchema { .. }
                | Self::DuplicateRecoveryId { .. }
                | Self::DuplicateRequestedPath { .. }
                | Self::DuplicateBranch { .. }
                | Self::CreatedIdentityMismatch { .. }
                | Self::GitRefTooLong { .. }
                | Self::PathTooLong { .. }
                | Self::Decode(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-worktree-registry-{name}-{}",
            WorktreeId::new()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn plan(root: &Path) -> WorktreeCreatePlan {
        let repository = root.join("repo");
        let project = repository.join("project");
        let worktree = root.join("task-worktree");
        fs::create_dir_all(&project).expect("project");
        WorktreeCreatePlan {
            base: WorktreeBaseSnapshot {
                repository_root: repository,
                project_root: project,
                project_relative_path: PathBuf::from("project"),
                source_branch: Some("main".to_owned()),
                base_commit: "abc123".to_owned(),
                dirty: false,
            },
            branch: "agent/task".to_owned(),
            worktree_path: worktree,
        }
    }

    #[test]
    fn planned_intent_is_durable_before_git_creation() {
        let root = fixture("plan");
        let state = root.join("state");
        let mut registry = WorktreeRegistry::open(&state, RuntimeLimits::default()).expect("open");
        let planned = registry
            .begin_creation(ProjectId::new(), &plan(&root))
            .expect("plan");
        assert_eq!(planned.state(), WorktreeRecoveryState::Planned);
        drop(registry);

        let reopened = WorktreeRegistry::open(&state, RuntimeLimits::default()).expect("reopen");
        assert_eq!(reopened.get(planned.id), Some(&planned));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn created_record_survives_reopen_after_external_worktree_removal() {
        let root = fixture("removed-created");
        let state = root.join("state");
        let plan = plan(&root);
        let id = {
            let mut registry =
                WorktreeRegistry::open(&state, RuntimeLimits::default()).expect("open");
            let planned = registry
                .begin_creation(ProjectId::new(), &plan)
                .expect("plan");
            fs::create_dir_all(&plan.worktree_path).expect("worktree path");
            let canonical = plan.worktree_path.canonicalize().expect("worktree root");
            registry
                .mark_created(
                    planned.id,
                    CreatedWorktree {
                        repository_root: plan.base.repository_root.clone(),
                        worktree_root: canonical.clone(),
                        execution_root: canonical,
                        branch: plan.branch.clone(),
                        base_commit: plan.base.base_commit.clone(),
                    },
                )
                .expect("mark created");
            planned.id
        };
        fs::remove_dir_all(&plan.worktree_path).expect("external worktree removal");

        let reopened = WorktreeRegistry::open(&state, RuntimeLimits::default())
            .expect("missing path is recovery state, not registry corruption");
        assert!(reopened.recovery_notice().is_none());
        assert!(reopened.get(id).is_some());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn created_record_can_retire_only_with_explicit_absence_proof() {
        let root = fixture("retire-created");
        let mut registry = WorktreeRegistry::ephemeral(RuntimeLimits::default());
        let plan = plan(&root);
        let planned = registry
            .begin_creation(ProjectId::new(), &plan)
            .expect("plan");
        fs::create_dir_all(&plan.worktree_path).expect("worktree path");
        let canonical = plan.worktree_path.canonicalize().expect("worktree root");
        registry
            .mark_created(
                planned.id,
                CreatedWorktree {
                    repository_root: plan.base.repository_root.clone(),
                    worktree_root: canonical.clone(),
                    execution_root: canonical,
                    branch: plan.branch.clone(),
                    base_commit: plan.base.base_commit.clone(),
                },
            )
            .expect("mark created");

        let partial = WorktreeRecoveryProbe::Partial {
            branch_exists: true,
            path_exists: false,
            detail: "branch remains".to_owned(),
        };
        assert!(matches!(
            registry.discard_proven_absent(planned.id, &partial),
            Err(WorktreeRegistryError::RecoveryStillPresent { id }) if id == planned.id
        ));
        assert!(registry.get(planned.id).is_some());

        registry
            .discard_proven_absent(planned.id, &WorktreeRecoveryProbe::NotCreated)
            .expect("absence proof retires record");
        assert!(registry.get(planned.id).is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn created_identity_is_committed_without_losing_original_plan() {
        let root = fixture("created");
        let state = root.join("state");
        let mut registry = WorktreeRegistry::open(&state, RuntimeLimits::default()).expect("open");
        let plan = plan(&root);
        let planned = registry
            .begin_creation(ProjectId::new(), &plan)
            .expect("plan");
        fs::create_dir_all(&plan.worktree_path).expect("worktree path");
        let created = CreatedWorktree {
            repository_root: plan.base.repository_root.clone(),
            worktree_root: plan.worktree_path.canonicalize().expect("worktree root"),
            execution_root: plan.worktree_path.canonicalize().expect("execution root"),
            branch: plan.branch.clone(),
            base_commit: plan.base.base_commit.clone(),
        };
        let updated = registry
            .mark_created(planned.id, created.clone())
            .expect("mark created");
        assert_eq!(updated.state(), WorktreeRecoveryState::Created);
        assert_eq!(updated.created, Some(created));
        drop(registry);

        let reopened = WorktreeRegistry::open(&state, RuntimeLimits::default()).expect("reopen");
        assert_eq!(reopened.get(planned.id), Some(&updated));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn duplicate_branch_or_target_reuses_existing_recovery_instead_of_overwriting_it() {
        let root = fixture("conflict");
        let mut registry = WorktreeRegistry::ephemeral(RuntimeLimits::default());
        let plan = plan(&root);
        let first = registry
            .begin_creation(ProjectId::new(), &plan)
            .expect("first");
        assert!(matches!(
            registry.begin_creation(ProjectId::new(), &plan),
            Err(WorktreeRegistryError::RecoveryConflict { existing_id }) if existing_id == first.id
        ));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn corrupt_registry_is_quarantined_without_touching_git_paths() {
        let root = fixture("corrupt");
        let state = root.join("state");
        fs::create_dir_all(&state).expect("state");
        fs::write(state.join("worktrees.json"), b"{not-json").expect("corrupt");
        let recovered = WorktreeRegistry::open(&state, RuntimeLimits::default()).expect("recover");
        assert!(recovered.records().is_empty());
        assert!(recovered.recovery_notice().is_some());
        assert!(state.join("worktree-registry-quarantine").is_dir());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
