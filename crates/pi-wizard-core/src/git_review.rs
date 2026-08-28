use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;
use crate::bounded::BoundedText;
use crate::environment::ResolvedLaunchEnvironment;
use crate::probe::{ProbeOutput, run_bounded_command};
use crate::worktree::inspect_worktree_base;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangedFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFileSummary {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub status: ChangedFileStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewSummary {
    pub repository_root: PathBuf,
    pub files: Vec<ChangedFileSummary>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiff {
    pub path: PathBuf,
    pub diff: String,
    pub truncated: bool,
    pub untracked: bool,
}

/// Reads a bounded changed-file summary for one immutable run execution root.
/// No repository state is retained and callers decide when to refresh.
pub async fn review_summary(
    execution_root: &Path,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<GitReviewSummary, GitReviewError> {
    let base = inspect_worktree_base(execution_root, environment, limits)
        .await
        .map_err(|error| GitReviewError::Repository(error.to_string()))?;

    let diff = git_raw_os(
        environment,
        execution_root,
        vec![
            OsString::from("diff"),
            OsString::from("--name-status"),
            OsString::from("--find-renames"),
            OsString::from("-z"),
            OsString::from("--relative"),
            OsString::from("HEAD"),
            OsString::from("--"),
            OsString::from("."),
        ],
        limits.max_git_command_output_bytes,
        limits,
    )
    .await?;
    if !diff.status.success() {
        return Err(GitReviewError::GitCommandFailed {
            operation: "read changed files",
            code: diff.status.code(),
        });
    }
    let mut truncated = diff.stdout_exceeded;
    let mut files =
        parse_name_status_prefix(&diff.stdout, diff.stdout_exceeded, limits, &mut truncated)?;

    let untracked = git_raw_os(
        environment,
        execution_root,
        vec![
            OsString::from("ls-files"),
            OsString::from("--others"),
            OsString::from("--exclude-standard"),
            OsString::from("-z"),
            OsString::from("--"),
            OsString::from("."),
        ],
        limits.max_git_command_output_bytes,
        limits,
    )
    .await?;
    if !untracked.status.success() {
        return Err(GitReviewError::GitCommandFailed {
            operation: "read untracked files",
            code: untracked.status.code(),
        });
    }
    if untracked.stdout_exceeded {
        truncated = true;
    }
    for token in complete_nul_tokens(&untracked.stdout, untracked.stdout_exceeded) {
        if files.len() >= limits.max_git_review_files {
            truncated = true;
            break;
        }
        if token.is_empty() {
            continue;
        }
        let path = parse_relative_path(token, limits)?;
        files.push(ChangedFileSummary {
            path,
            previous_path: None,
            status: ChangedFileStatus::Untracked,
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(GitReviewSummary {
        repository_root: base.repository_root,
        files,
        truncated,
    })
}

/// Loads one current tracked diff on demand. Untracked file contents are not
/// read automatically because they may be arbitrarily large or sensitive.
pub async fn review_file_diff(
    execution_root: &Path,
    relative_path: &Path,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<GitFileDiff, GitReviewError> {
    validate_relative_path(relative_path, limits)?;
    // Re-establish repository membership on each explicit request. This keeps
    // the review service independent from renderer-held summary state.
    inspect_worktree_base(execution_root, environment, limits)
        .await
        .map_err(|error| GitReviewError::Repository(error.to_string()))?;

    let untracked = git_raw_os(
        environment,
        execution_root,
        vec![
            OsString::from("ls-files"),
            OsString::from("--others"),
            OsString::from("--exclude-standard"),
            OsString::from("-z"),
            OsString::from("--"),
            relative_path.as_os_str().to_os_string(),
        ],
        limits.max_worktree_path_bytes,
        limits,
    )
    .await?;
    if !untracked.status.success() {
        return Err(GitReviewError::GitCommandFailed {
            operation: "classify review file",
            code: untracked.status.code(),
        });
    }
    if !untracked.stdout.is_empty() || untracked.stdout_exceeded {
        return Ok(GitFileDiff {
            path: relative_path.to_path_buf(),
            diff: "Untracked file content is not loaded automatically. Open it explicitly in an editor if review is required."
                .to_owned(),
            truncated: false,
            untracked: true,
        });
    }

    let output = git_raw_os(
        environment,
        execution_root,
        vec![
            OsString::from("diff"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-color"),
            OsString::from("--relative"),
            OsString::from("HEAD"),
            OsString::from("--"),
            relative_path.as_os_str().to_os_string(),
        ],
        limits.max_git_diff_bytes,
        limits,
    )
    .await?;
    if !output.status.success() {
        return Err(GitReviewError::GitCommandFailed {
            operation: "read file diff",
            code: output.status.code(),
        });
    }
    let rendered = String::from_utf8_lossy(&output.stdout);
    let mut diff = BoundedText::new(limits.max_git_diff_bytes);
    diff.replace(rendered.as_ref());
    Ok(GitFileDiff {
        path: relative_path.to_path_buf(),
        diff: diff.as_str().to_owned(),
        truncated: output.stdout_exceeded || diff.dropped_bytes() > 0,
        untracked: false,
    })
}

fn parse_name_status_prefix(
    bytes: &[u8],
    incomplete_tail: bool,
    limits: RuntimeLimits,
    truncated: &mut bool,
) -> Result<Vec<ChangedFileSummary>, GitReviewError> {
    let tokens = complete_nul_tokens(bytes, incomplete_tail);
    let mut files = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if files.len() >= limits.max_git_review_files {
            *truncated = true;
            break;
        }
        let status_token = tokens[index];
        index += 1;
        if status_token.is_empty() {
            continue;
        }
        let status_text =
            std::str::from_utf8(status_token).map_err(|_| GitReviewError::InvalidStatusEncoding)?;
        let status = classify_status(status_text);
        let requires_previous = matches!(
            status,
            ChangedFileStatus::Renamed | ChangedFileStatus::Copied
        );
        let required = if requires_previous { 2 } else { 1 };
        if tokens.len().saturating_sub(index) < required {
            *truncated = true;
            break;
        }
        let (previous_path, path) = if requires_previous {
            let previous = parse_relative_path(tokens[index], limits)?;
            let current = parse_relative_path(tokens[index + 1], limits)?;
            index += 2;
            (Some(previous), current)
        } else {
            let current = parse_relative_path(tokens[index], limits)?;
            index += 1;
            (None, current)
        };
        files.push(ChangedFileSummary {
            path,
            previous_path,
            status,
        });
    }
    Ok(files)
}

fn complete_nul_tokens(bytes: &[u8], incomplete_tail: bool) -> Vec<&[u8]> {
    let end = if incomplete_tail {
        bytes
            .iter()
            .rposition(|byte| *byte == 0)
            .map_or(0, |index| index + 1)
    } else {
        bytes.len()
    };
    bytes[..end]
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .collect()
}

fn classify_status(status: &str) -> ChangedFileStatus {
    match status.as_bytes().first().copied() {
        Some(b'A') => ChangedFileStatus::Added,
        Some(b'M') => ChangedFileStatus::Modified,
        Some(b'D') => ChangedFileStatus::Deleted,
        Some(b'R') => ChangedFileStatus::Renamed,
        Some(b'C') => ChangedFileStatus::Copied,
        Some(b'T') => ChangedFileStatus::TypeChanged,
        Some(b'U') => ChangedFileStatus::Unmerged,
        _ => ChangedFileStatus::Unknown,
    }
}

fn parse_relative_path(bytes: &[u8], limits: RuntimeLimits) -> Result<PathBuf, GitReviewError> {
    let text = std::str::from_utf8(bytes).map_err(|_| GitReviewError::NonUtf8Path)?;
    let path = PathBuf::from(text);
    validate_relative_path(&path, limits)?;
    Ok(path)
}

fn validate_relative_path(path: &Path, limits: RuntimeLimits) -> Result<(), GitReviewError> {
    let actual = path.as_os_str().to_string_lossy().len();
    if actual == 0 || actual > limits.max_worktree_path_bytes {
        return Err(GitReviewError::InvalidReviewPath {
            path: path.to_path_buf(),
        });
    }
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GitReviewError::InvalidReviewPath {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

async fn git_raw_os(
    environment: &ResolvedLaunchEnvironment,
    cwd: &Path,
    args: Vec<OsString>,
    max_bytes: usize,
    limits: RuntimeLimits,
) -> Result<ProbeOutput, GitReviewError> {
    let git = environment
        .git_executable()
        .ok_or(GitReviewError::GitUnavailable)?;
    run_bounded_command(
        git,
        &args,
        Some(cwd),
        environment.variables(),
        max_bytes.max(1),
        Duration::from_millis(limits.git_command_deadline_ms),
    )
    .await
    .map_err(|error| GitReviewError::GitExecution(error.to_string()))
}

#[derive(Debug, Error)]
pub enum GitReviewError {
    #[error("Git is not available in the resolved Pi launch environment")]
    GitUnavailable,
    #[error("could not resolve Git repository for review: {0}")]
    Repository(String),
    #[error("Git review command for {operation} failed with exit code {code:?}")]
    GitCommandFailed {
        operation: &'static str,
        code: Option<i32>,
    },
    #[error("Git review command could not execute: {0}")]
    GitExecution(String),
    #[error("Git changed-file status is not valid UTF-8")]
    InvalidStatusEncoding,
    #[error("Git changed-file path is not valid UTF-8 and cannot be represented in desktop IPC")]
    NonUtf8Path,
    #[error("invalid project-relative review path {path}")]
    InvalidReviewPath { path: PathBuf },
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
        project: PathBuf,
        environment: ResolvedLaunchEnvironment,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("pi-wizard-review-{name}-{}", RunId::new()));
            let repository = root.join("repo");
            let project = repository.join("project");
            fs::create_dir_all(&project).expect("create project");
            #[cfg(windows)]
            let pi = root.join("pi.cmd");
            #[cfg(not(windows))]
            let pi = root.join("pi");
            #[cfg(windows)]
            fs::write(&pi, "@echo off\r\nexit /b 0\r\n").expect("write Pi fixture");
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::write(&pi, "#!/bin/sh\nexit 0\n").expect("write Pi fixture");
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
            .expect("environment");
            let git = environment.git_executable().expect("git");
            run(git, &repository, &["init"]);
            fs::write(project.join("tracked.txt"), "one\n").expect("tracked file");
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
            Self {
                root,
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
            .expect("fixture Git");
        assert!(status.success(), "fixture Git failed: {args:?}");
    }

    #[tokio::test]
    async fn summary_lists_tracked_and_untracked_changes_without_reading_file_contents() {
        let fixture = Fixture::new("summary");
        fs::write(fixture.project.join("tracked.txt"), "two\n").expect("modify tracked");
        fs::write(
            fixture.project.join("untracked.txt"),
            "secret-sized-content\n",
        )
        .expect("untracked");
        let summary = review_summary(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("review summary");
        assert_eq!(summary.files.len(), 2);
        assert!(summary.files.iter().any(|file| {
            file.path == Path::new("tracked.txt") && file.status == ChangedFileStatus::Modified
        }));
        assert!(summary.files.iter().any(|file| {
            file.path == Path::new("untracked.txt") && file.status == ChangedFileStatus::Untracked
        }));
    }

    #[tokio::test]
    async fn tracked_diff_is_bounded_and_untracked_content_is_not_loaded() {
        let fixture = Fixture::new("diff");
        fs::write(fixture.project.join("tracked.txt"), "x".repeat(16 * 1024))
            .expect("large tracked change");
        fs::write(fixture.project.join("untracked.txt"), "private\n").expect("untracked");
        let limits = RuntimeLimits {
            max_git_diff_bytes: 512,
            ..RuntimeLimits::default()
        };
        let tracked = review_file_diff(
            &fixture.project,
            Path::new("tracked.txt"),
            &fixture.environment,
            limits,
        )
        .await
        .expect("tracked diff");
        assert!(tracked.truncated);
        assert!(tracked.diff.len() <= 512);

        let untracked = review_file_diff(
            &fixture.project,
            Path::new("untracked.txt"),
            &fixture.environment,
            limits,
        )
        .await
        .expect("untracked metadata");
        assert!(untracked.untracked);
        assert!(!untracked.diff.contains("private"));
    }

    #[tokio::test]
    async fn summary_preserves_rename_source_and_destination_paths() {
        let fixture = Fixture::new("rename");
        let git = fixture.environment.git_executable().expect("git");
        run(git, &fixture.project, &["mv", "tracked.txt", "renamed.txt"]);

        let summary = review_summary(
            &fixture.project,
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("rename summary");
        assert_eq!(summary.files.len(), 1);
        assert_eq!(summary.files[0].status, ChangedFileStatus::Renamed);
        assert_eq!(
            summary.files[0].previous_path,
            Some(PathBuf::from("tracked.txt"))
        );
        assert_eq!(summary.files[0].path, PathBuf::from("renamed.txt"));
    }

    #[test]
    fn status_parser_stops_at_file_count_limit_without_parsing_unbounded_tail() {
        let limits = RuntimeLimits {
            max_git_review_files: 1,
            ..RuntimeLimits::default()
        };
        let mut truncated = false;
        let files = parse_name_status_prefix(
            b"M\0first.txt\0M\0second.txt\0",
            false,
            limits,
            &mut truncated,
        )
        .expect("bounded status parse");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, PathBuf::from("first.txt"));
        assert!(truncated);
    }

    #[tokio::test]
    async fn review_rejects_parent_traversal_before_git_command() {
        let fixture = Fixture::new("traversal");
        let result = review_file_diff(
            &fixture.project,
            Path::new("../outside.txt"),
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(GitReviewError::InvalidReviewPath { .. })
        ));
    }
}
