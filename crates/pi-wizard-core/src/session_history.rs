use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::RuntimeLimits;
use crate::bounded::BoundedText;

const REVERSE_SCAN_BLOCK_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryCursor {
    pub session_id: String,
    pub before_offset: u64,
    pub next_entry_id: Option<String>,
    pub seek_latest: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTimelineKind {
    User,
    Assistant,
    Tool,
    Bash,
    Compaction,
    BranchSummary,
    Custom,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTimelineItem {
    pub entry_id: String,
    pub timestamp: Option<String>,
    pub kind: SessionTimelineKind,
    pub title: Option<String>,
    pub text: String,
    pub text_truncated: bool,
    pub is_error: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryPage {
    pub session_id: String,
    /// Newest persisted entry observed while locating the active leaf. This is
    /// suitable for seeding Pi's incremental `get_entries(since)` cursor when
    /// present; older pages leave it unset.
    pub append_cursor: Option<String>,
    /// Persisted leaf paired with `append_cursor`. Pi Wizard currently reads
    /// the latest persisted branch until a separate authoritative leaf is
    /// available from live session synchronization.
    pub leaf_id: Option<String>,
    pub items: Vec<SessionTimelineItem>,
    pub next_cursor: Option<SessionHistoryCursor>,
    pub scanned_bytes: usize,
    pub encoded_bytes: usize,
}

#[derive(Debug, Error)]
pub enum SessionHistoryError {
    #[error("failed to open Pi session file {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read Pi session file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Pi session header exceeds {limit} bytes")]
    HeaderTooLarge { limit: usize },
    #[error("Pi session header is missing or malformed")]
    InvalidHeader,
    #[error("Pi session id mismatch: expected {expected}, found {actual}")]
    SessionMismatch { expected: String, actual: String },
    #[error("Pi session belongs to {actual}, not expected project {expected}")]
    ProjectMismatch { expected: PathBuf, actual: PathBuf },
    #[error("history cursor does not belong to session {expected}")]
    CursorSessionMismatch { expected: String },
    #[error("history cursor entry id is {actual} bytes; limit is {limit}")]
    CursorEntryTooLong { actual: usize, limit: usize },
    #[error("history cursor offset {offset} is beyond session file size {file_size}")]
    CursorBeyondFile { offset: u64, file_size: u64 },
    #[error("session JSONL line exceeds history line limit {limit} bytes")]
    LineTooLarge { limit: usize },
    #[error("active session ancestry is broken before entry {entry_id}")]
    BrokenAncestry { entry_id: String },
    #[error("active session ancestry repeats entry id {entry_id}")]
    RepeatedEntry { entry_id: String },
    #[error("encoded history page is {actual} bytes; limit is {limit}")]
    PageTooLarge { actual: usize, limit: usize },
}

/// Reads one bounded page from the persisted active branch of a Pi JSONL
/// session without materializing the complete file or complete tree.
///
/// Pi appends parents before children. Starting at the newest persisted entry,
/// this reader scans backward and follows only matching `parentId` values. An
/// abandoned branch is therefore skipped without building a global id map.
pub fn read_session_history_page(
    session_path: &Path,
    expected_project_root: &Path,
    expected_session_id: &str,
    cursor: Option<&SessionHistoryCursor>,
    limits: RuntimeLimits,
) -> Result<SessionHistoryPage, SessionHistoryError> {
    let expected_project_root =
        expected_project_root
            .canonicalize()
            .map_err(|source| SessionHistoryError::Read {
                path: expected_project_root.to_path_buf(),
                source,
            })?;
    let mut file = File::open(session_path).map_err(|source| SessionHistoryError::Open {
        path: session_path.to_path_buf(),
        source,
    })?;
    let file_size = file
        .metadata()
        .map_err(|source| SessionHistoryError::Read {
            path: session_path.to_path_buf(),
            source,
        })?
        .len();
    validate_header(
        &mut file,
        session_path,
        &expected_project_root,
        expected_session_id,
        limits,
    )?;

    let (before_offset, mut target) = match cursor {
        Some(cursor) => {
            validate_cursor(cursor, expected_session_id, file_size, limits)?;
            let target = if cursor.seek_latest {
                HistoryTarget::Latest
            } else if let Some(id) = &cursor.next_entry_id {
                HistoryTarget::Entry(id.clone())
            } else {
                HistoryTarget::Done
            };
            (cursor.before_offset, target)
        }
        None => (file_size, HistoryTarget::Latest),
    };

    if matches!(target, HistoryTarget::Done) {
        return Ok(SessionHistoryPage {
            session_id: expected_session_id.to_owned(),
            append_cursor: None,
            leaf_id: None,
            items: Vec::new(),
            next_cursor: None,
            scanned_bytes: 0,
            encoded_bytes: 0,
        });
    }

    let mut reader = ReverseLineReader::new(
        &mut file,
        before_offset,
        limits.max_session_history_scan_bytes_per_page,
        limits.max_session_history_line_bytes,
    );
    let mut reverse_items = Vec::new();
    let mut resident_item_bytes = 0usize;
    let mut seen_chain_ids = HashSet::new();
    let mut next_cursor = None;
    let mut append_cursor = None;

    loop {
        if reverse_items.len() >= limits.max_session_history_page_items {
            next_cursor = cursor_for_target(expected_session_id, reader.before_offset(), &target);
            break;
        }

        let retry_before = reader.before_offset();
        let line = match reader.next_line(session_path)? {
            ReverseRead::Line(line) => line,
            ReverseRead::BudgetExhausted => {
                next_cursor =
                    cursor_for_target(expected_session_id, reader.before_offset(), &target);
                break;
            }
            ReverseRead::Eof => match &target {
                HistoryTarget::Latest => break,
                HistoryTarget::Entry(entry_id) => {
                    return Err(SessionHistoryError::BrokenAncestry {
                        entry_id: entry_id.clone(),
                    });
                }
                HistoryTarget::Done => break,
            },
        };
        if line.bytes.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(&line.bytes) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) == Some("session") {
            if let HistoryTarget::Entry(entry_id) = &target {
                return Err(SessionHistoryError::BrokenAncestry {
                    entry_id: entry_id.clone(),
                });
            }
            break;
        }
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() || id.len() > limits.max_session_cursor_bytes {
            continue;
        }

        let matches_target = match &target {
            HistoryTarget::Latest => true,
            HistoryTarget::Entry(expected) => id == expected,
            HistoryTarget::Done => false,
        };
        if !matches_target {
            continue;
        }
        if matches!(target, HistoryTarget::Latest) {
            append_cursor = Some(id.to_owned());
        }
        if !seen_chain_ids.insert(id.to_owned()) {
            return Err(SessionHistoryError::RepeatedEntry {
                entry_id: id.to_owned(),
            });
        }

        let parent_id = entry
            .get("parentId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if parent_id.as_deref() == Some(id) {
            return Err(SessionHistoryError::RepeatedEntry {
                entry_id: id.to_owned(),
            });
        }

        if let Some(item) = timeline_item(&entry, limits) {
            let item_bytes = serde_json::to_vec(&item).unwrap_or_default().len();
            if resident_item_bytes.saturating_add(item_bytes)
                > limits.max_session_history_page_bytes
            {
                next_cursor = Some(SessionHistoryCursor {
                    session_id: expected_session_id.to_owned(),
                    before_offset: retry_before,
                    next_entry_id: Some(id.to_owned()),
                    seek_latest: false,
                });
                break;
            }
            resident_item_bytes = resident_item_bytes.saturating_add(item_bytes);
            reverse_items.push(item);
        }

        target = match parent_id {
            Some(parent_id) => HistoryTarget::Entry(parent_id),
            None => HistoryTarget::Done,
        };
        if matches!(target, HistoryTarget::Done) {
            break;
        }
    }

    reverse_items.reverse();
    let mut page = SessionHistoryPage {
        session_id: expected_session_id.to_owned(),
        leaf_id: append_cursor.clone(),
        append_cursor,
        items: reverse_items,
        next_cursor,
        scanned_bytes: reader.scanned_bytes(),
        encoded_bytes: 0,
    };
    let encoded_bytes = serde_json::to_vec(&page).unwrap_or_default().len();
    if encoded_bytes > limits.max_session_history_page_bytes {
        return Err(SessionHistoryError::PageTooLarge {
            actual: encoded_bytes,
            limit: limits.max_session_history_page_bytes,
        });
    }
    page.encoded_bytes = encoded_bytes;
    Ok(page)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HistoryTarget {
    Latest,
    Entry(String),
    Done,
}

fn cursor_for_target(
    session_id: &str,
    before_offset: u64,
    target: &HistoryTarget,
) -> Option<SessionHistoryCursor> {
    match target {
        HistoryTarget::Done => None,
        HistoryTarget::Latest => Some(SessionHistoryCursor {
            session_id: session_id.to_owned(),
            before_offset,
            next_entry_id: None,
            seek_latest: true,
        }),
        HistoryTarget::Entry(entry_id) => Some(SessionHistoryCursor {
            session_id: session_id.to_owned(),
            before_offset,
            next_entry_id: Some(entry_id.clone()),
            seek_latest: false,
        }),
    }
}

fn validate_cursor(
    cursor: &SessionHistoryCursor,
    expected_session_id: &str,
    file_size: u64,
    limits: RuntimeLimits,
) -> Result<(), SessionHistoryError> {
    if cursor.session_id != expected_session_id {
        return Err(SessionHistoryError::CursorSessionMismatch {
            expected: expected_session_id.to_owned(),
        });
    }
    if let Some(entry_id) = &cursor.next_entry_id
        && (entry_id.is_empty() || entry_id.len() > limits.max_session_cursor_bytes)
    {
        return Err(SessionHistoryError::CursorEntryTooLong {
            actual: entry_id.len(),
            limit: limits.max_session_cursor_bytes,
        });
    }
    if cursor.before_offset > file_size {
        return Err(SessionHistoryError::CursorBeyondFile {
            offset: cursor.before_offset,
            file_size,
        });
    }
    if cursor.seek_latest && cursor.next_entry_id.is_some() {
        return Err(SessionHistoryError::BrokenAncestry {
            entry_id: cursor.next_entry_id.clone().unwrap_or_default(),
        });
    }
    Ok(())
}

fn validate_header(
    file: &mut File,
    path: &Path,
    expected_project_root: &Path,
    expected_session_id: &str,
    limits: RuntimeLimits,
) -> Result<(), SessionHistoryError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|source| SessionHistoryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let read_limit = limits.max_session_header_scan_bytes.saturating_add(1);
    let mut bytes = Vec::with_capacity(read_limit);
    (&mut *file)
        .take(u64::try_from(read_limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|source| SessionHistoryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let line_end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(bytes.len());
    if line_end > limits.max_session_header_scan_bytes
        || (line_end == bytes.len() && bytes.len() > limits.max_session_header_scan_bytes)
    {
        return Err(SessionHistoryError::HeaderTooLarge {
            limit: limits.max_session_header_scan_bytes,
        });
    }
    let header_bytes = bytes
        .get(..line_end)
        .ok_or(SessionHistoryError::InvalidHeader)?;
    let header: Value =
        serde_json::from_slice(header_bytes).map_err(|_| SessionHistoryError::InvalidHeader)?;
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return Err(SessionHistoryError::InvalidHeader);
    }
    let actual_id = header
        .get("id")
        .and_then(Value::as_str)
        .ok_or(SessionHistoryError::InvalidHeader)?;
    if actual_id != expected_session_id {
        return Err(SessionHistoryError::SessionMismatch {
            expected: expected_session_id.to_owned(),
            actual: actual_id.to_owned(),
        });
    }
    let cwd = header
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or(SessionHistoryError::InvalidHeader)?;
    let actual_project = Path::new(cwd)
        .canonicalize()
        .map_err(|_| SessionHistoryError::InvalidHeader)?;
    if actual_project != expected_project_root {
        return Err(SessionHistoryError::ProjectMismatch {
            expected: expected_project_root.to_path_buf(),
            actual: actual_project,
        });
    }
    Ok(())
}

struct ReverseLine {
    bytes: Vec<u8>,
}

enum ReverseRead {
    Line(ReverseLine),
    BudgetExhausted,
    Eof,
}

struct ReverseLineReader<'a> {
    file: &'a mut File,
    buffer: Vec<u8>,
    buffer_start: u64,
    cursor: usize,
    initial_before: u64,
    scanned_bytes: usize,
    max_scan_bytes: usize,
    max_line_bytes: usize,
}

impl<'a> ReverseLineReader<'a> {
    fn new(
        file: &'a mut File,
        before_offset: u64,
        max_scan_bytes: usize,
        max_line_bytes: usize,
    ) -> Self {
        Self {
            file,
            buffer: Vec::new(),
            buffer_start: before_offset,
            cursor: 0,
            initial_before: before_offset,
            scanned_bytes: 0,
            max_scan_bytes,
            max_line_bytes,
        }
    }

    fn before_offset(&self) -> u64 {
        if self.buffer.is_empty() {
            self.initial_before
        } else {
            self.buffer_start
                .saturating_add(u64::try_from(self.cursor).unwrap_or(u64::MAX))
        }
    }

    const fn scanned_bytes(&self) -> usize {
        self.scanned_bytes
    }

    fn next_line(&mut self, path: &Path) -> Result<ReverseRead, SessionHistoryError> {
        loop {
            if self.buffer.is_empty() || self.cursor == 0 {
                let end = if self.buffer.is_empty() {
                    self.initial_before
                } else {
                    self.buffer_start
                };
                match self.load_block_ending_at(end, path)? {
                    BlockLoad::Loaded => {}
                    BlockLoad::BudgetExhausted => return Ok(ReverseRead::BudgetExhausted),
                    BlockLoad::Eof => return Ok(ReverseRead::Eof),
                }
            }

            let mut end = self.cursor;
            if end > 0 && self.buffer[end - 1] == b'\n' {
                end -= 1;
            }
            if end > 0 && self.buffer[end - 1] == b'\r' {
                end -= 1;
            }

            if let Some(newline) = self.buffer[..end].iter().rposition(|byte| *byte == b'\n') {
                let start = newline + 1;
                let line = &self.buffer[start..end];
                if line.len() > self.max_line_bytes {
                    return Err(SessionHistoryError::LineTooLarge {
                        limit: self.max_line_bytes,
                    });
                }
                let bytes = line.to_vec();
                self.cursor = start;
                return Ok(ReverseRead::Line(ReverseLine { bytes }));
            }

            if self.buffer_start == 0 {
                let line = &self.buffer[..end];
                if line.len() > self.max_line_bytes {
                    return Err(SessionHistoryError::LineTooLarge {
                        limit: self.max_line_bytes,
                    });
                }
                let bytes = line.to_vec();
                self.cursor = 0;
                return Ok(ReverseRead::Line(ReverseLine { bytes }));
            }

            if end > self.max_line_bytes {
                return Err(SessionHistoryError::LineTooLarge {
                    limit: self.max_line_bytes,
                });
            }
            let fragment = self.buffer[..end].to_vec();
            match self.prepend_block(fragment, path)? {
                BlockLoad::Loaded => {}
                BlockLoad::BudgetExhausted => return Ok(ReverseRead::BudgetExhausted),
                BlockLoad::Eof => return Ok(ReverseRead::Eof),
            }
        }
    }

    fn load_block_ending_at(
        &mut self,
        end: u64,
        path: &Path,
    ) -> Result<BlockLoad, SessionHistoryError> {
        if end == 0 {
            return Ok(BlockLoad::Eof);
        }
        let remaining_budget = self.max_scan_bytes.saturating_sub(self.scanned_bytes);
        if remaining_budget == 0 {
            return Ok(BlockLoad::BudgetExhausted);
        }
        let block_len = usize::try_from(end)
            .unwrap_or(usize::MAX)
            .min(REVERSE_SCAN_BLOCK_BYTES)
            .min(remaining_budget);
        if block_len == 0 {
            return Ok(BlockLoad::BudgetExhausted);
        }
        let start = end.saturating_sub(u64::try_from(block_len).unwrap_or(u64::MAX));
        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|source| SessionHistoryError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        let mut block = vec![0u8; block_len];
        self.file
            .read_exact(&mut block)
            .map_err(|source| SessionHistoryError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        self.scanned_bytes = self.scanned_bytes.saturating_add(block_len);
        self.buffer = block;
        self.buffer_start = start;
        self.cursor = block_len;
        Ok(BlockLoad::Loaded)
    }

    fn prepend_block(
        &mut self,
        fragment: Vec<u8>,
        path: &Path,
    ) -> Result<BlockLoad, SessionHistoryError> {
        let end = self.buffer_start;
        if end == 0 {
            return Ok(BlockLoad::Eof);
        }
        let remaining_budget = self.max_scan_bytes.saturating_sub(self.scanned_bytes);
        if remaining_budget == 0 {
            return Ok(BlockLoad::BudgetExhausted);
        }
        let block_len = usize::try_from(end)
            .unwrap_or(usize::MAX)
            .min(REVERSE_SCAN_BLOCK_BYTES)
            .min(remaining_budget);
        let start = end.saturating_sub(u64::try_from(block_len).unwrap_or(u64::MAX));
        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|source| SessionHistoryError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        let mut combined = Vec::with_capacity(block_len.saturating_add(fragment.len()));
        combined.resize(block_len, 0);
        self.file
            .read_exact(&mut combined[..block_len])
            .map_err(|source| SessionHistoryError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        combined.extend_from_slice(&fragment);
        self.scanned_bytes = self.scanned_bytes.saturating_add(block_len);
        self.buffer = combined;
        self.buffer_start = start;
        self.cursor = self.buffer.len();
        Ok(BlockLoad::Loaded)
    }
}

enum BlockLoad {
    Loaded,
    BudgetExhausted,
    Eof,
}

fn timeline_item(entry: &Value, limits: RuntimeLimits) -> Option<SessionTimelineItem> {
    let id = entry.get("id")?.as_str()?.to_owned();
    let timestamp = entry
        .get("timestamp")
        .and_then(Value::as_str)
        .map(|value| bounded_prefix(value, 128).0);
    match entry.get("type").and_then(Value::as_str)? {
        "message" => message_item(entry.get("message")?, id, timestamp, limits),
        "compaction" => {
            let summary = entry.get("summary").and_then(Value::as_str).unwrap_or("");
            let (text, text_truncated) =
                bounded_prefix(summary, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id: id,
                timestamp,
                kind: SessionTimelineKind::Compaction,
                title: Some("Compaction".to_owned()),
                text,
                text_truncated,
                is_error: false,
            })
        }
        "branch_summary" => {
            let summary = entry.get("summary").and_then(Value::as_str).unwrap_or("");
            let (text, text_truncated) =
                bounded_prefix(summary, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id: id,
                timestamp,
                kind: SessionTimelineKind::BranchSummary,
                title: Some("Branch summary".to_owned()),
                text,
                text_truncated,
                is_error: false,
            })
        }
        "custom_message" => {
            let content = entry.get("content")?;
            let display = entry
                .get("display")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !display {
                return None;
            }
            let (text, text_truncated) =
                content_text(content, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id: id,
                timestamp,
                kind: SessionTimelineKind::Custom,
                title: entry
                    .get("customType")
                    .and_then(Value::as_str)
                    .map(|value| bounded_prefix(value, 256).0),
                text,
                text_truncated,
                is_error: false,
            })
        }
        _ => None,
    }
}

fn message_item(
    message: &Value,
    entry_id: String,
    timestamp: Option<String>,
    limits: RuntimeLimits,
) -> Option<SessionTimelineItem> {
    let role = message.get("role")?.as_str()?;
    match role {
        "user" => {
            let content = message.get("content")?;
            let (text, text_truncated) =
                content_text(content, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::User,
                title: Some("You".to_owned()),
                text,
                text_truncated,
                is_error: false,
            })
        }
        "assistant" => {
            let content = message.get("content")?;
            let (mut text, mut text_truncated) =
                assistant_text(content, limits.max_session_history_item_text_bytes);
            if text.is_empty()
                && let Some(error) = message.get("errorMessage").and_then(Value::as_str)
            {
                (text, text_truncated) =
                    bounded_prefix(error, limits.max_session_history_item_text_bytes);
            }
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::Assistant,
                title: message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(|value| bounded_prefix(value, 256).0),
                text,
                text_truncated,
                is_error: message.get("stopReason").and_then(Value::as_str) == Some("error"),
            })
        }
        "toolResult" => {
            let content = message.get("content")?;
            let (text, text_truncated) =
                content_text_tail(content, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::Tool,
                title: message
                    .get("toolName")
                    .and_then(Value::as_str)
                    .map(|value| bounded_prefix(value, 256).0),
                text,
                text_truncated,
                is_error: message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        }
        "bashExecution" => {
            let output = message.get("output").and_then(Value::as_str).unwrap_or("");
            let (text, text_truncated) =
                bounded_tail(output, limits.max_session_history_item_text_bytes);
            let exit_code = message.get("exitCode").and_then(Value::as_i64);
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::Bash,
                title: message
                    .get("command")
                    .and_then(Value::as_str)
                    .map(|value| bounded_prefix(value, 512).0),
                text,
                text_truncated: text_truncated
                    || message
                        .get("truncated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                is_error: exit_code.is_some_and(|code| code != 0),
            })
        }
        "custom" => {
            if !message
                .get("display")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            let content = message.get("content")?;
            let (text, text_truncated) =
                content_text(content, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::Custom,
                title: message
                    .get("customType")
                    .and_then(Value::as_str)
                    .map(|value| bounded_prefix(value, 256).0),
                text,
                text_truncated,
                is_error: false,
            })
        }
        "branchSummary" => {
            let summary = message.get("summary").and_then(Value::as_str).unwrap_or("");
            let (text, text_truncated) =
                bounded_prefix(summary, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::BranchSummary,
                title: Some("Branch summary".to_owned()),
                text,
                text_truncated,
                is_error: false,
            })
        }
        "compactionSummary" => {
            let summary = message.get("summary").and_then(Value::as_str).unwrap_or("");
            let (text, text_truncated) =
                bounded_prefix(summary, limits.max_session_history_item_text_bytes);
            Some(SessionTimelineItem {
                entry_id,
                timestamp,
                kind: SessionTimelineKind::Compaction,
                title: Some("Compaction".to_owned()),
                text,
                text_truncated,
                is_error: false,
            })
        }
        _ => None,
    }
}

