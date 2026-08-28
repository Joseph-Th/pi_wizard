use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::RuntimeLimits;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDirectorySource {
    Environment,
    Settings,
    Default,
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
    #[error("failed to read session directory {path}: {source}")]
    ReadDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("session file {path} is not a readable Pi session for this project")]
    InvalidProjectSession { path: PathBuf },
}

pub fn list_project_sessions(
    project_root: &Path,
    environment: &BTreeMap<OsString, OsString>,
    query: Option<&str>,
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

    let directory = resolve_session_directory(&project_root, environment, limits)?;
    let read_dir = match fs::read_dir(&directory.path) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionCatalogPage {
                sessions: Vec::new(),
                candidate_files: 0,
                scanned_files: 0,
                truncated: false,
                directory_source: directory.source,
            });
        }
        Err(source) => {
            return Err(SessionCatalogError::ReadDirectory {
                path: directory.path,
                source,
            });
        }
    };

    let mut candidates = Vec::new();
    let mut candidate_overflow = false;
    for entry in read_dir {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension() != Some(OsStr::new("jsonl")) {
            continue;
        }
        if candidates.len() >= limits.max_session_catalog_candidates {
            candidate_overflow = true;
            break;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        candidates.push((modified, path));
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));

    // Custom Pi session directories are flat and may contain sessions from
    // many projects. Spend the small header-scan budget first so unrelated
    // projects cannot consume the richer preview budget for this project.
    let project_candidates: Vec<_> = candidates
        .iter()
        .filter(|(_, path)| {
            session_header_matches_project(
                path,
                &project_root,
                limits.max_session_header_scan_bytes,
            )
        })
        .collect();

    let query_lower = query.map(str::to_lowercase);
    let mut sessions = Vec::new();
    let mut page_bytes = 0usize;
    let mut scanned_files = 0usize;
    let mut truncated = candidate_overflow;
    for (modified_unix_ms, path) in project_candidates
        .iter()
        .take(limits.max_session_catalog_scan_files)
    {
        scanned_files = scanned_files.saturating_add(1);
        let Some(mut session) = read_session_preview(path, &project_root, limits)? else {
            continue;
        };
        session.modified_unix_ms = *modified_unix_ms;
        if let Some(query) = query_lower.as_deref()
            && !entry_matches(&session, query)
        {
            continue;
        }
        if sessions.len() >= limits.max_session_catalog_page_entries {
            truncated = true;
            break;
        }
        let encoded = serde_json::to_vec(&session).unwrap_or_default().len();
        if page_bytes.saturating_add(encoded) > limits.max_session_catalog_page_bytes {
            truncated = true;
            break;
        }
        page_bytes = page_bytes.saturating_add(encoded);
        sessions.push(session);
    }
    if scanned_files < project_candidates.len() {
        truncated = true;
    }

    Ok(SessionCatalogPage {
        sessions,
        candidate_files: candidates.len(),
        scanned_files,
        truncated,
        directory_source: directory.source,
    })
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
    read_session_preview(&path, &project_root, limits)?
        .ok_or(SessionCatalogError::InvalidProjectSession { path })
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
        return nonempty(text);
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
        combined.push_str(text);
        if combined.len() >= 1024 {
            break;
        }
    }
    nonempty(&combined)
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
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
        let page = list_project_sessions(&project, &env, None, limits).expect("list sessions");
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
        let page = list_project_sessions(&project, &env, Some("alpha"), RuntimeLimits::default())
            .expect("list sessions");
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id, "session-one");
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
}
