import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";

import { AutomationView } from "../automation/AutomationView";
import { ExtensionDialogCard } from "../attention/ExtensionDialogCard";
import { NeedsAttentionView } from "../attention/NeedsAttentionView";
import type {
  AutomationChangedSignal,
  AutomationExecutionSnapshot,
  DesktopAutomationSnapshot,
} from "../automation/types";
import { ModelPicker } from "../models/ModelPicker";
import type { ModelSelection } from "../models/types";
import { ProjectLauncher } from "../projects/ProjectLauncher";
import { SessionCatalogBrowser } from "../sessions/SessionCatalogBrowser";
import { RecentSessionsView } from "../sessions/RecentSessionsView";
import { SupervisionView } from "../supervision/SupervisionView";
import type { SupervisionSnapshot } from "../supervision/types";
import { invokeDesktop } from "../../lib/desktop";
import { pathLeaf } from "../../lib/path";

export type ExtensionDialogKind =
  | { kind: "select"; title: string; options: string[] }
  | { kind: "confirm"; title: string; message: string }
  | { kind: "input"; title: string; placeholder: string | null }
  | { kind: "editor"; title: string; prefill: string | null };

export type ComposerAction = "send" | "steer" | "followUp" | "runCommand";
export type ComposerAvailability = "ready" | "agent_working" | "blocked_by_compaction" | "unavailable";
export type ProjectTrustPolicy = "inherit" | "approve" | "ignore";
export type ContextFilesPolicy = "inherit" | "disabled";
export type ExtensionDiscoveryPolicy = "inherit" | "disabled";
export type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
export type ExecutionIsolation = "local_checkout" | "git_worktree";
export type AppView =
  | "dashboard"
  | "automation"
  | "supervision"
  | "attention"
  | "sessions"
  | "launcher"
  | "run";

export interface StartRunResult {
  runId: string;
  initialTaskSubmitted: boolean;
  initialTaskError: string | null;
}

export function dialogTimeoutLabel(dialog: PendingExtensionDialog): string {
  const remaining = dialog.remainingTimeoutMs;
  if (remaining === null) return "No Pi-side timeout";
  if (remaining < 1_000) return "Timed request · <1s remaining at last sync";
  if (remaining < 60_000) return `Timed request · ~${Math.ceil(remaining / 1_000)}s remaining at last sync`;
  return `Timed request · ~${Math.ceil(remaining / 60_000)}m remaining at last sync`;
}

export interface DesktopProjectRecord {
  id: string;
  canonicalRoot: string;
  status: "present" | "missing" | "changed" | "unverifiable";
  detail: string | null;
}

export interface ModelSummary {
  provider: string;
  id: string;
  name: string | null;
  supportsImages: boolean | null;
}

export interface CommandSummary {
  name: string;
  description: string | null;
  source: string;
  location: string | null;
  path: string | null;
}

export interface RunCapabilities {
  revision: number;
  models: ModelSummary[] | null;
  thinkingLevels: ThinkingLevel[] | null;
  commands: CommandSummary[] | null;
}

export interface DraftSnapshot {
  text: string;
  images: DraftImageSnapshot[];
  generation: number;
  durability: "saved" | "dirty" | "saving" | "failed";
  persistenceError: string | null;
}

export interface DraftImageSnapshot {
  id: string;
  fileName: string;
  mimeType: string;
  decodedBytes: number;
}

export interface RuntimeAttachmentLimits {
  maxAttachments: number;
  maxImageBytes: number;
  maxAggregateBytes: number;
  maxNameBytes: number;
}

export interface RuntimeCapacitySnapshot {
  activeRuns: number;
  liveRunLimit: number;
  configuredMaxLiveRuns: number;
  preferenceRecoveryNotice: string | null;
}

export interface RunRuntimeDiagnostics {
  runId: string;
  processOwned: boolean;
  retainedRuntimeStateBytes: number;
  pendingRpcRequests: number;
  activeRpcCommands: number;
  pendingExtensionDialogs: number;
  assistantBlocks: number;
  activeTools: number;
  activeDirectBash: number;
  uiBacklogBytes: number;
  uiBacklogFrames: number;
  uiCoalescedFrames: number;
  uiDroppedDisplayFrames: number;
  uiDeliveredEvents: number;
  uiRehydrateRequired: boolean;
  rpcEventsPerSecond: number;
  rpcEventBytesPerSecond: number;
}

export interface RuntimeDiagnosticsSnapshot {
  runtimeRevision: number;
  ownedProcesses: number;
  runs: RunRuntimeDiagnostics[];
}

export interface DesktopRuntimeDiagnostics {
  runtime: RuntimeDiagnosticsSnapshot;
  activeGitReviewJobs: number;
  activeSessionCatalogJobs: number;
}

export interface AssistantContentSnapshot {
  contentIndex: number;
  kind: "text" | "thinking" | "tool_call";
  text: string;
  droppedBytes: number;
  complete: boolean;
}

export interface ToolPreviewSnapshot {
  toolCallId: string;
  toolName: string;
  output: string;
  droppedBytes: number;
}

