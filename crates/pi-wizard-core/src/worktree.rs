use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;
use crate::environment::ResolvedLaunchEnvironment;
use crate::probe::{ProbeOutput, run_bounded_command};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeBaseSnapshot {
    pub repository_root: PathBuf,
    pub project_root: PathBuf,
    pub project_relative_path: PathBuf,
    pub source_branch: Option<String>,
    pub base_commit: String,
    pub dirty: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum WorktreeCleanupResult {
    Removed,
    Partial {
        #[serde(rename = "branchExists")]
        branch_exists: bool,
        #[serde(rename = "pathExists")]
        path_exists: bool,
        detail: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum WorktreeRecoveryProbe {
    NotCreated,
    Exact {
        created: CreatedWorktree,
    },
    Partial {
        branch_exists: bool,
        path_exists: bool,
        detail: String,
    },
}

/// Removes an app-created worktree only when it is still the exact recorded
/// worktree, has no working-tree changes, and its branch has never advanced
/// beyond the captured creation base. The branch deletion uses an expected-old
/// object ID so a concurrent branch update is preserved rather than guessed
/// away. Any partial mutation remains observable through the recovery journal.
pub async fn cleanup_pristine_worktree(
    plan: &WorktreeCreatePlan,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<WorktreeCleanupResult, WorktreeError> {
    validate_ref_text(&plan.branch, limits)?;
    validate_path_bytes(&plan.worktree_path, limits)?;
    let created = verify_existing_worktree(
        &plan.base,
        &plan.branch,
        &plan.worktree_path,
        false,
        environment,
        limits,
    )
    .await?;

    let head = git_text(
        environment,
        &created.worktree_root,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        "verify cleanup worktree HEAD",
        limits,
    )
    .await?;
    if head != plan.base.base_commit {
        return Err(WorktreeError::CleanupContainsCommits {
            base_commit: plan.base.base_commit.clone(),
            head,
        });
    }

    let status = git_raw(
        environment,
        &created.worktree_root,
        ["status", "--porcelain=v1", "--untracked-files=normal"],
        1,
        limits,
    )
    .await?;
    if !status.status.success() {
        return Err(WorktreeError::GitCommandFailed {
            operation: "verify cleanup worktree status",
            code: status.status.code(),
        });
    }
    if status.stdout_exceeded || !status.stdout.is_empty() {
        return Err(WorktreeError::CleanupDirty {
            path: created.worktree_root,
        });
    }

    let remove = git_raw_os(
        environment,
        &plan.base.repository_root,
        vec![
            OsString::from("worktree"),
            OsString::from("remove"),
            path_argument_for_git(&created.worktree_root),
        ],
        limits.max_git_command_output_bytes,
        limits,
    )
    .await?;
    if !remove.status.success() {
        return Err(WorktreeError::CleanupCommandFailed {
            operation: "remove pristine worktree",
            code: remove.status.code(),
            path_exists: plan.worktree_path.exists(),
        });
    }

    let full_ref = format!("refs/heads/{}", plan.branch);
    let delete_branch = git_raw_os(
        environment,
        &plan.base.repository_root,
        vec![
            OsString::from("update-ref"),
            OsString::from("-d"),
            OsString::from(&full_ref),
            OsString::from(&plan.base.base_commit),
        ],
        limits.max_git_command_output_bytes,
        limits,
    )
    .await;
    match delete_branch {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            return cleanup_partial_result(
                plan,
                environment,
                limits,
                format!(
                    "worktree path was removed but exact branch deletion failed with exit code {:?}",
                    output.status.code()
                ),
            )
            .await;
        }
        Err(error) => {
            return cleanup_partial_result(
                plan,
                environment,
                limits,
                format!(
                    "worktree path was removed but branch deletion became indeterminate: {error}"
                ),
            )
            .await;
        }
    }

    match probe_worktree_recovery(plan, environment, limits).await? {
        WorktreeRecoveryProbe::NotCreated => Ok(WorktreeCleanupResult::Removed),
        WorktreeRecoveryProbe::Partial {
            branch_exists,
            path_exists,
            detail,
        } => Ok(WorktreeCleanupResult::Partial {
            branch_exists,
            path_exists,
            detail,
        }),
        WorktreeRecoveryProbe::Exact { .. } => Ok(WorktreeCleanupResult::Partial {
            branch_exists: true,
            path_exists: true,
            detail: "cleanup commands completed but the recorded worktree still exists".to_owned(),
        }),
    }
}

async fn cleanup_partial_result(
    plan: &WorktreeCreatePlan,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
    fallback_detail: String,
) -> Result<WorktreeCleanupResult, WorktreeError> {
    match probe_worktree_recovery(plan, environment, limits).await {
        Ok(WorktreeRecoveryProbe::NotCreated) => Ok(WorktreeCleanupResult::Removed),
        Ok(WorktreeRecoveryProbe::Partial {
            branch_exists,
            path_exists,
            detail,
        }) => Ok(WorktreeCleanupResult::Partial {
            branch_exists,
            path_exists,
            detail,
        }),
        Ok(WorktreeRecoveryProbe::Exact { .. }) => Ok(WorktreeCleanupResult::Partial {
            branch_exists: true,
            path_exists: true,
            detail: fallback_detail,
        }),
        Err(_) => Ok(WorktreeCleanupResult::Partial {
            branch_exists: true,
            path_exists: plan.worktree_path.exists(),
            detail: bounded_failure_detail(&fallback_detail, limits),
        }),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeCreatePlan {
    pub base: WorktreeBaseSnapshot,
    pub branch: String,
    pub worktree_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedWorktree {
    pub repository_root: PathBuf,
    pub worktree_root: PathBuf,
    pub execution_root: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktreeIdentity {
    pub repository_root: PathBuf,
    pub worktree_root: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

impl CreatedWorktree {
    #[must_use]
    pub fn identity(&self) -> GitWorktreeIdentity {
        GitWorktreeIdentity {
            repository_root: self.repository_root.clone(),
            worktree_root: self.worktree_root.clone(),
            branch: self.branch.clone(),
            base_commit: self.base_commit.clone(),
        }
    }
}

pub async fn inspect_worktree_base(
    project_root: &Path,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<WorktreeBaseSnapshot, WorktreeError> {
    let project_root =
        project_root
            .canonicalize()
            .map_err(|source| WorktreeError::CanonicalizeProject {
                path: project_root.to_path_buf(),
                source,
            })?;
    validate_path_bytes(&project_root, limits)?;
    let repository_root = PathBuf::from(
        git_text(
            environment,
            &project_root,
            &["rev-parse", "--show-toplevel"],
            "resolve repository root",
            limits,
        )
        .await?,
    )
    .canonicalize()
    .map_err(|source| WorktreeError::CanonicalizeRepository {
        path: project_root.clone(),
        source,
    })?;
    validate_path_bytes(&repository_root, limits)?;
    let project_relative_path = project_root
        .strip_prefix(&repository_root)
        .map_err(|_| WorktreeError::ProjectOutsideRepository {
            project: project_root.clone(),
            repository: repository_root.clone(),
        })?
        .to_path_buf();

    let base_commit = git_text(
        environment,
        &repository_root,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        "resolve base commit",
        limits,
    )
    .await?;
    validate_ref_text(&base_commit, limits)?;

    let branch_output = git_raw(
        environment,
        &repository_root,
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
        limits.max_git_ref_bytes,
        limits,
    )
    .await?;
    let source_branch = if branch_output.status.success() {
        let branch = bounded_utf8(branch_output, "resolve source branch", limits)?;
        validate_ref_text(&branch, limits)?;
        Some(branch)
    } else if branch_output.status.code() == Some(1) {
        None
    } else {
        return Err(WorktreeError::GitCommandFailed {
            operation: "resolve source branch",
            code: branch_output.status.code(),
        });
    };

    if !project_relative_path.as_os_str().is_empty() {
        let relative = project_relative_path
            .to_str()
            .ok_or_else(|| WorktreeError::NonUtf8ProjectRelativePath {
                path: project_relative_path.clone(),
            })?
            .replace('\\', "/");
        let object = format!("{base_commit}:{relative}");
        let output = git_raw_os(
            environment,
            &repository_root,
            vec![
                OsString::from("cat-file"),
                OsString::from("-e"),
                OsString::from(object),
            ],
            1,
            limits,
        )
        .await?;
        if !output.status.success() {
            return Err(WorktreeError::ProjectMissingFromBase {
                relative: project_relative_path.clone(),
                base_commit: base_commit.clone(),
            });
        }
    }

    let status = git_raw(
        environment,
        &repository_root,
        ["status", "--porcelain=v1", "--untracked-files=normal"],
        1,
        limits,
    )
    .await?;
    if !status.status.success() {
        return Err(WorktreeError::GitCommandFailed {
            operation: "inspect working tree status",
            code: status.status.code(),
        });
    }

    Ok(WorktreeBaseSnapshot {
        repository_root,
        project_root,
        project_relative_path,
        source_branch,
        base_commit,
        dirty: status.stdout_exceeded || !status.stdout.is_empty(),
    })
}

pub async fn create_worktree(
    plan: WorktreeCreatePlan,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<CreatedWorktree, WorktreeError> {
    validate_ref_text(&plan.branch, limits)?;
    validate_path_bytes(&plan.worktree_path, limits)?;
    if !plan.worktree_path.is_absolute() {
        return Err(WorktreeError::WorktreePathMustBeAbsolute {
            path: plan.worktree_path,
        });
    }
    if plan.worktree_path.exists() {
        return Err(WorktreeError::WorktreePathExists {
            path: plan.worktree_path,
        });
    }

    let current = inspect_worktree_base(&plan.base.project_root, environment, limits).await?;
    if current.repository_root != plan.base.repository_root
        || current.project_root != plan.base.project_root
        || current.project_relative_path != plan.base.project_relative_path
        || current.source_branch != plan.base.source_branch
        || current.base_commit != plan.base.base_commit
    {
        return Err(WorktreeError::SourceChanged {
            expected_branch: plan.base.source_branch,
            expected_commit: plan.base.base_commit,
            actual_branch: current.source_branch,
            actual_commit: current.base_commit,
        });
    }

    let check_ref = git_raw_os(
        environment,
        &current.repository_root,
        vec![
            OsString::from("check-ref-format"),
            OsString::from("--branch"),
            OsString::from(&plan.branch),
        ],
        limits.max_git_ref_bytes,
        limits,
    )
    .await?;
    if !check_ref.status.success() {
        return Err(WorktreeError::InvalidBranch {
            branch: plan.branch,
        });
    }

    let full_ref = format!("refs/heads/{}", plan.branch);
    let branch_exists = git_raw_os(
        environment,
        &current.repository_root,
        vec![
            OsString::from("show-ref"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(&full_ref),
        ],
        1,
        limits,
    )
    .await?;
    if branch_exists.status.success() {
        return Err(WorktreeError::BranchExists {
            branch: plan.branch,
        });
    }
    if branch_exists.status.code() != Some(1) {
        return Err(WorktreeError::GitCommandFailed {
            operation: "check target branch",
            code: branch_exists.status.code(),
        });
    }

    let parent =
        plan.worktree_path
            .parent()
            .ok_or_else(|| WorktreeError::WorktreeParentMissing {
                path: plan.worktree_path.clone(),
            })?;
    let parent =
        parent
            .canonicalize()
            .map_err(|source| WorktreeError::CanonicalizeWorktreeParent {
                path: parent.to_path_buf(),
                source,
            })?;
    let leaf =
        plan.worktree_path
            .file_name()
            .ok_or_else(|| WorktreeError::WorktreeParentMissing {
                path: plan.worktree_path.clone(),
            })?;
    let target = parent.join(leaf);
    validate_path_bytes(&target, limits)?;
    if target.starts_with(&current.repository_root) {
        return Err(WorktreeError::WorktreeInsideSourceRepository {
            path: target,
            repository: current.repository_root,
        });
    }

    let add = match git_raw_os(
        environment,
        &current.repository_root,
        vec![
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("-b"),
            OsString::from(&plan.branch),
            path_argument_for_git(&target),
            OsString::from(&current.base_commit),
        ],
        limits.max_git_command_output_bytes,
        limits,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            return Err(WorktreeError::CreateIndeterminate {
                branch: plan.branch,
                path: target.clone(),
                detail: error.to_string(),
                branch_may_exist: branch_exists_at(
                    environment,
                    &current.repository_root,
                    &full_ref,
                    limits,
                )
                .await,
                path_exists: target.exists(),
            });
        }
    };
    if !add.status.success() {
        return Err(WorktreeError::CreateFailed {
            branch: plan.branch,
            path: target.clone(),
            code: add.status.code(),
            branch_may_exist: branch_exists_at(
                environment,
                &current.repository_root,
                &full_ref,
                limits,
            )
            .await,
            path_exists: target.exists(),
        });
    }

    verify_existing_worktree(&current, &plan.branch, &target, true, environment, limits)
        .await
        .map_err(|error| WorktreeError::PostCreateVerification {
            path: target,
            detail: error.to_string(),
        })
}

/// Inspect a durable creation intent after restart without mutating Git.
///
/// `NotCreated` is returned only when both the requested path and branch are
/// proven absent. Any partial or conflicting state stays explicit so callers
/// cannot mistake an orphaned branch/path for a safe-to-forget intent.
pub async fn probe_worktree_recovery(
    plan: &WorktreeCreatePlan,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<WorktreeRecoveryProbe, WorktreeError> {
    validate_ref_text(&plan.branch, limits)?;
    validate_path_bytes(&plan.worktree_path, limits)?;
    if !plan.worktree_path.is_absolute() {
        return Err(WorktreeError::WorktreePathMustBeAbsolute {
            path: plan.worktree_path.clone(),
        });
    }
    let full_ref = format!("refs/heads/{}", plan.branch);
    let branch_exists =
        branch_exists_at_result(environment, &plan.base.repository_root, &full_ref, limits).await?;
    let path_exists = plan.worktree_path.exists();
    if !branch_exists && !path_exists {
        return Ok(WorktreeRecoveryProbe::NotCreated);
    }
    if path_exists {
        match verify_existing_worktree(
            &plan.base,
            &plan.branch,
            &plan.worktree_path,
            false,
            environment,
            limits,
        )
        .await
        {
            Ok(created) => return Ok(WorktreeRecoveryProbe::Exact { created }),
            Err(error) => {
                return Ok(WorktreeRecoveryProbe::Partial {
                    branch_exists,
                    path_exists,
                    detail: bounded_failure_detail(&error.to_string(), limits),
                });
            }
        }
    }
    Ok(WorktreeRecoveryProbe::Partial {
        branch_exists,
        path_exists,
        detail: "captured branch exists but requested worktree path does not".to_owned(),
    })
}

async fn verify_existing_worktree(
    base: &WorktreeBaseSnapshot,
    branch: &str,
    target: &Path,
    require_head_at_base: bool,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<CreatedWorktree, WorktreeError> {
    let worktree_root =
        target
            .canonicalize()
            .map_err(|source| WorktreeError::CanonicalizeExistingWorktree {
                path: target.to_path_buf(),
                source,
            })?;
    validate_path_bytes(&worktree_root, limits)?;
    let verified_root = PathBuf::from(
        git_text(
            environment,
            &worktree_root,
            &["rev-parse", "--show-toplevel"],
            "verify worktree repository root",
            limits,
        )
        .await?,
    )
    .canonicalize()
    .map_err(|source| WorktreeError::CanonicalizeExistingWorktree {
        path: worktree_root.clone(),
        source,
    })?;
    if verified_root != worktree_root {
        return Err(WorktreeError::ExistingWorktreeMismatch {
            detail: "Git resolved a different worktree root".to_owned(),
        });
    }

    let source_common = git_common_dir(
        environment,
        &base.repository_root,
        "resolve source Git common directory",
        limits,
    )
    .await?;
    let worktree_common = git_common_dir(
        environment,
        &worktree_root,
        "resolve worktree Git common directory",
        limits,
    )
    .await?;
    if source_common != worktree_common {
        return Err(WorktreeError::ExistingWorktreeMismatch {
            detail: "requested path belongs to a different Git repository".to_owned(),
        });
    }

    let verified_commit = git_text(
        environment,
        &worktree_root,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        "verify worktree base commit",
        limits,
    )
    .await?;
    let verified_branch = git_text(
        environment,
        &worktree_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "verify worktree branch",
        limits,
    )
    .await?;
    if verified_branch != branch {
        return Err(WorktreeError::ExistingWorktreeMismatch {
            detail: format!(
                "expected branch {branch}, found {verified_branch} at {verified_commit}"
            ),
        });
    }
    if require_head_at_base {
        if verified_commit != base.base_commit {
            return Err(WorktreeError::ExistingWorktreeMismatch {
                detail: format!(
                    "expected newly created worktree at {}, found {verified_commit}",
                    base.base_commit
                ),
            });
        }
    } else if !git_is_ancestor(
        environment,
        &worktree_root,
        &base.base_commit,
        "HEAD",
        limits,
    )
    .await?
    {
        return Err(WorktreeError::ExistingWorktreeMismatch {
            detail: format!(
                "captured base {} is not an ancestor of current HEAD {verified_commit}",
                base.base_commit
            ),
        });
    }
    let execution_root = worktree_root
        .join(&base.project_relative_path)
        .canonicalize()
        .map_err(|source| WorktreeError::CanonicalizeExistingWorktree {
            path: worktree_root.join(&base.project_relative_path),
            source,
        })?;
    Ok(CreatedWorktree {
        repository_root: base.repository_root.clone(),
        worktree_root,
        execution_root,
        branch: branch.to_owned(),
        base_commit: base.base_commit.clone(),
    })
}

async fn git_is_ancestor(
    environment: &ResolvedLaunchEnvironment,
    cwd: &Path,
    ancestor: &str,
    descendant: &str,
    limits: RuntimeLimits,
) -> Result<bool, WorktreeError> {
    let output = git_raw_os(
        environment,
        cwd,
        vec![
            OsString::from("merge-base"),
            OsString::from("--is-ancestor"),
            OsString::from(ancestor),
            OsString::from(descendant),
        ],
        limits.max_git_command_output_bytes,
        limits,
    )
    .await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        code => Err(WorktreeError::GitCommandFailed {
            operation: "verify captured base ancestry",
            code,
        }),
    }
}

async fn git_common_dir(
    environment: &ResolvedLaunchEnvironment,
    cwd: &Path,
    operation: &'static str,
    limits: RuntimeLimits,
) -> Result<PathBuf, WorktreeError> {
    let raw = PathBuf::from(
        git_text(
            environment,
            cwd,
            &["rev-parse", "--git-common-dir"],
            operation,
            limits,
        )
        .await?,
    );
    let path = if raw.is_absolute() {
        raw
    } else {
        cwd.join(raw)
    };
    path.canonicalize()
        .map_err(|source| WorktreeError::CanonicalizeGitCommonDir { path, source })
}

fn bounded_failure_detail(detail: &str, limits: RuntimeLimits) -> String {
    if detail.len() <= limits.max_failure_detail_bytes {
        return detail.to_owned();
    }
    let mut end = limits.max_failure_detail_bytes;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_owned()
}

async fn branch_exists_at(
    environment: &ResolvedLaunchEnvironment,
    repository_root: &Path,
    full_ref: &str,
    limits: RuntimeLimits,
) -> bool {
    branch_exists_at_result(environment, repository_root, full_ref, limits)
        .await
        .unwrap_or(false)
}

async fn branch_exists_at_result(
    environment: &ResolvedLaunchEnvironment,
    repository_root: &Path,
    full_ref: &str,
    limits: RuntimeLimits,
) -> Result<bool, WorktreeError> {
    let output = git_raw_os(
        environment,
        repository_root,
        vec![
            OsString::from("show-ref"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(full_ref),
        ],
        1,
        limits,
    )
    .await?;
    if output.status.success() {
        Ok(true)
    } else if output.status.code() == Some(1) {
        Ok(false)
    } else {
        Err(WorktreeError::GitCommandFailed {
            operation: "check recovery branch",
            code: output.status.code(),
        })
    }
}

async fn git_text(
    environment: &ResolvedLaunchEnvironment,
    cwd: &Path,
    args: &[&str],
    operation: &'static str,
    limits: RuntimeLimits,
) -> Result<String, WorktreeError> {
    let output = git_raw(
        environment,
        cwd,
        args.iter().copied(),
        limits.max_git_command_output_bytes,
        limits,
    )
    .await?;
    if !output.status.success() {
        return Err(WorktreeError::GitCommandFailed {
            operation,
            code: output.status.code(),
        });
    }
    bounded_utf8(output, operation, limits)
}

async fn git_raw<'a>(
    environment: &ResolvedLaunchEnvironment,
    cwd: &Path,
    args: impl IntoIterator<Item = &'a str>,
    max_bytes: usize,
    limits: RuntimeLimits,
) -> Result<ProbeOutput, WorktreeError> {
    git_raw_os(
        environment,
        cwd,
        args.into_iter().map(OsString::from).collect(),
        max_bytes,
        limits,
    )
    .await
}

async fn git_raw_os(
    environment: &ResolvedLaunchEnvironment,
    cwd: &Path,
    args: Vec<OsString>,
    max_bytes: usize,
    limits: RuntimeLimits,
) -> Result<ProbeOutput, WorktreeError> {
    let git = environment
        .git_executable()
        .ok_or(WorktreeError::GitUnavailable)?;
    run_bounded_command(
        git,
        &args,
        Some(cwd),
        environment.variables(),
        max_bytes.max(1),
        Duration::from_millis(limits.git_command_deadline_ms),
    )
    .await
    .map_err(|source| WorktreeError::GitExecution {
        detail: source.to_string(),
    })
}

fn bounded_utf8(
    output: ProbeOutput,
    operation: &'static str,
    limits: RuntimeLimits,
) -> Result<String, WorktreeError> {
    if output.stdout_exceeded {
        return Err(WorktreeError::GitOutputTooLarge {
            operation,
            limit: limits.max_git_command_output_bytes,
        });
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| WorktreeError::GitOutputInvalidUtf8 { operation })?
        .trim()
        .to_owned();
    if text.is_empty() {
        return Err(WorktreeError::GitOutputEmpty { operation });
    }
    Ok(text)
}

fn validate_ref_text(value: &str, limits: RuntimeLimits) -> Result<(), WorktreeError> {
    if value.is_empty() || value.len() > limits.max_git_ref_bytes {
        return Err(WorktreeError::GitRefTooLong {
            actual: value.len(),
            limit: limits.max_git_ref_bytes,
        });
    }
    Ok(())
}

fn path_argument_for_git(path: &Path) -> OsString {
    #[cfg(windows)]
    {
        let value = path.as_os_str().to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return OsString::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return OsString::from(rest);
        }
    }
    path.as_os_str().to_os_string()
}

fn validate_path_bytes(path: &Path, limits: RuntimeLimits) -> Result<(), WorktreeError> {
    let actual = path.as_os_str().to_string_lossy().len();
    if actual > limits.max_worktree_path_bytes {
        return Err(WorktreeError::PathTooLong {
            path: path.to_path_buf(),
            actual,
            limit: limits.max_worktree_path_bytes,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("Git is not available in the resolved Pi launch environment")]
    GitUnavailable,
    #[error("failed to canonicalize project path {path}: {source}")]
    CanonicalizeProject { path: PathBuf, source: io::Error },
    #[error("failed to canonicalize Git repository containing {path}: {source}")]
    CanonicalizeRepository { path: PathBuf, source: io::Error },
    #[error("failed to canonicalize existing worktree path {path}: {source}")]
    CanonicalizeExistingWorktree { path: PathBuf, source: io::Error },
    #[error("failed to canonicalize Git common directory {path}: {source}")]
    CanonicalizeGitCommonDir { path: PathBuf, source: io::Error },
    #[error("project {project} is not inside resolved repository {repository}")]
    ProjectOutsideRepository {
        project: PathBuf,
        repository: PathBuf,
    },
    #[error(
        "project-relative path is not representable as UTF-8 for Git object verification: {path}"
    )]
    NonUtf8ProjectRelativePath { path: PathBuf },
    #[error("project subdirectory {relative} does not exist in captured base commit {base_commit}")]
    ProjectMissingFromBase {
        relative: PathBuf,
        base_commit: String,
    },
    #[error("Git command for {operation} failed with exit code {code:?}")]
    GitCommandFailed {
        operation: &'static str,
        code: Option<i32>,
    },
    #[error("Git command could not execute: {detail}")]
    GitExecution { detail: String },
    #[error("Git output for {operation} exceeded {limit} bytes")]
    GitOutputTooLarge {
        operation: &'static str,
        limit: usize,
    },
    #[error("Git output for {operation} is not valid UTF-8")]
    GitOutputInvalidUtf8 { operation: &'static str },
    #[error("Git output for {operation} was unexpectedly empty")]
    GitOutputEmpty { operation: &'static str },
    #[error("Git ref or object identity is {actual} bytes; limit is {limit}")]
    GitRefTooLong { actual: usize, limit: usize },
    #[error("path {path} is {actual} bytes; worktree path limit is {limit}")]
    PathTooLong {
        path: PathBuf,
        actual: usize,
        limit: usize,
    },
    #[error(
        "worktree source moved since inspection: expected branch {expected_branch:?} at {expected_commit}, found {actual_branch:?} at {actual_commit}"
    )]
    SourceChanged {
        expected_branch: Option<String>,
        expected_commit: String,
        actual_branch: Option<String>,
        actual_commit: String,
    },
    #[error("invalid new Git branch name {branch}")]
    InvalidBranch { branch: String },
    #[error("Git branch {branch} already exists")]
    BranchExists { branch: String },
    #[error("worktree path must be absolute: {path}")]
    WorktreePathMustBeAbsolute { path: PathBuf },
    #[error("worktree path already exists: {path}")]
    WorktreePathExists { path: PathBuf },
    #[error("worktree path has no existing parent directory: {path}")]
    WorktreeParentMissing { path: PathBuf },
    #[error("failed to canonicalize worktree parent {path}: {source}")]
    CanonicalizeWorktreeParent { path: PathBuf, source: io::Error },
    #[error("worktree path {path} must not be nested inside source repository {repository}")]
    WorktreeInsideSourceRepository { path: PathBuf, repository: PathBuf },
    #[error(
        "Git worktree creation failed for branch {branch} at {path} with code {code:?}; branch may exist: {branch_may_exist}; path exists: {path_exists}"
    )]
    CreateFailed {
        branch: String,
        path: PathBuf,
        code: Option<i32>,
        branch_may_exist: bool,
        path_exists: bool,
    },
    #[error(
        "Git worktree creation became indeterminate for branch {branch} at {path}: {detail}; branch may exist: {branch_may_exist}; path exists: {path_exists}"
    )]
    CreateIndeterminate {
        branch: String,
        path: PathBuf,
        detail: String,
        branch_may_exist: bool,
        path_exists: bool,
    },
    #[error(
        "created worktree at {path} failed verification and was retained for explicit recovery: {detail}"
    )]
    PostCreateVerification { path: PathBuf, detail: String },
    #[error("existing worktree does not match captured recovery identity: {detail}")]
    ExistingWorktreeMismatch { detail: String },
    #[error("worktree cleanup refused because {path} has uncommitted or untracked changes")]
    CleanupDirty { path: PathBuf },
    #[error(
        "worktree cleanup refused because task branch advanced beyond captured base {base_commit}; current HEAD is {head}"
    )]
    CleanupContainsCommits { base_commit: String, head: String },
    #[error(
        "worktree cleanup command for {operation} failed with exit code {code:?}; path still exists: {path_exists}"
    )]
    CleanupCommandFailed {
        operation: &'static str,
        code: Option<i32>,
        path_exists: bool,
    },
}