fn assistant_text(content: &Value, max_bytes: usize) -> (String, bool) {
    let Some(blocks) = content.as_array() else {
        return content_text(content, max_bytes);
    };
    let mut collector = TextCollector::new(max_bytes);
    let mut tool_names = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    collector.append_segment(text);
                }
            }
            Some("toolCall") if tool_names.len() < 16 => {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    tool_names.push(name);
                }
            }
            _ => {}
        }
    }
    if collector.text.is_empty() && !tool_names.is_empty() {
        collector.append_segment("Tool calls:");
        for name in tool_names {
            collector.append_segment(name);
        }
    }
    collector.finish()
}

/// Tool and shell output is usually most useful at the end, where exit
/// diagnostics and compiler failures accumulate. Historical output therefore
/// follows the same newest-suffix policy as the live bounded tool projection.
fn content_text_tail(content: &Value, max_bytes: usize) -> (String, bool) {
    if let Some(text) = content.as_str() {
        return bounded_tail(text, max_bytes);
    }
    let Some(blocks) = content.as_array() else {
        return (String::new(), false);
    };
    let mut output = BoundedText::new(max_bytes);
    let mut has_content = false;
    for block in blocks {
        let segment = match block.get("type").and_then(Value::as_str) {
            Some("text") => block.get("text").and_then(Value::as_str),
            Some("image") => Some("[image attachment]"),
            _ => None,
        };
        let Some(segment) = segment else { continue };
        if has_content {
            output.append("\n");
        }
        output.append(segment);
        has_content = true;
    }
    (output.as_str().to_owned(), output.dropped_bytes() > 0)
}

