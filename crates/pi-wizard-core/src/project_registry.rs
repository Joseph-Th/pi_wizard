use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::project::{ProjectBinding, ProjectBindingError};
use crate::{ProjectId, RunId, RuntimeLimits};

pub const PROJECT_REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedProject {
    id: ProjectId,
    canonical_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedProjectRegistry {
    schema_version: u32,
    projects: Vec<PersistedProject>,
}

/// Recoverable app-owned mapping from stable ProjectId to canonical roots.
///
/// Pi session JSONL is not stored here. A corrupt registry can be quarantined
/// and rebuilt from explicit user registrations without touching Pi data.
#[derive(Debug)]
pub struct ProjectRegistry {
    registry_path: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
    limits: RuntimeLimits,
    by_id: HashMap<ProjectId, ProjectBinding>,
    by_root: HashMap<PathBuf, ProjectId>,
    recovery_notice: Option<String>,
}

impl ProjectRegistry {
    #[must_use]
    pub fn ephemeral(limits: RuntimeLimits) -> Self {
        Self {
            registry_path: None,
            quarantine_dir: None,
            limits,
            by_id: HashMap::new(),
            by_root: HashMap::new(),
            recovery_notice: None,
        }
    }

    pub fn open(
        root: impl AsRef<Path>,
        limits: RuntimeLimits,
    ) -> Result<Self, ProjectRegistryError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|source| ProjectRegistryError::CreateDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let registry_path = root.join("projects.json");
        let quarantine_dir = root.join("project-registry-quarantine");
        let mut registry = Self {
            registry_path: Some(registry_path.clone()),
            quarantine_dir: Some(quarantine_dir),
            limits,
            by_id: HashMap::new(),
            by_root: HashMap::new(),
            recovery_notice: None,
        };

        match registry.load_from_disk() {
            Ok(()) => Ok(registry),
            Err(ProjectRegistryError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(registry)
            }
            Err(error) if error.is_recoverable_corruption() => {
                let notice = error.to_string();
                registry.quarantine_corrupt_registry(error)?;
                registry.by_id.clear();
                registry.by_root.clear();
                registry.recovery_notice = Some(notice);
                Ok(registry)
            }
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    #[must_use]
    pub fn recovery_notice(&self) -> Option<&str> {
        self.recovery_notice.as_deref()
    }

    #[must_use]
    pub fn get(&self, id: ProjectId) -> Option<&ProjectBinding> {
        self.by_id.get(&id)
    }

    pub fn resolve_or_register(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<ProjectBinding, ProjectRegistryError> {
        let candidate = ProjectBinding::register(path).map_err(ProjectRegistryError::Binding)?;
        let root = candidate.canonical_root().to_path_buf();
        if let Some(id) = self.by_root.get(&root) {
            return self
                .by_id
                .get(id)
                .cloned()
                .ok_or(ProjectRegistryError::IndexInconsistent { project_id: *id });
        }
        if self.by_id.len() >= self.limits.max_project_registry_entries {
            return Err(ProjectRegistryError::EntryLimit {
                attempted: self.by_id.len().saturating_add(1),
                limit: self.limits.max_project_registry_entries,
            });
        }

        let mut next = self.persisted_entries();
        next.push(PersistedProject {
            id: candidate.id(),
            canonical_root: root.clone(),
        });
        self.persist_entries(&next)?;
        self.by_root.insert(root, candidate.id());
        self.by_id.insert(candidate.id(), candidate.clone());
        Ok(candidate)
    }

    pub fn relocate_explicit(
        &mut self,
        id: ProjectId,
        new_root: impl AsRef<Path>,
    ) -> Result<ProjectBinding, ProjectRegistryError> {
        let current = self
            .by_id
            .get(&id)
            .cloned()
            .ok_or(ProjectRegistryError::UnknownProject { project_id: id })?;
        let mut relocated = current.clone();
        relocated
            .relocate_explicit(new_root)
            .map_err(ProjectRegistryError::Binding)?;
        let new_root = relocated.canonical_root().to_path_buf();
        if let Some(other) = self.by_root.get(&new_root)
            && *other != id
        {
            return Err(ProjectRegistryError::RootAlreadyRegistered {
                path: new_root,
                project_id: *other,
            });
        }

        let mut next = self.persisted_entries();
        let entry = next
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(ProjectRegistryError::IndexInconsistent { project_id: id })?;
        entry.canonical_root = new_root.clone();
        self.persist_entries(&next)?;

        self.by_root.remove(current.canonical_root());
        self.by_root.insert(new_root, id);
        self.by_id.insert(id, relocated.clone());
        Ok(relocated)
    }

    fn load_from_disk(&mut self) -> Result<(), ProjectRegistryError> {
        let path = self
            .registry_path
            .as_ref()
            .expect("persistent registry path");
        let metadata = fs::metadata(path).map_err(|source| ProjectRegistryError::Read {
            path: path.clone(),
            source,
        })?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > self.limits.max_project_registry_bytes {
            return Err(ProjectRegistryError::ByteLimit {
                attempted: size,
                limit: self.limits.max_project_registry_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|source| ProjectRegistryError::Read {
            path: path.clone(),
            source,
        })?;
        let persisted: PersistedProjectRegistry =
            serde_json::from_slice(&bytes).map_err(ProjectRegistryError::Decode)?;
        if persisted.schema_version != PROJECT_REGISTRY_SCHEMA_VERSION {
            return Err(ProjectRegistryError::UnsupportedSchema {
                actual: persisted.schema_version,
                supported: PROJECT_REGISTRY_SCHEMA_VERSION,
            });
        }
        if persisted.projects.len() > self.limits.max_project_registry_entries {
            return Err(ProjectRegistryError::EntryLimit {
                attempted: persisted.projects.len(),
                limit: self.limits.max_project_registry_entries,
            });
        }

        let mut ids = HashSet::with_capacity(persisted.projects.len());
        let mut roots = HashSet::with_capacity(persisted.projects.len());
        let mut by_id = HashMap::with_capacity(persisted.projects.len());
        let mut by_root = HashMap::with_capacity(persisted.projects.len());
        for entry in persisted.projects {
            if !ids.insert(entry.id) {
                return Err(ProjectRegistryError::DuplicateProjectId {
                    project_id: entry.id,
                });
            }
            if !roots.insert(entry.canonical_root.clone()) {
                return Err(ProjectRegistryError::DuplicateRoot {
                    path: entry.canonical_root,
                });
            }
            let binding = ProjectBinding::restore_registered(entry.id, entry.canonical_root)
                .map_err(ProjectRegistryError::Binding)?;
            by_root.insert(binding.canonical_root().to_path_buf(), binding.id());
            by_id.insert(binding.id(), binding);
        }
        self.by_id = by_id;
        self.by_root = by_root;
        Ok(())
    }

    fn persisted_entries(&self) -> Vec<PersistedProject> {
        let mut entries: Vec<_> = self
            .by_id
            .values()
            .map(|binding| PersistedProject {
                id: binding.id(),
                canonical_root: binding.canonical_root().to_path_buf(),
            })
            .collect();
        entries.sort_by_key(|entry| entry.id.to_string());
        entries
    }

    fn persist_entries(&self, projects: &[PersistedProject]) -> Result<(), ProjectRegistryError> {
        let Some(path) = self.registry_path.as_ref() else {
            return Ok(());
        };
        let encoded = serde_json::to_vec(&PersistedProjectRegistry {
            schema_version: PROJECT_REGISTRY_SCHEMA_VERSION,
            projects: projects.to_vec(),
        })
        .map_err(ProjectRegistryError::Encode)?;
        if encoded.len() > self.limits.max_project_registry_bytes {
            return Err(ProjectRegistryError::ByteLimit {
                attempted: encoded.len(),
                limit: self.limits.max_project_registry_bytes,
            });
        }
        let mut file = AtomicWriteFile::options().open(path).map_err(|source| {
            ProjectRegistryError::OpenAtomic {
                path: path.clone(),
                source,
            }
        })?;
        file.write_all(&encoded)
            .map_err(|source| ProjectRegistryError::Write {
                path: path.clone(),
                source,
            })?;
        file.commit()
            .map_err(|source| ProjectRegistryError::Commit {
                path: path.clone(),
                source,
            })
    }

    fn quarantine_corrupt_registry(
        &self,
        cause: ProjectRegistryError,
    ) -> Result<(), ProjectRegistryError> {
        let path = self
            .registry_path
            .as_ref()
            .expect("persistent registry path");
        let quarantine_dir = self
            .quarantine_dir
            .as_ref()
            .expect("persistent quarantine directory");
        if let Err(source) = fs::create_dir_all(quarantine_dir) {
            return Err(ProjectRegistryError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            });
        }
        let quarantine_path = quarantine_dir.join(format!("{}-projects.json", RunId::new()));
        fs::rename(path, &quarantine_path).map_err(|source| {
            ProjectRegistryError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            }
        })
    }
}

#[derive(Debug, Error)]
pub enum ProjectRegistryError {
    #[error(transparent)]
    Binding(#[from] ProjectBindingError),
    #[error("project registry contains {attempted} entries, exceeding limit {limit}")]
    EntryLimit { attempted: usize, limit: usize },
    #[error("project registry uses {attempted} bytes, exceeding limit {limit}")]
    ByteLimit { attempted: usize, limit: usize },
    #[error("project registry uses schema {actual}; supported schema is {supported}")]
    UnsupportedSchema { actual: u32, supported: u32 },
    #[error("project registry contains duplicate project id {project_id}")]
    DuplicateProjectId { project_id: ProjectId },
    #[error("project registry contains duplicate canonical root {path}")]
    DuplicateRoot { path: PathBuf },
    #[error("project root {path} is already registered to {project_id}")]
    RootAlreadyRegistered {
        path: PathBuf,
        project_id: ProjectId,
    },
    #[error("project {project_id} is not registered")]
    UnknownProject { project_id: ProjectId },
    #[error("project registry indexes are inconsistent for {project_id}")]
    IndexInconsistent { project_id: ProjectId },
    #[error("could not create project-registry directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not read project registry {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not decode project registry: {0}")]
    Decode(serde_json::Error),
    #[error("could not encode project registry: {0}")]
    Encode(serde_json::Error),
    #[error("could not open atomic project registry {path}: {source}")]
    OpenAtomic {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write project registry {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not commit project registry {path}: {source}")]
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid project registry {path} could not be quarantined: {cause}; {source}")]
    QuarantineFailed {
        path: PathBuf,
        cause: Box<ProjectRegistryError>,
        source: std::io::Error,
    },
}

impl ProjectRegistryError {
    fn is_recoverable_corruption(&self) -> bool {
        matches!(
            self,
            Self::Binding(_)
                | Self::EntryLimit { .. }
                | Self::ByteLimit { .. }
                | Self::UnsupportedSchema { .. }
                | Self::DuplicateProjectId { .. }
                | Self::DuplicateRoot { .. }
                | Self::Decode(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-project-registry-{name}-{}",
            ProjectId::new()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    #[test]
    fn reopen_preserves_project_id_for_exact_canonical_root() {
        let root = fixture("reopen");
        let project = root.join("project");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("project");
        let id = {
            let mut registry =
                ProjectRegistry::open(&state, RuntimeLimits::default()).expect("registry");
            registry
                .resolve_or_register(&project)
                .expect("register")
                .id()
        };
        let mut reopened = ProjectRegistry::open(&state, RuntimeLimits::default()).expect("reopen");
        assert_eq!(
            reopened
                .resolve_or_register(&project)
                .expect("resolve")
                .id(),
            id
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn missing_registered_project_remains_detached_and_is_not_rebound() {
        let root = fixture("missing");
        let project = root.join("project");
        let other = root.join("other");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&other).expect("other");
        let id = {
            let mut registry =
                ProjectRegistry::open(&state, RuntimeLimits::default()).expect("registry");
            registry
                .resolve_or_register(&project)
                .expect("register")
                .id()
        };
        fs::remove_dir_all(&project).expect("remove project");
        let mut reopened = ProjectRegistry::open(&state, RuntimeLimits::default()).expect("reopen");
        assert_eq!(
            reopened
                .get(id)
                .expect("detached registration")
                .verify_registered_location(),
            crate::project::ProjectRegisteredLocation::Missing
        );
        assert_ne!(
            reopened.resolve_or_register(&other).expect("other").id(),
            id
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn explicit_relocation_preserves_id_across_reopen() {
        let root = fixture("relocate");
        let first = root.join("first");
        let second = root.join("second");
        let state = root.join("state");
        fs::create_dir_all(&first).expect("first");
        fs::create_dir_all(&second).expect("second");
        let id = {
            let mut registry =
                ProjectRegistry::open(&state, RuntimeLimits::default()).expect("registry");
            let id = registry.resolve_or_register(&first).expect("register").id();
            let relocated = registry.relocate_explicit(id, &second).expect("relocate");
            assert_eq!(relocated.id(), id);
            id
        };
        let reopened = ProjectRegistry::open(&state, RuntimeLimits::default()).expect("reopen");
        assert_eq!(
            reopened.get(id).expect("binding").canonical_root(),
            second.canonicalize().unwrap()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn corrupt_registry_is_quarantined_without_blocking_safe_startup() {
        let root = fixture("corrupt");
        let state = root.join("state");
        fs::create_dir_all(&state).expect("state");
        fs::write(state.join("projects.json"), b"{not-json").expect("corrupt registry");
        let registry =
            ProjectRegistry::open(&state, RuntimeLimits::default()).expect("recover registry");
        assert!(registry.is_empty());
        assert!(registry.recovery_notice().is_some());
        assert!(!state.join("projects.json").exists());
        assert!(state.join("project-registry-quarantine").is_dir());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_registration_write_does_not_mutate_in_memory_registry() {
        let root = fixture("register-write-failure");
        let project = root.join("project");
        let state = root.join("state");
        fs::create_dir_all(&project).expect("project");
        let mut registry =
            ProjectRegistry::open(&state, RuntimeLimits::default()).expect("registry");
        fs::create_dir_all(state.join("projects.json"))
            .expect("block registry file with directory");

        assert!(registry.resolve_or_register(&project).is_err());
        assert!(registry.is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_relocation_write_keeps_previous_binding_and_indexes() {
        let root = fixture("relocate-write-failure");
        let first = root.join("first");
        let second = root.join("second");
        let state = root.join("state");
        fs::create_dir_all(&first).expect("first");
        fs::create_dir_all(&second).expect("second");
        let mut registry =
            ProjectRegistry::open(&state, RuntimeLimits::default()).expect("registry");
        let original = registry.resolve_or_register(&first).expect("register");
        fs::remove_file(state.join("projects.json")).expect("remove registry file");
        fs::create_dir_all(state.join("projects.json"))
            .expect("block registry file with directory");

        assert!(registry.relocate_explicit(original.id(), &second).is_err());
        assert_eq!(
            registry
                .get(original.id())
                .expect("original registration remains")
                .canonical_root(),
            original.canonical_root()
        );
        assert_eq!(
            registry
                .resolve_or_register(&first)
                .expect("original root still indexed")
                .id(),
            original.id()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn duplicate_and_oversized_registry_files_are_quarantined() {
        let duplicate_root = fixture("duplicate");
        let duplicate_state = duplicate_root.join("state");
        let project = duplicate_root.join("project");
        fs::create_dir_all(&duplicate_state).expect("state");
        fs::create_dir_all(&project).expect("project");
        let canonical = project.canonicalize().expect("canonical project");
        let duplicate = PersistedProjectRegistry {
            schema_version: PROJECT_REGISTRY_SCHEMA_VERSION,
            projects: vec![
                PersistedProject {
                    id: ProjectId::new(),
                    canonical_root: canonical.clone(),
                },
                PersistedProject {
                    id: ProjectId::new(),
                    canonical_root: canonical,
                },
            ],
        };
        fs::write(
            duplicate_state.join("projects.json"),
            serde_json::to_vec(&duplicate).expect("encode duplicate"),
        )
        .expect("write duplicate registry");
        let recovered = ProjectRegistry::open(&duplicate_state, RuntimeLimits::default())
            .expect("duplicate quarantine");
        assert!(recovered.is_empty());
        assert!(recovered.recovery_notice().is_some());
        fs::remove_dir_all(duplicate_root).expect("duplicate cleanup");

        let oversized_root = fixture("oversized");
        let oversized_state = oversized_root.join("state");
        fs::create_dir_all(&oversized_state).expect("state");
        fs::write(oversized_state.join("projects.json"), vec![b'x'; 128])
            .expect("write oversized registry");
        let limits = RuntimeLimits {
            max_project_registry_bytes: 64,
            ..RuntimeLimits::default()
        };
        let recovered =
            ProjectRegistry::open(&oversized_state, limits).expect("oversized quarantine");
        assert!(recovered.is_empty());
        assert!(recovered.recovery_notice().is_some());
        fs::remove_dir_all(oversized_root).expect("oversized cleanup");
    }
}
