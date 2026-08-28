//! Tauri-independent runtime foundation for Pi Wizard.
//!
//! This crate owns protocol framing, launch specifications, bounded projections,
//! typed identities, and the live run state machine. Desktop framework types do
//! not belong here.

pub mod bounded;
pub mod compatibility;
pub mod draft;
pub mod draft_persistence;
pub mod environment;
pub mod git_review;
pub mod ids;
pub mod launch;
pub mod limits;
pub mod preferences;
mod probe;
pub mod process;
pub mod project;
pub mod project_registry;
pub mod rpc;
pub mod runtime;
pub mod session_activation;
pub mod session_catalog;
pub mod session_history;
pub mod worktree;
pub mod worktree_registry;

pub use ids::{DraftImageId, PiSessionId, ProjectId, RequestId, RunId, WorktreeId};
pub use limits::{LimitsError, RuntimeLimits};
