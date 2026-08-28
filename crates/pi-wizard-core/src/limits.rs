use serde::{Deserialize, Serialize};
use thiserror::Error;

const HARD_MAX_RPC_FRAME_BYTES: usize = 64 * 1024 * 1024;
const HARD_MAX_RENDER_BUFFER_BYTES: usize = 32 * 1024 * 1024;
const HARD_MAX_ATTACHMENTS_PER_PROMPT: usize = 32;
const HARD_MAX_ATTACHMENT_NAME_BYTES: usize = 4096;
const HARD_MAX_STREAM_CONTENT_BLOCKS: usize = 4096;
const HARD_MAX_RECOVERED_QUEUE_MESSAGES: usize = 4096;
const HARD_MAX_CAPABILITY_ENTRIES: usize = 16 * 1024;
const HARD_MAX_PROJECT_REGISTRY_ENTRIES: usize = 64 * 1024;
const HARD_MAX_PREFERENCES_BYTES: usize = 1024 * 1024;
const HARD_MAX_SESSION_PAGE_ENTRIES: usize = 8 * 1024;
const HARD_MAX_SESSION_CATALOG_CANDIDATES: usize = 64 * 1024;
const HARD_MAX_SESSION_CATALOG_SCAN_FILES: usize = 8 * 1024;
const HARD_MAX_SESSION_CATALOG_PAGE_ENTRIES: usize = 1024;
const HARD_MAX_SESSION_HEADER_SCAN_BYTES: usize = 1024 * 1024;
const HARD_MAX_SESSION_HISTORY_PAGE_ITEMS: usize = 1024;
const HARD_MAX_SESSION_HISTORY_LINE_BYTES: usize = 8 * 1024 * 1024;
const HARD_MAX_SESSION_HISTORY_SCAN_BYTES: usize = 64 * 1024 * 1024;
const HARD_MAX_RUNTIME_CHANNEL_ENTRIES: usize = 64 * 1024;
const HARD_MAX_LIVE_RUNS: usize = 256;
const HARD_MAX_RETAINED_TERMINAL_RUNS: usize = 4096;
const HARD_MAX_CACHED_DRAFT_RECORDS: usize = 4096;
const HARD_MAX_GIT_COMMAND_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const HARD_MAX_GIT_REF_BYTES: usize = 4 * 1024;
const HARD_MAX_WORKTREE_PATH_BYTES: usize = 64 * 1024;
const HARD_MAX_WORKTREE_REGISTRY_ENTRIES: usize = 16 * 1024;
const HARD_MAX_WORKTREE_RECOVERY_PAGE_ENTRIES: usize = 1024;
const HARD_MAX_GIT_REVIEW_FILES: usize = 16 * 1024;
const HARD_MAX_GIT_DIFF_BYTES: usize = 8 * 1024 * 1024;
const HARD_MAX_GIT_DIFF_PAGE_BYTES: usize = 1024 * 1024;
const HARD_MAX_GIT_DIFF_SCAN_BYTES: usize = 64 * 1024 * 1024;
const HARD_MAX_GIT_DIFF_HUNKS_PER_PAGE: usize = 4096;

