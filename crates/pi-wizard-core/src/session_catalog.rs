use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::RuntimeLimits;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDirectorySource {
    Environment,
    Settings,
    Default,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCatalogCursor {
    pub modified_unix_ms: u64,
    pub path: PathBuf,
    pub scope_sha256: String,
    pub snapshot_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSessionDirectory {
    pub path: PathBuf,
    pub source: SessionDirectorySource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCatalogEntry {
    pub path: PathBuf,
    pub id: String,
    pub name: Option<String>,
    pub first_message: Option<String>,
    pub modified_unix_ms: u64,
    pub preview_incomplete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCatalogPage {
    pub sessions: Vec<SessionCatalogEntry>,
    pub candidate_files: usize,
    pub scanned_files: usize,
    pub truncated: bool,
    pub next_cursor: Option<SessionCatalogCursor>,
    pub directory_source: SessionDirectorySource,
}

#[derive(Debug, Error)]
pub enum SessionCatalogError {
    #[error("project path could not be canonicalized: {0}")]
    ProjectPath(std::io::Error),
    #[error("Pi home directory is unavailable in the resolved launch environment")]
    HomeUnavailable,
    #[error("Pi path setting {value:?} requires a home directory for ~ expansion")]
    TildeWithoutHome { value: String },
    #[error("Pi settings file {path} is {actual} bytes; catalog read limit is {limit}")]
    SettingsTooLarge {
        path: PathBuf,
        actual: usize,
        limit: usize,
    },
    #[error("failed to read Pi settings file {path}: {source}")]
    ReadSettings {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("session catalog query is {actual} bytes; limit is {limit}")]
    QueryTooLarge { actual: usize, limit: usize },
    #[error("session catalog cursor path is {actual} bytes; limit is {limit}")]
    CursorTooLarge { actual: usize, limit: usize },
    #[error("session catalog cursor digest is invalid")]
    InvalidCursorDigest,
    #[error("session catalog cursor belongs to another project, directory, or query")]
    CursorScopeMismatch,
    #[error("session catalog cursor no longer identifies a candidate in this catalog")]
    CursorPositionMissing,
    #[error("session catalog changed while paging; restart the search from the newest page")]
    CatalogChanged,
    #[error("one session catalog entry is {actual} bytes; page limit is {limit}")]
    PageEntryTooLarge { actual: usize, limit: usize },
    #[error("failed to read session directory {path}: {source}")]
    ReadDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("session file {path} is not a readable Pi session for this project")]
    InvalidProjectSession { path: PathBuf },
    #[error(
        "session file {path} does not end with an LF record delimiter; resume was refused because current Pi may corrupt the next append to an unterminated JSONL tail. The file was not modified"
    )]
    UnterminatedSessionTail { path: PathBuf },
}

pub fn list_project_sessions(
    project_root: &Path,
    environment: &BTreeMap<OsString, OsString>,
    query: Option<&str>,
    cursor: Option<&SessionCatalogCursor>,
    limits: RuntimeLimits,
) -> Result<SessionCatalogPage, SessionCatalogError> {
    let project_root = project_root
        .canonicalize()
        .map_err(SessionCatalogError::ProjectPath)?;
    let query = query.map(str::trim).filter(|value| !value.is_empty());
    if let Some(query) = query
        && query.len() > limits.max_session_catalog_query_bytes
    {
        return Err(SessionCatalogError::QueryTooLarge {
            actual: query.len(),
            limit: limits.max_session_catalog_query_bytes,
        });
    }
    if let Some(cursor) = cursor {
        let actual = cursor.path.as_os_str().len();
        if actual > limits.max_session_cursor_bytes {
            return Err(SessionCatalogError::CursorTooLarge {
                actual,
                limit: limits.max_session_cursor_bytes,
            });
        }
        if !valid_digest(&cursor.scope_sha256) || !valid_digest(&cursor.snapshot_sha256) {
            return Err(SessionCatalogError::InvalidCursorDigest);
        }
    }

    let directory = resolve_session_directory(&project_root, environment, limits)?;
    let query_lower = query.map(str::to_lowercase);
    let scope_sha256 = catalog_scope_sha256(&project_root, &directory.path, query_lower.as_deref());
    if cursor.is_some_and(|cursor| cursor.scope_sha256.as_str() != scope_sha256.as_str()) {
        return Err(SessionCatalogError::CursorScopeMismatch);
    }
    let read_dir = match fs::read_dir(&directory.path) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && cursor.is_none() => {
            return Ok(SessionCatalogPage {
                sessions: Vec::new(),
                candidate_files: 0,
                scanned_files: 0,
                truncated: false,
                next_cursor: None,
                directory_source: directory.source,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SessionCatalogError::CatalogChanged);
        }
        Err(source) => {
            return Err(SessionCatalogError::ReadDirectory {
                path: directory.path,
                source,
            });
        }
    };

    let cursor_key = cursor.map(|cursor| (cursor.modified_unix_ms, cursor.path.clone()));
    let candidate_window_limit = limits.max_session_catalog_candidates.min(
        limits
            .max_session_catalog_scan_files
            .saturating_mul(8)
            .max(1),
    );
    let mut candidates = BinaryHeap::<Reverse<(u64, PathBuf)>>::new();
    let mut candidate_files = 0usize;
    let mut candidates_after_cursor = 0usize;
    let mut snapshot_xor = [0_u8; 32];
    let mut cursor_position_seen = cursor.is_none();
    for entry in read_dir {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension() != Some(OsStr::new("jsonl")) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        candidate_files = candidate_files.saturating_add(1);
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        xor_catalog_entry_digest(&mut snapshot_xor, modified, &path);
        let key = (modified, path);
        if cursor_key.as_ref().is_some_and(|cursor| key == *cursor) {
            cursor_position_seen = true;
        }
        if cursor_key
            .as_ref()
            .is_some_and(|cursor| key.0 > cursor.0 || (key.0 == cursor.0 && key.1 >= cursor.1))
        {
            continue;
        }
        candidates_after_cursor = candidates_after_cursor.saturating_add(1);
        if candidates.len() < candidate_window_limit {
            candidates.push(Reverse(key));
        } else if candidates.peek().is_some_and(|oldest_kept| {
            key.0 > oldest_kept.0.0 || (key.0 == oldest_kept.0.0 && key.1 > oldest_kept.0.1)
        }) {
            candidates.pop();
            candidates.push(Reverse(key));
        }
    }
    let snapshot_sha256 = catalog_snapshot_sha256(snapshot_xor, candidate_files);
    if cursor.is_some_and(|cursor| cursor.snapshot_sha256 != snapshot_sha256) {
        return Err(SessionCatalogError::CatalogChanged);
    }
    if !cursor_position_seen {
        return Err(SessionCatalogError::CursorPositionMissing);
    }
    let mut candidates: Vec<_> = candidates.into_iter().map(|Reverse(key)| key).collect();
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));

    let mut sessions = Vec::new();
    let mut page_bytes = 0usize;
    let mut scanned_files = 0usize;
    let mut last_consumed: Option<SessionCatalogCursor> = None;
    let mut stopped_early = false;
    for (modified_unix_ms, path) in &candidates {
        let current_cursor = SessionCatalogCursor {
            modified_unix_ms: *modified_unix_ms,
            path: path.clone(),
            scope_sha256: scope_sha256.clone(),
            snapshot_sha256: snapshot_sha256.clone(),
        };
        // Custom Pi session directories are flat and may contain sessions from
        // many projects. Header filtering is cheaper than a full metadata
        // preview and does not consume the detailed per-page scan budget.
        if !session_header_matches_project(
            path,
            &project_root,
            limits.max_session_header_scan_bytes,
        ) {
            last_consumed = Some(current_cursor);
            continue;
        }
        if scanned_files >= limits.max_session_catalog_scan_files {
            stopped_early = true;
            break;
        }
        scanned_files = scanned_files.saturating_add(1);
        let Some(mut session) = read_session_preview(path, &project_root, limits)? else {
            last_consumed = Some(current_cursor);
            continue;
        };
        session.modified_unix_ms = *modified_unix_ms;
        if let Some(query) = query_lower.as_deref()
            && !entry_matches(&session, query)
        {
            last_consumed = Some(current_cursor);
            continue;
        }
        if sessions.len() >= limits.max_session_catalog_page_entries {
            stopped_early = true;
            break;
        }
        let encoded = serde_json::to_vec(&session).unwrap_or_default().len();
        if page_bytes.saturating_add(encoded) > limits.max_session_catalog_page_bytes {
            if sessions.is_empty() {
                return Err(SessionCatalogError::PageEntryTooLarge {
                    actual: encoded,
                    limit: limits.max_session_catalog_page_bytes,
                });
            }
            stopped_early = true;
            break;
        }
        page_bytes = page_bytes.saturating_add(encoded);
        sessions.push(session);
        last_consumed = Some(current_cursor);
    }
    let window_has_older = candidates_after_cursor > candidates.len();
    let next_cursor = (stopped_early || window_has_older)
        .then_some(last_consumed)
        .flatten();
    let truncated = next_cursor.is_some();

    Ok(SessionCatalogPage {
        sessions,
        candidate_files,
        scanned_files,
        truncated,
        next_cursor,
        directory_source: directory.source,
    })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn catalog_scope_sha256(project_root: &Path, directory: &Path, query: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pi-wizard-session-catalog-scope-v1\0");
    hasher.update(project_root.as_os_str().to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(directory.as_os_str().to_string_lossy().as_bytes());
    hasher.update(b"\0");
    if let Some(query) = query {
        hasher.update(query.as_bytes());
    }
    digest_hex(hasher.finalize().as_slice())
}

fn xor_catalog_entry_digest(accumulator: &mut [u8; 32], modified_unix_ms: u64, path: &Path) {
    let mut hasher = Sha256::new();
    hasher.update(b"pi-wizard-session-catalog-entry-v1\0");
    hasher.update(modified_unix_ms.to_le_bytes());
    hasher.update(path.as_os_str().to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    for (target, byte) in accumulator.iter_mut().zip(digest) {
        *target ^= byte;
    }
}

fn catalog_snapshot_sha256(entry_xor: [u8; 32], candidate_files: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pi-wizard-session-catalog-snapshot-v1\0");
    hasher.update(entry_xor);
    hasher.update(candidate_files.to_le_bytes());
    digest_hex(hasher.finalize().as_slice())
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn session_header_matches_project(path: &Path, project_root: &Path, max_bytes: usize) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut bytes = Vec::with_capacity(max_bytes.min(16 * 1024));
    if file
        .take(u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .is_err()
    {
        return false;
    }
    let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
        return false;
    };
    if newline > max_bytes {
        return false;
    }
    let Ok(header) = serde_json::from_slice::<Value>(&bytes[..newline]) else {
        return false;
    };
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return false;
    }
    let Some(cwd) = header.get("cwd").and_then(Value::as_str) else {
        return false;
    };
    Path::new(cwd)
        .canonicalize()
        .is_ok_and(|session_cwd| session_cwd == project_root)
}

pub fn validate_project_session(
    project_root: &Path,
    session_path: &Path,
    limits: RuntimeLimits,
) -> Result<SessionCatalogEntry, SessionCatalogError> {
    let project_root = project_root
        .canonicalize()
        .map_err(SessionCatalogError::ProjectPath)?;
    let path =
        session_path
            .canonicalize()
            .map_err(|_| SessionCatalogError::InvalidProjectSession {
                path: session_path.to_path_buf(),
            })?;
    let preview = read_session_preview(&path, &project_root, limits)?
        .ok_or_else(|| SessionCatalogError::InvalidProjectSession { path: path.clone() })?;
    require_writable_jsonl_tail(&path)?;
    Ok(preview)
}

/// Resume is a write-capable operation. Current Pi releases can accept an
/// unterminated final JSONL record and then concatenate the next append onto
/// that tail, corrupting later cold recovery. Listing/history remain read-only
/// and tolerant; only the write-capable resume preflight requires an LF tail.
fn require_writable_jsonl_tail(path: &Path) -> Result<(), SessionCatalogError> {
    let mut file = File::open(path).map_err(|_| SessionCatalogError::InvalidProjectSession {
        path: path.to_path_buf(),
    })?;
    let len = file
        .metadata()
        .map_err(|_| SessionCatalogError::InvalidProjectSession {
            path: path.to_path_buf(),
        })?
        .len();
    if len == 0 {
        return Err(SessionCatalogError::InvalidProjectSession {
            path: path.to_path_buf(),
        });
    }
    file.seek(SeekFrom::End(-1))
        .map_err(|_| SessionCatalogError::InvalidProjectSession {
            path: path.to_path_buf(),
        })?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)
        .map_err(|_| SessionCatalogError::InvalidProjectSession {
            path: path.to_path_buf(),
        })?;
    if last[0] != b'\n' {
        return Err(SessionCatalogError::UnterminatedSessionTail {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

pub fn resolve_session_directory(
    project_root: &Path,
    environment: &BTreeMap<OsString, OsString>,
    limits: RuntimeLimits,
) -> Result<ResolvedSessionDirectory, SessionCatalogError> {
    let home = home_dir(environment);
    let agent_dir = if let Some(value) = env_value(environment, "PI_CODING_AGENT_DIR") {
        resolve_pi_path(&value.to_string_lossy(), project_root, home.as_deref())?
    } else {
        home.clone()
            .ok_or(SessionCatalogError::HomeUnavailable)?
            .join(".pi")
            .join("agent")
    };

    if let Some(value) = env_value(environment, "PI_CODING_AGENT_SESSION_DIR") {
        let path = resolve_pi_path(&value.to_string_lossy(), project_root, home.as_deref())?;
        return Ok(ResolvedSessionDirectory {
            path,
            source: SessionDirectorySource::Environment,
        });
    }

    let global = read_session_dir_setting(&agent_dir.join("settings.json"), limits)?;
    let project =
        read_session_dir_setting(&project_root.join(".pi").join("settings.json"), limits)?;
    if let Some(value) = project.or(global) {
        let path = resolve_pi_path(&value, project_root, home.as_deref())?;
        return Ok(ResolvedSessionDirectory {
            path,
            source: SessionDirectorySource::Settings,
        });
    }

    Ok(ResolvedSessionDirectory {
        path: agent_dir
            .join("sessions")
            .join(default_project_session_dir_name(project_root)),
        source: SessionDirectorySource::Default,
    })
}

fn env_value<'a>(environment: &'a BTreeMap<OsString, OsString>, key: &str) -> Option<&'a OsString> {
    #[cfg(windows)]
    {
        environment.iter().find_map(|(candidate, value)| {
            candidate
                .to_string_lossy()
                .eq_ignore_ascii_case(key)
                .then_some(value)
        })
    }
    #[cfg(not(windows))]
    {
        environment.get(OsStr::new(key))
    }
}

fn home_dir(environment: &BTreeMap<OsString, OsString>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(profile) = env_value(environment, "USERPROFILE") {
            return Some(PathBuf::from(profile));
        }
        let drive = env_value(environment, "HOMEDRIVE")?;
        let path = env_value(environment, "HOMEPATH")?;
        Some(PathBuf::from(format!(
            "{}{}",
            drive.to_string_lossy(),
            path.to_string_lossy()
        )))
    }
    #[cfg(not(windows))]
    {
        env_value(environment, "HOME").map(PathBuf::from)
    }
}

fn resolve_pi_path(
    value: &str,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, SessionCatalogError> {
    let expanded = if value == "~" {
        home.ok_or_else(|| SessionCatalogError::TildeWithoutHome {
            value: value.to_owned(),
        })?
        .to_path_buf()
    } else if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        home.ok_or_else(|| SessionCatalogError::TildeWithoutHome {
            value: value.to_owned(),
        })?
        .join(rest)
    } else {
        PathBuf::from(value)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    Ok(absolute.canonicalize().unwrap_or(absolute))
}

fn default_project_session_dir_name(project_root: &Path) -> String {
    let mut value = project_root.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            value = format!(r"\\{rest}");
        } else if let Some(rest) = value.strip_prefix(r"\\?\") {
            value = rest.to_owned();
        }
    }
    let value = value.trim_start_matches(['/', '\\']);
    let safe: String = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect();
    format!("--{safe}--")
}

