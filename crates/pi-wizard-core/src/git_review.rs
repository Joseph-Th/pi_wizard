use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::timeout;

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
    pub binary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffCursor {
    pub path: PathBuf,
    pub offset: usize,
    pub prefix_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffHunk {
    /// Zero-based line index within this bounded diff page.
    pub line_index: usize,
    pub header: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiffPage {
    pub path: PathBuf,
    pub diff: String,
    pub next_cursor: Option<GitDiffCursor>,
    pub untracked: bool,
    pub binary: bool,
    pub scanned_bytes: usize,
    pub hunks: Vec<GitDiffHunk>,
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

/// Streams one bounded window of a tracked text diff. A continuation cursor is
/// bound to both the file path and a SHA-256 digest of every raw diff byte
/// before the next window. Later pages re-run Git and stream-discard the prefix
/// rather than retaining an ever-growing patch in memory. If the diff changes,
/// the cursor fails closed instead of showing pages from two repository states.
pub async fn review_file_diff_page(
    execution_root: &Path,
    relative_path: &Path,
    cursor: Option<&GitDiffCursor>,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<GitFileDiffPage, GitReviewError> {
    validate_relative_path(relative_path, limits)?;
    if let Some(cursor) = cursor {
        validate_diff_cursor(cursor, relative_path, limits)?;
    }
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
        if cursor.is_some() {
            return Err(GitReviewError::StaleDiffCursor);
        }
        return Ok(GitFileDiffPage {
            path: relative_path.to_path_buf(),
            diff: "Untracked file content is not loaded automatically. Open it explicitly in an editor if review is required."
                .to_owned(),
            next_cursor: None,
            untracked: true,
            binary: false,
            scanned_bytes: 0,
            hunks: Vec::new(),
        });
    }

    let numstat = git_raw_os(
        environment,
        execution_root,
        vec![
            OsString::from("diff"),
            OsString::from("--numstat"),
            OsString::from("--relative"),
            OsString::from("HEAD"),
            OsString::from("--"),
            relative_path.as_os_str().to_os_string(),
        ],
        limits
            .max_worktree_path_bytes
            .saturating_add(128)
            .min(limits.max_git_command_output_bytes),
        limits,
    )
    .await?;
    if !numstat.status.success() {
        return Err(GitReviewError::GitCommandFailed {
            operation: "classify binary review file",
            code: numstat.status.code(),
        });
    }
    if numstat.stdout.starts_with(b"-\t-\t") {
        if cursor.is_some() {
            return Err(GitReviewError::StaleDiffCursor);
        }
        return Ok(GitFileDiffPage {
            path: relative_path.to_path_buf(),
            diff: "Binary file changed. Text diff is not available.".to_owned(),
            next_cursor: None,
            untracked: false,
            binary: true,
            scanned_bytes: 0,
            hunks: Vec::new(),
        });
    }

    stream_text_diff_page(execution_root, relative_path, cursor, environment, limits).await
}

fn validate_diff_cursor(
    cursor: &GitDiffCursor,
    relative_path: &Path,
    limits: RuntimeLimits,
) -> Result<(), GitReviewError> {
    if cursor.path != relative_path
        || cursor.offset == 0
        || cursor.offset >= limits.max_git_diff_scan_bytes_per_page
        || cursor.prefix_sha256.len() != 64
        || !cursor
            .prefix_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(GitReviewError::InvalidDiffCursor);
    }
    Ok(())
}

async fn stream_text_diff_page(
    execution_root: &Path,
    relative_path: &Path,
    cursor: Option<&GitDiffCursor>,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<GitFileDiffPage, GitReviewError> {
    let git = environment
        .git_executable()
        .ok_or(GitReviewError::GitUnavailable)?;
    let args = vec![
        OsString::from("diff"),
        OsString::from("--no-ext-diff"),
        OsString::from("--no-color"),
        OsString::from("--relative"),
        OsString::from("HEAD"),
        OsString::from("--"),
        relative_path.as_os_str().to_os_string(),
    ];
    let deadline = Duration::from_millis(limits.git_command_deadline_ms);
    let page_limit = limits
        .max_git_diff_page_bytes
        .min(limits.max_git_diff_bytes);
    let result = timeout(deadline, async {
        let mut command = Command::new(git);
        command
            .args(&args)
            .current_dir(execution_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear()
            .envs(environment.variables());
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|error| GitReviewError::GitExecution(error.to_string()))?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            GitReviewError::GitExecution("Git diff stdout pipe is missing".to_owned())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            GitReviewError::GitExecution("Git diff stderr pipe is missing".to_owned())
        })?;
        let stderr_task = tokio::spawn(drain_discard(stderr));

        let mut prefix_hasher = Sha256::new();
        let mut skip_remaining = cursor.map_or(0, |cursor| cursor.offset);
        let expected_prefix = cursor.map(|cursor| cursor.prefix_sha256.as_str());
        let start_offset = skip_remaining;
        let mut captured = Vec::with_capacity(page_limit.min(16 * 1024));
        let mut scanned_bytes = 0usize;
        let mut has_more = false;
        let mut chunk = [0_u8; 8 * 1024];

        loop {
            let remaining_scan = limits
                .max_git_diff_scan_bytes_per_page
                .saturating_sub(scanned_bytes);
            let remaining_capture = page_limit.saturating_sub(captured.len());
            // Read only what is needed to verify the skipped prefix, fill this
            // page, and observe at most one byte proving that another page
            // exists. This prevents an 8 KiB transport chunk from consuming a
            // much smaller but otherwise valid scan budget.
            let desired = skip_remaining
                .saturating_add(remaining_capture)
                .saturating_add(1)
                .max(1);
            let read_limit = chunk
                .len()
                .min(desired)
                .min(remaining_scan.saturating_add(1));
            let read = stdout
                .read(&mut chunk[..read_limit])
                .await
                .map_err(|error| GitReviewError::GitExecution(error.to_string()))?;
            if read == 0 {
                break;
            }
            if read > remaining_scan {
                let _ = child.start_kill();
                let _ = child.wait().await;
                stderr_task.abort();
                return Err(GitReviewError::DiffScanLimit {
                    limit: limits.max_git_diff_scan_bytes_per_page,
                });
            }
            scanned_bytes += read;

            let mut position = 0usize;
            if skip_remaining > 0 {
                let take = skip_remaining.min(read);
                prefix_hasher.update(&chunk[..take]);
                skip_remaining -= take;
                position += take;
                if skip_remaining == 0 {
                    let actual = digest_hex(prefix_hasher.clone().finalize().as_slice());
                    if expected_prefix != Some(actual.as_str()) {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        stderr_task.abort();
                        return Err(GitReviewError::StaleDiffCursor);
                    }
                }
            }

            if position < read && captured.len() < page_limit {
                let keep = (page_limit - captured.len()).min(read - position);
                captured.extend_from_slice(&chunk[position..position + keep]);
                position += keep;
            }
            if position < read {
                has_more = true;
                break;
            }
        }

        if skip_remaining > 0 {
            let _ = child.start_kill();
            let _ = child.wait().await;
            stderr_task.abort();
            return Err(GitReviewError::StaleDiffCursor);
        }

        if has_more {
            let _ = child.start_kill();
            let _ = child.wait().await;
        } else {
            let status = child
                .wait()
                .await
                .map_err(|error| GitReviewError::GitExecution(error.to_string()))?;
            if !status.success() {
                stderr_task.abort();
                return Err(GitReviewError::GitCommandFailed {
                    operation: "read paged file diff",
                    code: status.code(),
                });
            }
        }
        let _ = stderr_task.await;

        let valid_bytes = match std::str::from_utf8(&captured) {
            Ok(_) => captured.len(),
            Err(error) if has_more && error.error_len().is_none() && error.valid_up_to() > 0 => {
                error.valid_up_to()
            }
            Err(_) => return Err(GitReviewError::NonUtf8Diff),
        };
        if valid_bytes < captured.len() {
            captured.truncate(valid_bytes);
            has_more = true;
        }
        let diff = String::from_utf8(captured).map_err(|_| GitReviewError::NonUtf8Diff)?;
        let hunks = diff_hunks(&diff, limits);
        let next_cursor = if has_more {
            let mut cursor_hasher = prefix_hasher;
            cursor_hasher.update(diff.as_bytes());
            Some(GitDiffCursor {
                path: relative_path.to_path_buf(),
                offset: start_offset.saturating_add(diff.len()),
                prefix_sha256: digest_hex(cursor_hasher.finalize().as_slice()),
            })
        } else {
            None
        };
        Ok(GitFileDiffPage {
            path: relative_path.to_path_buf(),
            diff,
            next_cursor,
            untracked: false,
            binary: false,
            scanned_bytes,
            hunks,
        })
    })
    .await;

    match result {
        Ok(result) => result,
        Err(_) => Err(GitReviewError::GitExecution(
            "paged Git diff exceeded its deadline".to_owned(),
        )),
    }
}

fn diff_hunks(diff: &str, limits: RuntimeLimits) -> Vec<GitDiffHunk> {
    let mut hunks = Vec::new();
    for (line_index, line) in diff.lines().enumerate() {
        if !line.starts_with("@@ ") || hunks.len() >= limits.max_git_diff_hunks_per_page {
            continue;
        }
        let max_header_bytes = limits.max_git_ref_bytes.min(512);
        let mut end = line.len().min(max_header_bytes);
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        hunks.push(GitDiffHunk {
            line_index,
            header: line[..end].to_owned(),
        });
    }
    hunks
}

async fn drain_discard<R>(mut reader: R) -> Result<(), std::io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        if reader.read(&mut chunk).await? == 0 {
            return Ok(());
        }
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
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
            binary: false,
        });
    }

    let numstat = git_raw_os(
        environment,
        execution_root,
        vec![
            OsString::from("diff"),
            OsString::from("--numstat"),
            OsString::from("--relative"),
            OsString::from("HEAD"),
            OsString::from("--"),
            relative_path.as_os_str().to_os_string(),
        ],
        limits
            .max_worktree_path_bytes
            .saturating_add(128)
            .min(limits.max_git_command_output_bytes),
        limits,
    )
    .await?;
    if !numstat.status.success() {
        return Err(GitReviewError::GitCommandFailed {
            operation: "classify binary review file",
            code: numstat.status.code(),
        });
    }
    let binary = numstat.stdout.starts_with(b"-\t-\t");
    if binary {
        return Ok(GitFileDiff {
            path: relative_path.to_path_buf(),
            diff: "Binary file changed. Text diff is not available.".to_owned(),
            truncated: false,
            untracked: false,
            binary: true,
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
        binary: false,
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
    #[error("Git diff content is not valid UTF-8 and cannot be represented in desktop IPC")]
    NonUtf8Diff,
    #[error("invalid project-relative review path {path}")]
    InvalidReviewPath { path: PathBuf },
    #[error("invalid Git diff continuation cursor")]
    InvalidDiffCursor,
    #[error("Git diff changed while paging; refresh the file review")]
    StaleDiffCursor,
    #[error("Git diff paging exceeded the bounded scan limit of {limit} bytes")]
    DiffScanLimit { limit: usize },
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
        assert!(!tracked.binary);
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
        assert!(!untracked.binary);
        assert!(!untracked.diff.contains("private"));
    }

    #[tokio::test]
    async fn paged_diff_reaches_later_bytes_without_retaining_the_whole_patch() {
        let fixture = Fixture::new("paged-diff");
        let replacement = (0..500)
            .map(|index| format!("line-{index:04}-changed\n"))
            .collect::<String>();
        fs::write(fixture.project.join("tracked.txt"), replacement).expect("large text change");
        let limits = RuntimeLimits {
            max_git_diff_page_bytes: 256,
            max_git_diff_scan_bytes_per_page: 64 * 1024,
            ..RuntimeLimits::default()
        };

        let git = fixture.environment.git_executable().expect("git");
        let expected = Command::new(git)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--relative",
                "HEAD",
                "--",
                "tracked.txt",
            ])
            .current_dir(&fixture.project)
            .output()
            .expect("expected diff");
        assert!(expected.status.success());
        let expected = String::from_utf8(expected.stdout).expect("UTF-8 fixture diff");

        let mut cursor = None;
        let mut combined = String::new();
        let mut pages = 0usize;
        loop {
            let page = review_file_diff_page(
                &fixture.project,
                Path::new("tracked.txt"),
                cursor.as_ref(),
                &fixture.environment,
                limits,
            )
            .await
            .expect("paged diff");
            assert!(page.diff.len() <= limits.max_git_diff_page_bytes);
            assert!(page.scanned_bytes <= limits.max_git_diff_scan_bytes_per_page);
            combined.push_str(&page.diff);
            pages += 1;
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
            assert!(pages < 128, "paged diff must make bounded forward progress");
        }
        assert!(pages > 1);
        assert_eq!(combined, expected);
    }

    #[tokio::test]
    async fn paged_diff_never_splits_utf8_and_reconstructs_exact_patch() {
        let fixture = Fixture::new("paged-diff-utf8");
        let replacement = format!("{}🙂suffix\n", "ascii-prefix-".repeat(16));
        fs::write(fixture.project.join("tracked.txt"), replacement).expect("unicode text change");
        let git = fixture.environment.git_executable().expect("git");
        let expected = Command::new(git)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--relative",
                "HEAD",
                "--",
                "tracked.txt",
            ])
            .current_dir(&fixture.project)
            .output()
            .expect("unicode reference diff");
        assert!(expected.status.success());
        let emoji = "🙂".as_bytes();
        let emoji_offset = expected
            .stdout
            .windows(emoji.len())
            .position(|window| window == emoji)
            .expect("emoji appears in Git diff");
        let limits = RuntimeLimits {
            // Deliberately retain only the first byte of the four-byte emoji
            // in the raw capture. The page owner must back up to the preceding
            // UTF-8 boundary and re-read the whole scalar on the next page.
            max_git_diff_page_bytes: emoji_offset + 1,
            max_git_diff_scan_bytes_per_page: 64 * 1024,
            ..RuntimeLimits::default()
        };

        let mut cursor = None;
        let mut reconstructed = Vec::new();
        let mut pages = 0usize;
        loop {
            let page = review_file_diff_page(
                &fixture.project,
                Path::new("tracked.txt"),
                cursor.as_ref(),
                &fixture.environment,
                limits,
            )
            .await
            .expect("unicode-safe paged diff");
            assert!(std::str::from_utf8(page.diff.as_bytes()).is_ok());
            reconstructed.extend_from_slice(page.diff.as_bytes());
            pages += 1;
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
            assert!(pages < 8, "unicode paging must make forward progress");
        }
        assert!(pages > 1);
        assert_eq!(reconstructed, expected.stdout);
    }

    #[test]
    fn diff_hunk_projection_is_line_indexed_header_bounded_and_count_bounded() {
        let limits = RuntimeLimits {
            max_git_diff_hunks_per_page: 2,
            max_git_ref_bytes: 16,
            ..RuntimeLimits::default()
        };
        let diff = concat!(
            "diff --git a/file b/file\n",
            "@@ -1,1 +1,1 @@ first-long-context-name\n",
            "-old\n+new\n",
            "@@ -10,1 +10,1 @@ second\n",
            "-old2\n+new2\n",
            "@@ -20,1 +20,1 @@ third\n",
        );
        let hunks = diff_hunks(diff, limits);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].line_index, 1);
        assert_eq!(hunks[1].line_index, 4);
        assert!(hunks.iter().all(|hunk| hunk.header.len() <= 16));
        assert!(hunks[0].header.starts_with("@@ -1,1 +1,1 @@"));
    }

    #[tokio::test]
    #[ignore = "scale fixture; exercised by full verification"]
    async fn multi_megabyte_diff_pages_reconstruct_exact_patch_with_fixed_page_and_scan_bounds() {
        let fixture = Fixture::new("scale-multi-megabyte-diff");
        let git = fixture.environment.git_executable().expect("git");
        let changed = (0..36_000usize)
            .map(|index| format!("changed-{index:05}-{}\n", "x".repeat(48)))
            .collect::<String>();
        fs::write(fixture.project.join("tracked.txt"), changed).expect("large tracked change");
        let limits = RuntimeLimits {
            max_git_diff_page_bytes: 128 * 1024,
            max_git_diff_scan_bytes_per_page: 8 * 1024 * 1024,
            ..RuntimeLimits::default()
        };

        let expected = Command::new(git)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--relative",
                "HEAD",
                "--",
                "tracked.txt",
            ])
            .current_dir(&fixture.project)
            .output()
            .expect("reference Git diff");
        assert!(expected.status.success());
        assert!(expected.stdout.len() > 2 * 1024 * 1024);

        let mut cursor = None;
        let mut reconstructed = Vec::new();
        let mut pages = 0usize;
        loop {
            let page = review_file_diff_page(
                &fixture.project,
                Path::new("tracked.txt"),
                cursor.as_ref(),
                &fixture.environment,
                limits,
            )
            .await
            .expect("paged large diff");
            assert!(!page.binary);
            assert!(!page.untracked);
            assert!(page.diff.len() <= limits.max_git_diff_page_bytes);
            assert!(page.scanned_bytes <= limits.max_git_diff_scan_bytes_per_page);
            reconstructed.extend_from_slice(page.diff.as_bytes());
            pages += 1;
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert!(pages > 8, "fixture must exercise repeated paging");
        assert_eq!(reconstructed, expected.stdout);
    }

    #[tokio::test]
    async fn diff_cursor_fails_closed_when_earlier_patch_bytes_change() {
        let fixture = Fixture::new("stale-paged-diff");
        fs::write(
            fixture.project.join("tracked.txt"),
            (0..200)
                .map(|index| format!("first-version-{index:04}\n"))
                .collect::<String>(),
        )
        .expect("first diff");
        let limits = RuntimeLimits {
            max_git_diff_page_bytes: 192,
            max_git_diff_scan_bytes_per_page: 64 * 1024,
            ..RuntimeLimits::default()
        };
        let first = review_file_diff_page(
            &fixture.project,
            Path::new("tracked.txt"),
            None,
            &fixture.environment,
            limits,
        )
        .await
        .expect("first page");
        let cursor = first.next_cursor.expect("continuation cursor");

        fs::write(
            fixture.project.join("tracked.txt"),
            (0..200)
                .map(|index| format!("second-version-{index:04}\n"))
                .collect::<String>(),
        )
        .expect("changed diff");
        assert!(matches!(
            review_file_diff_page(
                &fixture.project,
                Path::new("tracked.txt"),
                Some(&cursor),
                &fixture.environment,
                limits,
            )
            .await,
            Err(GitReviewError::StaleDiffCursor)
        ));
    }

    #[tokio::test]
    async fn small_scan_budget_allows_early_pages_then_stops_before_unbounded_rescan() {
        let fixture = Fixture::new("paged-scan-limit");
        fs::write(
            fixture.project.join("tracked.txt"),
            (0..500)
                .map(|index| format!("scan-budget-{index:04}-changed\n"))
                .collect::<String>(),
        )
        .expect("large text change");
        let limits = RuntimeLimits {
            max_git_diff_page_bytes: 128,
            max_git_diff_scan_bytes_per_page: 512,
            ..RuntimeLimits::default()
        };

        let first = review_file_diff_page(
            &fixture.project,
            Path::new("tracked.txt"),
            None,
            &fixture.environment,
            limits,
        )
        .await
        .expect("first bounded page must fit small scan budget");
        assert_eq!(first.diff.len(), 128);
        assert!(first.scanned_bytes <= 512);
        let mut cursor = first.next_cursor.expect("large diff continuation");
        let mut successful_pages = 1usize;

        loop {
            match review_file_diff_page(
                &fixture.project,
                Path::new("tracked.txt"),
                Some(&cursor),
                &fixture.environment,
                limits,
            )
            .await
            {
                Ok(page) => {
                    successful_pages += 1;
                    assert!(page.scanned_bytes <= 512);
                    cursor = page
                        .next_cursor
                        .expect("fixture remains larger than scan budget");
                    assert!(successful_pages < 8);
                }
                Err(GitReviewError::DiffScanLimit { limit }) => {
                    assert_eq!(limit, 512);
                    break;
                }
                Err(error) => panic!("unexpected paged scan result: {error}"),
            }
        }
        assert!(successful_pages > 1);
    }

    #[tokio::test]
    async fn binary_change_returns_metadata_marker_without_rendering_binary_patch() {
        let fixture = Fixture::new("binary");
        let git = fixture.environment.git_executable().expect("git");
        let binary_path = fixture.project.join("binary.bin");
        fs::write(&binary_path, b"before\0binary").expect("binary baseline");
        run(git, &fixture.project, &["add", "binary.bin"]);
        run(
            git,
            &fixture.project,
            &[
                "-c",
                "user.name=Pi Wizard Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "binary baseline",
            ],
        );
        fs::write(&binary_path, b"after\0binary\0content").expect("binary change");

        let diff = review_file_diff(
            &fixture.project,
            Path::new("binary.bin"),
            &fixture.environment,
            RuntimeLimits::default(),
        )
        .await
        .expect("binary review");
        assert!(diff.binary);
        assert!(!diff.truncated);
        assert!(!diff.untracked);
        assert_eq!(
            diff.diff,
            "Binary file changed. Text diff is not available."
        );
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