/// Centralized resource ceilings for data Pi Wizard owns in memory.
///
/// These limits are configuration, not suggestions. Protocol and projection
/// owners receive this value explicitly so no subsystem silently invents an
/// unbounded buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLimits {
    pub max_rpc_frame_bytes: usize,
    pub max_outbound_rpc_bytes: usize,
    pub max_stderr_bytes_per_run: usize,
    pub max_stream_text_bytes_per_run: usize,
    pub max_stream_content_blocks_per_message: usize,
    pub max_tool_preview_bytes: usize,
    pub max_ui_backlog_bytes_per_run: usize,
    pub max_active_tools_per_run: usize,
    pub max_pending_rpc_requests_per_run: usize,
    pub max_pending_ui_requests_per_run: usize,
    pub max_extension_ui_entries_per_run: usize,
    pub max_extension_ui_bytes_per_run: usize,
    pub max_failure_detail_bytes: usize,
    pub max_attachments_per_prompt: usize,
    pub max_attachment_name_bytes: usize,
    pub max_attachment_bytes_per_image: usize,
    pub max_attachment_bytes_per_prompt: usize,
    pub max_draft_bytes_per_session: usize,
    pub max_cached_draft_records: usize,
    pub max_recovered_queue_messages_per_run: usize,
    pub max_recovered_queue_bytes_per_run: usize,
    pub max_project_registry_entries: usize,
    pub max_project_registry_bytes: usize,
    pub max_preferences_bytes: usize,
    pub max_environment_probe_bytes: usize,
    pub max_version_probe_bytes: usize,
    pub max_capability_entries_per_run: usize,
    pub max_capability_bytes_per_run: usize,
    pub max_runtime_state_bytes_per_run: usize,
    pub max_session_entry_page_entries: usize,
    pub max_session_entry_page_bytes: usize,
    pub max_session_cursor_bytes: usize,
    pub max_session_catalog_candidates: usize,
    pub max_session_catalog_scan_files: usize,
    pub max_session_catalog_page_entries: usize,
    pub max_session_catalog_page_bytes: usize,
    pub max_session_header_scan_bytes: usize,
    pub max_session_metadata_scan_bytes: usize,
    pub max_session_catalog_query_bytes: usize,
    pub max_session_history_page_items: usize,
    pub max_session_history_page_bytes: usize,
    pub max_session_history_scan_bytes_per_page: usize,
    pub max_session_history_line_bytes: usize,
    pub max_session_history_item_text_bytes: usize,
    pub max_live_runs: usize,
    pub max_retained_terminal_runs: usize,
    pub max_runtime_command_queue: usize,
    pub max_process_event_queue: usize,
    pub max_git_command_output_bytes: usize,
    pub max_git_ref_bytes: usize,
    pub max_worktree_path_bytes: usize,
    pub max_worktree_registry_entries: usize,
    pub max_worktree_registry_bytes: usize,
    pub max_worktree_recovery_page_entries: usize,
    pub max_git_review_files: usize,
    pub max_git_diff_bytes: usize,
    pub max_git_diff_page_bytes: usize,
    pub max_git_diff_scan_bytes_per_page: usize,
    pub max_git_diff_hunks_per_page: usize,
    pub environment_probe_deadline_ms: u64,
    pub version_probe_deadline_ms: u64,
    pub startup_rpc_deadline_ms: u64,
    pub draft_save_debounce_ms: u64,
    pub draft_flush_deadline_ms: u64,
    pub stop_abort_deadline_ms: u64,
    pub stop_termination_deadline_ms: u64,
    pub git_command_deadline_ms: u64,
}