export interface DirectBashSnapshot {
  requestId: string;
  output: string;
  droppedBytes: number;
}

export interface LiveProjectionSnapshot {
  reasoning: string;
  reasoningDroppedBytes: number;
  assistantBlocks: AssistantContentSnapshot[];
  activeTools: ToolPreviewSnapshot[];
  directBash: DirectBashSnapshot[];
}

export interface ComposerSubmitResult {
  action: ComposerAction;
  accepted: boolean;
  draftCleared: boolean;
  error: string | null;
}

export interface RuntimeStopResult {
  recoveredSteering: string[];
  recoveredFollowUp: string[];
  draftRestored: boolean;
  draftRestoreError: string | null;
  processTerminated: boolean;
  quarantined: boolean;
}

export interface RuntimeCloseResult {
  processTerminated: boolean;
  quarantined: boolean;
}

export interface SessionContextUsage {
  tokens: number | null;
  contextWindow: number;
  percent: number | null;
}

export interface SessionStats {
  sessionFile: string;
  sessionId: string;
  userMessages: number;
  assistantMessages: number;
  toolCalls: number;
  toolResults: number;
  totalMessages: number;
  tokens: {
    input: number;
    output: number;
    cacheRead: number;
    cacheWrite: number;
    total: number;
  };
  cost: number;
  contextUsage: SessionContextUsage | null;
}

export interface CompactionResult {
  firstKeptEntryId: string;
  tokensBefore: number;
  estimatedTokensAfter: number;
}

export interface PendingExtensionDialog {
  request: {
    id: string;
    timeoutMs: number | null;
    kind: ExtensionDialogKind;
  };
  remainingTimeoutMs: number | null;
}

export interface ExtensionUiSnapshot {
  statuses: Array<{ key: string; text: string }>;
  widgets: Array<{
    key: string;
    widget: {
      lines: string[];
      placement: "aboveEditor" | "belowEditor";
    };
  }>;
  title: string | null;
}

export interface RunRetrySnapshot {
  attempt: number;
  maxAttempts: number;
  delayMs: number;
  errorMessage: string;
  errorTruncated: boolean;
  waiting: boolean;
  finished: boolean;
  success: boolean | null;
  finalError: string | null;
  finalErrorTruncated: boolean;
}

export interface RunSummarizationRetrySnapshot {
  attempt: number;
  maxAttempts: number;
  delayMs: number;
  errorMessage: string;
  errorTruncated: boolean;
  source: string | null;
  reason: string | null;
  finished: boolean;
}

export interface RunCompactionSnapshot {
  reason: string;
  reasonTruncated: boolean;
  finished: boolean;
  aborted: boolean;
  willRetry: boolean;
  errorMessage: string | null;
  errorTruncated: boolean;
}

export interface RunExtensionErrorSnapshot {
  extensionPath: string;
  event: string;
  error: string;
  detailTruncated: boolean;
}

export interface GitWorktreeIdentity {
  repositoryRoot: string;
  worktreeRoot: string;
  branch: string;
  baseCommit: string;
}

export interface RunFailureSnapshot {
  kind: "spawn" | "protocol" | "unexpected_exit" | "stop" | "internal";
  detail: string;
  detailTruncated: boolean;
}

export interface RunHydration {
  run: {
    id: string;
    projectId: string;
    executionRoot: string;
    executionIsolation: ExecutionIsolation;
    worktree: GitWorktreeIdentity | null;
    projectTrust: ProjectTrustPolicy;
    startedUnixMs: number;
    terminalUnixMs: number | null;
    changeRevision: number;
    agentWorking: boolean;
    compacting: boolean;
    queue: {
      steering: number;
      followUp: number;
    };
    process: "starting" | "ready" | "stopping" | "exited" | "failed" | "quarantined";
    exitCode: number | null;
    failure: RunFailureSnapshot | null;
    revision: number;
    session: {
      sessionFile: string | null;
      sessionId: string | null;
      sessionName: string | null;
      model: ModelSummary | null;
      thinkingLevel: ThinkingLevel | null;
      autoCompactionEnabled: boolean | null;
      messageCount: number | null;
      pendingMessageCount: number | null;
    };
  };
  draft: DraftSnapshot | null;
  composerAvailability: ComposerAvailability;
  composerSubmissionPending: boolean;
  draftRestorePending: boolean;
  rpc: {
    capabilities: RunCapabilities;
    pendingDialogs: PendingExtensionDialog[];
    live: LiveProjectionSnapshot;
    extensionUi: ExtensionUiSnapshot;
    compaction: RunCompactionSnapshot | null;
    retry: RunRetrySnapshot | null;
    summarizationRetry: RunSummarizationRetrySnapshot | null;
    lastExtensionError: RunExtensionErrorSnapshot | null;
    streamStalled: boolean;
  } | null;
}

export interface WorktreeBaseSnapshot {
  repositoryRoot: string;
  projectRoot: string;
  projectRelativePath: string;
  sourceBranch: string | null;
  baseCommit: string;
  dirty: boolean;
}

