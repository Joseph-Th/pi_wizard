use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable application identity for one registered project.
///
/// This ID is intentionally independent from display names, Git remotes, and
/// paths so a stale project record cannot be silently rebound to a different
/// checkout merely because some metadata happens to match.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(Uuid);

impl ProjectId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Exact Pi session identity generated for an app-created persisted session.
///
/// Pi's `--session-id` creates the session when the ID does not already exist,
/// so this type is used for creation, not for reopening arbitrary history.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PiSessionId(Uuid);

impl PiSessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for PiSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PiSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one app-owned Pi process lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one app-owned Git worktree recovery transaction.
///
/// A transaction exists before Git mutation begins, so it cannot reuse RunId:
/// Pi process creation may never happen, while the Git mutation still needs a
/// durable identity for crash recovery.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorktreeId(Uuid);

impl WorktreeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for WorktreeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorktreeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one reusable saved automation chain.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationChainId(Uuid);

impl AutomationChainId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for AutomationChainId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AutomationChainId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one finite execution of a saved automation chain.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationExecutionId(Uuid);

impl AutomationExecutionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for AutomationExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AutomationExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one independently owned supervision session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SupervisionId(Uuid);

impl SupervisionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for SupervisionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SupervisionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one image attached to a session-scoped composer draft.
///
/// This survives renderer reload and draft persistence so removal is keyed by
/// backend identity rather than browser-local array position or object URLs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DraftImageId(Uuid);

impl DraftImageId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for DraftImageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DraftImageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Correlation identity generated by Pi Wizard for outbound RPC commands.
///
/// Pi's protocol accepts string IDs, so this remains a string on the wire while
/// constructors generate collision-resistant UUIDv7 values.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    #[must_use]
    pub fn from_wire(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
