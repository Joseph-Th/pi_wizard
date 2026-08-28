use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Safe activation decision for a Pi session whose header declares `session_cwd`.
///
/// Pi can replace its runtime with a session from another cwd. Pi Wizard does
/// not allow that operation to mutate an existing run's immutable execution
/// root because Git/worktree/review ownership is attached to that root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionActivationPlan {
    ReplaceInProcess,
    SpawnNewRun { canonical_target_root: PathBuf },
}

pub fn plan_session_activation(
    current_execution_root: &Path,
    session_cwd: &Path,
) -> Result<SessionActivationPlan, SessionActivationError> {
    let canonical_current = current_execution_root.canonicalize().map_err(|source| {
        SessionActivationError::CanonicalizeCurrentRoot {
            path: current_execution_root.to_path_buf(),
            source,
        }
    })?;
    let canonical_target = session_cwd.canonicalize().map_err(|source| {
        SessionActivationError::CanonicalizeSessionRoot {
            path: session_cwd.to_path_buf(),
            source,
        }
    })?;

    if canonical_current == canonical_target {
        Ok(SessionActivationPlan::ReplaceInProcess)
    } else {
        Ok(SessionActivationPlan::SpawnNewRun {
            canonical_target_root: canonical_target,
        })
    }
}

#[derive(Debug, Error)]
pub enum SessionActivationError {
    #[error("failed to canonicalize current execution root {path}: {source}")]
    CanonicalizeCurrentRoot { path: PathBuf, source: io::Error },
    #[error("failed to canonicalize session cwd {path}: {source}")]
    CanonicalizeSessionRoot { path: PathBuf, source: io::Error },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::RunId;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-session-activation-{name}-{}",
            RunId::new()
        ));
        fs::create_dir_all(&root).expect("create fixture");
        root
    }

    #[test]
    fn same_canonical_root_may_replace_session_in_process() {
        let root = fixture("same");

        assert_eq!(
            plan_session_activation(&root, &root).expect("plan"),
            SessionActivationPlan::ReplaceInProcess
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn different_root_requires_new_run_instead_of_retargeting_process() {
        let current = fixture("current");
        let target = fixture("target");
        let expected = target.canonicalize().expect("canonical target");

        assert_eq!(
            plan_session_activation(&current, &target).expect("plan"),
            SessionActivationPlan::SpawnNewRun {
                canonical_target_root: expected
            }
        );

        fs::remove_dir_all(current).expect("remove current");
        fs::remove_dir_all(target).expect("remove target");
    }

    #[test]
    fn missing_target_is_explicit_failure_not_current_root_fallback() {
        let current = fixture("missing-current");
        let missing = current.join("gone");

        assert!(matches!(
            plan_session_activation(&current, &missing),
            Err(SessionActivationError::CanonicalizeSessionRoot { .. })
        ));

        fs::remove_dir_all(current).expect("remove current");
    }
}