impl RuntimeLimits {
    pub fn validate(self) -> Result<Self, LimitsError> {
        validate_nonzero("max_rpc_frame_bytes", self.max_rpc_frame_bytes)?;
        validate_nonzero("max_outbound_rpc_bytes", self.max_outbound_rpc_bytes)?;
        validate_nonzero("max_stderr_bytes_per_run", self.max_stderr_bytes_per_run)?;
        validate_nonzero(
            "max_stream_text_bytes_per_run",
            self.max_stream_text_bytes_per_run,
        )?;
        validate_nonzero(
            "max_stream_content_blocks_per_message",
            self.max_stream_content_blocks_per_message,
        )?;
        validate_nonzero("max_tool_preview_bytes", self.max_tool_preview_bytes)?;
        validate_nonzero(
            "max_ui_backlog_bytes_per_run",
            self.max_ui_backlog_bytes_per_run,
        )?;
        validate_nonzero("max_active_tools_per_run", self.max_active_tools_per_run)?;
        validate_nonzero(
            "max_pending_rpc_requests_per_run",
            self.max_pending_rpc_requests_per_run,
        )?;
        validate_nonzero(
            "max_pending_ui_requests_per_run",
            self.max_pending_ui_requests_per_run,
        )?;
        validate_nonzero(
            "max_extension_ui_entries_per_run",
            self.max_extension_ui_entries_per_run,
        )?;
        validate_nonzero(
            "max_extension_ui_bytes_per_run",
            self.max_extension_ui_bytes_per_run,
        )?;
        validate_nonzero("max_failure_detail_bytes", self.max_failure_detail_bytes)?;
        validate_nonzero(
            "max_attachments_per_prompt",
            self.max_attachments_per_prompt,
        )?;
        validate_nonzero("max_attachment_name_bytes", self.max_attachment_name_bytes)?;
        validate_nonzero(
            "max_attachment_bytes_per_image",
            self.max_attachment_bytes_per_image,
        )?;
        validate_nonzero(
            "max_attachment_bytes_per_prompt",
            self.max_attachment_bytes_per_prompt,
        )?;
        validate_nonzero(
            "max_draft_bytes_per_session",
            self.max_draft_bytes_per_session,
        )?;
        validate_nonzero("max_cached_draft_records", self.max_cached_draft_records)?;
        validate_nonzero(
            "max_recovered_queue_messages_per_run",
            self.max_recovered_queue_messages_per_run,
        )?;
        validate_nonzero(
            "max_recovered_queue_bytes_per_run",
            self.max_recovered_queue_bytes_per_run,
        )?;
        validate_nonzero(
            "max_project_registry_entries",
            self.max_project_registry_entries,
        )?;
        validate_nonzero(
            "max_project_registry_bytes",
            self.max_project_registry_bytes,
        )?;
        validate_nonzero("max_preferences_bytes", self.max_preferences_bytes)?;
        validate_nonzero(
            "max_environment_probe_bytes",
            self.max_environment_probe_bytes,
        )?;
        validate_nonzero("max_version_probe_bytes", self.max_version_probe_bytes)?;
        validate_nonzero(
            "max_capability_entries_per_run",
            self.max_capability_entries_per_run,
        )?;
        validate_nonzero(
            "max_capability_bytes_per_run",
            self.max_capability_bytes_per_run,
        )?;
        validate_nonzero(
            "max_runtime_state_bytes_per_run",
            self.max_runtime_state_bytes_per_run,
        )?;
        validate_nonzero(
            "max_session_entry_page_entries",
            self.max_session_entry_page_entries,
        )?;
        validate_nonzero(
            "max_session_entry_page_bytes",
            self.max_session_entry_page_bytes,
        )?;
        validate_nonzero("max_session_cursor_bytes", self.max_session_cursor_bytes)?;
        validate_nonzero(
            "max_session_catalog_candidates",
            self.max_session_catalog_candidates,
        )?;
        validate_nonzero(
            "max_session_catalog_scan_files",
            self.max_session_catalog_scan_files,
        )?;
        validate_nonzero(
            "max_session_catalog_page_entries",
            self.max_session_catalog_page_entries,
        )?;
        validate_nonzero(
            "max_session_catalog_page_bytes",
            self.max_session_catalog_page_bytes,
        )?;
        validate_nonzero(
            "max_session_header_scan_bytes",
            self.max_session_header_scan_bytes,
        )?;
        validate_nonzero(
            "max_session_metadata_scan_bytes",
            self.max_session_metadata_scan_bytes,
        )?;
        validate_nonzero(
            "max_session_catalog_query_bytes",
            self.max_session_catalog_query_bytes,
        )?;
        validate_nonzero(
            "max_session_history_page_items",
            self.max_session_history_page_items,
        )?;
        validate_nonzero(
            "max_session_history_page_bytes",
            self.max_session_history_page_bytes,
        )?;
        validate_nonzero(
            "max_session_history_scan_bytes_per_page",
            self.max_session_history_scan_bytes_per_page,
        )?;
        validate_nonzero(
            "max_session_history_line_bytes",
            self.max_session_history_line_bytes,
        )?;
        validate_nonzero(
            "max_session_history_item_text_bytes",
            self.max_session_history_item_text_bytes,
        )?;
        validate_nonzero("max_live_runs", self.max_live_runs)?;
        validate_nonzero(
            "max_retained_terminal_runs",
            self.max_retained_terminal_runs,
        )?;
        validate_nonzero("max_runtime_command_queue", self.max_runtime_command_queue)?;
        validate_nonzero("max_process_event_queue", self.max_process_event_queue)?;
        validate_nonzero(
            "max_git_command_output_bytes",
            self.max_git_command_output_bytes,
        )?;
        validate_nonzero("max_git_ref_bytes", self.max_git_ref_bytes)?;
        validate_nonzero("max_worktree_path_bytes", self.max_worktree_path_bytes)?;
        validate_nonzero(
            "max_worktree_registry_entries",
            self.max_worktree_registry_entries,
        )?;
        validate_nonzero(
            "max_worktree_registry_bytes",
            self.max_worktree_registry_bytes,
        )?;
        validate_nonzero(
            "max_worktree_recovery_page_entries",
            self.max_worktree_recovery_page_entries,
        )?;
        validate_nonzero("max_git_review_files", self.max_git_review_files)?;
        validate_nonzero("max_git_diff_bytes", self.max_git_diff_bytes)?;
        validate_nonzero("max_git_diff_page_bytes", self.max_git_diff_page_bytes)?;
        validate_nonzero(
            "max_git_diff_scan_bytes_per_page",
            self.max_git_diff_scan_bytes_per_page,
        )?;
        validate_nonzero(
            "max_git_diff_hunks_per_page",
            self.max_git_diff_hunks_per_page,
        )?;
        validate_nonzero_u64(
            "environment_probe_deadline_ms",
            self.environment_probe_deadline_ms,
        )?;
        validate_nonzero_u64("version_probe_deadline_ms", self.version_probe_deadline_ms)?;
        validate_nonzero_u64("startup_rpc_deadline_ms", self.startup_rpc_deadline_ms)?;
        validate_nonzero_u64("draft_save_debounce_ms", self.draft_save_debounce_ms)?;
        validate_nonzero_u64("draft_flush_deadline_ms", self.draft_flush_deadline_ms)?;
        validate_nonzero_u64("stop_abort_deadline_ms", self.stop_abort_deadline_ms)?;
        validate_nonzero_u64(
            "stop_termination_deadline_ms",
            self.stop_termination_deadline_ms,
        )?;
        validate_nonzero_u64("git_command_deadline_ms", self.git_command_deadline_ms)?;

        for (field, value, hard_maximum) in [
            (
                "max_git_command_output_bytes",
                self.max_git_command_output_bytes,
                HARD_MAX_GIT_COMMAND_OUTPUT_BYTES,
            ),
            (
                "max_git_ref_bytes",
                self.max_git_ref_bytes,
                HARD_MAX_GIT_REF_BYTES,
            ),
            (
                "max_worktree_path_bytes",
                self.max_worktree_path_bytes,
                HARD_MAX_WORKTREE_PATH_BYTES,
            ),
            (
                "max_worktree_registry_entries",
                self.max_worktree_registry_entries,
                HARD_MAX_WORKTREE_REGISTRY_ENTRIES,
            ),
            (
                "max_worktree_recovery_page_entries",
                self.max_worktree_recovery_page_entries,
                HARD_MAX_WORKTREE_RECOVERY_PAGE_ENTRIES,
            ),
            (
                "max_git_review_files",
                self.max_git_review_files,
                HARD_MAX_GIT_REVIEW_FILES,
            ),
            (
                "max_git_diff_bytes",
                self.max_git_diff_bytes,
                HARD_MAX_GIT_DIFF_BYTES,
            ),
            (
                "max_preferences_bytes",
                self.max_preferences_bytes,
                HARD_MAX_PREFERENCES_BYTES,
            ),
            (
                "max_git_diff_page_bytes",
                self.max_git_diff_page_bytes,
                HARD_MAX_GIT_DIFF_PAGE_BYTES,
            ),
            (
                "max_git_diff_scan_bytes_per_page",
                self.max_git_diff_scan_bytes_per_page,
                HARD_MAX_GIT_DIFF_SCAN_BYTES,
            ),
            (
                "max_git_diff_hunks_per_page",
                self.max_git_diff_hunks_per_page,
                HARD_MAX_GIT_DIFF_HUNKS_PER_PAGE,
            ),
            (
                "max_retained_terminal_runs",
                self.max_retained_terminal_runs,
                HARD_MAX_RETAINED_TERMINAL_RUNS,
            ),
            (
                "max_cached_draft_records",
                self.max_cached_draft_records,
                HARD_MAX_CACHED_DRAFT_RECORDS,
            ),
        ] {
            if value > hard_maximum {
                return Err(LimitsError::AboveHardMaximum {
                    field,
                    value,
                    hard_maximum,
                });
            }
        }
        if self.max_live_runs > HARD_MAX_LIVE_RUNS {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_live_runs",
                value: self.max_live_runs,
                hard_maximum: HARD_MAX_LIVE_RUNS,
            });
        }
        if self.max_live_runs > self.max_cached_draft_records {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_live_runs",
                larger: "max_cached_draft_records",
            });
        }
        if self.max_git_diff_page_bytes > self.max_git_diff_scan_bytes_per_page {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_git_diff_page_bytes",
                larger: "max_git_diff_scan_bytes_per_page",
            });
        }
        if self.max_worktree_recovery_page_entries > self.max_worktree_registry_entries {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_worktree_recovery_page_entries",
                larger: "max_worktree_registry_entries",
            });
        }

        if self.max_rpc_frame_bytes > HARD_MAX_RPC_FRAME_BYTES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_rpc_frame_bytes",
                value: self.max_rpc_frame_bytes,
                hard_maximum: HARD_MAX_RPC_FRAME_BYTES,
            });
        }
        if self.max_session_history_page_items > HARD_MAX_SESSION_HISTORY_PAGE_ITEMS {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_history_page_items",
                value: self.max_session_history_page_items,
                hard_maximum: HARD_MAX_SESSION_HISTORY_PAGE_ITEMS,
            });
        }
        if self.max_session_history_line_bytes > HARD_MAX_SESSION_HISTORY_LINE_BYTES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_history_line_bytes",
                value: self.max_session_history_line_bytes,
                hard_maximum: HARD_MAX_SESSION_HISTORY_LINE_BYTES,
            });
        }
        if self.max_session_history_scan_bytes_per_page > HARD_MAX_SESSION_HISTORY_SCAN_BYTES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_history_scan_bytes_per_page",
                value: self.max_session_history_scan_bytes_per_page,
                hard_maximum: HARD_MAX_SESSION_HISTORY_SCAN_BYTES,
            });
        }
        if self.max_session_history_line_bytes > self.max_session_history_scan_bytes_per_page {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_history_line_bytes",
                larger: "max_session_history_scan_bytes_per_page",
            });
        }
        if self.max_session_history_item_text_bytes > self.max_session_history_page_bytes {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_history_item_text_bytes",
                larger: "max_session_history_page_bytes",
            });
        }
        if self.max_session_header_scan_bytes > HARD_MAX_SESSION_HEADER_SCAN_BYTES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_header_scan_bytes",
                value: self.max_session_header_scan_bytes,
                hard_maximum: HARD_MAX_SESSION_HEADER_SCAN_BYTES,
            });
        }
        if self.max_session_history_page_bytes > self.max_ui_backlog_bytes_per_run {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_history_page_bytes",
                larger: "max_ui_backlog_bytes_per_run",
            });
        }
        if self.max_session_catalog_candidates > HARD_MAX_SESSION_CATALOG_CANDIDATES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_catalog_candidates",
                value: self.max_session_catalog_candidates,
                hard_maximum: HARD_MAX_SESSION_CATALOG_CANDIDATES,
            });
        }
        if self.max_session_header_scan_bytes > self.max_session_metadata_scan_bytes {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_header_scan_bytes",
                larger: "max_session_metadata_scan_bytes",
            });
        }
        if self.max_session_catalog_scan_files > HARD_MAX_SESSION_CATALOG_SCAN_FILES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_catalog_scan_files",
                value: self.max_session_catalog_scan_files,
                hard_maximum: HARD_MAX_SESSION_CATALOG_SCAN_FILES,
            });
        }
        if self.max_session_catalog_page_entries > HARD_MAX_SESSION_CATALOG_PAGE_ENTRIES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_catalog_page_entries",
                value: self.max_session_catalog_page_entries,
                hard_maximum: HARD_MAX_SESSION_CATALOG_PAGE_ENTRIES,
            });
        }
        if self.max_session_catalog_scan_files > self.max_session_catalog_candidates {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_catalog_scan_files",
                larger: "max_session_catalog_candidates",
            });
        }
        if self.max_session_catalog_page_entries > self.max_session_catalog_scan_files {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_catalog_page_entries",
                larger: "max_session_catalog_scan_files",
            });
        }
        if self.max_project_registry_entries > HARD_MAX_PROJECT_REGISTRY_ENTRIES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_project_registry_entries",
                value: self.max_project_registry_entries,
                hard_maximum: HARD_MAX_PROJECT_REGISTRY_ENTRIES,
            });
        }
        if self.max_session_catalog_page_bytes > self.max_ui_backlog_bytes_per_run {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_catalog_page_bytes",
                larger: "max_ui_backlog_bytes_per_run",
            });
        }
        if self.max_recovered_queue_messages_per_run > HARD_MAX_RECOVERED_QUEUE_MESSAGES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_recovered_queue_messages_per_run",
                value: self.max_recovered_queue_messages_per_run,
                hard_maximum: HARD_MAX_RECOVERED_QUEUE_MESSAGES,
            });
        }
        if self.max_session_entry_page_entries > HARD_MAX_SESSION_PAGE_ENTRIES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_session_entry_page_entries",
                value: self.max_session_entry_page_entries,
                hard_maximum: HARD_MAX_SESSION_PAGE_ENTRIES,
            });
        }
        if self.max_recovered_queue_bytes_per_run > self.max_draft_bytes_per_session {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_recovered_queue_bytes_per_run",
                larger: "max_draft_bytes_per_session",
            });
        }
        for (field, value) in [
            ("max_runtime_command_queue", self.max_runtime_command_queue),
            ("max_process_event_queue", self.max_process_event_queue),
        ] {
            if value > HARD_MAX_RUNTIME_CHANNEL_ENTRIES {
                return Err(LimitsError::AboveHardMaximum {
                    field,
                    value,
                    hard_maximum: HARD_MAX_RUNTIME_CHANNEL_ENTRIES,
                });
            }
        }
        if self.max_capability_entries_per_run > HARD_MAX_CAPABILITY_ENTRIES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_capability_entries_per_run",
                value: self.max_capability_entries_per_run,
                hard_maximum: HARD_MAX_CAPABILITY_ENTRIES,
            });
        }
        if self.max_stream_content_blocks_per_message > HARD_MAX_STREAM_CONTENT_BLOCKS {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_stream_content_blocks_per_message",
                value: self.max_stream_content_blocks_per_message,
                hard_maximum: HARD_MAX_STREAM_CONTENT_BLOCKS,
            });
        }
        if self.max_attachments_per_prompt > HARD_MAX_ATTACHMENTS_PER_PROMPT {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_attachments_per_prompt",
                value: self.max_attachments_per_prompt,
                hard_maximum: HARD_MAX_ATTACHMENTS_PER_PROMPT,
            });
        }
        if self.max_attachment_name_bytes > HARD_MAX_ATTACHMENT_NAME_BYTES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_attachment_name_bytes",
                value: self.max_attachment_name_bytes,
                hard_maximum: HARD_MAX_ATTACHMENT_NAME_BYTES,
            });
        }
        if self.max_attachment_bytes_per_image > self.max_attachment_bytes_per_prompt {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_attachment_bytes_per_image",
                larger: "max_attachment_bytes_per_prompt",
            });
        }
        if self.max_session_entry_page_bytes > self.max_ui_backlog_bytes_per_run {
            return Err(LimitsError::InvalidRelationship {
                smaller: "max_session_entry_page_bytes",
                larger: "max_ui_backlog_bytes_per_run",
            });
        }
        if self.max_outbound_rpc_bytes > HARD_MAX_RPC_FRAME_BYTES {
            return Err(LimitsError::AboveHardMaximum {
                field: "max_outbound_rpc_bytes",
                value: self.max_outbound_rpc_bytes,
                hard_maximum: HARD_MAX_RPC_FRAME_BYTES,
            });
        }

        for (field, value) in [
            (
                "max_stream_text_bytes_per_run",
                self.max_stream_text_bytes_per_run,
            ),
            ("max_tool_preview_bytes", self.max_tool_preview_bytes),
            (
                "max_ui_backlog_bytes_per_run",
                self.max_ui_backlog_bytes_per_run,
            ),
            (
                "max_extension_ui_bytes_per_run",
                self.max_extension_ui_bytes_per_run,
            ),
            ("max_failure_detail_bytes", self.max_failure_detail_bytes),
            (
                "max_attachment_bytes_per_image",
                self.max_attachment_bytes_per_image,
            ),
            (
                "max_attachment_bytes_per_prompt",
                self.max_attachment_bytes_per_prompt,
            ),
            (
                "max_draft_bytes_per_session",
                self.max_draft_bytes_per_session,
            ),
            (
                "max_recovered_queue_bytes_per_run",
                self.max_recovered_queue_bytes_per_run,
            ),
            (
                "max_project_registry_bytes",
                self.max_project_registry_bytes,
            ),
            (
                "max_worktree_registry_bytes",
                self.max_worktree_registry_bytes,
            ),
            (
                "max_environment_probe_bytes",
                self.max_environment_probe_bytes,
            ),
            ("max_version_probe_bytes", self.max_version_probe_bytes),
            (
                "max_capability_bytes_per_run",
                self.max_capability_bytes_per_run,
            ),
            (
                "max_runtime_state_bytes_per_run",
                self.max_runtime_state_bytes_per_run,
            ),
            (
                "max_session_entry_page_bytes",
                self.max_session_entry_page_bytes,
            ),
            ("max_session_cursor_bytes", self.max_session_cursor_bytes),
            (
                "max_session_catalog_page_bytes",
                self.max_session_catalog_page_bytes,
            ),
            (
                "max_session_header_scan_bytes",
                self.max_session_header_scan_bytes,
            ),
            (
                "max_session_metadata_scan_bytes",
                self.max_session_metadata_scan_bytes,
            ),
            (
                "max_session_catalog_query_bytes",
                self.max_session_catalog_query_bytes,
            ),
            (
                "max_session_history_page_bytes",
                self.max_session_history_page_bytes,
            ),
            (
                "max_session_history_scan_bytes_per_page",
                self.max_session_history_scan_bytes_per_page,
            ),
            (
                "max_session_history_line_bytes",
                self.max_session_history_line_bytes,
            ),
            (
                "max_session_history_item_text_bytes",
                self.max_session_history_item_text_bytes,
            ),
        ] {
            if value > HARD_MAX_RENDER_BUFFER_BYTES {
                return Err(LimitsError::AboveHardMaximum {
                    field,
                    value,
                    hard_maximum: HARD_MAX_RENDER_BUFFER_BYTES,
                });
            }
        }

        Ok(self)
    }
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            // Images are base64 encoded on the wire. These ceilings leave room for a
            // bounded multi-image prompt without making the decoder pre-allocate them.
            max_rpc_frame_bytes: 32 * 1024 * 1024,
            max_outbound_rpc_bytes: 20 * 1024 * 1024,
            max_stderr_bytes_per_run: 128 * 1024,
            max_stream_text_bytes_per_run: 2 * 1024 * 1024,
            max_stream_content_blocks_per_message: 256,
            max_tool_preview_bytes: 256 * 1024,
            max_ui_backlog_bytes_per_run: 2 * 1024 * 1024,
            max_active_tools_per_run: 128,
            max_pending_rpc_requests_per_run: 128,
            max_pending_ui_requests_per_run: 64,
            max_extension_ui_entries_per_run: 64,
            max_extension_ui_bytes_per_run: 256 * 1024,
            max_failure_detail_bytes: 64 * 1024,
            max_attachments_per_prompt: 8,
            max_attachment_name_bytes: 512,
            max_attachment_bytes_per_image: 8 * 1024 * 1024,
            max_attachment_bytes_per_prompt: 12 * 1024 * 1024,
            max_draft_bytes_per_session: 1024 * 1024,
            max_cached_draft_records: 256,
            max_recovered_queue_messages_per_run: 256,
            max_recovered_queue_bytes_per_run: 512 * 1024,
            max_project_registry_entries: 4096,
            max_project_registry_bytes: 2 * 1024 * 1024,
            max_preferences_bytes: 64 * 1024,
            max_worktree_registry_entries: 2048,
            max_worktree_registry_bytes: 2 * 1024 * 1024,
            max_worktree_recovery_page_entries: 64,
            max_environment_probe_bytes: 2 * 1024 * 1024,
            max_version_probe_bytes: 64 * 1024,
            max_capability_entries_per_run: 4096,
            max_capability_bytes_per_run: 2 * 1024 * 1024,
            max_runtime_state_bytes_per_run: 256 * 1024,
            max_session_entry_page_entries: 512,
            max_session_entry_page_bytes: 512 * 1024,
            max_session_cursor_bytes: 4 * 1024,
            max_session_catalog_candidates: 2048,
            max_session_catalog_scan_files: 256,
            max_session_catalog_page_entries: 64,
            max_session_catalog_page_bytes: 256 * 1024,
            max_session_header_scan_bytes: 16 * 1024,
            max_session_metadata_scan_bytes: 128 * 1024,
            max_session_catalog_query_bytes: 1024,
            max_session_history_page_items: 48,
            max_session_history_page_bytes: 512 * 1024,
            max_session_history_scan_bytes_per_page: 4 * 1024 * 1024,
            max_session_history_line_bytes: 2 * 1024 * 1024,
            max_session_history_item_text_bytes: 64 * 1024,
            max_live_runs: 8,
            max_retained_terminal_runs: 32,
            max_runtime_command_queue: 256,
            max_process_event_queue: 1024,
            max_git_command_output_bytes: 256 * 1024,
            max_git_ref_bytes: 1024,
            max_worktree_path_bytes: 16 * 1024,
            max_git_review_files: 2048,
            max_git_diff_bytes: 1024 * 1024,
            max_git_diff_page_bytes: 128 * 1024,
            max_git_diff_scan_bytes_per_page: 8 * 1024 * 1024,
            max_git_diff_hunks_per_page: 512,
            environment_probe_deadline_ms: 2_000,
            version_probe_deadline_ms: 2_000,
            startup_rpc_deadline_ms: 5_000,
            draft_save_debounce_ms: 400,
            draft_flush_deadline_ms: 1_500,
            stop_abort_deadline_ms: 3_000,
            stop_termination_deadline_ms: 5_000,
            git_command_deadline_ms: 10_000,
        }
    }
}

