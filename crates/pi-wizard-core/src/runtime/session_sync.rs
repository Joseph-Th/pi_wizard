use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;
use crate::rpc::SessionEntriesPage;

/// Cursor-only state for live Pi session synchronization.
///
/// Entries themselves are deliberately not retained here. They remain
/// ephemeral output for the history/timeline owner while this state records
/// only the durable append cursor and current leaf identity.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSyncState {
    initialized: bool,
    cursor: Option<String>,
    leaf_id: Option<String>,
    resync_required: bool,
    revision: u64,
}

impl SessionSyncState {
    #[must_use]
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    #[must_use]
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    #[must_use]
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    #[must_use]
    pub const fn resync_required(&self) -> bool {
        self.resync_required
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn validate_request(
        &self,
        since: Option<&str>,
        limits: RuntimeLimits,
    ) -> Result<(), SessionSyncError> {
        validate_cursor(since, limits)?;
        if self.resync_required && since.is_some() {
            return Err(SessionSyncError::ResyncRequired);
        }
        if self.initialized && !self.resync_required && since != self.cursor.as_deref() {
            return Err(SessionSyncError::CursorMismatch {
                expected: self.cursor.clone(),
                actual: since.map(str::to_owned),
            });
        }
        Ok(())
    }

    /// Seeds the cursor after an offline/catalog resynchronization without
    /// loading session entries into this owner.
    pub fn seed(
        &mut self,
        cursor: Option<String>,
        leaf_id: Option<String>,
        limits: RuntimeLimits,
    ) -> Result<(), SessionSyncError> {
        validate_cursor(cursor.as_deref(), limits)?;
        validate_cursor(leaf_id.as_deref(), limits)?;
        self.initialized = true;
        self.cursor = cursor;
        self.leaf_id = leaf_id;
        self.resync_required = false;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn apply_page(
        &mut self,
        requested_since: Option<&str>,
        page: &SessionEntriesPage,
        limits: RuntimeLimits,
    ) -> Result<SessionSyncApplied, SessionSyncError> {
        self.validate_request(requested_since, limits)?;

        let mut ids = HashSet::with_capacity(page.entries.len());
        for entry in &page.entries {
            if !ids.insert(entry.id.as_str()) {
                return Err(SessionSyncError::DuplicateEntryId {
                    id: entry.id.clone(),
                });
            }
        }

        let previous_cursor = self.cursor.clone();
        let previous_leaf = self.leaf_id.clone();
        if let Some(last) = page.entries.last() {
            self.cursor = Some(last.id.clone());
        } else if !self.initialized {
            self.cursor = requested_since.map(str::to_owned);
        }
        self.leaf_id = page.leaf_id.clone();
        self.initialized = true;
        self.resync_required = false;

        let cursor_changed = self.cursor != previous_cursor;
        let leaf_changed = self.leaf_id != previous_leaf;
        if cursor_changed || leaf_changed || !page.entries.is_empty() {
            self.revision = self.revision.saturating_add(1);
        }
        Ok(SessionSyncApplied {
            appended_entries: page.entries.len(),
            cursor_changed,
            leaf_changed,
            revision: self.revision,
        })
    }

    pub fn mark_resync_required(
        &mut self,
        rejected_since: &str,
        limits: RuntimeLimits,
    ) -> Result<SessionSyncResync, SessionSyncError> {
        validate_cursor(Some(rejected_since), limits)?;
        self.resync_required = true;
        self.revision = self.revision.saturating_add(1);
        Ok(SessionSyncResync {
            rejected_since: rejected_since.to_owned(),
            revision: self.revision,
        })
    }

    /// Invalidates the live append cursor when Pi returned a successful page
    /// that cannot fit the app's bounded incremental projection. The persisted
    /// JSONL remains authoritative, so this is a cold-history resync condition,
    /// not a transport/protocol failure that should terminate the Pi process.
    pub fn mark_projection_resync_required(&mut self) -> u64 {
        self.resync_required = true;
        self.revision = self.revision.saturating_add(1);
        self.revision
    }

    pub fn reset_for_session_replacement(&mut self) {
        *self = Self::default();
    }
}

fn validate_cursor(cursor: Option<&str>, limits: RuntimeLimits) -> Result<(), SessionSyncError> {
    if let Some(cursor) = cursor
        && (cursor.is_empty() || cursor.len() > limits.max_session_cursor_bytes)
    {
        return Err(SessionSyncError::InvalidCursorLength {
            actual: cursor.len(),
            limit: limits.max_session_cursor_bytes,
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionSyncApplied {
    pub appended_entries: usize,
    pub cursor_changed: bool,
    pub leaf_changed: bool,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSyncResync {
    pub rejected_since: String,
    pub revision: u64,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SessionSyncError {
    #[error("session cursor is {actual} bytes; expected 1..={limit}")]
    InvalidCursorLength { actual: usize, limit: usize },
    #[error("session synchronization requires an explicit resync before another cursor request")]
    ResyncRequired,
    #[error("session synchronization expected cursor {expected:?}, got {actual:?}")]
    CursorMismatch {
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("get_entries page contains duplicate entry id {id}")]
    DuplicateEntryId { id: String },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::rpc::SessionEntryEnvelope;

    fn page(ids: &[&str], leaf: Option<&str>) -> SessionEntriesPage {
        SessionEntriesPage {
            entries: ids
                .iter()
                .map(|id| SessionEntryEnvelope {
                    id: (*id).to_owned(),
                    parent_id: None,
                    entry_type: "message".to_owned(),
                    timestamp: None,
                    raw: json!({"type":"message","id":id}),
                })
                .collect(),
            leaf_id: leaf.map(str::to_owned),
            encoded_bytes: 0,
        }
    }

    #[test]
    fn append_cursor_advances_to_last_entry_while_leaf_moves_independently() {
        let limits = RuntimeLimits::default();
        let mut sync = SessionSyncState::default();
        sync.seed(Some("a".to_owned()), Some("a".to_owned()), limits)
            .expect("seed");
        let applied = sync
            .apply_page(Some("a"), &page(&["b", "c"], Some("b")), limits)
            .expect("page");

        assert_eq!(sync.cursor(), Some("c"));
        assert_eq!(sync.leaf_id(), Some("b"));
        assert_eq!(applied.appended_entries, 2);
        assert!(applied.cursor_changed);
        assert!(applied.leaf_changed);
    }

    #[test]
    fn stale_cursor_is_rejected_before_outbound_rpc() {
        let limits = RuntimeLimits::default();
        let mut sync = SessionSyncState::default();
        sync.seed(Some("current".to_owned()), None, limits)
            .expect("seed");

        assert_eq!(
            sync.validate_request(Some("old"), limits),
            Err(SessionSyncError::CursorMismatch {
                expected: Some("current".to_owned()),
                actual: Some("old".to_owned()),
            })
        );
    }

    #[test]
    fn rejected_cursor_blocks_incremental_tail_until_explicit_resync_seed() {
        let limits = RuntimeLimits::default();
        let mut sync = SessionSyncState::default();
        sync.seed(Some("a".to_owned()), Some("a".to_owned()), limits)
            .expect("seed");
        sync.mark_resync_required("a", limits).expect("mark resync");

        assert_eq!(
            sync.validate_request(Some("a"), limits),
            Err(SessionSyncError::ResyncRequired)
        );
        sync.seed(Some("z".to_owned()), Some("z".to_owned()), limits)
            .expect("offline resync seed");
        assert_eq!(sync.cursor(), Some("z"));
        assert!(!sync.resync_required());
    }

    #[test]
    fn local_projection_overflow_marks_resync_without_destroying_cursor_identity() {
        let limits = RuntimeLimits::default();
        let mut sync = SessionSyncState::default();
        sync.seed(Some("a".to_owned()), Some("a".to_owned()), limits)
            .expect("seed");
        let revision = sync.mark_projection_resync_required();

        assert!(sync.resync_required());
        assert_eq!(sync.cursor(), Some("a"));
        assert_eq!(sync.leaf_id(), Some("a"));
        assert_eq!(revision, 2);
        assert_eq!(
            sync.validate_request(Some("a"), limits),
            Err(SessionSyncError::ResyncRequired)
        );
    }

    #[test]
    fn duplicate_page_ids_do_not_advance_cursor() {
        let limits = RuntimeLimits::default();
        let mut sync = SessionSyncState::default();
        sync.seed(Some("a".to_owned()), None, limits).expect("seed");

        assert_eq!(
            sync.apply_page(Some("a"), &page(&["b", "b"], Some("b")), limits),
            Err(SessionSyncError::DuplicateEntryId { id: "b".to_owned() })
        );
        assert_eq!(sync.cursor(), Some("a"));
    }
}