impl WorktreeError {
    #[must_use]
    pub const fn may_have_mutated(&self) -> bool {
        match self {
            Self::CreateFailed {
                branch_may_exist,
                path_exists,
                ..
            } => *branch_may_exist || *path_exists,
            Self::CreateIndeterminate { .. } | Self::PostCreateVerification { .. } => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::process::Command;

    use super::*;
    use crate::RunId;
    use crate::environment::{LaunchEnvironmentInput, resolve_launch_environment};

    struct Fixture {
        root: PathBuf,
        repository: PathBuf,
        project: PathBuf,
        environment: ResolvedLaunchEnvironment,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("pi-wizard-worktree-{name}-{}", RunId::new()));
            let repository = root.join("repo");
            let project = repository.join("project");
            fs::create_dir_all(&project).expect("create fixture project");

            #[cfg(windows)]
            let pi = root.join("pi.cmd");
            #[cfg(not(windows))]
            let pi = root.join("pi");
            #[cfg(windows)]
            fs::write(&pi, "@echo off\r\nexit /b 0\r\n").expect("write fake Pi");
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::write(&pi, "#!/bin/sh\nexit 0\n").expect("write fake Pi");
                let mut permissions = fs::metadata(&pi).expect("metadata").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&pi, permissions).expect("permissions");
            }