fn read_session_dir_setting(
    path: &Path,
    limits: RuntimeLimits,
) -> Result<Option<String>, SessionCatalogError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SessionCatalogError::ReadSettings {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let actual = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if actual > limits.max_session_entry_page_bytes {
        return Err(SessionCatalogError::SettingsTooLarge {
            path: path.to_path_buf(),
            actual,
            limit: limits.max_session_entry_page_bytes,
        });
    }
    let text = fs::read_to_string(path).map_err(|source| SessionCatalogError::ReadSettings {
        path: path.to_path_buf(),
        source,
    })?;
    let normalized = remove_jsonc_comments_and_trailing_commas(&text);
    let Ok(value) = serde_json::from_str::<Value>(&normalized) else {
        // Pi reports malformed settings as a diagnostic and continues with the
        // remaining scope. Catalog lookup mirrors that non-fatal behavior.
        return Ok(None);
    };
    Ok(value
        .get("sessionDir")
        .and_then(Value::as_str)
        .map(str::to_owned))
}

fn remove_jsonc_comments_and_trailing_commas(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut without_comments = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            without_comments.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            without_comments.push(byte);
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            without_comments.extend_from_slice(b"  ");
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                without_comments.push(b' ');
                index += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            without_comments.extend_from_slice(b"  ");
            index += 2;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    without_comments.extend_from_slice(b"  ");
                    index += 2;
                    break;
                }
                without_comments.push(if bytes[index] == b'\n' { b'\n' } else { b' ' });
                index += 1;
            }
            continue;
        }
        without_comments.push(byte);
        index += 1;
    }

    let mut output = Vec::with_capacity(without_comments.len());
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < without_comments.len() {
        let byte = without_comments[index];
        if in_string {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            output.push(byte);
            index += 1;
            continue;
        }
        if byte == b',' {
            let mut lookahead = index + 1;
            while lookahead < without_comments.len()
                && without_comments[lookahead].is_ascii_whitespace()
            {
                lookahead += 1;
            }
            if matches!(without_comments.get(lookahead), Some(b'}' | b']')) {
                index += 1;
                continue;
            }
        }
        output.push(byte);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn read_session_preview(
    path: &Path,
    expected_project_root: &Path,
    limits: RuntimeLimits,
) -> Result<Option<SessionCatalogEntry>, SessionCatalogError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return Ok(None),
    };
    let file_len = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let budget = limits.max_session_metadata_scan_bytes;
    let head_budget = if file_len <= budget {
        budget
    } else {
        budget * 2 / 3
    };
    let head_len = file_len.min(head_budget);
    let mut head = vec![0u8; head_len];
    if file.read_exact(&mut head).is_err() {
        return Ok(None);
    }
    let tail_len = if file_len > head_len {
        (budget.saturating_sub(head_len)).min(file_len.saturating_sub(head_len))
    } else {
        0
    };
    let mut tail = vec![0u8; tail_len];
    if tail_len > 0 {
        let offset = i64::try_from(tail_len).unwrap_or(i64::MAX);
        if file.seek(SeekFrom::End(-offset)).is_err() || file.read_exact(&mut tail).is_err() {
            tail.clear();
        }
    }

    let head_text = String::from_utf8_lossy(&head);
    let Some(header_line) = head_text.lines().next() else {
        return Ok(None);
    };
    let Ok(header) = serde_json::from_str::<Value>(header_line) else {
        return Ok(None);
    };
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return Ok(None);
    }
    let Some(id) = header.get("id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(cwd) = header.get("cwd").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Ok(session_cwd) = Path::new(cwd).canonicalize() else {
        return Ok(None);
    };
    if session_cwd != expected_project_root {
        return Ok(None);
    }

    let mut name = None;
    let mut first_message = None;
    scan_preview_lines(&head_text, &mut name, &mut first_message);
    if !tail.is_empty() {
        let tail_text = String::from_utf8_lossy(&tail);
        scan_preview_lines(&tail_text, &mut name, &mut first_message);
    }

    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Ok(Some(SessionCatalogEntry {
        path,
        id: truncate_prefix(id, limits.max_session_cursor_bytes.min(512)),
        name: name.map(|value| truncate_prefix(&value, 512)),
        first_message: first_message.map(|value| truncate_prefix(&value, 512)),
        modified_unix_ms: 0,
        preview_incomplete: file_len > head.len().saturating_add(tail.len()),
    }))
}