fn validate_nonzero(field: &'static str, value: usize) -> Result<(), LimitsError> {
    if value == 0 {
        return Err(LimitsError::Zero { field });
    }
    Ok(())
}

fn validate_nonzero_u64(field: &'static str, value: u64) -> Result<(), LimitsError> {
    if value == 0 {
        return Err(LimitsError::Zero { field });
    }
    Ok(())
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum LimitsError {
    #[error("runtime limit {field} must be greater than zero")]
    Zero { field: &'static str },
    #[error("runtime limit {field}={value} exceeds hard maximum {hard_maximum}")]
    AboveHardMaximum {
        field: &'static str,
        value: usize,
        hard_maximum: usize,
    },
    #[error("runtime limit {smaller} must not exceed {larger}")]
    InvalidRelationship {
        smaller: &'static str,
        larger: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(
            RuntimeLimits::default().validate(),
            Ok(RuntimeLimits::default())
        );

        let draft_cache = RuntimeLimits {
            max_cached_draft_records: HARD_MAX_CACHED_DRAFT_RECORDS + 1,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            draft_cache.validate(),
            Err(LimitsError::AboveHardMaximum {
                field: "max_cached_draft_records",
                value: HARD_MAX_CACHED_DRAFT_RECORDS + 1,
                hard_maximum: HARD_MAX_CACHED_DRAFT_RECORDS,
            })
        );

        let too_few_draft_slots = RuntimeLimits {
            max_live_runs: 3,
            max_cached_draft_records: 2,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            too_few_draft_slots.validate(),
            Err(LimitsError::InvalidRelationship {
                smaller: "max_live_runs",
                larger: "max_cached_draft_records",
            })
        );
    }

    #[test]
    fn live_run_limit_has_nonzero_and_hard_maximum_bounds() {
        let zero = RuntimeLimits {
            max_live_runs: 0,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            zero.validate(),
            Err(LimitsError::Zero {
                field: "max_live_runs"
            })
        );

        let oversized = RuntimeLimits {
            max_live_runs: HARD_MAX_LIVE_RUNS + 1,
            max_cached_draft_records: HARD_MAX_LIVE_RUNS + 1,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            oversized.validate(),
            Err(LimitsError::AboveHardMaximum {
                field: "max_live_runs",
                value: HARD_MAX_LIVE_RUNS + 1,
                hard_maximum: HARD_MAX_LIVE_RUNS,
            })
        );

        let terminal_history = RuntimeLimits {
            max_retained_terminal_runs: HARD_MAX_RETAINED_TERMINAL_RUNS + 1,
            ..RuntimeLimits::default()
        };
        assert_eq!(
            terminal_history.validate(),
            Err(LimitsError::AboveHardMaximum {
                field: "max_retained_terminal_runs",
                value: HARD_MAX_RETAINED_TERMINAL_RUNS + 1,
                hard_maximum: HARD_MAX_RETAINED_TERMINAL_RUNS,
            })
        );
    }

    #[test]
    fn rejects_zero_limits() {
        let limits = RuntimeLimits {
            max_tool_preview_bytes: 0,
            ..RuntimeLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(LimitsError::Zero {
                field: "max_tool_preview_bytes"
            })
        );
    }

    #[test]
    fn rejects_per_image_limit_larger_than_prompt_limit() {
        let limits = RuntimeLimits {
            max_attachment_bytes_per_image: 9,
            max_attachment_bytes_per_prompt: 8,
            ..RuntimeLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(LimitsError::InvalidRelationship {
                smaller: "max_attachment_bytes_per_image",
                larger: "max_attachment_bytes_per_prompt",
            })
        );
    }

    #[test]
    fn rejects_history_pages_that_cannot_fit_renderer_backlog() {
        let limits = RuntimeLimits {
            max_ui_backlog_bytes_per_run: 1024,
            max_session_entry_page_bytes: 2048,
            max_session_catalog_page_bytes: 1024,
            max_session_history_page_bytes: 1024,
            max_session_history_item_text_bytes: 512,
            ..RuntimeLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(LimitsError::InvalidRelationship {
                smaller: "max_session_entry_page_bytes",
                larger: "max_ui_backlog_bytes_per_run",
            })
        );
    }

    #[test]
    fn rejects_catalog_pages_that_cannot_fit_renderer_backlog() {
        let limits = RuntimeLimits {
            max_ui_backlog_bytes_per_run: 1024,
            max_session_entry_page_bytes: 1024,
            max_session_catalog_page_bytes: 2048,
            max_session_history_page_bytes: 1024,
            max_session_history_item_text_bytes: 512,
            ..RuntimeLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(LimitsError::InvalidRelationship {
                smaller: "max_session_catalog_page_bytes",
                larger: "max_ui_backlog_bytes_per_run",
            })
        );
    }

    #[test]
    fn rejects_cold_history_pages_that_cannot_fit_renderer_backlog() {
        let limits = RuntimeLimits {
            max_ui_backlog_bytes_per_run: 1024,
            max_session_entry_page_bytes: 1024,
            max_session_catalog_page_bytes: 1024,
            max_session_history_page_bytes: 2048,
            max_session_history_item_text_bytes: 512,
            ..RuntimeLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(LimitsError::InvalidRelationship {
                smaller: "max_session_history_page_bytes",
                larger: "max_ui_backlog_bytes_per_run",
            })
        );
    }
}