fn content_text(content: &Value, max_bytes: usize) -> (String, bool) {
    if let Some(text) = content.as_str() {
        return bounded_prefix(text, max_bytes);
    }
    let Some(blocks) = content.as_array() else {
        return (String::new(), false);
    };
    let mut collector = TextCollector::new(max_bytes);
    let mut images = 0usize;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    collector.append_segment(text);
                }
            }
            Some("image") => images = images.saturating_add(1),
            _ => {}
        }
    }
    if collector.text.is_empty() && images > 0 {
        collector.append_segment(if images == 1 {
            "[1 image attachment]"
        } else {
            "[image attachments]"
        });
    }
    collector.finish()
}

struct TextCollector {
    text: String,
    max_bytes: usize,
    truncated: bool,
}

impl TextCollector {
    fn new(max_bytes: usize) -> Self {
        Self {
            text: String::new(),
            max_bytes,
            truncated: false,
        }
    }

    fn append_segment(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let separator = usize::from(!self.text.is_empty());
        let available = self
            .max_bytes
            .saturating_sub(self.text.len())
            .saturating_sub(separator);
        if available == 0 {
            self.truncated = true;
            return;
        }
        if separator == 1 {
            self.text.push('\n');
        }
        let (prefix, truncated) = bounded_prefix(value, available);
        self.text.push_str(&prefix);
        self.truncated |= truncated;
    }