fn scan_preview_lines(text: &str, name: &mut Option<String>, first_message: &mut Option<String>) {
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("session_info") => {
                *name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
            }
            Some("message") if first_message.is_none() => {
                let Some(message) = entry.get("message") else {
                    continue;
                };
                if message.get("role").and_then(Value::as_str) != Some("user") {
                    continue;
                }
                *first_message = extract_message_text(message);
            }
            _ => {}
        }
    }
}

fn extract_message_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return catalog_user_preview(text);
    }
    let blocks = content.as_array()?;
    let mut combined = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = block.get("text").and_then(Value::as_str) else {
            continue;
        };
        if !combined.is_empty() {
            combined.push(' ');
        }
        let remaining = 1024usize.saturating_sub(combined.len());
        if remaining == 0 {
            break;
        }
        combined.push_str(&truncate_prefix(text, remaining));
        if combined.len() >= 1024 {
            break;
        }
    }
    catalog_user_preview(&combined)
}

/// Pi expands `/skill:name args` before persisting the user message, so the
/// first stored text can begin with a complete SKILL.md payload. Catalog rows
/// should identify the user's task, not reproduce generated context. This is a
/// read-model normalization only; the authoritative JSONL remains untouched.
fn catalog_user_preview(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(after_name) = value.strip_prefix("<skill name=\"")
        && let Some(name_end) = after_name.find('"')
        && let Some(skill_end) = value.find("</skill>")
    {
        let name = &after_name[..name_end];
        let trailing = value[skill_end + "</skill>".len()..].trim();
        let trailing = trailing
            .strip_prefix("User:")
            .map(str::trim)
            .unwrap_or(trailing);
        if !trailing.is_empty() {
            return Some(truncate_prefix(trailing, 1024));
        }
        return Some(format!("[skill] {}", truncate_prefix(name, 256)));
    }

    Some(truncate_prefix(value, 1024))
}