            let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
            let environment = resolve_launch_environment(LaunchEnvironmentInput {
                configured_pi: Some(pi),
                desktop_environment,
                ..LaunchEnvironmentInput::default()
            })
            .expect("resolve fixture environment");
            let git = environment
                .git_executable()
                .expect("Git required for worktree tests");
            run(git, &repository, &["init"]);
            fs::write(project.join("file.txt"), "initial\n").expect("fixture file");
            run(git, &repository, &["add", "."]);
            run(
                git,
                &repository,
                &[
                    "-c",
                    "user.name=Pi Wizard Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-m",
                    "initial",
                ],
            );
            run(git, &repository, &["branch", "-M", "fixture-base"]);

            Self {
                root,
                repository,
                project,
                environment,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run(git: &Path, cwd: &Path, args: &[&str]) {
        let status = Command::new(git)
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("run fixture Git");
        assert!(status.success(), "fixture Git failed: {args:?}");
    }

    #[tokio::test]
    async fn inspection_captures_exact_branch_commit_subdirectory_and_dirty_state() {
        let fixture = Fixture::new("inspect");
        let clean = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect clean base");
        assert_eq!(clean.source_branch.as_deref(), Some("fixture-base"));
        assert_eq!(clean.project_relative_path, PathBuf::from("project"));
        assert!(!clean.dirty);
        assert!(!clean.base_commit.is_empty());

        fs::write(fixture.project.join("untracked.txt"), "dirty\n").expect("write untracked");
        let dirty = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect dirty base");
        assert!(dirty.dirty);
        assert_eq!(dirty.base_commit, clean.base_commit);
    }

    #[tokio::test]
    async fn creation_rejects_source_commit_movement_before_git_mutation() {
        let fixture = Fixture::new("stale");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        fs::write(fixture.project.join("second.txt"), "second\n").expect("second file");
        let git = fixture.environment.git_executable().expect("git");
        run(git, &fixture.repository, &["add", "."]);
        run(
            git,
            &fixture.repository,
            &[
                "-c",
                "user.name=Pi Wizard Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "second",
            ],
        );
        let target = fixture.root.join("stale-worktree");
        let result = create_worktree(
            WorktreeCreatePlan {
                base,
                branch: "stale-branch".to_owned(),
                worktree_path: target.clone(),
            },
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await;
        assert!(matches!(result, Err(WorktreeError::SourceChanged { .. })));
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn creation_verifies_branch_base_and_project_execution_root() {
        let fixture = Fixture::new("create");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let target = fixture.root.join("created-worktree");
        let created = create_worktree(
            WorktreeCreatePlan {
                base: base.clone(),
                branch: "agent-feature".to_owned(),
                worktree_path: target,
            },
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("create worktree");

        assert_eq!(created.branch, "agent-feature");
        assert_eq!(created.base_commit, base.base_commit);
        assert_eq!(
            created.execution_root,
            created
                .worktree_root
                .join("project")
                .canonicalize()
                .expect("execution root")
        );
    }

    #[tokio::test]
    async fn pristine_created_worktree_can_be_explicitly_removed_with_its_unchanged_branch() {
        let fixture = Fixture::new("cleanup-pristine");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let target = fixture.root.join("cleanup-pristine-worktree");
        let plan = WorktreeCreatePlan {
            base: base.clone(),
            branch: "cleanup-pristine".to_owned(),
            worktree_path: target.clone(),
        };
        create_worktree(plan.clone(), &fixture.environment, RuntimeLimits::default())
            .await
            .expect("create worktree");

        assert_eq!(
            cleanup_pristine_worktree(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("cleanup worktree"),
            WorktreeCleanupResult::Removed
        );
        assert!(!target.exists());
        assert!(matches!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("post-cleanup probe"),
            WorktreeRecoveryProbe::NotCreated
        ));
    }

    #[test]
    fn cleanup_result_wire_shape_matches_desktop_contract() {
        assert_eq!(
            serde_json::to_value(WorktreeCleanupResult::Removed).expect("removed wire shape"),
            serde_json::json!({"kind":"removed"})
        );
        assert_eq!(
            serde_json::to_value(WorktreeCleanupResult::Partial {
                branch_exists: true,
                path_exists: false,
                detail: "branch retained".to_owned(),
            })
            .expect("partial wire shape"),
            serde_json::json!({
                "kind":"partial",
                "branchExists":true,
                "pathExists":false,
                "detail":"branch retained"
            })
        );
    }

    #[tokio::test]
    async fn cleanup_refuses_dirty_worktree_without_removing_path_or_branch() {
        let fixture = Fixture::new("cleanup-dirty");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let target = fixture.root.join("cleanup-dirty-worktree");
        let plan = WorktreeCreatePlan {
            base,
            branch: "cleanup-dirty".to_owned(),
            worktree_path: target.clone(),
        };
        let created = create_worktree(plan.clone(), &fixture.environment, RuntimeLimits::default())
            .await
            .expect("create worktree");
        fs::write(
            created.execution_root.join("untracked.txt"),
            "preserve me\n",
        )
        .expect("dirty worktree");

        assert!(matches!(
            cleanup_pristine_worktree(&plan, &fixture.environment, RuntimeLimits::default()).await,
            Err(WorktreeError::CleanupDirty { .. })
        ));
        assert!(target.exists());
        assert!(matches!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("dirty recovery probe"),
            WorktreeRecoveryProbe::Exact { .. }
        ));
    }

    #[tokio::test]
    async fn cleanup_refuses_task_commits_even_when_worktree_is_clean() {
        let fixture = Fixture::new("cleanup-commits");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let target = fixture.root.join("cleanup-commit-worktree");
        let plan = WorktreeCreatePlan {
            base,
            branch: "cleanup-commits".to_owned(),
            worktree_path: target.clone(),
        };
        let created = create_worktree(plan.clone(), &fixture.environment, RuntimeLimits::default())
            .await
            .expect("create worktree");
        fs::write(created.execution_root.join("task.txt"), "task commit\n").expect("task file");
        let git = fixture.environment.git_executable().expect("git");
        run(git, &created.worktree_root, &["add", "."]);
        run(
            git,
            &created.worktree_root,
            &[
                "-c",
                "user.name=Pi Wizard Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "task commit",
            ],
        );

        assert!(matches!(
            cleanup_pristine_worktree(&plan, &fixture.environment, RuntimeLimits::default()).await,
            Err(WorktreeError::CleanupContainsCommits { .. })
        ));
        assert!(target.exists());
        assert!(matches!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("committed recovery probe"),
            WorktreeRecoveryProbe::Exact { .. }
        ));
    }

    #[tokio::test]
    async fn existing_target_is_rejected_before_branch_creation() {
        let fixture = Fixture::new("existing-target");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let target = fixture.root.join("occupied");
        fs::create_dir_all(&target).expect("occupied target");
        let result = create_worktree(
            WorktreeCreatePlan {
                base,
                branch: "must-not-exist".to_owned(),
                worktree_path: target,
            },
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(WorktreeError::WorktreePathExists { .. })
        ));
        let git = fixture.environment.git_executable().expect("git");
        let status = Command::new(git)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/must-not-exist",
            ])
            .current_dir(&fixture.repository)
            .status()
            .expect("check branch");
        assert!(!status.success());
    }

    #[tokio::test]
    async fn recovery_probe_discards_only_when_branch_and_path_are_both_absent() {
        let fixture = Fixture::new("recovery-absent");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let plan = WorktreeCreatePlan {
            base,
            branch: "recovery-absent".to_owned(),
            worktree_path: fixture.root.join("recovery-absent-worktree"),
        };

        assert_eq!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("probe absent recovery"),
            WorktreeRecoveryProbe::NotCreated
        );
    }

    #[tokio::test]
    async fn recovery_probe_recovers_exact_created_worktree_identity() {
        let fixture = Fixture::new("recovery-exact");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let plan = WorktreeCreatePlan {
            base,
            branch: "recovery-exact".to_owned(),
            worktree_path: fixture.root.join("recovery-exact-worktree"),
        };
        let created = create_worktree(plan.clone(), &fixture.environment, RuntimeLimits::default())
            .await
            .expect("create worktree");

        assert_eq!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("probe created recovery"),
            WorktreeRecoveryProbe::Exact { created }
        );
    }

    #[tokio::test]
    async fn recovery_probe_accepts_task_commits_descended_from_captured_base() {
        let fixture = Fixture::new("recovery-descendant");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let plan = WorktreeCreatePlan {
            base,
            branch: "recovery-descendant".to_owned(),
            worktree_path: fixture.root.join("recovery-descendant-worktree"),
        };
        let created = create_worktree(plan.clone(), &fixture.environment, RuntimeLimits::default())
            .await
            .expect("create worktree");
        fs::write(
            created.execution_root.join("agent-change.txt"),
            "agent change\n",
        )
        .expect("agent change");
        let git = fixture.environment.git_executable().expect("git");
        run(git, &created.worktree_root, &["add", "."]);
        run(
            git,
            &created.worktree_root,
            &[
                "-c",
                "user.name=Pi Wizard Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "agent change",
            ],
        );

        assert_eq!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("probe descendant recovery"),
            WorktreeRecoveryProbe::Exact { created }
        );
    }

    #[tokio::test]
    async fn recovery_probe_rejects_same_path_after_branch_switch() {
        let fixture = Fixture::new("recovery-wrong-branch");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let plan = WorktreeCreatePlan {
            base,
            branch: "recovery-original".to_owned(),
            worktree_path: fixture.root.join("recovery-wrong-branch-worktree"),
        };
        let created = create_worktree(plan.clone(), &fixture.environment, RuntimeLimits::default())
            .await
            .expect("create worktree");
        let git = fixture.environment.git_executable().expect("git");
        run(
            git,
            &created.worktree_root,
            &["switch", "-c", "different-task"],
        );

        assert!(matches!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("probe wrong branch"),
            WorktreeRecoveryProbe::Partial {
                branch_exists: true,
                path_exists: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn recovery_probe_preserves_branch_only_partial_mutation() {
        let fixture = Fixture::new("recovery-branch-only");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let branch = "recovery-branch-only";
        let target = fixture.root.join("missing-recovery-worktree");
        let git = fixture.environment.git_executable().expect("git");
        run(
            git,
            &fixture.repository,
            &["branch", branch, &base.base_commit],
        );
        let plan = WorktreeCreatePlan {
            base,
            branch: branch.to_owned(),
            worktree_path: target,
        };

        assert!(matches!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("probe branch-only recovery"),
            WorktreeRecoveryProbe::Partial {
                branch_exists: true,
                path_exists: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn recovery_probe_preserves_path_only_or_conflicting_target() {
        let fixture = Fixture::new("recovery-path-only");
        let base = inspect_worktree_base(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("inspect base");
        let target = fixture.root.join("occupied-recovery-target");
        fs::create_dir_all(&target).expect("occupied target");
        fs::write(target.join("unrelated.txt"), "keep\n").expect("unrelated file");
        let plan = WorktreeCreatePlan {
            base,
            branch: "recovery-path-only".to_owned(),
            worktree_path: target,
        };

        assert!(matches!(
            probe_worktree_recovery(&plan, &fixture.environment, RuntimeLimits::default())
                .await
                .expect("probe path-only recovery"),
            WorktreeRecoveryProbe::Partial {
                branch_exists: false,
                path_exists: true,
                ..
            }
        ));
    }

    #[test]
    fn mutation_classification_never_calls_ambiguous_git_failure_clean() {
        assert!(
            !WorktreeError::BranchExists {
                branch: "existing".to_owned()
            }
            .may_have_mutated()
        );
        assert!(
            WorktreeError::CreateFailed {
                branch: "partial".to_owned(),
                path: PathBuf::from("partial"),
                code: Some(128),
                branch_may_exist: true,
                path_exists: false,
            }
            .may_have_mutated()
        );
        assert!(
            WorktreeError::CreateIndeterminate {
                branch: "unknown".to_owned(),
                path: PathBuf::from("unknown"),
                detail: "timeout".to_owned(),
                branch_may_exist: false,
                path_exists: false,
            }
            .may_have_mutated()
        );
    }

    #[cfg(windows)]
    #[test]
    fn git_path_argument_removes_windows_verbatim_prefix() {
        assert_eq!(
            path_argument_for_git(Path::new(r"\\?\C:\work\tree")),
            OsString::from(r"C:\work\tree")
        );
        assert_eq!(
            path_argument_for_git(Path::new(r"\\?\UNC\server\share\tree")),
            OsString::from(r"\\server\share\tree")
        );
    }
}