    fn finish(self) -> (String, bool) {
        (self.text, self.truncated)
    }
}

fn bounded_prefix(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

fn bounded_tail(value: &str, max_bytes: usize) -> (String, bool) {
    let mut bounded = BoundedText::new(max_bytes);
    bounded.replace(value);
    (bounded.as_str().to_owned(), bounded.dropped_bytes() > 0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;
    use crate::RunId;

    struct Fixture {
        root: PathBuf,
        project: PathBuf,
        session: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("pi-wizard-session-history-{name}-{}", RunId::new()));
            let project = root.join("project");
            fs::create_dir_all(&project).expect("project fixture");
            let session = root.join("session.jsonl");
            Self {
                root,
                project,
                session,
            }
        }

        fn write(&self, entries: &[Value]) {
            let mut file = File::create(&self.session).expect("session fixture");
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type":"session",
                    "version":3,
                    "id":"session-1",
                    "timestamp":"2026-08-27T00:00:00.000Z",
                    "cwd":self.project.canonicalize().unwrap(),
                })
            )
            .unwrap();
            for entry in entries {
                writeln!(file, "{entry}").unwrap();
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn user(id: &str, parent: Option<&str>, text: &str) -> Value {
        serde_json::json!({
            "type":"message","id":id,"parentId":parent,
            "timestamp":"2026-08-27T00:00:01.000Z",
            "message":{"role":"user","content":text}
        })
    }

    fn assistant(id: &str, parent: Option<&str>, text: &str) -> Value {
        serde_json::json!({
            "type":"message","id":id,"parentId":parent,
            "timestamp":"2026-08-27T00:00:02.000Z",
            "message":{"role":"assistant","content":[{"type":"text","text":text}],"model":"fixture","stopReason":"stop"}
        })
    }

    #[test]
    fn latest_page_follows_only_the_persisted_active_branch() {
        let fixture = Fixture::new("active-branch");
        fixture.write(&[
            user("u1", None, "root"),
            assistant("a1", Some("u1"), "root answer"),
            user("abandoned-u", Some("a1"), "abandoned"),
            assistant("abandoned-a", Some("abandoned-u"), "wrong branch"),
            serde_json::json!({"type":"model_change","id":"model","parentId":"a1","timestamp":"2026-08-27T00:00:03.000Z","provider":"x","modelId":"y"}),
            user("u2", Some("model"), "active"),
            assistant("a2", Some("u2"), "active answer"),
        ]);

        let page = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            None,
            RuntimeLimits::default(),
        )
        .expect("latest history");
        let ids: Vec<_> = page
            .items
            .iter()
            .map(|item| item.entry_id.as_str())
            .collect();
        assert_eq!(ids, ["u1", "a1", "u2", "a2"]);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn older_cursor_continues_without_repeating_newer_items() {
        let fixture = Fixture::new("paging");
        fixture.write(&[
            user("u1", None, "one"),
            assistant("a1", Some("u1"), "one-a"),
            user("u2", Some("a1"), "two"),
            assistant("a2", Some("u2"), "two-a"),
            user("u3", Some("a2"), "three"),
            assistant("a3", Some("u3"), "three-a"),
        ]);
        let limits = RuntimeLimits {
            max_session_history_page_items: 2,
            ..RuntimeLimits::default()
        };
        let latest = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            None,
            limits,
        )
        .expect("latest page");
        assert_eq!(
            latest
                .items
                .iter()
                .map(|item| item.entry_id.as_str())
                .collect::<Vec<_>>(),
            ["u3", "a3"]
        );
        let older = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            latest.next_cursor.as_ref(),
            limits,
        )
        .expect("older page");
        assert_eq!(
            older
                .items
                .iter()
                .map(|item| item.entry_id.as_str())
                .collect::<Vec<_>>(),
            ["u2", "a2"]
        );
    }

    #[test]
    fn scan_budget_returns_a_progress_cursor_instead_of_loading_the_whole_file() {
        let fixture = Fixture::new("scan-budget");
        let mut entries = vec![user("root", None, "root")];
        let mut parent = "root".to_owned();
        for index in 0..80 {
            let id = format!("m{index:03}");
            entries.push(serde_json::json!({
                "type":"model_change","id":id,"parentId":parent,
                "timestamp":"2026-08-27T00:00:02.000Z","provider":"fake","modelId":"fake"
            }));
            parent = id;
        }
        entries.push(assistant("leaf", Some(&parent), "latest"));
        fixture.write(&entries);
        let limits = RuntimeLimits {
            max_session_history_scan_bytes_per_page: 1024,
            max_session_history_line_bytes: 512,
            ..RuntimeLimits::default()
        };

        let page = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            None,
            limits,
        )
        .expect("bounded page");
        assert!(page.scanned_bytes <= limits.max_session_history_scan_bytes_per_page);
        assert!(page.next_cursor.is_some());
        assert!(page.items.iter().any(|item| item.entry_id == "leaf"));
    }

    #[test]
    fn cursor_cannot_be_reused_for_another_session() {
        let fixture = Fixture::new("cursor-session");
        fixture.write(&[user("u1", None, "one")]);
        let cursor = SessionHistoryCursor {
            session_id: "other-session".to_owned(),
            before_offset: fs::metadata(&fixture.session).unwrap().len(),
            next_entry_id: Some("u1".to_owned()),
            seek_latest: false,
        };
        assert!(matches!(
            read_session_history_page(
                &fixture.session,
                &fixture.project,
                "session-1",
                Some(&cursor),
                RuntimeLimits::default(),
            ),
            Err(SessionHistoryError::CursorSessionMismatch { .. })
        ));
    }

    #[test]
    fn history_text_and_jsonl_lines_have_independent_hard_bounds() {
        let fixture = Fixture::new("text-bounds");
        fixture.write(&[user("u1", None, &"x".repeat(1024))]);
        let limits = RuntimeLimits {
            max_session_history_item_text_bytes: 32,
            max_session_history_line_bytes: 2048,
            max_session_history_scan_bytes_per_page: 4096,
            ..RuntimeLimits::default()
        };
        let page = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            None,
            limits,
        )
        .expect("truncated item");
        assert_eq!(page.items[0].text.len(), 32);
        assert!(page.items[0].text_truncated);

        let too_small = RuntimeLimits {
            max_session_history_item_text_bytes: 16,
            max_session_history_line_bytes: 128,
            max_session_history_scan_bytes_per_page: 512,
            ..RuntimeLimits::default()
        };
        assert!(matches!(
            read_session_history_page(
                &fixture.session,
                &fixture.project,
                "session-1",
                None,
                too_small,
            ),
            Err(SessionHistoryError::LineTooLarge { limit: 128 })
        ));
    }

    #[test]
    fn historical_tool_and_bash_output_keep_the_newest_bounded_suffix() {
        let fixture = Fixture::new("output-tail");
        fixture.write(&[
            serde_json::json!({
                "type":"message","id":"tool","parentId":null,
                "timestamp":"2026-08-27T00:00:01.000Z",
                "message":{
                    "role":"toolResult","toolCallId":"call","toolName":"bash",
                    "content":[{"type":"text","text":"0123456789"}],"isError":true
                }
            }),
            serde_json::json!({
                "type":"message","id":"bash","parentId":"tool",
                "timestamp":"2026-08-27T00:00:02.000Z",
                "message":{
                    "role":"bashExecution","command":"fixture","output":"abcdefghij",
                    "exitCode":1,"cancelled":false,"truncated":false
                }
            }),
        ]);
        let limits = RuntimeLimits {
            max_session_history_item_text_bytes: 6,
            ..RuntimeLimits::default()
        };
        let page = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            None,
            limits,
        )
        .expect("history page");
        assert_eq!(page.items[0].text, "456789");
        assert_eq!(page.items[1].text, "efghij");
        assert!(page.items.iter().all(|item| item.text_truncated));
        assert!(page.items.iter().all(|item| item.is_error));
    }

    #[test]
    #[ignore = "scale fixture; exercised by full verification"]
    fn twenty_five_megabyte_session_opens_latest_page_with_bounded_scan_and_page_memory() {
        let fixture = Fixture::new("scale-25mb");
        let mut file = File::create(&fixture.session).expect("large session fixture");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type":"session",
                "version":3,
                "id":"session-1",
                "timestamp":"2026-08-27T00:00:00.000Z",
                "cwd":fixture.project.canonicalize().unwrap(),
            })
        )
        .unwrap();
        let payload = "x".repeat(2600);
        let mut parent: Option<String> = None;
        for index in 0..10_000usize {
            let id = format!("entry-{index:05}");
            writeln!(file, "{}", user(&id, parent.as_deref(), &payload)).unwrap();
            parent = Some(id);
        }
        file.flush().expect("flush large session fixture");
        let file_bytes = fs::metadata(&fixture.session)
            .expect("large session metadata")
            .len();
        assert!(file_bytes >= 25 * 1024 * 1024, "fixture must exceed 25 MiB");

        let limits = RuntimeLimits::default();
        let page = read_session_history_page(
            &fixture.session,
            &fixture.project,
            "session-1",
            None,
            limits,
        )
        .expect("bounded latest page from large session");
        assert_eq!(page.items.len(), limits.max_session_history_page_items);
        assert!(page.next_cursor.is_some());
        assert!(page.scanned_bytes <= limits.max_session_history_scan_bytes_per_page);
        assert!(page.encoded_bytes <= limits.max_session_history_page_bytes);
        assert!(
            page.scanned_bytes as u64 * 20 < file_bytes,
            "latest-page open must not scan a material fraction of the full session"
        );
    }
}