export interface CreatedWorktree {
  repositoryRoot: string;
  worktreeRoot: string;
  executionRoot: string;
  branch: string;
  baseCommit: string;
}

export interface WorktreeRecoveryRecord {
  id: string;
  projectId: string;
  base: WorktreeBaseSnapshot;
  branch: string;
  requestedPath: string;
  created: CreatedWorktree | null;
}

export interface WorktreeRecoveryPage {
  records: WorktreeRecoveryRecord[];
  truncated: boolean;
  recoveryNotice: string | null;
}

export type WorktreeRecoveryProbe =
  | { kind: "notCreated" }
  | { kind: "exact"; created: CreatedWorktree }
  | { kind: "partial"; branchExists: boolean; pathExists: boolean; detail: string };

export type WorktreeCleanupResult =
  | { kind: "removed" }
  | { kind: "partial"; branchExists: boolean; pathExists: boolean; detail: string };

export interface WorktreeRecoveryInspection {
  record: WorktreeRecoveryRecord | null;
  probe: WorktreeRecoveryProbe;
}

export type ChangedFileStatus =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "copied"
  | "type_changed"
  | "unmerged"
  | "untracked"
  | "unknown";

export interface ChangedFileSummary {
  path: string;
  previousPath: string | null;
  status: ChangedFileStatus;
}

export interface GitReviewSummary {
  repositoryRoot: string;
  files: ChangedFileSummary[];
  truncated: boolean;
}

export interface GitDiffCursor {
  path: string;
  offset: number;
  prefixSha256: string;
}

export interface GitDiffHunk {
  lineIndex: number;
  header: string;
}

export interface GitFileDiffPage {
  path: string;
  diff: string;
  nextCursor: GitDiffCursor | null;
  untracked: boolean;
  binary: boolean;
  scannedBytes: number;
  hunks: GitDiffHunk[];
}

export function diffPageSegments(page: GitFileDiffPage): Array<{ hunk: GitDiffHunk | null; text: string }> {
  const lines = page.diff.match(/[^\n]*\n|[^\n]+$/g) ?? [];
  if (page.hunks.length === 0) return [{ hunk: null, text: page.diff }];
  const segments: Array<{ hunk: GitDiffHunk | null; text: string }> = [];
  const first = page.hunks[0]?.lineIndex ?? 0;
  if (first > 0) segments.push({ hunk: null, text: lines.slice(0, first).join("") });
  page.hunks.forEach((hunk, index) => {
    const end = page.hunks[index + 1]?.lineIndex ?? lines.length;
    segments.push({ hunk, text: lines.slice(hunk.lineIndex, end).join("") });
  });
  return segments;
}

export interface SessionCatalogEntry {
  path: string;
  id: string;
  name: string | null;
  firstMessage: string | null;
  modifiedUnixMs: number;
  previewIncomplete: boolean;
}

export interface SessionCatalogPage {
  sessions: SessionCatalogEntry[];
  candidateFiles: number;
  scannedFiles: number;
  truncated: boolean;
  nextCursor: SessionCatalogCursor | null;
  directorySource: "environment" | "settings" | "default";
}

export interface SessionCatalogCursor {
  modifiedUnixMs: number;
  path: string;
  scopeSha256: string;
  snapshotSha256: string;
}

export interface SessionHistoryCursor {
  sessionId: string;
  beforeOffset: number;
  nextEntryId: string | null;
  seekLatest: boolean;
}

export type SessionTimelineKind =
  | "user"
  | "assistant"
  | "tool"
  | "bash"
  | "compaction"
  | "branch_summary"
  | "custom";

export interface SessionTimelineItem {
  entryId: string;
  timestamp: string | null;
  kind: SessionTimelineKind;
  title: string | null;
  text: string;
  textTruncated: boolean;
  reasoning: string | null;
  reasoningTruncated: boolean;
  isError: boolean;
}

export interface SessionHistoryPage {
  sessionId: string;
  items: SessionTimelineItem[];
  nextCursor: SessionHistoryCursor | null;
  scannedBytes: number;
  encodedBytes: number;
}

export interface SessionTreeNode {
  id: string;
  parentId: string | null;
  entryType: string;
  role: string | null;
  timestamp: string | null;
  label: string | null;
  labelTimestamp: string | null;
  depth: number;
  childCount: number;
  preview: string | null;
  previewTruncated: boolean;
}

export interface SessionTreeSnapshot {
  nodes: SessionTreeNode[];
  leafId: string | null;
  truncated: boolean;
  encodedBytes: number;
}

export interface RuntimeHydration {
  schemaVersion: number;
  runtimeRevision: number;
  runs: RunHydration[];
}

export const RUNTIME_HYDRATION_SCHEMA_VERSION = 10;

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kib = bytes / 1024;
  if (kib < 1024) return `${kib.toFixed(kib >= 10 ? 0 : 1)} KiB`;
  const mib = kib / 1024;
  return `${mib.toFixed(mib >= 10 ? 0 : 1)} MiB`;
}