fn truncate_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn entry_matches(entry: &SessionCatalogEntry, query: &str) -> bool {
    entry.id.to_lowercase().contains(query)
        || entry
            .path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|value| value.to_lowercase().contains(query))
        || entry
            .name
            .as_deref()
            .is_some_and(|value| value.to_lowercase().contains(query))
        || entry
            .first_message
            .as_deref()
            .is_some_and(|value| value.to_lowercase().contains(query))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::RunId;

    fn fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pi-wizard-session-catalog-{}", RunId::new()));
        let project = root.join("project");
        fs::create_dir_all(&project).expect("project fixture");
        (root, project)
    }

    fn environment(home: &Path) -> BTreeMap<OsString, OsString> {
        let mut environment = BTreeMap::new();
        #[cfg(windows)]
        environment.insert(OsString::from("USERPROFILE"), home.as_os_str().to_owned());
        #[cfg(not(windows))]
        environment.insert(OsString::from("HOME"), home.as_os_str().to_owned());
        environment
    }

    #[test]
    fn default_directory_matches_pi_cwd_encoding() {
        assert_eq!(
            default_project_session_dir_name(Path::new("/Users/test/code/pi")),
            "--Users-test-code-pi--"
        );
    }

    #[test]
    fn project_jsonc_session_dir_overrides_global_setting() {
        let (root, project) = fixture();
        let agent = root.join("home").join(".pi").join("agent");
        fs::create_dir_all(&agent).expect("agent dir");
        fs::create_dir_all(project.join(".pi")).expect("project settings dir");
        fs::write(
            agent.join("settings.json"),
            r#"{"sessionDir":"global-sessions"}"#,
        )
        .expect("global settings");
        fs::write(
            project.join(".pi").join("settings.json"),
            "{ // project wins\n \"sessionDir\": \"local-sessions\",\n}",
        )
        .expect("project settings");
        let resolved = resolve_session_directory(
            &project.canonicalize().expect("canonical project"),
            &environment(&root.join("home")),
            RuntimeLimits::default(),
        )
        .expect("resolve directory");
        assert_eq!(resolved.source, SessionDirectorySource::Settings);
        assert_eq!(
            resolved.path,
            project.canonicalize().unwrap().join("local-sessions")
        );
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn writable_resume_refuses_valid_but_unterminated_jsonl_tail_without_modifying_it() {
        let (root, project) = fixture();
        let session = root.join("unterminated-valid.jsonl");
        let canonical_project = project.canonicalize().expect("project");
        let bytes = format!(
            "{}\n{}",
            serde_json::json!({"type":"session","version":3,"id":"tail-valid","timestamp":"x","cwd":canonical_project}),
            serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"x","message":{"role":"user","content":"keep me"}})
        );
        fs::write(&session, bytes.as_bytes()).expect("write fixture");
        let before = fs::read(&session).expect("read before");

        assert!(matches!(
            validate_project_session(&project, &session, RuntimeLimits::default()),
            Err(SessionCatalogError::UnterminatedSessionTail { .. })
        ));
        assert_eq!(fs::read(&session).expect("read after"), before);
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn writable_resume_refuses_invalid_unterminated_fragment_tail_without_modifying_it() {
        let (root, project) = fixture();
        let session = root.join("unterminated-invalid.jsonl");
        let canonical_project = project.canonicalize().expect("project");
        let bytes = format!(
            "{}\n{}\n{{\"type\":\"message\"",
            serde_json::json!({"type":"session","version":3,"id":"tail-invalid","timestamp":"x","cwd":canonical_project}),
            serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"x","message":{"role":"user","content":"keep me"}})
        );
        fs::write(&session, bytes.as_bytes()).expect("write fixture");
        let before = fs::read(&session).expect("read before");

        assert!(matches!(
            validate_project_session(&project, &session, RuntimeLimits::default()),
            Err(SessionCatalogError::UnterminatedSessionTail { .. })
        ));
        assert_eq!(fs::read(&session).expect("read after"), before);
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn catalog_cursor_is_bound_to_the_original_query() {
        let (root, project) = fixture();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("session dir");
        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_owned(),
        );
        let canonical_project = project.canonicalize().expect("canonical project");
        for index in 0..2usize {
            let mut file =
                File::create(sessions.join(format!("query-{index}.jsonl"))).expect("session file");
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"session","version":3,"id":format!("query-{index}"),"timestamp":"x","cwd":canonical_project})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"x","message":{"role":"user","content":"alpha task"}})
            )
            .unwrap();
        }
        let limits = RuntimeLimits {
            max_session_catalog_scan_files: 1,
            max_session_catalog_page_entries: 1,
            ..RuntimeLimits::default()
        };
        let first = list_project_sessions(&project, &env, Some("alpha"), None, limits)
            .expect("first query page");
        let cursor = first.next_cursor.expect("query should have another page");
        assert!(matches!(
            list_project_sessions(&project, &env, Some("beta"), Some(&cursor), limits),
            Err(SessionCatalogError::CursorScopeMismatch)
        ));
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn catalog_cursor_fails_stale_when_candidate_snapshot_changes() {
        let (root, project) = fixture();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("session dir");
        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_owned(),
        );
        let canonical_project = project.canonicalize().expect("canonical project");
        let write_session = |name: &str, id: &str| {
            let mut file = File::create(sessions.join(name)).expect("session file");
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"session","version":3,"id":id,"timestamp":"x","cwd":canonical_project})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"x","message":{"role":"user","content":"task"}})
            )
            .unwrap();
        };
        write_session("one.jsonl", "one");
        write_session("two.jsonl", "two");
        let limits = RuntimeLimits {
            max_session_catalog_scan_files: 1,
            max_session_catalog_page_entries: 1,
            ..RuntimeLimits::default()
        };
        let first =
            list_project_sessions(&project, &env, None, None, limits).expect("first catalog page");
        let cursor = first.next_cursor.expect("catalog should have another page");
        write_session("three.jsonl", "three");
        assert!(matches!(
            list_project_sessions(&project, &env, None, Some(&cursor), limits),
            Err(SessionCatalogError::CatalogChanged)
        ));
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn unrelated_flat_directory_sessions_do_not_consume_detailed_scan_budget() {
        use std::time::Duration;

        let (root, project) = fixture();
        let other = root.join("other");
        fs::create_dir_all(&other).expect("other project");
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("session dir");
        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_owned(),
        );

        let write_session = |path: &Path, cwd: &Path, id: &str, prompt: &str| {
            let mut file = File::create(path).expect("session file");
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"session","version":3,"id":id,"timestamp":"2026-08-27T00:00:00.000Z","cwd":cwd.canonicalize().unwrap()})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"2026-08-27T00:00:01.000Z","message":{"role":"user","content":prompt}})
            )
            .unwrap();
        };

        write_session(
            &sessions.join("target.jsonl"),
            &project,
            "target-session",
            "target prompt",
        );
        std::thread::sleep(Duration::from_millis(10));
        write_session(
            &sessions.join("other-newer.jsonl"),
            &other,
            "other-session",
            "other prompt",
        );

        let limits = RuntimeLimits {
            max_session_catalog_scan_files: 1,
            max_session_catalog_page_entries: 1,
            ..RuntimeLimits::default()
        };
        let page =
            list_project_sessions(&project, &env, None, None, limits).expect("list sessions");
        assert_eq!(page.scanned_files, 1);
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id, "target-session");
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn flat_custom_directory_is_filtered_by_canonical_session_cwd() {
        let (root, project) = fixture();
        let other = root.join("other");
        fs::create_dir_all(&other).expect("other project");
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("session dir");
        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_owned(),
        );
        for (name, cwd, id, prompt) in [
            ("one.jsonl", &project, "session-one", "alpha task"),
            ("two.jsonl", &other, "session-two", "wrong project"),
        ] {
            let mut file = File::create(sessions.join(name)).expect("session file");
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"session","version":3,"id":id,"timestamp":"2026-08-27T00:00:00.000Z","cwd":cwd.canonicalize().unwrap()})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"2026-08-27T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":prompt}]}})
            )
            .unwrap();
        }
        let page = list_project_sessions(
            &project,
            &env,
            Some("alpha"),
            None,
            RuntimeLimits::default(),
        )
        .expect("list sessions");
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id, "session-one");
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    #[ignore = "scale fixture; exercised by full verification"]
    fn thousand_session_catalog_remains_page_and_scan_bounded() {
        let (root, project) = fixture();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("session dir");
        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_owned(),
        );
        let canonical_project = project.canonicalize().expect("canonical project");
        for index in 0..1_200usize {
            let mut file = File::create(sessions.join(format!("session-{index:04}.jsonl")))
                .expect("session file");
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type":"session",
                    "version":3,
                    "id":format!("scale-session-{index:04}"),
                    "timestamp":"2026-08-27T00:00:00.000Z",
                    "cwd":canonical_project
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type":"message",
                    "id":"m1",
                    "parentId":null,
                    "timestamp":"2026-08-27T00:00:01.000Z",
                    "message":{"role":"user","content":format!("historical task {index}")}
                })
            )
            .unwrap();
        }

        let limits = RuntimeLimits {
            max_session_catalog_candidates: 1_500,
            max_session_catalog_scan_files: 64,
            max_session_catalog_page_entries: 32,
            ..RuntimeLimits::default()
        };
        let mut cursor = None;
        let mut seen = Vec::with_capacity(1_200);
        let mut pages = 0usize;
        loop {
            let page = list_project_sessions(&project, &env, None, cursor.as_ref(), limits)
                .expect("scale catalog page");
            pages = pages.saturating_add(1);
            assert_eq!(page.candidate_files, 1_200);
            assert!(page.scanned_files <= 64);
            assert!(page.sessions.len() <= 32);
            seen.extend(page.sessions.iter().map(|session| session.id.clone()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
            assert!(pages < 50, "catalog paging did not converge");
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 1_200);
        assert!(pages > 1, "scale fixture must exercise continuation paging");
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn catalog_cursor_pages_without_skipping_boundary_sessions() {
        let (root, project) = fixture();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("session dir");
        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_owned(),
        );
        let canonical_project = project.canonicalize().expect("canonical project");
        for index in 0..5usize {
            let mut file = File::create(sessions.join(format!("session-{index}.jsonl")))
                .expect("session file");
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type":"session",
                    "version":3,
                    "id":format!("paged-{index}"),
                    "timestamp":"2026-08-27T00:00:00.000Z",
                    "cwd":canonical_project
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type":"message",
                    "id":"m1",
                    "parentId":null,
                    "timestamp":"2026-08-27T00:00:01.000Z",
                    "message":{"role":"user","content":format!("task {index}")}
                })
            )
            .unwrap();
        }

        let limits = RuntimeLimits {
            max_session_catalog_scan_files: 2,
            max_session_catalog_page_entries: 2,
            ..RuntimeLimits::default()
        };
        let mut cursor = None;
        let mut ids = Vec::new();
        for _ in 0..4 {
            let page = list_project_sessions(&project, &env, None, cursor.as_ref(), limits)
                .expect("paged catalog");
            ids.extend(page.sessions.iter().map(|session| session.id.clone()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids,
            (0..5)
                .map(|index| format!("paged-{index}"))
                .collect::<Vec<_>>()
        );
        assert!(
            cursor.is_none(),
            "cursor should reach the end of the catalog"
        );
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn preview_reads_head_and_tail_without_materializing_middle_history() {
        let (root, project) = fixture();
        let session = root.join("session.jsonl");
        let mut file = File::create(&session).expect("session file");
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"session","version":3,"id":"bounded","timestamp":"2026-08-27T00:00:00.000Z","cwd":project.canonicalize().unwrap()})
        )
        .unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"x","message":{"role":"user","content":"first prompt"}})).unwrap();
        for _ in 0..200 {
            writeln!(file, "{}", serde_json::json!({"type":"custom","id":"x","parentId":null,"timestamp":"x","data":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"})).unwrap();
        }
        writeln!(file, "{}", serde_json::json!({"type":"session_info","id":"n","parentId":null,"timestamp":"x","name":"latest name"})).unwrap();
        drop(file);
        let limits = RuntimeLimits {
            max_session_metadata_scan_bytes: 4096,
            ..RuntimeLimits::default()
        };
        let preview = validate_project_session(&project, &session, limits).expect("preview");
        assert_eq!(preview.first_message.as_deref(), Some("first prompt"));
        assert_eq!(preview.name.as_deref(), Some("latest name"));
        assert!(preview.preview_incomplete);
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn skill_expansion_preview_keeps_user_task_instead_of_skill_body() {
        let expanded = concat!(
            "<skill name=\"agent-reach\" location=\"C:/skills/agent-reach/SKILL.md\">\n",
            "# Agent Reach\n",
            "generated instructions that should not identify the session\n",
            "</skill>\n\n",
            "User: research websocket reconnect failures"
        );
        assert_eq!(
            catalog_user_preview(expanded).as_deref(),
            Some("research websocket reconnect failures")
        );

        let bare = concat!(
            "<skill name=\"pdf-tools\" location=\"C:/skills/pdf-tools/SKILL.md\">\n",
            "large generated skill body\n",
            "</skill>"
        );
        assert_eq!(
            catalog_user_preview(bare).as_deref(),
            Some("[skill] pdf-tools")
        );
    }

    #[test]
    fn catalog_search_uses_normalized_skill_arguments() {
        let (root, project) = fixture();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).expect("sessions");
        let canonical_project = project.canonicalize().expect("project");
        let session = sessions.join("skill-session.jsonl");
        let mut file = File::create(&session).expect("session file");
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"session","version":3,"id":"skill-search","timestamp":"x","cwd":canonical_project})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type":"message",
                "id":"m1",
                "parentId":null,
                "timestamp":"x",
                "message":{
                    "role":"user",
                    "content":"<skill name=\"agent-reach\" location=\"C:/skills/a/SKILL.md\">\nnoise noise noise\n</skill>\n\nUser: investigate reconnect regression"
                }
            })
        )
        .unwrap();
        drop(file);

        let mut env = environment(&root.join("home"));
        env.insert(
            OsString::from("PI_CODING_AGENT_SESSION_DIR"),
            sessions.as_os_str().to_os_string(),
        );
        let page = list_project_sessions(
            &project,
            &env,
            Some("reconnect regression"),
            None,
            RuntimeLimits::default(),
        )
        .expect("catalog search");
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(
            page.sessions[0].first_message.as_deref(),
            Some("investigate reconnect regression")
        );
        fs::remove_dir_all(root).expect("cleanup fixture");
    }
}
