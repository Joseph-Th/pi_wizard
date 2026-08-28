use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ProjectId;

/// Stable app registration bound to one canonical filesystem location.
///
/// A missing/moved project never falls back to another directory. Relocation
/// is an explicit operation that produces a new canonical binding while
/// retaining the same app-level `ProjectId`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBinding {
    id: ProjectId,
    canonical_root: PathBuf,
}

impl ProjectBinding {
    pub fn register(path: impl AsRef<Path>) -> Result<Self, ProjectBindingError> {
        Self::register_with_id(ProjectId::new(), path)
    }

    pub fn register_with_id(
        id: ProjectId,
        path: impl AsRef<Path>,
    ) -> Result<Self, ProjectBindingError> {
        let canonical_root = canonicalize_project(path.as_ref())?;
        Ok(Self { id, canonical_root })
    }

    /// Restores an app-owned registration from a path that was canonical when
    /// it was persisted. The path is intentionally not re-canonicalized here:
    /// a missing project must remain representable as detached rather than
    /// disappearing from the registry during startup.
    pub fn restore_registered(
        id: ProjectId,
        canonical_root: PathBuf,
    ) -> Result<Self, ProjectBindingError> {
        if !canonical_root.is_absolute() {
            return Err(ProjectBindingError::PersistedRootNotAbsolute {
                path: canonical_root,
            });
        }
        Ok(Self { id, canonical_root })
    }

    #[must_use]
    pub const fn id(&self) -> ProjectId {
        self.id
    }

    #[must_use]
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn verify_candidate(
        &self,
        candidate: impl AsRef<Path>,
    ) -> Result<ProjectPathMatch, ProjectBindingError> {
        let candidate = canonicalize_project(candidate.as_ref())?;
        if candidate == self.canonical_root {
            Ok(ProjectPathMatch::Exact)
        } else {
            Ok(ProjectPathMatch::Different { candidate })
        }
    }

    pub fn verify_registered_location(&self) -> ProjectRegisteredLocation {
        match self.canonical_root.canonicalize() {
            Ok(current) if current == self.canonical_root => ProjectRegisteredLocation::Present,
            Ok(current) => ProjectRegisteredLocation::Changed { current },
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                ProjectRegisteredLocation::Missing
            }
            Err(source) => ProjectRegisteredLocation::Unverifiable {
                error: source.to_string(),
            },
        }
    }

    /// Explicitly move this app registration to a user-selected existing path.
    /// Nothing calls this implicitly during startup/navigation recovery.
    pub fn relocate_explicit(
        &mut self,
        new_root: impl AsRef<Path>,
    ) -> Result<(), ProjectBindingError> {
        self.canonical_root = canonicalize_project(new_root.as_ref())?;
        Ok(())
    }
}

fn canonicalize_project(path: &Path) -> Result<PathBuf, ProjectBindingError> {
    path.canonicalize()
        .map_err(|source| ProjectBindingError::Canonicalize {
            path: path.to_path_buf(),
            source,
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectPathMatch {
    Exact,
    Different { candidate: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRegisteredLocation {
    Present,
    Missing,
    Changed { current: PathBuf },
    Unverifiable { error: String },
}

#[derive(Debug, Error)]
pub enum ProjectBindingError {
    #[error("failed to canonicalize project path {path}: {source}")]
    Canonicalize { path: PathBuf, source: io::Error },
    #[error("persisted project root must be absolute: {path}")]
    PersistedRootNotAbsolute { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-project-binding-{name}-{}",
            ProjectId::new()
        ));
        fs::create_dir_all(&root).expect("create fixture");
        root
    }

    #[test]
    fn registration_uses_canonical_path_and_rejects_different_candidate() {
        let first = fixture("first");
        let second = fixture("second");
        let binding = ProjectBinding::register(&first).expect("register first");

        assert_eq!(
            binding.verify_candidate(&first).expect("verify first"),
            ProjectPathMatch::Exact
        );
        assert!(matches!(
            binding.verify_candidate(&second).expect("verify second"),
            ProjectPathMatch::Different { .. }
        ));

        fs::remove_dir_all(first).expect("remove first");
        fs::remove_dir_all(second).expect("remove second");
    }

    #[test]
    fn missing_registered_path_never_falls_back_to_another_project() {
        let first = fixture("missing");
        let second = fixture("other");
        let binding = ProjectBinding::register(&first).expect("register first");
        fs::remove_dir_all(&first).expect("remove registered root");

        assert_eq!(
            binding.verify_registered_location(),
            ProjectRegisteredLocation::Missing
        );
        assert!(matches!(
            binding
                .verify_candidate(&second)
                .expect("other still exists"),
            ProjectPathMatch::Different { .. }
        ));

        fs::remove_dir_all(second).expect("remove second");
    }

    #[test]
    fn relocation_is_explicit_and_preserves_project_id() {
        let first = fixture("relocate-from");
        let second = fixture("relocate-to");
        let mut binding = ProjectBinding::register(&first).expect("register first");
        let id = binding.id();

        binding
            .relocate_explicit(&second)
            .expect("explicit relocation");

        assert_eq!(binding.id(), id);
        assert_eq!(
            binding.verify_candidate(&second).expect("verify relocated"),
            ProjectPathMatch::Exact
        );

        fs::remove_dir_all(first).expect("remove first");
        fs::remove_dir_all(second).expect("remove second");
    }
}
