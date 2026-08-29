import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";

import { AutomationView } from "../features/automation/AutomationView";
import { ExtensionDialogCard } from "../features/attention/ExtensionDialogCard";
import { NeedsAttentionView } from "../features/attention/NeedsAttentionView";
import type {
  AutomationChangedSignal,
  AutomationExecutionSnapshot,
  DesktopAutomationSnapshot,
} from "../features/automation/types";
import { ModelPicker } from "../features/models/ModelPicker";
import type { ModelSelection } from "../features/models/types";
import { ProjectManager } from "../features/projects/ProjectManager";
import { ProjectLauncher } from "../features/projects/ProjectLauncher";
import { SessionCatalogBrowser } from "../features/sessions/SessionCatalogBrowser";
import { RecentSessionsView } from "../features/sessions/RecentSessionsView";
import { SupervisionView } from "../features/supervision/SupervisionView";
import type { SupervisionSnapshot } from "../features/supervision/types";
import { invokeDesktop } from "../lib/desktop";
import { pathLeaf } from "../lib/path";

type ExtensionDialogKind =
  | { kind: "select"; title: string; options: string[] }
  | { kind: "confirm"; title: string; message: string }
  | { kind: "input"; title: string; placeholder: string | null }
  | { kind: "editor"; title: string; prefill: string | null };

type ComposerAction = "send" | "steer" | "followUp" | "runCommand";
type ComposerAvailability = "ready" | "agent_working" | "blocked_by_compaction" | "unavailable";
type ProjectTrustPolicy = "inherit" | "approve" | "ignore";
type ContextFilesPolicy = "inherit" | "disabled";
type ExtensionDiscoveryPolicy = "inherit" | "disabled";
type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
type ExecutionIsolation = "local_checkout" | "git_worktree";
type AppView =
  | "dashboard"
  | "automation"
  | "supervision"
  | "attention"
  | "sessions"
  | "launcher"
  | "run";

interface StartRunResult {
  runId: string;
  initialTaskSubmitted: boolean;
  initialTaskError: string | null;
}

function dialogTimeoutLabel(dialog: PendingExtensionDialog): string {
  const remaining = dialog.remainingTimeoutMs;
  if (remaining === null) return "No Pi-side timeout";
  if (remaining < 1_000) return "Timed request · <1s remaining at last sync";
  if (remaining < 60_000) return `Timed request · ~${Math.ceil(remaining / 1_000)}s remaining at last sync`;
  return `Timed request · ~${Math.ceil(remaining / 60_000)}m remaining at last sync`;
}

interface DesktopProjectRecord {
  id: string;
  canonicalRoot: string;
  status: "present" | "missing" | "changed" | "unverifiable";
  detail: string | null;
}

interface ModelSummary {
  provider: string;
  id: string;
  name: string | null;
  supportsImages: boolean | null;
}

interface CommandSummary {
  name: string;
  description: string | null;
  source: string;
  location: string | null;
  path: string | null;
}

interface RunCapabilities {
  revision: number;
  models: ModelSummary[] | null;
  thinkingLevels: ThinkingLevel[] | null;
  commands: CommandSummary[] | null;
}

interface DraftSnapshot {
  text: string;
  images: DraftImageSnapshot[];
  generation: number;
  durability: "saved" | "dirty" | "saving" | "failed";
  persistenceError: string | null;
}

interface DraftImageSnapshot {
  id: string;
  fileName: string;
  mimeType: string;
  decodedBytes: number;
}

interface RuntimeAttachmentLimits {
  maxAttachments: number;
  maxImageBytes: number;
  maxAggregateBytes: number;
  maxNameBytes: number;
}

interface RuntimeCapacitySnapshot {
  activeRuns: number;
  liveRunLimit: number;
  configuredMaxLiveRuns: number;
  preferenceRecoveryNotice: string | null;
}

interface RunRuntimeDiagnostics {
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

interface RuntimeDiagnosticsSnapshot {
  runtimeRevision: number;
  ownedProcesses: number;
  runs: RunRuntimeDiagnostics[];
}

interface DesktopRuntimeDiagnostics {
  runtime: RuntimeDiagnosticsSnapshot;
  activeGitReviewJobs: number;
  activeSessionCatalogJobs: number;
}

interface AssistantContentSnapshot {
  contentIndex: number;
  kind: "text" | "thinking" | "tool_call";
  text: string;
  droppedBytes: number;
  complete: boolean;
}

interface ToolPreviewSnapshot {
  toolCallId: string;
  toolName: string;
  output: string;
  droppedBytes: number;
}

interface DirectBashSnapshot {
  requestId: string;
  output: string;
  droppedBytes: number;
}

interface LiveProjectionSnapshot {
  assistantBlocks: AssistantContentSnapshot[];
  activeTools: ToolPreviewSnapshot[];
  directBash: DirectBashSnapshot[];
}

interface ComposerSubmitResult {
  action: ComposerAction;
  accepted: boolean;
  draftCleared: boolean;
  error: string | null;
}

interface RuntimeStopResult {
  recoveredSteering: string[];
  recoveredFollowUp: string[];
  draftRestored: boolean;
  draftRestoreError: string | null;
  processTerminated: boolean;
  quarantined: boolean;
}

interface RuntimeCloseResult {
  processTerminated: boolean;
  quarantined: boolean;
}

interface SessionContextUsage {
  tokens: number | null;
  contextWindow: number;
  percent: number | null;
}

interface SessionStats {
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

interface CompactionResult {
  firstKeptEntryId: string;
  tokensBefore: number;
  estimatedTokensAfter: number;
}

interface PendingExtensionDialog {
  request: {
    id: string;
    timeoutMs: number | null;
    kind: ExtensionDialogKind;
  };
  remainingTimeoutMs: number | null;
}

interface ExtensionUiSnapshot {
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

interface RunRetrySnapshot {
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

interface RunSummarizationRetrySnapshot {
  attempt: number;
  maxAttempts: number;
  delayMs: number;
  errorMessage: string;
  errorTruncated: boolean;
  source: string | null;
  reason: string | null;
  finished: boolean;
}

interface RunCompactionSnapshot {
  reason: string;
  reasonTruncated: boolean;
  finished: boolean;
  aborted: boolean;
  willRetry: boolean;
  errorMessage: string | null;
  errorTruncated: boolean;
}

interface RunExtensionErrorSnapshot {
  extensionPath: string;
  event: string;
  error: string;
  detailTruncated: boolean;
}

interface GitWorktreeIdentity {
  repositoryRoot: string;
  worktreeRoot: string;
  branch: string;
  baseCommit: string;
}

interface RunFailureSnapshot {
  kind: "spawn" | "protocol" | "unexpected_exit" | "stop" | "internal";
  detail: string;
  detailTruncated: boolean;
}

interface RunHydration {
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

interface WorktreeBaseSnapshot {
  repositoryRoot: string;
  projectRoot: string;
  projectRelativePath: string;
  sourceBranch: string | null;
  baseCommit: string;
  dirty: boolean;
}

interface CreatedWorktree {
  repositoryRoot: string;
  worktreeRoot: string;
  executionRoot: string;
  branch: string;
  baseCommit: string;
}

interface WorktreeRecoveryRecord {
  id: string;
  projectId: string;
  base: WorktreeBaseSnapshot;
  branch: string;
  requestedPath: string;
  created: CreatedWorktree | null;
}

interface WorktreeRecoveryPage {
  records: WorktreeRecoveryRecord[];
  truncated: boolean;
  recoveryNotice: string | null;
}

type WorktreeRecoveryProbe =
  | { kind: "notCreated" }
  | { kind: "exact"; created: CreatedWorktree }
  | { kind: "partial"; branchExists: boolean; pathExists: boolean; detail: string };

type WorktreeCleanupResult =
  | { kind: "removed" }
  | { kind: "partial"; branchExists: boolean; pathExists: boolean; detail: string };

interface WorktreeRecoveryInspection {
  record: WorktreeRecoveryRecord | null;
  probe: WorktreeRecoveryProbe;
}

type ChangedFileStatus =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "copied"
  | "type_changed"
  | "unmerged"
  | "untracked"
  | "unknown";

interface ChangedFileSummary {
  path: string;
  previousPath: string | null;
  status: ChangedFileStatus;
}

interface GitReviewSummary {
  repositoryRoot: string;
  files: ChangedFileSummary[];
  truncated: boolean;
}

interface GitDiffCursor {
  path: string;
  offset: number;
  prefixSha256: string;
}

interface GitDiffHunk {
  lineIndex: number;
  header: string;
}

interface GitFileDiffPage {
  path: string;
  diff: string;
  nextCursor: GitDiffCursor | null;
  untracked: boolean;
  binary: boolean;
  scannedBytes: number;
  hunks: GitDiffHunk[];
}

function diffPageSegments(page: GitFileDiffPage): Array<{ hunk: GitDiffHunk | null; text: string }> {
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

interface SessionCatalogEntry {
  path: string;
  id: string;
  name: string | null;
  firstMessage: string | null;
  modifiedUnixMs: number;
  previewIncomplete: boolean;
}

interface SessionCatalogPage {
  sessions: SessionCatalogEntry[];
  candidateFiles: number;
  scannedFiles: number;
  truncated: boolean;
  nextCursor: SessionCatalogCursor | null;
  directorySource: "environment" | "settings" | "default";
}

interface SessionCatalogCursor {
  modifiedUnixMs: number;
  path: string;
  scopeSha256: string;
  snapshotSha256: string;
}

interface SessionHistoryCursor {
  sessionId: string;
  beforeOffset: number;
  nextEntryId: string | null;
  seekLatest: boolean;
}

type SessionTimelineKind =
  | "user"
  | "assistant"
  | "tool"
  | "bash"
  | "compaction"
  | "branch_summary"
  | "custom";

interface SessionTimelineItem {
  entryId: string;
  timestamp: string | null;
  kind: SessionTimelineKind;
  title: string | null;
  text: string;
  textTruncated: boolean;
  isError: boolean;
}

interface SessionHistoryPage {
  sessionId: string;
  items: SessionTimelineItem[];
  nextCursor: SessionHistoryCursor | null;
  scannedBytes: number;
  encodedBytes: number;
}

interface SessionTreeNode {
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

interface SessionTreeSnapshot {
  nodes: SessionTreeNode[];
  leafId: string | null;
  truncated: boolean;
  encodedBytes: number;
}

interface RuntimeHydration {
  schemaVersion: number;
  runtimeRevision: number;
  runs: RunHydration[];
}

const RUNTIME_HYDRATION_SCHEMA_VERSION = 9;

function historyLabel(item: SessionTimelineItem): string {
  if (item.title) return item.title;
  switch (item.kind) {
    case "user":
      return "You";
    case "assistant":
      return "Assistant";
    case "tool":
      return "Tool";
    case "bash":
      return "Shell";
    case "compaction":
      return "Compaction";
    case "branch_summary":
      return "Branch summary";
    case "custom":
      return "Extension";
  }
}

function historyTimestamp(timestamp: string | null): string | undefined {
  if (!timestamp) return undefined;
  const parsed = Date.parse(timestamp);
  return Number.isNaN(parsed) ? timestamp : new Date(parsed).toLocaleString();
}

function HistoryTimeline(props: {
  run: RunHydration;
  forkDisabled: boolean;
  onFork: (entryId: string) => Promise<unknown>;
}) {
  const MAX_HISTORY_RENDER_PAGES = 4;
  type LoadedHistoryPage = {
    cursor: SessionHistoryCursor | null;
    page: SessionHistoryPage;
  };

  const [pages, setPages] = createSignal<LoadedHistoryPage[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [newerCursors, setNewerCursors] = createSignal<(SessionHistoryCursor | null)[]>([]);
  const [loadedMessageCount, setLoadedMessageCount] = createSignal<number | null>(null);
  let requestSequence = 0;
  let loadedSessionId: string | undefined;
  let historyViewport: HTMLDivElement | undefined;

  const fetchPage = async (
    cursor: SessionHistoryCursor | null,
    expectedSessionId: string,
  ): Promise<LoadedHistoryPage | undefined> => {
    const sequence = ++requestSequence;
    setLoading(true);
    setError(undefined);
    try {
      const result = await invokeDesktop<SessionHistoryPage>("runtime_read_session_history", {
        request: {
          runId: props.run.run.id,
          cursor,
        },
      });
      if (
        sequence !== requestSequence ||
        result.sessionId !== expectedSessionId ||
        props.run.run.session.sessionId !== expectedSessionId
      )
        return undefined;
      if (cursor === null) setLoadedMessageCount(props.run.run.session.messageCount);
      return { cursor, page: result };
    } catch (historyError) {
      if (sequence === requestSequence) setError(String(historyError));
      return undefined;
    } finally {
      if (sequence === requestSequence) setLoading(false);
    }
  };

  const keepBoundedNewerCursor = (cursor: SessionHistoryCursor | null) => {
    const history = newerCursors();
    setNewerCursors(
      history.length >= 64
        ? [...history.slice(history.length - 63), cursor]
        : [...history, cursor],
    );
  };

  const scrollToBottom = () => {
    queueMicrotask(() => {
      if (historyViewport) historyViewport.scrollTop = historyViewport.scrollHeight;
    });
  };

  createEffect(() => {
    const sessionId = props.run.run.session.sessionId;
    const sessionFile = props.run.run.session.sessionFile;
    if (!sessionId || !sessionFile) {
      requestSequence += 1;
      loadedSessionId = undefined;
      setPages([]);
      setError(undefined);
      setLoading(false);
      setNewerCursors([]);
      setLoadedMessageCount(null);
      return;
    }
    if (sessionId === loadedSessionId) return;
    loadedSessionId = sessionId;
    setPages([]);
    setNewerCursors([]);
    setLoadedMessageCount(null);
    void (async () => {
      const latest = await fetchPage(null, sessionId);
      if (!latest) return;
      setPages([latest]);
      setNewerCursors([]);
      scrollToBottom();
    })();
  });

  const loadOlder = async () => {
    const current = pages();
    const cursor = current[0]?.page.nextCursor;
    const sessionId = props.run.run.session.sessionId;
    if (!cursor || !sessionId || loading()) return;
    const previousHeight = historyViewport?.scrollHeight ?? 0;
    const older = await fetchPage(cursor, sessionId);
    if (!older) return;
    const next = [older, ...pages()];
    if (next.length > MAX_HISTORY_RENDER_PAGES) {
      const droppedNewest = next.pop();
      if (droppedNewest) keepBoundedNewerCursor(droppedNewest.cursor);
    }
    setPages(next);
    queueMicrotask(() => {
      if (!historyViewport) return;
      historyViewport.scrollTop += historyViewport.scrollHeight - previousHeight;
    });
  };

  const loadNewer = async () => {
    const history = newerCursors();
    const cursor = history.at(-1);
    const sessionId = props.run.run.session.sessionId;
    if (cursor === undefined || !sessionId || loading()) return;
    const newer = await fetchPage(cursor, sessionId);
    if (!newer) return;
    const next = [...pages(), newer];
    if (next.length > MAX_HISTORY_RENDER_PAGES) next.shift();
    setPages(next);
    setNewerCursors(history.slice(0, -1));
    scrollToBottom();
  };

  const loadLatest = () => {
    const sessionId = props.run.run.session.sessionId;
    if (!sessionId || loading()) return;
    void (async () => {
      const latest = await fetchPage(null, sessionId);
      if (!latest) return;
      setPages([latest]);
      setNewerCursors([]);
      scrollToBottom();
    })();
  };

  const hasNewActivity = () => {
    const current = props.run.run.session.messageCount;
    const loaded = loadedMessageCount();
    return current !== null && loaded !== null && current > loaded;
  };

  const atLatestWindow = () =>
    pages().at(-1)?.cursor === null && newerCursors().length === 0;

  const scannedBytes = () =>
    pages().reduce((total, loaded) => total + loaded.page.scannedBytes, 0);

  return (
    <Show when={props.run.run.session.sessionFile && props.run.run.session.sessionId}>
      <section class="history-timeline" aria-label="Persisted Pi session history">
        <div class="history-toolbar">
          <strong>Session history</strong>
          <span>Bounded active-branch window · {pages().length}/{MAX_HISTORY_RENDER_PAGES} pages</span>
          <div>
            <button
              type="button"
              disabled={loading() || newerCursors().length === 0}
              onClick={() => void loadNewer()}
            >
              Newer
            </button>
            <button
              type="button"
              disabled={loading() || !pages()[0]?.page.nextCursor}
              onClick={() => void loadOlder()}
            >
              Load older
            </button>
            <button
              type="button"
              disabled={loading() || (atLatestWindow() && !hasNewActivity())}
              onClick={loadLatest}
            >
              Latest
            </button>
          </div>
        </div>
        <Show when={hasNewActivity()}>
          <p class="history-note">New persisted activity is available. Latest refreshes the bounded window.</p>
        </Show>
        <Show when={pages().length > 0}>
          <div class="history-window" ref={historyViewport}>
            <For each={pages()}>
              {(loaded) => (
                <div class="history-page">
                  <For each={loaded.page.items}>
                    {(item) => {
                      const collapseCompletedOutput =
                        (item.kind === "tool" || item.kind === "bash") &&
                        !item.isError &&
                        item.text.length > 0;
                      const itemClass = `history-item history-${item.kind}${item.isError ? " history-error" : ""}`;
                      const itemHeader = (
                        <>
                          <strong>{historyLabel(item)}</strong>
                          <div>
                            <Show when={collapseCompletedOutput}>
                              <span>
                                Completed{item.textTruncated ? " · bounded output" : " · show output"}
                              </span>
                            </Show>
                            <Show when={historyTimestamp(item.timestamp)}>
                              {(timestamp) => <span>{timestamp()}</span>}
                            </Show>
                            <Show when={item.kind === "user"}>
                              <button
                                type="button"
                                disabled={props.forkDisabled || loading()}
                                onClick={() => void props.onFork(item.entryId)}
                              >
                                Fork here
                              </button>
                            </Show>
                          </div>
                        </>
                      );
                      return collapseCompletedOutput ? (
                        <details class={`${itemClass} history-collapsible`} data-timeline-row="true">
                          <summary>{itemHeader}</summary>
                          <pre>{item.text}</pre>
                          <Show when={item.textTruncated}>
                            <span class="truncation-note">History preview truncated</span>
                          </Show>
                        </details>
                      ) : (
                        <article class={itemClass} data-timeline-row="true">
                          <header>{itemHeader}</header>
                          <Show when={item.text}>
                            <pre>{item.text}</pre>
                          </Show>
                          <Show when={item.textTruncated}>
                            <span class="truncation-note">History preview truncated</span>
                          </Show>
                        </article>
                      );
                    }}
                  </For>
                  <Show when={loaded.page.items.length === 0}>
                    <p class="history-note">
                      {loaded.page.nextCursor
                        ? "No displayable entries in this bounded scan window. Load older to scan farther back on the active branch."
                        : "No persisted displayable history is available for this session yet."}
                    </p>
                  </Show>
                </div>
              )}
            </For>
          </div>
          <span class="history-footnote">
            Read {scannedBytes().toLocaleString()} session bytes across the visible bounded window.
          </span>
        </Show>
        <Show when={loading() && pages().length === 0}>
          <p class="history-note">Loading bounded session history.</p>
        </Show>
        <Show when={error()}>
          {(message) => <p class="error">Session history failed: {message()}</p>}
        </Show>
      </section>
    </Show>
  );
}

function runModelLabel(run: RunHydration): string {
  const model = run.run.session.model;
  if (!model) return "Model pending";
  return model.name ? `${model.name} · ${model.provider}` : `${model.provider}/${model.id}`;
}

function runThinkingLabel(run: RunHydration): string {
  return run.run.session.thinkingLevel ? `Thinking ${run.run.session.thinkingLevel}` : "Thinking pending";
}

function formatElapsedDuration(elapsedMs: number): string {
  const totalMinutes = Math.floor(Math.max(0, elapsedMs) / 60_000);
  if (totalMinutes < 1) return "<1m";
  if (totalMinutes < 60) return `${totalMinutes}m`;
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours < 24) return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`;
  const days = Math.floor(hours / 24);
  const remainingHours = hours % 24;
  return remainingHours === 0 ? `${days}d` : `${days}d ${remainingHours}h`;
}

function runElapsedLabel(run: RunHydration, nowUnixMs: number): string {
  const end = run.run.terminalUnixMs ?? nowUnixMs;
  return `${formatElapsedDuration(end - run.run.startedUnixMs)} elapsed`;
}

function isTerminalRun(run: RunHydration): boolean {
  return ["exited", "failed", "quarantined"].includes(run.run.process);
}

function SessionTreeInspector(props: {
  run: RunHydration;
  forkDisabled: boolean;
  onFork: (entryId: string) => Promise<unknown>;
}) {
  const [tree, setTree] = createSignal<SessionTreeSnapshot>();
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let requestSequence = 0;
  let loadedSessionId: string | undefined;

  const load = async () => {
    const sessionId = props.run.run.session.sessionId;
    if (!sessionId || loading() || props.run.run.process !== "ready") return;
    const sequence = ++requestSequence;
    setLoading(true);
    setError(undefined);
    try {
      const result = await invokeDesktop<SessionTreeSnapshot>("runtime_session_tree", {
        request: { runId: props.run.run.id },
      });
      if (
        sequence !== requestSequence ||
        props.run.run.session.sessionId !== sessionId ||
        loadedSessionId !== sessionId
      )
        return;
      setTree(result);
    } catch (treeError) {
      if (sequence === requestSequence) setError(String(treeError));
    } finally {
      if (sequence === requestSequence) setLoading(false);
    }
  };

  createEffect(() => {
    const sessionId = props.run.run.session.sessionId ?? undefined;
    if (sessionId === loadedSessionId) return;
    loadedSessionId = sessionId;
    requestSequence += 1;
    setTree(undefined);
    setLoading(false);
    setError(undefined);
    if (sessionId) void load();
  });

  onCleanup(() => {
    requestSequence += 1;
  });

  const nodeKind = (node: SessionTreeNode) => node.role ?? node.entryType.replaceAll("_", " ");

  return (
    <Show when={props.run.run.session.sessionId}>
      <section class="session-tree-inspector" aria-label="Pi session tree">
        <div class="session-tree-toolbar">
          <strong>Session tree</strong>
          <Show when={tree()}>
            {(snapshot) => (
              <span>
                {snapshot().nodes.length} entries
                {snapshot().truncated ? " · bounded tree" : ""}
              </span>
            )}
          </Show>
          <button
            type="button"
            disabled={loading() || props.run.run.process !== "ready"}
            onClick={() => void load()}
          >
            {loading() ? "Reading tree" : "Refresh"}
          </button>
        </div>
        <Show when={tree()} fallback={<p class="history-note">Loading Pi session tree.</p>}>
          {(snapshot) => (
            <div class="session-tree-list">
              <For each={snapshot().nodes}>
                {(node) => {
                  const isLeaf = () => snapshot().leafId === node.id;
                  const timestamp = () => historyTimestamp(node.timestamp ?? node.labelTimestamp);
                  const depthClass = `tree-depth-${Math.min(node.depth, 24)}`;
                  return (
                    <article
                      class={`session-tree-node ${depthClass}${isLeaf() ? " active-leaf" : ""}`}
                    >
                      <div class="session-tree-node-main">
                        <div>
                          <strong>{node.label ?? nodeKind(node)}</strong>
                          <span>
                            {nodeKind(node)} · {node.id.slice(0, 12)}
                            {node.childCount > 1 ? ` · ${node.childCount} branches` : ""}
                            {isLeaf() ? " · active leaf" : ""}
                          </span>
                        </div>
                        <div class="session-tree-node-actions">
                          <Show when={timestamp()}>{(value) => <span>{value()}</span>}</Show>
                          <Show when={node.entryType === "message" && node.role === "user"}>
                            <button
                              type="button"
                              disabled={props.forkDisabled || loading()}
                              onClick={() => void props.onFork(node.id)}
                            >
                              Fork here
                            </button>
                          </Show>
                        </div>
                      </div>
                      <Show when={node.preview}>
                        {(preview) => <pre>{preview()}</pre>}
                      </Show>
                      <Show when={node.previewTruncated}>
                        <span class="truncation-note">Tree preview truncated</span>
                      </Show>
                    </article>
                  );
                }}
              </For>
              <Show when={snapshot().nodes.length === 0}>
                <p class="history-note">Pi returned an empty session tree.</p>
              </Show>
              <Show when={snapshot().truncated}>
                <p class="history-note">
                  The inspector stopped at the configured renderer tree limit. Session history remains authoritative in Pi.
                </p>
              </Show>
              <span class="history-footnote">
                Retained {formatBytes(snapshot().encodedBytes)} of bounded tree metadata.
              </span>
            </div>
          )}
        </Show>
        <Show when={error()}>
          {(message) => <p class="error">Session tree failed: {message()}</p>}
        </Show>
      </section>
    </Show>
  );
}

interface RuntimeManagerSignal {
  kind: "runDirty";
  runId: string;
}

interface RuntimeUiEvent {
  kind: string;
  runId?: string;
  message?: string;
  notifyType?: "info" | "warning" | "error";
}

interface UiNotification {
  id: number;
  runId: string;
  message: string;
  notifyType: "info" | "warning" | "error";
}

interface RuntimeUiDrain {
  events: RuntimeUiEvent[];
  rehydrateRequired: boolean;
  pendingEditorText: string | null;
  hasMore: boolean;
}

interface PiProbeReport {
  environment: {
    pathSource: "configured" | "desktop_process" | "shell_probe";
    pi: { path: string; source: "configured" | "path" };
    git: { path: string; source: "configured" | "path" } | null;
    environmentEntryCount: number;
  };
  version: {
    display: string;
  };
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kib = bytes / 1024;
  if (kib < 1024) return `${kib.toFixed(kib >= 10 ? 0 : 1)} KiB`;
  const mib = kib / 1024;
  return `${mib.toFixed(mib >= 10 ? 0 : 1)} MiB`;
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error(`Could not read ${file.name}`));
    reader.onload = () => {
      if (typeof reader.result !== "string") {
        reject(new Error(`Could not encode ${file.name}`));
        return;
      }
      const separator = reader.result.indexOf(",");
      if (separator < 0) {
        reject(new Error(`Could not encode ${file.name}`));
        return;
      }
      resolve(reader.result.slice(separator + 1));
    };
    reader.readAsDataURL(file);
  });
}

type ExtensionDialogResponse =
  | { kind: "value"; id: string; value: string }
  | { kind: "confirmation"; id: string; confirmed: boolean }
  | { kind: "cancelled"; id: string };

function createComposerState(runId: string, initialText: string) {
  const [value, setValue] = createSignal(initialText);
  const [syncError, setSyncError] = createSignal<string>();
  let pendingText: string | undefined;
  let syncPromise: Promise<void> | undefined;
  let lastSyncedText = initialText;
  let retryBlocked = false;

  const startSync = () => {
    if (syncPromise || pendingText === undefined || retryBlocked) return;
    syncPromise = (async () => {
      while (pendingText !== undefined) {
        const text = pendingText;
        pendingText = undefined;
        try {
          const snapshot = await invokeDesktop<DraftSnapshot>("runtime_edit_draft", {
            request: { runId, text },
          });
          lastSyncedText = snapshot.text;
          retryBlocked = false;
          setSyncError(undefined);
        } catch (error) {
          // Keep the visible value intact and stop. A later edit or explicit
          // submit retries the newest value rather than replaying stale text.
          retryBlocked = true;
          setSyncError(String(error));
          return;
        }
      }
    })().finally(() => {
      syncPromise = undefined;
      if (pendingText !== undefined && !retryBlocked) startSync();
    });
  };

  const edit = (text: string) => {
    setValue(text);
    retryBlocked = false;
    pendingText = text;
    startSync();
  };

  const flush = async () => {
    retryBlocked = false;
    if (pendingText === undefined && lastSyncedText !== value()) {
      pendingText = value();
    }
    startSync();
    while (syncPromise) await syncPromise;
    if (lastSyncedText !== value()) {
      throw new Error(syncError() ?? "Draft did not synchronize to the backend");
    }
  };

  const applyBackend = (draft: DraftSnapshot | null) => {
    const localDirty = lastSyncedText !== value();
    if (!draft || syncPromise || pendingText !== undefined || retryBlocked || localDirty) return;
    lastSyncedText = draft.text;
    if (value() !== draft.text) setValue(draft.text);
  };

  return { value, syncError, edit, flush, applyBackend };
}

type ComposerState = ReturnType<typeof createComposerState>;

function DroppedBytes(props: { count: number }) {
  return (
    <Show when={props.count > 0}>
      <span class="truncation-note">Oldest {props.count} bytes omitted</span>
    </Show>
  );
}

function LiveTimeline(props: { live: LiveProjectionSnapshot | undefined }) {
  const VERBOSE_THINKING_BYTES = 480;
  const hasContent = () =>
    Boolean(
      props.live &&
        (props.live.assistantBlocks.length > 0 ||
          props.live.activeTools.length > 0 ||
          props.live.directBash.length > 0),
    );

  return (
    <Show when={hasContent()}>
      <section class="live-timeline" aria-label="Live Pi output">
        <For each={props.live?.assistantBlocks ?? []}>
          {(block) => {
            const label =
              block.kind === "thinking"
                ? "Thinking"
                : block.kind === "tool_call"
                  ? "Tool call"
                  : "Assistant";
            const collapseThinking =
              block.kind === "thinking" &&
              block.complete &&
              block.text.length > VERBOSE_THINKING_BYTES;
            return collapseThinking ? (
              <details class="live-block live-thinking live-thinking-collapsed" data-timeline-row="true">
                <summary>
                  <strong>{label}</strong>
                  <span>
                    Complete{block.droppedBytes > 0 ? " · bounded" : " · show reasoning"}
                  </span>
                </summary>
                <pre>{block.text}</pre>
                <DroppedBytes count={block.droppedBytes} />
              </details>
            ) : (
              <article class={`live-block live-${block.kind}`} data-timeline-row="true">
                <header>
                  <strong>{label}</strong>
                  <span>{block.complete ? "Complete" : "Streaming"}</span>
                </header>
                <pre>{block.text}</pre>
                <DroppedBytes count={block.droppedBytes} />
              </article>
            );
          }}
        </For>
        <For each={props.live?.activeTools ?? []}>
          {(tool) => (
            <article class="live-block live-tool" data-timeline-row="true">
              <header>
                <strong>{tool.toolName}</strong>
                <span>Tool running</span>
              </header>
              <pre>{tool.output}</pre>
              <DroppedBytes count={tool.droppedBytes} />
            </article>
          )}
        </For>
        <For each={props.live?.directBash ?? []}>
          {(bash) => (
            <article class="live-block live-bash" data-timeline-row="true">
              <header>
                <strong>Shell</strong>
                <span>{bash.requestId.slice(0, 8)}</span>
              </header>
              <pre>{bash.output}</pre>
              <DroppedBytes count={bash.droppedBytes} />
            </article>
          )}
        </For>
      </section>
    </Show>
  );
}

function modelKey(model: ModelSummary): string {
  return `${encodeURIComponent(model.provider)}:${encodeURIComponent(model.id)}`;
}

function parseModelKey(value: string): { provider: string; modelId: string } | undefined {
  const separator = value.indexOf(":");
  if (separator <= 0 || separator === value.length - 1) return undefined;
  try {
    return {
      provider: decodeURIComponent(value.slice(0, separator)),
      modelId: decodeURIComponent(value.slice(separator + 1)),
    };
  } catch {
    return undefined;
  }
}

function slashCommandName(text: string): string | undefined {
  const trimmed = text.trimStart();
  if (!trimmed.startsWith("/")) return undefined;
  const command = trimmed.slice(1).split(/\s/, 1)[0];
  return command && command.length > 0 ? command : undefined;
}

function ComposerCard(props: {
  run: RunHydration;
  state: ComposerState;
  attachmentLimits: RuntimeAttachmentLimits | undefined;
  onResolved: () => Promise<unknown>;
  onReviewSummary: (runId: string, summary: GitReviewSummary) => void;
}) {
  type InspectorKind = "details" | "changes" | "tree";
  const [activeInspector, setActiveInspector] = createSignal<InspectorKind>();
  const [submitError, setSubmitError] = createSignal<string>();
  const [submitting, setSubmitting] = createSignal(false);
  const [stopping, setStopping] = createSignal(false);
  const [stopError, setStopError] = createSignal<string>();
  const [stopNotice, setStopNotice] = createSignal<string>();
  const [controlBusy, setControlBusy] = createSignal(false);
  const [controlError, setControlError] = createSignal<string>();
  const [attaching, setAttaching] = createSignal(false);
  const [attachmentError, setAttachmentError] = createSignal<string>();
  const [sessionStats, setSessionStats] = createSignal<SessionStats>();
  const [statsBusy, setStatsBusy] = createSignal(false);
  const [statsError, setStatsError] = createSignal<string>();
  const [lastCompaction, setLastCompaction] = createSignal<CompactionResult>();
  const [lastAutoRetryCommand, setLastAutoRetryCommand] = createSignal<boolean>();
  const [commandSuggestionIndex, setCommandSuggestionIndex] = createSignal(0);
  const [reviewSummary, setReviewSummary] = createSignal<GitReviewSummary>();
  const [reviewChangeRevision, setReviewChangeRevision] = createSignal<number>();
  const [reviewBusy, setReviewBusy] = createSignal(false);
  const [reviewError, setReviewError] = createSignal<string>();
  const [reviewDiff, setReviewDiff] = createSignal<GitFileDiffPage>();
  const [reviewDiffPath, setReviewDiffPath] = createSignal<string>();
  const [reviewDiffCursor, setReviewDiffCursor] = createSignal<GitDiffCursor | null>(null);
  const [reviewDiffBackCursors, setReviewDiffBackCursors] = createSignal<
    Array<GitDiffCursor | null>
  >([]);
  const [sessionNameDraft, setSessionNameDraft] = createSignal(
    props.run.run.session.sessionName ?? "",
  );
  const [sessionNameDirty, setSessionNameDirty] = createSignal(false);
  let reviewRequestSequence = 0;
  let reviewDiffRequestSequence = 0;
  let fileInput!: HTMLInputElement;
  let commandSuggestionList: HTMLDivElement | undefined;

  createEffect(() => props.state.applyBackend(props.run.draft));
  onCleanup(() => {
    reviewRequestSequence += 1;
    reviewDiffRequestSequence += 1;
    if (reviewBusy() || reviewDiffPath()) {
      void invokeDesktop<boolean>("runtime_cancel_git_review", {
        request: { runId: props.run.run.id },
      });
    }
  });
  createEffect(() => {
    if (!sessionNameDirty()) {
      setSessionNameDraft(props.run.run.session.sessionName ?? "");
    }
  });

  const commands = () => props.run.rpc?.capabilities.commands ?? [];
  const exactCommand = () => {
    const name = slashCommandName(props.state.value());
    if (!name) return undefined;
    return commands().find((command) => command.name === name);
  };

  const setAutoRetry = async (enabled: boolean) => {
    const accepted = await runControl("runtime_set_auto_retry", {
      runId: props.run.run.id,
      enabled,
    });
    if (accepted) setLastAutoRetryCommand(enabled);
  };

  const toggleInspector = (inspector: InspectorKind) => {
    const current = activeInspector();
    const next = current === inspector ? undefined : inspector;
    if (current === "changes" && next !== "changes" && (reviewBusy() || reviewDiffPath())) {
      void cancelReview();
    }
    setActiveInspector(next);
    if (
      next === "changes" &&
      (!reviewSummary() || reviewChangeRevision() !== props.run.run.changeRevision) &&
      !reviewBusy()
    )
      void refreshReview();
  };

  createEffect(() => {
    const currentRevision = props.run.run.changeRevision;
    const reviewedRevision = reviewChangeRevision();
    if (reviewedRevision === undefined || reviewedRevision === currentRevision) return;

    // A known Pi tool/Bash completion invalidates detail derived from the old
    // working-tree observation. Keep the cheap summary visibly stale, but do
    // not launch Git until the user explicitly opens/refreshes Changes.
    reviewDiffRequestSequence += 1;
    setReviewDiff(undefined);
    setReviewDiffPath(undefined);
    setReviewDiffCursor(null);
    setReviewDiffBackCursors([]);
    if (reviewBusy()) void cancelReview();
  });
  const runningExtensionCommand = () =>
    props.run.composerAvailability === "agent_working" && exactCommand()?.source === "extension";
  const commandSuggestions = () => {
    const text = props.state.value().trimStart();
    if (!text.startsWith("/")) return [];
    const token = text.slice(1);
    if (/\s/.test(token)) return [];
    const query = token.toLowerCase();
    return commands()
      .filter((command) => command.name.toLowerCase().startsWith(query))
      .slice(0, 8);
  };
  const stageCommandSuggestion = (command: CommandSummary) => {
    props.state.edit(`/${command.name} `);
    setCommandSuggestionIndex(0);
  };
  const selectCommandSuggestion = (next: number) => {
    setCommandSuggestionIndex(next);
    queueMicrotask(() => {
      commandSuggestionList
        ?.querySelector<HTMLElement>(`[data-command-index="${next}"]`)
        ?.scrollIntoView({ block: "nearest" });
    });
  };
  createEffect(() => {
    const count = commandSuggestions().length;
    setCommandSuggestionIndex((current) => (count === 0 ? 0 : Math.min(current, count - 1)));
  });

  const currentModelKey = () => {
    const model = props.run.run.session.model;
    return model ? modelKey(model) : "";
  };

  const currentModelSupportsImages = () => props.run.run.session.model?.supportsImages;
  const imageDraftBlocked = () =>
    (props.run.draft?.images?.length ?? 0) > 0 && currentModelSupportsImages() === false;

  const controlDisabled = () =>
    controlBusy() ||
    submitting() ||
    stopping() ||
    props.run.run.process !== "ready" ||
    props.run.composerAvailability === "blocked_by_compaction";

  const attachmentDisabled = () =>
    attaching() ||
    stopping() ||
    props.run.draftRestorePending ||
    props.run.run.process !== "ready" ||
    props.run.composerAvailability === "unavailable";

  const attachmentAddDisabled = () =>
    attachmentDisabled() || currentModelSupportsImages() === false;

  const addFiles = async (files: File[]) => {
    if (files.length === 0) return;
    if (currentModelSupportsImages() === false) {
      setAttachmentError(
        "Current Pi model is configured for text-only input. Switch models before adding images.",
      );
      return;
    }
    if (attachmentDisabled()) return;
    const limits = props.attachmentLimits;
    if (!limits) {
      setAttachmentError("Attachment limits are not available yet");
      return;
    }

    setAttaching(true);
    setAttachmentError(undefined);
    let count = props.run.draft?.images?.length ?? 0;
    let aggregate = (props.run.draft?.images ?? []).reduce(
      (total, image) => total + image.decodedBytes,
      0,
    );
    try {
      for (const file of files) {
        if (!file.type.startsWith("image/") || file.type.length > 128) {
          throw new Error(`${file.name || "Attachment"} is not a supported image MIME type`);
        }
        const name = file.name.trim() || "image";
        const nameBytes = new TextEncoder().encode(name).byteLength;
        if (nameBytes > limits.maxNameBytes) {
          throw new Error(
            `${name} has a ${nameBytes}-byte name; the limit is ${limits.maxNameBytes} bytes`,
          );
        }
        if (file.size === 0) throw new Error(`${name} is empty`);
        if (file.size > limits.maxImageBytes) {
          throw new Error(
            `${name} is ${formatBytes(file.size)}; the per-image limit is ${formatBytes(limits.maxImageBytes)}`,
          );
        }
        if (count + 1 > limits.maxAttachments) {
          throw new Error(`A draft can contain at most ${limits.maxAttachments} images`);
        }
        if (aggregate + file.size > limits.maxAggregateBytes) {
          throw new Error(
            `Draft images would total ${formatBytes(aggregate + file.size)}; the limit is ${formatBytes(limits.maxAggregateBytes)}`,
          );
        }

        // Do not materialize base64 until all cheap metadata bounds pass. The
        // encoded string is sent directly to the backend and never stored in a
        // Solid signal or hydration snapshot.
        const data = await fileToBase64(file);
        const snapshot = await invokeDesktop<DraftSnapshot>("runtime_attach_draft_image", {
          request: {
            runId: props.run.run.id,
            fileName: name,
            mimeType: file.type,
            data,
          },
        });
        count = snapshot.images.length;
        aggregate = snapshot.images.reduce((total, image) => total + image.decodedBytes, 0);
      }
    } catch (error) {
      setAttachmentError(String(error));
    } finally {
      setAttaching(false);
      if (fileInput) fileInput.value = "";
      await props.onResolved();
    }
  };

  const refreshSessionStats = async () => {
    if (statsBusy() || props.run.run.process !== "ready") return;
    setStatsBusy(true);
    setStatsError(undefined);
    try {
      const stats = await invokeDesktop<SessionStats>("runtime_session_stats", {
        request: { runId: props.run.run.id },
      });
      setSessionStats(stats);
    } catch (error) {
      setStatsError(String(error));
    } finally {
      setStatsBusy(false);
    }
  };

  const compactSession = async () => {
    if (controlDisabled() || props.run.composerAvailability !== "ready") return;
    setControlBusy(true);
    setControlError(undefined);
    try {
      const result = await invokeDesktop<CompactionResult>("runtime_compact_session", {
        request: { runId: props.run.run.id },
      });
      setLastCompaction(result);
      await props.onResolved();
      await refreshSessionStats();
    } catch (error) {
      setControlError(String(error));
      await props.onResolved();
    } finally {
      setControlBusy(false);
    }
  };

  const refreshReview = async () => {
    const sequence = ++reviewRequestSequence;
    // A summary refresh invalidates any in-flight file detail derived from the
    // older repository observation. The backend begins the replacement as a
    // new owned review job and aborts the superseded Git subprocess.
    reviewDiffRequestSequence += 1;
    setReviewBusy(true);
    setReviewError(undefined);
    setReviewDiff(undefined);
    setReviewDiffPath(undefined);
    setReviewDiffCursor(null);
    setReviewDiffBackCursors([]);
    try {
      const summary = await invokeDesktop<GitReviewSummary>("runtime_git_review_summary", {
        request: { runId: props.run.run.id },
      });
      if (sequence === reviewRequestSequence) {
        setReviewSummary(summary);
        setReviewChangeRevision(props.run.run.changeRevision);
        props.onReviewSummary(props.run.run.id, summary);
      }
    } catch (error) {
      if (sequence === reviewRequestSequence) setReviewError(String(error));
    } finally {
      if (sequence === reviewRequestSequence) setReviewBusy(false);
    }
  };

  const loadReviewDiff = async (
    path: string,
    cursor: GitDiffCursor | null = null,
    backCursors: Array<GitDiffCursor | null> = [],
  ) => {
    const sequence = ++reviewDiffRequestSequence;
    setReviewDiffPath(path);
    setReviewError(undefined);
    try {
      const diff = await invokeDesktop<GitFileDiffPage>("runtime_git_review_file_page", {
        request: { runId: props.run.run.id, path, cursor },
      });
      if (sequence === reviewDiffRequestSequence) {
        setReviewDiff(diff);
        setReviewDiffCursor(cursor);
        setReviewDiffBackCursors(backCursors);
      }
    } catch (error) {
      if (sequence === reviewDiffRequestSequence) {
        setReviewDiff(undefined);
        setReviewDiffCursor(null);
        setReviewDiffBackCursors([]);
        setReviewError(String(error));
      }
    } finally {
      if (sequence === reviewDiffRequestSequence) setReviewDiffPath(undefined);
    }
  };

  const cancelReview = async () => {
    if (!reviewBusy() && !reviewDiffPath()) return;
    reviewRequestSequence += 1;
    reviewDiffRequestSequence += 1;
    setReviewBusy(false);
    setReviewDiffPath(undefined);
    setReviewError(undefined);
    try {
      await invokeDesktop<boolean>("runtime_cancel_git_review", {
        request: { runId: props.run.run.id },
      });
    } catch (error) {
      setReviewError(String(error));
    }
  };

  const loadNextReviewDiffPage = () => {
    const page = reviewDiff();
    const nextCursor = page?.nextCursor;
    if (!page || !nextCursor || reviewDiffPath()) return;
    void loadReviewDiff(page.path, nextCursor, [
      ...reviewDiffBackCursors(),
      reviewDiffCursor(),
    ]);
  };

  const loadPreviousReviewDiffPage = () => {
    const page = reviewDiff();
    const back = reviewDiffBackCursors();
    if (!page || back.length === 0 || reviewDiffPath()) return;
    const previousCursor = back[back.length - 1] ?? null;
    void loadReviewDiff(page.path, previousCursor, back.slice(0, -1));
  };

  const removeImage = async (imageId: string) => {
    if (attachmentDisabled()) return;
    setAttaching(true);
    setAttachmentError(undefined);
    try {
      await invokeDesktop<DraftSnapshot>("runtime_remove_draft_image", {
        request: { runId: props.run.run.id, imageId },
      });
    } catch (error) {
      setAttachmentError(String(error));
    } finally {
      setAttaching(false);
      await props.onResolved();
    }
  };

  const runControl = async (command: string, request: Record<string, unknown>) => {
    if (controlDisabled()) return false;
    setControlBusy(true);
    setControlError(undefined);
    try {
      await invokeDesktop<void>(command, { request });
      await props.onResolved();
      return true;
    } catch (error) {
      setControlError(String(error));
      await props.onResolved();
      return false;
    } finally {
      setControlBusy(false);
    }
  };

  const forkSession = async (entryId: string) => {
    if (controlDisabled() || props.run.composerAvailability !== "ready") return;
    setControlBusy(true);
    setControlError(undefined);
    try {
      const result = await invokeDesktop<{ cancelled: boolean }>("runtime_fork_session", {
        request: { runId: props.run.run.id, entryId },
      });
      if (result.cancelled) setControlError("Fork was cancelled by a Pi extension");
      await props.onResolved();
    } catch (error) {
      setControlError(String(error));
      await props.onResolved();
    } finally {
      setControlBusy(false);
    }
  };

  const saveSessionName = async () => {
    const saved = await runControl("runtime_set_session_name", {
      runId: props.run.run.id,
      name: sessionNameDraft(),
    });
    if (saved) setSessionNameDirty(false);
  };

  const cloneSession = async () => {
    if (controlDisabled() || props.run.composerAvailability !== "ready") return;
    setControlBusy(true);
    setControlError(undefined);
    try {
      const result = await invokeDesktop<{ cancelled: boolean }>("runtime_clone_session", {
        request: { runId: props.run.run.id },
      });
      if (result.cancelled) setControlError("Clone was cancelled by a Pi extension");
      await props.onResolved();
    } catch (error) {
      setControlError(String(error));
      await props.onResolved();
    } finally {
      setControlBusy(false);
    }
  };

  const submit = async (action: ComposerAction) => {
    if (
      submitting() ||
      stopping() ||
      props.run.composerSubmissionPending ||
      props.run.draftRestorePending
    )
      return;
    setSubmitting(true);
    setSubmitError(undefined);
    try {
      await props.state.flush();
      const result = await invokeDesktop<ComposerSubmitResult>("runtime_submit_draft", {
        request: { runId: props.run.run.id, action },
      });
      if (!result.accepted) {
        setSubmitError(result.error ?? "Pi rejected the composer submission");
      }
      await props.onResolved();
    } catch (error) {
      setSubmitError(String(error));
      await props.onResolved();
    } finally {
      setSubmitting(false);
    }
  };

  const stop = async () => {
    if (stopping()) return;
    setStopping(true);
    setStopError(undefined);
    setStopNotice(undefined);
    let localSyncError: string | undefined;
    try {
      try {
        await props.state.flush();
      } catch (error) {
        // Stop remains a lifecycle control even if draft synchronization is
        // unhealthy. The composer state retains the unsynced local value so a
        // subsequent hydration cannot silently overwrite it.
        localSyncError = String(error);
      }
      const result = await invokeDesktop<RuntimeStopResult>("runtime_stop", {
        request: { runId: props.run.run.id },
      });
      const recoveredCount = result.recoveredSteering.length + result.recoveredFollowUp.length;
      if (recoveredCount > 0 && !result.draftRestored) {
        setStopError(
          `Stopped, but ${recoveredCount} recovered queued message${recoveredCount === 1 ? "" : "s"} could not be merged into the draft: ${result.draftRestoreError ?? "unknown recovery error"}`,
        );
      } else if (localSyncError) {
        setStopError(`Stopped, but the local draft did not synchronize first: ${localSyncError}`);
      } else if (result.quarantined) {
        setStopError(
          "Stop could not confirm Pi process termination. The run is quarantined and remains visible for inspection.",
        );
      } else if (result.processTerminated) {
        setStopNotice(
          "Stop required terminating this Pi process instead of leaving the RPC child reusable. Pi session history and any recovered queued draft text remain available for Resume.",
        );
      }
      await props.onResolved();
    } catch (error) {
      setStopError(String(error));
      await props.onResolved();
    } finally {
      setStopping(false);
    }
  };

  const unavailableLabel = () => {
    switch (props.run.composerAvailability) {
      case "blocked_by_compaction":
        return "Composer blocked while Pi compacts the session";
      case "unavailable":
        return "Composer unavailable until the Pi runtime is ready";
      default:
        return undefined;
    }
  };

  const disabled = () => submitting() || stopping() || props.run.composerSubmissionPending;
  const composerDisabled = () => disabled() || props.run.draftRestorePending;

  return (
    <article class="composer-card">
      <header>
        <div class="run-identity">
          <strong>
            {props.run.run.session.sessionName ??
              props.run.run.session.sessionId?.slice(0, 12) ??
              `Run ${props.run.run.id.slice(0, 8)}`}
          </strong>
          <span title={props.run.run.executionRoot}>
            {props.run.run.executionIsolation === "git_worktree" ? "Git-isolated worktree" : "Local checkout"}
            {" · "}{props.run.run.executionRoot}
          </span>
          <Show when={props.run.run.worktree}>
            {(worktree) => (
              <span title={`${worktree().branch} @ ${worktree().baseCommit}`}>
                {worktree().branch} · base {worktree().baseCommit.slice(0, 12)}
              </span>
            )}
          </Show>
        </div>
        <span>
          {props.run.draftRestorePending
            ? "Restoring draft"
            : stopping()
              ? "Stopping"
            : props.run.composerSubmissionPending || submitting()
              ? "Submitting"
              : props.run.draft?.durability ?? "draft unavailable"}
        </span>
      </header>
      <nav class="inspector-tabs" aria-label="Run inspectors">
        <button
          type="button"
          aria-pressed={activeInspector() === "details"}
          onClick={() => toggleInspector("details")}
        >
          Run details
        </button>
        <button
          type="button"
          aria-pressed={activeInspector() === "changes"}
          onClick={() => toggleInspector("changes")}
        >
          Changes
        </button>
        <button
          type="button"
          aria-pressed={activeInspector() === "tree"}
          disabled={!props.run.run.session.sessionId || props.run.run.process !== "ready"}
          onClick={() => toggleInspector("tree")}
        >
          Session tree
        </button>
      </nav>
      <Show when={activeInspector() === "details"}>
        <section class="run-details-inspector" aria-label="Run details">
        <div class="run-controls" aria-label="Pi session controls">
        <Show when={(props.run.rpc?.capabilities.models?.length ?? 0) > 0}>
          <label>
            <span>Model</span>
            <select
              value={currentModelKey()}
              disabled={controlDisabled()}
              onChange={(event) => {
                const model = parseModelKey(event.currentTarget.value);
                if (!model || event.currentTarget.value === currentModelKey()) return;
                void runControl("runtime_set_model", {
                  runId: props.run.run.id,
                  provider: model.provider,
                  modelId: model.modelId,
                });
              }}
            >
              <For each={props.run.rpc?.capabilities.models ?? []}>
                {(model) => (
                  <option value={modelKey(model)}>
                    {model.name ?? model.id} · {model.provider}
                    {model.supportsImages === true
                      ? " · images"
                      : model.supportsImages === false
                        ? " · text only"
                        : ""}
                  </option>
                )}
              </For>
            </select>
          </label>
        </Show>
        <div class="auto-retry-control" role="group" aria-label="Automatic provider retry">
          <span>Automatic retry</span>
          <div>
            <button
              type="button"
              disabled={controlDisabled()}
              onClick={() => void setAutoRetry(true)}
            >
              Enable
            </button>
            <button
              type="button"
              disabled={controlDisabled()}
              onClick={() => void setAutoRetry(false)}
            >
              Disable
            </button>
          </div>
          <small>
            Pi RPC does not report the current retry-enabled flag in get_state.
            {lastAutoRetryCommand() == null
              ? " Choose an explicit policy for this live run if needed."
              : ` Last accepted command in this view: ${lastAutoRetryCommand() ? "enabled" : "disabled"}.`}
          </small>
        </div>
        <Show when={(props.run.rpc?.capabilities.thinkingLevels?.length ?? 0) > 0}>
          <label>
            <span>Thinking</span>
            <select
              value={props.run.run.session.thinkingLevel ?? "off"}
              disabled={controlDisabled()}
              onChange={(event) => {
                const level = event.currentTarget.value as ThinkingLevel;
                if (level === props.run.run.session.thinkingLevel) return;
                void runControl("runtime_set_thinking_level", {
                  runId: props.run.run.id,
                  level,
                });
              }}
            >
              <For each={props.run.rpc?.capabilities.thinkingLevels ?? []}>
                {(level) => <option value={level}>{level}</option>}
              </For>
            </select>
          </label>
        </Show>
        <Show when={props.run.run.session.autoCompactionEnabled != null}>
          <label>
            <span>Automatic compaction</span>
            <select
              value={props.run.run.session.autoCompactionEnabled ? "enabled" : "disabled"}
              disabled={controlDisabled()}
              onChange={(event) => {
                const enabled = event.currentTarget.value === "enabled";
                if (enabled === props.run.run.session.autoCompactionEnabled) return;
                void runControl("runtime_set_auto_compaction", {
                  runId: props.run.run.id,
                  enabled,
                });
              }}
            >
              <option value="enabled">Enabled</option>
              <option value="disabled">Disabled</option>
            </select>
          </label>
        </Show>
        <form
          class="session-name-control"
          onSubmit={(event) => {
            event.preventDefault();
            if (sessionNameDirty()) void saveSessionName();
          }}
        >
          <label>
            <span>Session name</span>
            <input
              value={sessionNameDraft()}
              disabled={controlDisabled()}
              placeholder="Unnamed session"
              onInput={(event) => {
                setSessionNameDraft(event.currentTarget.value);
                setSessionNameDirty(true);
              }}
            />
          </label>
          <button type="submit" disabled={controlDisabled() || !sessionNameDirty()}>
            Save name
          </button>
        </form>
        <button
          type="button"
          disabled={controlDisabled() || props.run.composerAvailability !== "ready"}
          onClick={() => void cloneSession()}
        >
          Clone session
        </button>
        <button
          type="button"
          disabled={controlDisabled() || props.run.composerAvailability !== "ready"}
          onClick={() => void compactSession()}
          title="Ask Pi to summarize older context and keep the session going"
        >
          Compact context
        </button>
        <button
          type="button"
          disabled={statsBusy() || props.run.run.process !== "ready"}
          onClick={() => void refreshSessionStats()}
        >
          {statsBusy() ? "Reading usage" : "Session usage"}
        </button>
        </div>
        <Show when={lastCompaction()}>
          {(result) => (
            <p class="session-usage-note">
              Compacted {result().tokensBefore.toLocaleString()} → approximately{" "}
              {result().estimatedTokensAfter.toLocaleString()} context tokens.
            </p>
          )}
        </Show>
        <Show when={sessionStats()}>
          {(stats) => (
            <div class="session-usage" aria-label="Pi session usage">
              <strong>
                {stats().contextUsage?.percent == null
                  ? "Context usage unknown"
                  : `${stats().contextUsage!.percent!.toFixed(1)}% context`}
              </strong>
              <span>
                {stats().contextUsage?.tokens == null
                  ? `window ${stats().contextUsage?.contextWindow.toLocaleString() ?? "unknown"}`
                  : `${stats().contextUsage!.tokens!.toLocaleString()} / ${stats().contextUsage!.contextWindow.toLocaleString()} tokens`}
              </span>
              <span>
                {stats().tokens.total.toLocaleString()} session tokens · ${stats().cost.toFixed(4)}
              </span>
            </div>
          )}
        </Show>
        <Show when={statsError()}>{(error) => <p class="error">Session usage: {error()}</p>}</Show>
        </section>
      </Show>
      <Show when={controlError()}>{(error) => <p class="error">Session control: {error()}</p>}</Show>
      <Show when={activeInspector() === "changes"}>
        <div class="git-review" aria-label="Changes inspector">
        <div class="git-review-toolbar">
          <button type="button" onClick={() => void refreshReview()}>
            {reviewBusy()
              ? "Restart changes"
              : reviewSummary()
                ? "Refresh changes"
                : "Changes"}
          </button>
          <Show when={reviewBusy() || Boolean(reviewDiffPath())}>
            <button type="button" onClick={() => void cancelReview()}>
              Cancel review
            </button>
          </Show>
          <Show when={reviewSummary()}>
            {(summary) => (
              <span>
                {summary().files.length} changed file{summary().files.length === 1 ? "" : "s"}
                {summary().truncated ? " · bounded list" : ""}
                {reviewChangeRevision() !== props.run.run.changeRevision ? " · may be stale" : ""}
              </span>
            )}
          </Show>
        </div>
        <Show when={reviewSummary()}>
          {(summary) => (
            <div class="git-review-files">
              <Show when={summary().files.length === 0}>
                <span>No current changes under this run’s execution root.</span>
              </Show>
              <For each={summary().files}>
                {(file) => (
                  <button
                    type="button"
                    disabled={reviewBusy()}
                    class={reviewDiff()?.path === file.path ? "selected" : undefined}
                    onClick={() => void loadReviewDiff(file.path, null, [])}
                  >
                    <span>{file.status.replace("_", " ")}</span>
                    <strong title={file.path}>{file.path}</strong>
                    <Show when={file.previousPath}>
                      {(previous) => <small title={previous()}>from {previous()}</small>}
                    </Show>
                    <Show when={reviewDiffPath() === file.path}>
                      <small>Loading diff</small>
                    </Show>
                  </button>
                )}
              </For>
            </div>
          )}
        </Show>
        <Show when={reviewDiff()}>
          {(diff) => (
            <div class="git-review-diff">
              <div>
                <strong>{diff().path}</strong>
                <span>
                  {diff().untracked
                    ? "untracked metadata only"
                    : diff().binary
                      ? "binary file · no text patch"
                      : `page ${reviewDiffBackCursors().length + 1}${diff().nextCursor ? " · more available" : ""}`}
                </span>
              </div>
              <Show
                when={diff().diff}
                fallback={<pre>No current tracked diff for this file.</pre>}
              >
                <Show when={diff().hunks.length > 0}>
                  <nav class="git-review-hunks" aria-label="Diff hunks on this page">
                    <For each={diff().hunks}>
                      {(hunk, index) => (
                        <button
                          type="button"
                          title={hunk.header}
                          onClick={() =>
                            document
                              .getElementById(
                                `git-hunk-${props.run.run.id}-${reviewDiffBackCursors().length}-${index()}`,
                              )
                              ?.scrollIntoView({ block: "nearest" })
                          }
                        >
                          Hunk {index() + 1}
                        </button>
                      )}
                    </For>
                  </nav>
                </Show>
                <div class="git-review-diff-segments">
                  <For each={diffPageSegments(diff())}>
                    {(segment, index) => (
                      <pre
                        id={
                          segment.hunk
                            ? `git-hunk-${props.run.run.id}-${reviewDiffBackCursors().length}-${Math.max(0, diff().hunks.findIndex((hunk) => hunk.lineIndex === segment.hunk?.lineIndex))}`
                            : undefined
                        }
                        class={segment.hunk ? "git-review-hunk" : undefined}
                      >
                        {segment.text}
                      </pre>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={!diff().untracked && !diff().binary}>
                <div class="git-review-page-actions">
                  <button
                    type="button"
                    disabled={reviewDiffBackCursors().length === 0 || Boolean(reviewDiffPath())}
                    onClick={loadPreviousReviewDiffPage}
                  >
                    Previous page
                  </button>
                  <span>
                    Scanned {formatBytes(diff().scannedBytes)} for this bounded page
                  </span>
                  <button
                    type="button"
                    disabled={!diff().nextCursor || Boolean(reviewDiffPath())}
                    onClick={loadNextReviewDiffPage}
                  >
                    Next page
                  </button>
                </div>
              </Show>
            </div>
          )}
        </Show>
        <Show when={reviewError()}>{(error) => <p class="error">Change review: {error()}</p>}</Show>
        </div>
      </Show>
      <Show when={activeInspector() === "tree"}>
        <SessionTreeInspector
          run={props.run}
          forkDisabled={controlDisabled() || props.run.composerAvailability !== "ready"}
          onFork={forkSession}
        />
      </Show>
      <HistoryTimeline
        run={props.run}
        forkDisabled={controlDisabled() || props.run.composerAvailability !== "ready"}
        onFork={forkSession}
      />
      <LiveTimeline live={props.run.rpc?.live} />
      <div
        class="composer-input"
        onDragOver={(event) => {
          if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
        }}
        onDrop={(event) => {
          const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
            file.type.startsWith("image/"),
          );
          if (files.length === 0) return;
          event.preventDefault();
          void addFiles(files);
        }}
      >
        <textarea
          value={props.state.value()}
          disabled={
            stopping() ||
            props.run.draftRestorePending ||
            props.run.composerAvailability === "unavailable"
          }
          placeholder="Message Pi"
          aria-label={`Composer for run ${props.run.run.id}`}
          aria-keyshortcuts="Control+Enter Meta+Enter ArrowUp ArrowDown"
          onInput={(event) => {
            setCommandSuggestionIndex(0);
            props.state.edit(event.currentTarget.value);
          }}
          onKeyDown={(event) => {
            const suggestions = commandSuggestions();
            if (
              !composerDisabled() &&
              !event.ctrlKey &&
              !event.metaKey &&
              !event.altKey &&
              suggestions.length > 0
            ) {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                const direction = event.key === "ArrowDown" ? 1 : -1;
                selectCommandSuggestion(
                  (commandSuggestionIndex() + direction + suggestions.length) % suggestions.length,
                );
                return;
              }
              if (event.key === "Enter") {
                event.preventDefault();
                const selected = suggestions[commandSuggestionIndex()] ?? suggestions[0];
                if (selected) stageCommandSuggestion(selected);
                return;
              }
            }
            if (event.key !== "Enter" || (!event.ctrlKey && !event.metaKey)) return;
            event.preventDefault();
            if (composerDisabled() || imageDraftBlocked()) return;
            if (props.run.composerAvailability === "ready") {
              void submit("send");
            } else if (props.run.composerAvailability === "agent_working") {
              void submit(runningExtensionCommand() ? "runCommand" : "steer");
            }
          }}
          onPaste={(event) => {
            const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
              file.type.startsWith("image/"),
            );
            if (files.length === 0) return;
            event.preventDefault();
            void addFiles(files);
          }}
        />
        <div class="attachment-toolbar">
          <input
            ref={fileInput}
            class="attachment-picker"
            type="file"
            accept="image/*"
            multiple
            aria-label="Choose draft images"
            disabled={attachmentAddDisabled()}
            onChange={(event) => void addFiles(Array.from(event.currentTarget.files ?? []))}
          />
          <button
            type="button"
            disabled={attachmentAddDisabled() || !props.attachmentLimits}
            onClick={() => fileInput.click()}
          >
            {attaching() ? "Updating images" : "Add image"}
          </button>
          <Show when={props.attachmentLimits}>
            {(limits) => (
              <span>
                Drop or paste images · {limits().maxAttachments} max · {formatBytes(limits().maxImageBytes)} each
              </span>
            )}
          </Show>
        </div>
        <Show when={currentModelSupportsImages() === false}>
          <p class="attachment-warning">
            Current model is declared text-only by Pi. Images are preserved in the draft but will
            not be submitted until you remove them or switch to an image-capable model.
          </p>
        </Show>
        <Show when={(props.run.draft?.images?.length ?? 0) > 0}>
          <div class="attachment-list" aria-label="Draft image attachments">
            <For each={props.run.draft?.images ?? []}>
              {(image) => (
                <div class="attachment-item">
                  <div>
                    <strong>{image.fileName}</strong>
                    <span>{image.mimeType} · {formatBytes(image.decodedBytes)}</span>
                  </div>
                  <button
                    type="button"
                    disabled={attachmentDisabled()}
                    aria-label={`Remove ${image.fileName}`}
                    onClick={() => void removeImage(image.id)}
                  >
                    Remove
                  </button>
                </div>
              )}
            </For>
          </div>
        </Show>
      </div>
      <Show when={commandSuggestions().length > 0}>
        <div ref={commandSuggestionList} class="command-suggestions" aria-label="Pi commands">
          <For each={commandSuggestions()}>
            {(command, index) => (
              <button
                type="button"
                disabled={composerDisabled()}
                data-command-index={index()}
                class={index() === commandSuggestionIndex() ? "selected" : undefined}
                aria-current={index() === commandSuggestionIndex() ? "true" : undefined}
                onMouseEnter={() => setCommandSuggestionIndex(index())}
                onClick={() => stageCommandSuggestion(command)}
              >
                <strong>/{command.name}</strong>
                <span>{command.source}</span>
                <Show when={command.description}>
                  {(description) => <small>{description()}</small>}
                </Show>
              </button>
            )}
          </For>
        </div>
      </Show>
      <div class="composer-actions">
        <Show when={props.run.composerAvailability === "ready"}>
          <button
            type="button"
            disabled={composerDisabled() || imageDraftBlocked()}
            onClick={() => void submit("send")}
          >
            Send
          </button>
        </Show>
        <Show when={props.run.composerAvailability === "agent_working"}>
          <Show
            when={runningExtensionCommand()}
            fallback={
              <>
                <button
                  type="button"
                  disabled={composerDisabled() || imageDraftBlocked()}
                  onClick={() => void submit("steer")}
                >
                  Steer
                </button>
                <button
                  type="button"
                  disabled={composerDisabled() || imageDraftBlocked()}
                  onClick={() => void submit("followUp")}
                >
                  Follow up
                </button>
              </>
            }
          >
            <button
              type="button"
              disabled={composerDisabled() || imageDraftBlocked()}
              onClick={() => void submit("runCommand")}
            >
              Run command
            </button>
          </Show>
        </Show>
        <Show
          when={runHasStoppableActivity(props.run)}
        >
          <button type="button" disabled={stopping()} onClick={() => void stop()}>
            Stop
          </button>
        </Show>
        <Show when={unavailableLabel()}>{(label) => <span>{label()}</span>}</Show>
      </div>
      <Show when={props.state.syncError()}>
        {(error) => <p class="error">Draft sync failed: {error()}</p>}
      </Show>
      <Show when={props.run.draft?.persistenceError}>
        {(error) => <p class="error">Draft persistence failed: {error()}</p>}
      </Show>
      <Show when={submitError()}>{(error) => <p class="error">{error()}</p>}</Show>
      <Show when={stopError()}>{(error) => <p class="error">{error()}</p>}</Show>
      <Show when={stopNotice()}>{(notice) => <p>{notice()}</p>}</Show>
      <Show when={attachmentError()}>
        {(error) => <p class="error">Image attachment: {error()}</p>}
      </Show>
    </article>
  );
}

function needsHydration(events: RuntimeUiEvent[]): boolean {
  return events.some((event) =>
    [
      "stateChanged",
      "capabilitiesChanged",
      "sessionSyncChanged",
      "extensionDialogsChanged",
      "extensionUiStateChanged",
      "editorTextChanged",
      "draftChanged",
      "composerChanged",
      "processTerminal",
      "assistantMessageReset",
      "assistantBlockUpdated",
      "toolUpdated",
      "toolFinished",
      "directBashUpdated",
    ].includes(event.kind),
  );
}

function runTitle(run: RunHydration): string {
  return (
    run.run.session.sessionName ??
    run.run.session.sessionId?.slice(0, 12) ??
    `Run ${run.run.id.slice(0, 8)}`
  );
}

function runStateLabel(run: RunHydration): string {
  if (run.run.process === "exited") return "done";
  if (run.run.process === "failed") return "failed";
  if (run.run.process === "quarantined") return "termination uncertain";
  if (run.run.process !== "ready") return run.run.process.replaceAll("_", " ");
  if ((run.rpc?.pendingDialogs.length ?? 0) > 0) return "needs attention";
  if (run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished) return "summary retry";
  if (run.run.compacting) return "compacting";
  if (run.rpc?.retry && !run.rpc.retry.finished) return "retrying";
  if (run.rpc?.streamStalled) return "possibly stalled";
  if (run.run.agentWorking) return "working";
  if (run.run.queue.steering + run.run.queue.followUp > 0) return "queued";
  return "ready";
}

function runQueuedCount(run: RunHydration): number {
  return run.run.queue.steering + run.run.queue.followUp;
}

function runActivityLabel(run: RunHydration): string {
  const pending = run.rpc?.pendingDialogs[0];
  if (pending) return `Waiting for input: ${pending.request.kind.title}`;
  const summaryRetry = run.rpc?.summarizationRetry;
  if (summaryRetry && !summaryRetry.finished) {
    const source = summaryRetry.source === "branchSummary" ? "branch summary" : "context summary";
    return summaryRetry.source
      ? `Retrying ${source} ${summaryRetry.attempt}/${summaryRetry.maxAttempts}`
      : `Summary retry ${summaryRetry.attempt}/${summaryRetry.maxAttempts} scheduled`;
  }
  if (run.run.compacting) {
    const reason = run.rpc?.compaction?.reason;
    return reason ? `Compacting context · ${reason}` : "Compacting context";
  }
  const retry = run.rpc?.retry;
  if (retry && !retry.finished) {
    return retry.waiting
      ? `Provider retry ${retry.attempt}/${retry.maxAttempts} in ~${Math.ceil(retry.delayMs / 1_000)}s`
      : `Provider retry ${retry.attempt}/${retry.maxAttempts} running`;
  }
  if (run.rpc?.streamStalled) return "No Pi RPC event for about 2 minutes";
  const tool = run.rpc?.live.activeTools[0];
  if (tool) return `Running tool: ${tool.toolName}`;
  if ((run.rpc?.live.directBash.length ?? 0) > 0) return "Running shell command";
  if (run.run.agentWorking) return "Pi is working";
  const queued = runQueuedCount(run);
  if (queued > 0) return `${queued} queued message${queued === 1 ? "" : "s"}`;
  if (run.run.process === "ready") return "Ready for input";
  if (run.run.process === "exited") return "Run finished";
  if (run.run.process === "quarantined") return "Process termination is uncertain";
  if (run.run.failure) return `${run.run.failure.kind.replaceAll("_", " ")} failure`;
  return run.run.process.replaceAll("_", " ");
}

function runHasStoppableActivity(run: RunHydration): boolean {
  return (
    run.run.agentWorking ||
    run.run.compacting ||
    Boolean(run.rpc?.retry && !run.rpc.retry.finished) ||
    Boolean(run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished)
  );
}

function canCloseRun(run: RunHydration): boolean {
  if (run.run.process === "starting") return true;
  if (run.run.process !== "ready") return false;
  return (
    !run.run.agentWorking &&
    run.composerAvailability === "ready" &&
    !run.composerSubmissionPending &&
    !run.draftRestorePending &&
    (run.rpc?.pendingDialogs.length ?? 0) === 0
  );
}

function runDisplayPriority(run: RunHydration): number {
  if ((run.rpc?.pendingDialogs.length ?? 0) > 0) return 0;
  if (
    run.run.process === "ready" &&
    (run.run.agentWorking ||
      Boolean(run.rpc?.retry && !run.rpc.retry.finished) ||
      Boolean(run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished))
  )
    return 1;
  if (!["exited", "failed", "quarantined"].includes(run.run.process)) return 2;
  return 3;
}

function ExtensionUiPanel(props: { snapshot: ExtensionUiSnapshot | undefined }) {
  const hasContent = () =>
    Boolean(
      props.snapshot?.title ||
        props.snapshot?.statuses.length ||
        props.snapshot?.widgets.length,
    );
  return (
    <Show when={hasContent()}>
      <section class="extension-ui-panel" aria-label="Extension status">
        <Show when={props.snapshot?.title}>
          {(title) => <strong class="extension-ui-title">{title()}</strong>}
        </Show>
        <For each={props.snapshot?.statuses ?? []}>
          {(status) => (
            <div class="extension-status-row">
              <span>{status.key}</span>
              <strong>{status.text}</strong>
            </div>
          )}
        </For>
        <For each={props.snapshot?.widgets ?? []}>
          {(entry) => (
            <article class="extension-widget">
              <header>
                <strong>{entry.key}</strong>
                <span>{entry.widget.placement === "aboveEditor" ? "Above editor" : "Below editor"}</span>
              </header>
              <For each={entry.widget.lines}>{(line) => <pre>{line}</pre>}</For>
            </article>
          )}
        </For>
      </section>
    </Show>
  );
}

function PiRuntimeNoticePanel(props: { rpc: RunHydration["rpc"] | undefined }) {
  const compaction = () => props.rpc?.compaction;
  const retry = () => props.rpc?.retry;
  const summaryRetry = () => props.rpc?.summarizationRetry;
  const extensionError = () => props.rpc?.lastExtensionError;
  const streamStalled = () => Boolean(props.rpc?.streamStalled);
  const visible = () =>
    Boolean(
      (retry() && (!retry()!.finished || retry()!.success === false)) ||
        (compaction() &&
          (compaction()!.aborted || Boolean(compaction()!.errorMessage) || compaction()!.willRetry)) ||
        (summaryRetry() && !summaryRetry()!.finished) ||
        extensionError() ||
        streamStalled(),
    );

  return (
    <Show when={visible()}>
      <section class="pi-runtime-notices" aria-label="Pi runtime recovery status">
        <Show
          when={
            compaction() &&
            (compaction()!.aborted || Boolean(compaction()!.errorMessage) || compaction()!.willRetry)
              ? compaction()
              : undefined
          }
        >
          {(state) => (
            <details class="pi-runtime-notice runtime-warning" open>
              <summary>
                Compaction {state().reason}
                {state().aborted
                  ? " · aborted"
                  : state().errorMessage
                    ? " · failed"
                    : state().willRetry
                      ? " · prompt retry pending"
                      : ""}
              </summary>
              <Show when={state().willRetry && !state().errorMessage}>
                <p>
                  Pi compacted after context overflow and reports that it will automatically retry
                  the prompt. Pi Wizard waits for Pi’s subsequent events and does not resubmit it.
                </p>
              </Show>
              <Show when={state().aborted}>
                <p>Pi reports that this compaction was aborted. No successful summary is implied.</p>
              </Show>
              <Show when={state().errorMessage}>
                {(error) => <pre>{error()}</pre>}
              </Show>
              <Show when={state().reasonTruncated || state().errorTruncated}>
                <span class="truncation-note">Compaction detail truncated</span>
              </Show>
            </details>
          )}
        </Show>
        <Show when={streamStalled()}>
          <details class="pi-runtime-notice runtime-warning" open>
            <summary>Pi stream quiet</summary>
            <p>
              No Pi RPC event has arrived for about two minutes while Pi still reports this run as
              working. This can be a long provider/tool operation or a stalled stream. Pi Wizard did
              not probe, retry, or resubmit anything automatically; the first new Pi event clears
              this advisory and Stop remains available.
            </p>
          </details>
        </Show>
        <Show when={retry() && !retry()!.finished ? retry() : undefined}>
          {(active) => (
            <details class="pi-runtime-notice" open>
              <summary>
                {active().waiting ? "Provider retry scheduled" : "Provider retry running"} · {active().attempt}/{active().maxAttempts}
              </summary>
              <p>
                {active().waiting
                  ? `Pi is waiting about ${Math.ceil(active().delayMs / 1_000)} seconds before retrying. Stop cancels this retry delay through Pi’s abort_retry RPC.`
                  : "The retry attempt has started. Stop uses Pi’s normal agent abort for the active provider stream."}
              </p>
              <pre>{active().errorMessage}</pre>
              <Show when={active().errorTruncated}>
                <span class="truncation-note">Retry error detail truncated</span>
              </Show>
            </details>
          )}
        </Show>
        <Show when={retry()?.finished && retry()?.success === false ? retry() : undefined}>
          {(failed) => (
            <details class="pi-runtime-notice runtime-warning" open>
              <summary>Provider retry exhausted · {failed().attempt} attempts</summary>
              <pre>{failed().finalError ?? failed().errorMessage}</pre>
              <Show when={failed().finalErrorTruncated || failed().errorTruncated}>
                <span class="truncation-note">Retry error detail truncated</span>
              </Show>
            </details>
          )}
        </Show>
        <Show when={summaryRetry() && !summaryRetry()!.finished ? summaryRetry() : undefined}>
          {(active) => (
            <details class="pi-runtime-notice runtime-warning" open>
              <summary>
                Summarization retry · {active().attempt}/{active().maxAttempts}
                {active().source ? ` · ${active().source}` : ""}
              </summary>
              <p>
                Pi is retrying a summary operation after a transient provider error. Pi does not
                expose a dedicated RPC to cancel this retry loop, so Stop fails closed through the
                exact owned process if it remains active.
              </p>
              <pre>{active().errorMessage}</pre>
              <Show when={active().errorTruncated}>
                <span class="truncation-note">Summarization error detail truncated</span>
              </Show>
            </details>
          )}
        </Show>
        <Show when={extensionError()}>
          {(lastError) => (
            <details class="pi-runtime-notice runtime-warning">
              <summary>
                Last extension error · {pathLeaf(lastError().extensionPath)} · {lastError().event}
              </summary>
              <pre>{lastError().error}</pre>
              <Show when={lastError().detailTruncated}>
                <span class="truncation-note">Extension error detail truncated</span>
              </Show>
            </details>
          )}
        </Show>
      </section>
    </Show>
  );
}

export function App() {
  const [runtime, setRuntime] = createSignal<RuntimeHydration>();
  const [runtimeError, setRuntimeError] = createSignal<string>();
  const [capacity, setCapacity] = createSignal<RuntimeCapacitySnapshot>();
  const [capacityError, setCapacityError] = createSignal<string>();
  const [automation, setAutomation] = createSignal<DesktopAutomationSnapshot>();
  const [automationError, setAutomationError] = createSignal<string>();
  const [supervision, setSupervision] = createSignal<SupervisionSnapshot[]>([]);
  const [supervisionError, setSupervisionError] = createSignal<string>();
  const [capacityBusy, setCapacityBusy] = createSignal(false);
  const [liveRunLimitDraft, setLiveRunLimitDraft] = createSignal("");
  const [piProbe, setPiProbe] = createSignal<PiProbeReport>();
  const [piProbeError, setPiProbeError] = createSignal<string>();
  const [attachmentLimits, setAttachmentLimits] = createSignal<RuntimeAttachmentLimits>();
  const [deliveredEvents, setDeliveredEvents] = createSignal(0);
  const [diagnostics, setDiagnostics] = createSignal<DesktopRuntimeDiagnostics>();
  const [diagnosticsBusy, setDiagnosticsBusy] = createSignal(false);
  const [diagnosticsError, setDiagnosticsError] = createSignal<string>();
  const [mountedTimelineRows, setMountedTimelineRows] = createSignal(0);
  const [longTaskMetrics, setLongTaskMetrics] = createSignal({
    count: 0,
    maxDurationMs: 0,
    lastDurationMs: 0,
  });
  const [view, setView] = createSignal<AppView>("dashboard");
  const [selectedRunId, setSelectedRunId] = createSignal<string>();
  const [preferredProjectPath, setPreferredProjectPath] = createSignal("");
  const [projects, setProjects] = createSignal<DesktopProjectRecord[]>([]);
  const [projectRefreshKey, setProjectRefreshKey] = createSignal(0);
  const [runActionError, setRunActionError] = createSignal<string>();
  const [closingRunId, setClosingRunId] = createSignal<string>();
  const [dismissingRunId, setDismissingRunId] = createSignal<string>();
  const [openingFolderRunId, setOpeningFolderRunId] = createSignal<string>();
  const [notifications, setNotifications] = createSignal<UiNotification[]>([]);
  const [elapsedClockUnixMs, setElapsedClockUnixMs] = createSignal(Date.now());
  const [knownChangeSummaries, setKnownChangeSummaries] = createSignal<
    Record<string, { fileCount: number; truncated: boolean; changeRevision: number }>
  >({});
  const drainingRuns = new Set<string>();
  const redrainRuns = new Set<string>();
  const hydrationNeededRuns = new Set<string>();
  const composerStates = new Map<string, ComposerState>();
  let hydrationRequestSequence = 0;
  let lastAppliedHydrationRequest = 0;
  let automationRequestSequence = 0;
  let lastAppliedAutomationRequest = 0;
  let supervisionRequestSequence = 0;
  let lastAppliedSupervisionRequest = 0;
  let notificationSequence = 0;
  let elapsedClockTimer: number | undefined;
  let longTaskObserver: PerformanceObserver | undefined;
  let disposed = false;

  const applyHydration = (snapshot: RuntimeHydration, requestSequence: number) => {
    if (disposed || requestSequence < lastAppliedHydrationRequest) return;
    lastAppliedHydrationRequest = requestSequence;
    if (snapshot.schemaVersion !== RUNTIME_HYDRATION_SCHEMA_VERSION) {
      setRuntimeError(
        `Unsupported runtime hydration schema ${snapshot.schemaVersion}; this renderer requires schema ${RUNTIME_HYDRATION_SCHEMA_VERSION}. Reload the updated application instead of applying incompatible runtime state.`,
      );
      return;
    }
    setRuntime(snapshot);
    setRuntimeError(undefined);
  };

  const refreshDiagnostics = async () => {
    if (diagnosticsBusy()) return;
    setDiagnosticsBusy(true);
    setDiagnosticsError(undefined);
    try {
      const snapshot = await invokeDesktop<DesktopRuntimeDiagnostics>("runtime_diagnostics");
      if (!disposed) {
        setDiagnostics(snapshot);
        setMountedTimelineRows(document.querySelectorAll('[data-timeline-row="true"]').length);
      }
    } catch (error) {
      if (!disposed) setDiagnosticsError(String(error));
    } finally {
      if (!disposed) setDiagnosticsBusy(false);
    }
  };

  const rememberChangeSummary = (runId: string, summary: GitReviewSummary) => {
    const run = runById(runId);
    if (!run) return;
    setKnownChangeSummaries((current) => ({
      ...current,
      [runId]: {
        fileCount: summary.files.length,
        truncated: summary.truncated,
        changeRevision: run.run.changeRevision,
      },
    }));
  };

  const forgetChangeSummary = (runId: string) => {
    setKnownChangeSummaries((current) => {
      if (!(runId in current)) return current;
      const next = { ...current };
      delete next[runId];
      return next;
    });
  };

  const refreshCapacity = async () => {
    try {
      const snapshot = await invokeDesktop<RuntimeCapacitySnapshot>("runtime_capacity");
      if (!disposed) {
        setCapacity(snapshot);
        setLiveRunLimitDraft(String(snapshot.liveRunLimit));
        setCapacityError(undefined);
      }
      return snapshot;
    } catch (error) {
      if (!disposed) setCapacityError(String(error));
      return undefined;
    }
  };

  const refreshAutomation = async () => {
    const requestSequence = ++automationRequestSequence;
    try {
      const snapshot = await invokeDesktop<DesktopAutomationSnapshot>("runtime_automation_snapshot");
      if (!disposed && requestSequence >= lastAppliedAutomationRequest) {
        lastAppliedAutomationRequest = requestSequence;
        setAutomation(snapshot);
        setAutomationError(undefined);
      }
      return snapshot;
    } catch (error) {
      if (!disposed) setAutomationError(String(error));
      return undefined;
    }
  };

  const refreshAutomationExecutions = async () => {
    if (!automation()) return refreshAutomation();
    const requestSequence = ++automationRequestSequence;
    try {
      const executions = await invokeDesktop<AutomationExecutionSnapshot[]>("runtime_automation_executions");
      if (!disposed && requestSequence >= lastAppliedAutomationRequest) {
        lastAppliedAutomationRequest = requestSequence;
        setAutomation((current) => current ? { ...current, executions } : current);
        setAutomationError(undefined);
      }
      return executions;
    } catch (error) {
      if (!disposed) setAutomationError(String(error));
      return undefined;
    }
  };

  const refreshSupervision = async () => {
    const requestSequence = ++supervisionRequestSequence;
    try {
      const snapshots = await invokeDesktop<SupervisionSnapshot[]>("runtime_supervision_snapshot");
      if (!disposed && requestSequence >= lastAppliedSupervisionRequest) {
        lastAppliedSupervisionRequest = requestSequence;
        setSupervision(snapshots);
        setSupervisionError(undefined);
      }
      return snapshots;
    } catch (error) {
      if (!disposed) setSupervisionError(String(error));
      return undefined;
    }
  };

  createEffect(() => {
    if (view() === "automation") void refreshAutomation();
    if (view() === "supervision") void refreshSupervision();
  });

  const openRunFolder = async (run: RunHydration) => {
    if (openingFolderRunId()) return;
    setOpeningFolderRunId(run.run.id);
    setRunActionError(undefined);
    try {
      await invokeDesktop<void>("runtime_open_run_folder", {
        request: { runId: run.run.id },
      });
    } catch (error) {
      setRunActionError(String(error));
    } finally {
      setOpeningFolderRunId(undefined);
    }
  };

  const setLiveRunLimit = async () => {
    const next = Number.parseInt(liveRunLimitDraft(), 10);
    const current = capacity();
    if (
      capacityBusy() ||
      !current ||
      !Number.isInteger(next) ||
      next < 1 ||
      next > current.configuredMaxLiveRuns
    )
      return;
    setCapacityBusy(true);
    setCapacityError(undefined);
    try {
      const snapshot = await invokeDesktop<RuntimeCapacitySnapshot>("runtime_set_live_run_limit", {
        request: { limit: next },
      });
      if (!disposed) {
        setCapacity(snapshot);
        setLiveRunLimitDraft(String(snapshot.liveRunLimit));
      }
    } catch (error) {
      if (!disposed) setCapacityError(String(error));
    } finally {
      if (!disposed) setCapacityBusy(false);
    }
  };

  const refreshRuntimeState = async () => {
    const [snapshot] = await Promise.all([
      refreshHydration(),
      refreshCapacity(),
    ]);
    return snapshot;
  };

  const activeRunIdForExecutionRoot = (path: string) =>
    runtime()?.runs.find(
      (run) =>
        !["exited", "failed", "quarantined"].includes(run.run.process) &&
        run.run.executionRoot === path,
    )?.run.id;

  const activeRunIdForSessionPath = (path: string) =>
    runtime()?.runs.find(
      (run) =>
        !["exited", "failed", "quarantined"].includes(run.run.process) &&
        run.run.session.sessionFile === path,
    )?.run.id;

  const isExecutionRootActive = (path: string) =>
    Boolean(activeRunIdForExecutionRoot(path));

  const pendingDialogs = () =>
    runtime()?.runs.flatMap((run) =>
      (run.rpc?.pendingDialogs ?? []).map((dialog) => ({ runId: run.run.id, dialog })),
    ) ?? [];

  const sortedRuns = () =>
    [...(runtime()?.runs ?? [])].sort((left, right) => {
      const priority = runDisplayPriority(left) - runDisplayPriority(right);
      if (priority !== 0) return priority;
      // Run IDs are UUIDv7, so descending lexical order is descending creation time.
      return right.run.id.localeCompare(left.run.id);
    });

  const runById = (runId: string) => runtime()?.runs.find((run) => run.run.id === runId);

  const projectForRun = (run: RunHydration) =>
    projects().find((project) => project.id === run.run.projectId);

  const projectLabelForRun = (run: RunHydration) => {
    const project = projectForRun(run);
    return project ? pathLeaf(project.canonicalRoot) : `Project ${run.run.projectId.slice(0, 8)}`;
  };

  const selectedRun = () => {
    const id = selectedRunId();
    return id ? runById(id) : undefined;
  };

  const openRun = (runId: string) => {
    setRunActionError(undefined);
    setSelectedRunId(runId);
    setView("run");
  };

  const composerState = (run: RunHydration) => {
    let state = composerStates.get(run.run.id);
    if (!state) {
      state = createComposerState(run.run.id, run.draft?.text ?? "");
      composerStates.set(run.run.id, state);
    }
    return state;
  };

  const handleStarted = async (result: StartRunResult) => {
    await refreshRuntimeState();
    setProjectRefreshKey((value) => value + 1);
    openRun(result.runId);
    if (result.initialTaskError) {
      setNotifications((current) =>
        [
          ...current,
          {
            id: ++notificationSequence,
            runId: result.runId,
            message: `Run started, but the initial task was not sent automatically: ${result.initialTaskError}`,
            notifyType: "error" as const,
          },
        ].slice(-5),
      );
    }
  };

  const dismissRun = async (run: RunHydration) => {
    if (!isTerminalRun(run) || dismissingRunId()) return;
    setDismissingRunId(run.run.id);
    setRunActionError(undefined);
    try {
      await invokeDesktop<void>("runtime_dismiss_terminal_run", {
        request: { runId: run.run.id },
      });
      composerStates.delete(run.run.id);
      forgetChangeSummary(run.run.id);
      await Promise.all([refreshHydration(), refreshCapacity()]);
      if (selectedRunId() === run.run.id) {
        setSelectedRunId(undefined);
        setView("dashboard");
      }
    } catch (error) {
      setRunActionError(String(error));
      await refreshHydration();
    } finally {
      setDismissingRunId(undefined);
    }
  };

  const stopRunFromDashboard = async (run: RunHydration) => {
    if (run.run.process !== "ready" || !runHasStoppableActivity(run)) return;
    setRunActionError(undefined);
    try {
      await composerState(run).flush();
      const result = await invokeDesktop<RuntimeStopResult>("runtime_stop", {
        request: { runId: run.run.id },
      });
      await refreshHydration();
      if (result.quarantined) {
        setRunActionError(
          "Stop could not confirm Pi process termination. The run was quarantined for inspection.",
        );
      } else if (result.processTerminated) {
        setRunActionError(
          "Stop required terminating the Pi process. Its Pi session remains available from Recent Sessions.",
        );
      }
    } catch (error) {
      setRunActionError(String(error));
      await refreshHydration();
    }
  };

  const closeRun = async (run: RunHydration) => {
    if (!canCloseRun(run) || closingRunId()) return;
    setClosingRunId(run.run.id);
    setRunActionError(undefined);
    try {
      // Close is destructive to the renderer-owned unsynced editor value, so
      // unlike Stop it fails closed if the local draft cannot first reach the
      // backend. The backend then independently waits for durable persistence.
      await composerState(run).flush();
      const result = await invokeDesktop<RuntimeCloseResult>("runtime_close", {
        request: { runId: run.run.id },
      });
      await Promise.all([refreshHydration(), refreshCapacity()]);
      if (result.quarantined || !result.processTerminated) {
        setRunActionError(
          "Pi process termination could not be confirmed. The run was quarantined and remains visible for inspection.",
        );
        return;
      }
      if (selectedRunId() === run.run.id) {
        setSelectedRunId(undefined);
        setView("dashboard");
      }
    } catch (error) {
      setRunActionError(String(error));
      await Promise.all([refreshHydration(), refreshCapacity()]);
    } finally {
      setClosingRunId(undefined);
    }
  };

  createEffect(() => {
    if (view() !== "run") return;
    const id = selectedRunId();
    if (!id || runById(id)) return;
    setSelectedRunId(undefined);
    setView("dashboard");
  });

  createEffect(() => {
    const runs = runtime()?.runs ?? [];
    const retainedIds = new Set(runs.map((run) => run.run.id));
    setKnownChangeSummaries((current) => {
      const stale = Object.keys(current).filter((runId) => !retainedIds.has(runId));
      if (stale.length === 0) return current;
      const next = { ...current };
      for (const runId of stale) delete next[runId];
      return next;
    });

    const hasLiveRun = runs.some((run) => !isTerminalRun(run));
    if (hasLiveRun && elapsedClockTimer === undefined) {
      setElapsedClockUnixMs(Date.now());
      elapsedClockTimer = window.setInterval(() => setElapsedClockUnixMs(Date.now()), 60_000);
    } else if (!hasLiveRun && elapsedClockTimer !== undefined) {
      window.clearInterval(elapsedClockTimer);
      elapsedClockTimer = undefined;
    }
  });

  const refreshHydration = async () => {
    const requestSequence = ++hydrationRequestSequence;
    try {
      const snapshot = await invokeDesktop<RuntimeHydration>("runtime_hydrate");
      applyHydration(snapshot, requestSequence);
      return snapshot;
    } catch (error) {
      if (!disposed) setRuntimeError(String(error));
      return undefined;
    }
  };

  const recoverRun = async (runId: string) => {
    const requestSequence = ++hydrationRequestSequence;
    const snapshot = await invokeDesktop<RuntimeHydration>("runtime_recover_ui", {
      request: { runId },
    });
    applyHydration(snapshot, requestSequence);
  };

  const drainRun = async (runId: string) => {
    if (drainingRuns.has(runId)) {
      redrainRuns.add(runId);
      return;
    }
    drainingRuns.add(runId);
    let continueLater = false;
    try {
      // Bound work per browser task. `hasMore` continues the same backend
      // backlog without introducing an interval/polling loop.
      for (let batch = 0; batch < 8; batch += 1) {
        const drained = await invokeDesktop<RuntimeUiDrain>("runtime_drain", {
          request: { runId, maxEvents: 64 },
        });
        if (disposed) return;
        setDeliveredEvents((count) => count + drained.events.length);
        const incomingNotifications = drained.events.flatMap((event) =>
          event.kind === "extensionNotification" &&
          event.runId &&
          event.message &&
          event.notifyType
            ? [
                {
                  id: ++notificationSequence,
                  runId: event.runId,
                  message: event.message,
                  notifyType: event.notifyType,
                } satisfies UiNotification,
              ]
            : [],
        );
        if (incomingNotifications.length > 0) {
          setNotifications((current) => [...current, ...incomingNotifications].slice(-5));
        }
        if (needsHydration(drained.events)) hydrationNeededRuns.add(runId);
        if (drained.rehydrateRequired) {
          // Recovery is an explicit per-run transaction. Normal hydration is
          // non-destructive so it cannot erase another run's transient events.
          hydrationNeededRuns.delete(runId);
          await recoverRun(runId);
          return;
        }
        if (!drained.hasMore) return;
        if (batch === 7) continueLater = true;
      }
    } catch (error) {
      if (!disposed) setRuntimeError(`Runtime event drain failed: ${String(error)}`);
    } finally {
      drainingRuns.delete(runId);
      const wasRedirtied = redrainRuns.delete(runId);
      if (!disposed && (continueLater || wasRedirtied)) {
        queueMicrotask(() => void drainRun(runId));
      } else if (!disposed && hydrationNeededRuns.delete(runId)) {
        void refreshHydration();
      }
    }
  };

  onMount(() => {
    const unlisteners: UnlistenFn[] = [];
    void (async () => {
      try {
        // Subscribe before hydrating. If a run changes during startup, either
        // the event is observed or the subsequent authoritative hydration
        // includes it; there is no interval-based recovery dependency.
        unlisteners.push(
          await listen<RuntimeManagerSignal>("runtime://dirty", ({ payload }) => {
            void drainRun(payload.runId);
          }),
        );
        unlisteners.push(
          await listen("runtime://rehydrate", () => {
            void refreshHydration();
          }),
        );
        unlisteners.push(
          await listen<AutomationChangedSignal>("automation://changed", ({ payload }) => {
            if (view() !== "automation") return;
            if (payload === "catalog") void refreshAutomation();
            else void refreshAutomationExecutions();
          }),
        );
        unlisteners.push(
          await listen("supervision://changed", () => {
            if (view() === "supervision") void refreshSupervision();
          }),
        );
        await refreshRuntimeState();
      } catch (error) {
        if (!disposed) setRuntimeError(`Runtime listener setup failed: ${String(error)}`);
      }
    })();

    void invokeDesktop<PiProbeReport>("probe_pi_environment")
      .then((report) => {
        if (!disposed) setPiProbe(report);
      })
      .catch((error) => {
        if (!disposed) setPiProbeError(String(error));
      });

    void invokeDesktop<RuntimeAttachmentLimits>("runtime_attachment_limits").then((limits) => {
      if (!disposed) setAttachmentLimits(limits);
    });

    if (import.meta.env.DEV && typeof PerformanceObserver !== "undefined") {
      try {
        longTaskObserver = new PerformanceObserver((list) => {
          const entries = list.getEntries();
          if (entries.length === 0 || disposed) return;
          setLongTaskMetrics((current) => {
            let maxDurationMs = current.maxDurationMs;
            let lastDurationMs = current.lastDurationMs;
            for (const entry of entries) {
              maxDurationMs = Math.max(maxDurationMs, entry.duration);
              lastDurationMs = entry.duration;
            }
            return {
              count: current.count + entries.length,
              maxDurationMs,
              lastDurationMs,
            };
          });
        });
        longTaskObserver.observe({ entryTypes: ["longtask"] });
      } catch {
        longTaskObserver = undefined;
      }
    }

    onCleanup(() => {
      disposed = true;
      hydrationNeededRuns.clear();
      if (elapsedClockTimer !== undefined) window.clearInterval(elapsedClockTimer);
      longTaskObserver?.disconnect();
      for (const unlisten of unlisteners) unlisten();
    });
  });

  return (
    <>
      <a class="skip-link" href="#main-content">Skip to main content</a>
      <div class="app-shell">
        <aside class="app-sidebar" aria-label="Pi Wizard navigation">
          <header class="app-brand">
            <strong>Pi Wizard</strong>
            <span>
              <Show when={piProbe()} fallback={"Pi …"}>
                {(report) => `Pi ${report().version.display}`}
              </Show>
            </span>
          </header>

          <nav class="primary-nav" aria-label="Main views">
            <button
              type="button"
              class={view() === "dashboard" ? "active" : undefined}
              aria-current={view() === "dashboard" ? "page" : undefined}
              onClick={() => setView("dashboard")}
            >
              Dashboard
            </button>
            <button
              type="button"
              class={view() === "automation" ? "active" : undefined}
              aria-current={view() === "automation" ? "page" : undefined}
              onClick={() => setView("automation")}
            >
              Automation
            </button>
            <button
              type="button"
              class={view() === "supervision" ? "active" : undefined}
              aria-current={view() === "supervision" ? "page" : undefined}
              onClick={() => setView("supervision")}
            >
              Supervision
            </button>
            <button
              type="button"
              class={view() === "attention" ? "active" : undefined}
              aria-current={view() === "attention" ? "page" : undefined}
              onClick={() => setView("attention")}
            >
              <span>Needs attention</span>
              <strong class="nav-count">{pendingDialogs().length}</strong>
            </button>
            <button
              type="button"
              class={view() === "sessions" ? "active" : undefined}
              aria-current={view() === "sessions" ? "page" : undefined}
              onClick={() => setView("sessions")}
            >
              Recent sessions
            </button>
            <button
              type="button"
              class={view() === "launcher" ? "active" : undefined}
              aria-current={view() === "launcher" ? "page" : undefined}
              onClick={() => setView("launcher")}
            >
              New run
            </button>
          </nav>

          <section class="sidebar-runs" aria-label="Runs">
            <div class="sidebar-heading">
              <strong>Runs</strong>
              <span>{runtime()?.runs.length ?? 0}</span>
            </div>
            <For each={sortedRuns()}>
              {(run) => (
                <button
                  type="button"
                  class={`sidebar-run${selectedRunId() === run.run.id && view() === "run" ? " active" : ""}`}
                  aria-current={selectedRunId() === run.run.id && view() === "run" ? "page" : undefined}
                  onClick={() => openRun(run.run.id)}
                >
                  <strong>{runTitle(run)}</strong>
                  <span>{runStateLabel(run)}</span>
                  <small title={run.run.executionRoot}>
                    {projectLabelForRun(run)} · {run.run.executionIsolation === "git_worktree" ? "worktree" : "local"}
                  </small>
                </button>
              )}
            </For>
            <Show when={(runtime()?.runs.length ?? 0) === 0}>
              <p class="sidebar-note">No runs yet.</p>
            </Show>
          </section>

          <ProjectManager
            refreshKey={projectRefreshKey()}
            onProjects={setProjects}
            onUse={(path) => {
              setPreferredProjectPath(path);
              setView("launcher");
            }}
          />

          <details class="runtime-details">
            <summary>Runtime</summary>
            <div class="runtime-detail-grid" aria-live="polite">
              <span>Backend</span>
              <strong>{runtime() ? `ready · ${runtime()!.runs.length} runs` : "connecting"}</strong>
              <span>Events</span>
              <strong>{deliveredEvents()}</strong>
              <span>Pi</span>
              <strong>{piProbe()?.version.display ?? "probing"}</strong>
            </div>
            <div class="runtime-diagnostics-actions">
              <button type="button" disabled={diagnosticsBusy()} onClick={() => void refreshDiagnostics()}>
                {diagnosticsBusy() ? "Reading diagnostics" : "Refresh diagnostics"}
              </button>
              <small>Explicit snapshot only. No diagnostic polling or logging.</small>
            </div>
            <Show when={diagnostics()}>
              {(snapshot) => (
                <>
                  <div class="runtime-detail-grid runtime-diagnostic-summary">
                    <span>Owned processes</span>
                    <strong>{snapshot().runtime.ownedProcesses}</strong>
                    <span>Git review jobs</span>
                    <strong>{snapshot().activeGitReviewJobs}</strong>
                    <span>Session catalog jobs</span>
                    <strong>{snapshot().activeSessionCatalogJobs}</strong>
                    <span>Mounted timeline rows</span>
                    <strong>{mountedTimelineRows()}</strong>
                    <span>Runtime revision</span>
                    <strong>{snapshot().runtime.runtimeRevision}</strong>
                    <Show when={import.meta.env.DEV}>
                      <span>Renderer long tasks</span>
                      <strong>
                        {longTaskMetrics().count} · last {longTaskMetrics().lastDurationMs.toFixed(0)} ms · max {longTaskMetrics().maxDurationMs.toFixed(0)} ms
                      </strong>
                    </Show>
                  </div>
                  <details class="runtime-diagnostic-runs">
                    <summary>Per-run counters</summary>
                    <For each={snapshot().runtime.runs}>
                      {(run) => (
                        <article class="runtime-diagnostic-run">
                          <strong title={run.runId}>{run.runId.slice(0, 8)}</strong>
                          <span>
                            {run.processOwned ? "process owned" : "no process"} · state {formatBytes(run.retainedRuntimeStateBytes)}
                          </span>
                          <span>
                            RPC {run.rpcEventsPerSecond}/s · {formatBytes(run.rpcEventBytesPerSecond)}/s · {run.pendingRpcRequests} pending · {run.activeRpcCommands} active
                          </span>
                          <span>
                            UI {formatBytes(run.uiBacklogBytes)} / {run.uiBacklogFrames} frames · {run.uiCoalescedFrames} coalesced · {run.uiDroppedDisplayFrames} dropped · {run.uiDeliveredEvents} delivered
                          </span>
                          <span>
                            Live {run.assistantBlocks} blocks · {run.activeTools} tools · {run.activeDirectBash} shell · {run.pendingExtensionDialogs} dialogs
                            {run.uiRehydrateRequired ? " · rehydration required" : ""}
                          </span>
                        </article>
                      )}
                    </For>
                  </details>
                </>
              )}
            </Show>
            <Show when={diagnosticsError()}>
              {(error) => <p class="error">Diagnostics: {error()}</p>}
            </Show>
            <Show when={capacity()}>
              {(current) => (
                <form
                  class="capacity-control"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void setLiveRunLimit();
                  }}
                >
                  <label>
                    <span>Concurrent run limit</span>
                    <input
                      type="number"
                      min="1"
                      max={current().configuredMaxLiveRuns}
                      value={liveRunLimitDraft()}
                      disabled={capacityBusy()}
                      aria-label="Live Pi run admission limit"
                      onInput={(event) => setLiveRunLimitDraft(event.currentTarget.value)}
                    />
                  </label>
                  <button type="submit" disabled={capacityBusy()}>
                    {capacityBusy() ? "Applying" : "Apply"}
                  </button>
                </form>
              )}
            </Show>
            <Show when={capacityError()}>{(error) => <p class="error">{error()}</p>}</Show>
            <Show when={capacity()?.preferenceRecoveryNotice}>
              {(notice) => <p class="error">Saved run limit was reset: {notice()}</p>}
            </Show>
            <Show when={piProbeError()}>{(error) => <p class="error">Pi: {error()}</p>}</Show>
          </details>
        </aside>

        <main class="app-main" id="main-content" tabIndex={-1}>
          <Show when={notifications().length > 0}>
            <section class="notification-stack" aria-label="Extension notifications" aria-live="polite">
              <For each={notifications()}>
                {(notification) => (
                  <article class={`extension-notification notification-${notification.notifyType}`}>
                    <div>
                      <strong>
                        {runById(notification.runId)
                          ? runTitle(runById(notification.runId)!)
                          : notification.runId.slice(0, 8)}
                      </strong>
                      <span>{notification.message}</span>
                    </div>
                    <button
                      type="button"
                      aria-label="Dismiss notification"
                      onClick={() =>
                        setNotifications((current) =>
                          current.filter((item) => item.id !== notification.id),
                        )
                      }
                    >
                      ×
                    </button>
                  </article>
                )}
              </For>
            </section>
          </Show>
          <Show when={runtimeError() && !runtime()}>
            <section class="runtime-recovery" aria-label="Renderer recovery">
              <div>
                <strong>Backend connection failed</strong>
                <span>Retry rehydrates backend state without restarting Pi runs.</span>
              </div>
              <div class="runtime-recovery-actions">
                <button type="button" onClick={() => void refreshRuntimeState()}>
                  Retry
                </button>
                <button type="button" onClick={() => window.location.reload()}>
                  Reload UI
                </button>
              </div>
            </section>
          </Show>

          <Show when={runtime() ? runtimeError() : undefined}>
            {(error) => <p class="app-error">Runtime update failed: {error()}</p>}
          </Show>

          <Show when={view() === "automation"}>
            <Show when={automationError()}>
              {(message) => <p class="app-error">Automation state failed: {message()}</p>}
            </Show>
            <AutomationView
              snapshot={automation()}
              projects={projects()}
              capacity={capacity()}
              piReady={Boolean(piProbe())}
              onRefresh={refreshAutomation}
              onOpenRun={openRun}
            />
          </Show>

          <Show when={view() === "supervision"}>
            <Show when={supervisionError()}>
              {(message) => <p class="app-error">Supervision state failed: {message()}</p>}
            </Show>
            <SupervisionView
              snapshots={supervision()}
              projects={projects()}
              capacity={capacity()}
              piReady={Boolean(piProbe())}
              onRefresh={refreshSupervision}
              onOpenRun={openRun}
            />
          </Show>

          <Show when={view() === "launcher"}>
            <header class="surface-heading">
              <div>
                <h1>New run</h1>
                <p>Start Pi in a local checkout or an isolated Git worktree.</p>
              </div>
              <button type="button" onClick={() => setView("dashboard")}>Cancel</button>
            </header>
            <ProjectLauncher
              piReady={Boolean(piProbe())}
              preferredProjectPath={preferredProjectPath()}
              onStarted={handleStarted}
              onOpenRun={openRun}
              isExecutionRootActive={isExecutionRootActive}
              activeRunIdForExecutionRoot={activeRunIdForExecutionRoot}
              activeRunIdForSessionPath={activeRunIdForSessionPath}
            />
          </Show>

          <Show when={view() === "sessions"}>
            <RecentSessionsView
              projects={projects()}
              preferredProjectPath={preferredProjectPath()}
              piReady={Boolean(piProbe())}
              onStarted={handleStarted}
              onOpenRun={openRun}
              onNewRun={(path) => {
                setPreferredProjectPath(path);
                setView("launcher");
              }}
              activeRunIdForExecutionRoot={activeRunIdForExecutionRoot}
              activeRunIdForSessionPath={activeRunIdForSessionPath}
            />
          </Show>

          <Show when={view() === "attention"}>
            <NeedsAttentionView
              runs={runtime()?.runs ?? []}
              onOpenRun={openRun}
              onResolved={refreshHydration}
              projectLabel={(projectId) => {
                const project = projects().find((candidate) => candidate.id === projectId);
                return project ? pathLeaf(project.canonicalRoot) : `Project ${projectId.slice(0, 8)}`;
              }}
            />
          </Show>

          <Show when={view() === "run"}>
            <Show when={selectedRun()} fallback={<p class="empty-state">That run is no longer retained.</p>}>
              {(run) => (
                <section class="active-run-surface" aria-label={`Run ${runTitle(run())}`}>
                  <header class="surface-heading run-surface-heading">
                    <div>
                      <h1>{runTitle(run())}</h1>
                      <p title={run().run.executionRoot}>
                        {projectLabelForRun(run())} · {run().run.executionIsolation === "git_worktree" ? "Git-isolated worktree" : "Local checkout"} · {run().run.executionRoot}
                      </p>
                      <div class="run-identity-strip" aria-label="Run identity">
                        <span class={`run-state state-${runStateLabel(run()).replaceAll(" ", "-")}`}>
                          {runStateLabel(run())}
                        </span>
                        <span>{runModelLabel(run())}</span>
                        <span>{runThinkingLabel(run())}</span>
                        <span title={`Started ${new Date(run().run.startedUnixMs).toLocaleString()}`}>
                          {runElapsedLabel(run(), elapsedClockUnixMs())}
                        </span>
                        <Show when={run().run.worktree}>
                          {(worktree) => <span>Branch {worktree().branch}</span>}
                        </Show>
                        <Show when={runQueuedCount(run()) > 0}>
                          <span>{runQueuedCount(run())} queued</span>
                        </Show>
                      </div>
                    </div>
                    <div class="run-surface-actions">
                      <button
                        type="button"
                        disabled={Boolean(openingFolderRunId())}
                        onClick={() => void openRunFolder(run())}
                      >
                        {openingFolderRunId() === run().run.id ? "Opening" : "Open folder"}
                      </button>
                      <Show
                        when={isTerminalRun(run())}
                        fallback={
                          <button
                            type="button"
                            disabled={!canCloseRun(run()) || Boolean(closingRunId())}
                            onClick={() => void closeRun(run())}
                          >
                            {closingRunId() === run().run.id ? "Closing" : "Close run"}
                          </button>
                        }
                      >
                        <button
                          type="button"
                          disabled={Boolean(dismissingRunId())}
                          onClick={() => void dismissRun(run())}
                        >
                          {dismissingRunId() === run().run.id ? "Dismissing" : "Dismiss"}
                        </button>
                      </Show>
                      <button type="button" onClick={() => setView("dashboard")}>Dashboard</button>
                    </div>
                  </header>

                  <Show when={runActionError()}>
                    {(error) => <p class="app-error">Run action failed: {error()}</p>}
                  </Show>

                  <Show when={run().run.failure}>
                    {(failure) => (
                      <section class="run-failure-panel" role="alert" aria-label="Run failure">
                        <strong>
                          {run().run.process === "quarantined"
                            ? "Termination uncertain"
                            : `Run failed · ${failure().kind.replaceAll("_", " ")}`}
                        </strong>
                        <span>{failure().detail}</span>
                        <small>
                          {run().run.exitCode == null ? "No process exit code" : `Process exit code ${run().run.exitCode}`}
                          {failure().detailTruncated ? " · detail truncated by backend limit" : ""}
                        </small>
                      </section>
                    )}
                  </Show>

                  <Show when={(run().rpc?.pendingDialogs.length ?? 0) > 0}>
                    <section class="attention" aria-label="Needs attention">
                      <h2>Needs Attention</h2>
                      <For each={run().rpc?.pendingDialogs ?? []}>
                        {(dialog) => (
                          <ExtensionDialogCard
                            runId={run().run.id}
                            dialog={dialog}
                            onResolved={refreshHydration}
                          />
                        )}
                      </For>
                    </section>
                  </Show>

                  <PiRuntimeNoticePanel rpc={run().rpc} />

                  <ExtensionUiPanel snapshot={run().rpc?.extensionUi} />

                  <ComposerCard
                    run={run()}
                    state={composerState(run())}
                    attachmentLimits={attachmentLimits()}
                    onResolved={refreshHydration}
                    onReviewSummary={rememberChangeSummary}
                  />
                </section>
              )}
            </Show>
          </Show>

          <Show when={view() === "dashboard"}>
            <section class="dashboard" aria-label="Run dashboard">
              <header class="surface-heading">
                <div>
                  <h1>Runs</h1>
                  <p>Open a session for details. Runs keep working when you switch views.</p>
                </div>
                <button type="button" onClick={() => setView("launcher")}>New run</button>
              </header>

              <Show when={(runtime()?.runs.length ?? 0) > 0} fallback={
                <div class="empty-state">
                  <strong>No runs yet</strong>
                  <span>Start one and give Pi the task up front.</span>
                  <button type="button" onClick={() => setView("launcher")}>Start a run</button>
                </div>
              }>
                <div class="run-grid">
                  <For each={sortedRuns()}>
                    {(run) => (
                      <article class="run-card">
                        <div>
                          <strong>{runTitle(run)}</strong>
                          <span class={`run-state state-${runStateLabel(run).replaceAll(" ", "-")}`}>
                            {runStateLabel(run)}
                          </span>
                        </div>
                        <span title={run.run.executionRoot} class="run-path">
                          {projectLabelForRun(run)} · {run.run.executionIsolation === "git_worktree" ? "Git-isolated worktree" : "Local checkout"} · {run.run.executionRoot}
                        </span>
                        <Show when={run.run.worktree}>
                          {(worktree) => <small>{worktree().branch} · {worktree().baseCommit.slice(0, 12)}</small>}
                        </Show>
                        <small>{runModelLabel(run)} · {runThinkingLabel(run)}</small>
                        <small class="run-activity">{runActivityLabel(run)}</small>
                        <div class="run-card-meta">
                          <small title={`Started ${new Date(run.run.startedUnixMs).toLocaleString()}`}>
                            {runElapsedLabel(run, elapsedClockUnixMs())}
                          </small>
                          <Show when={knownChangeSummaries()[run.run.id]}>
                            {(known) => (
                              <small
                                class={known().changeRevision === run.run.changeRevision ? undefined : "review-stale"}
                                title={
                                  known().changeRevision === run.run.changeRevision
                                    ? "Last explicitly loaded Git review summary"
                                    : "Pi completed tool or shell activity after this Git review; open Changes to refresh"
                                }
                              >
                                {known().changeRevision === run.run.changeRevision ? "Last review" : "Review stale"} · {known().fileCount} changed file{known().fileCount === 1 ? "" : "s"}
                                {known().truncated ? "+" : ""}
                              </small>
                            )}
                          </Show>
                        </div>
                        <Show when={(run.rpc?.pendingDialogs.length ?? 0) > 0}>
                          <strong class="attention-count">{run.rpc!.pendingDialogs.length} request{run.rpc!.pendingDialogs.length === 1 ? "" : "s"} need attention</strong>
                        </Show>
                        <div class="run-card-actions">
                          <button type="button" onClick={() => openRun(run.run.id)}>Open</button>
                          <button
                            type="button"
                            disabled={Boolean(openingFolderRunId())}
                            onClick={() => void openRunFolder(run)}
                          >
                            {openingFolderRunId() === run.run.id ? "Opening" : "Folder"}
                          </button>
                          <Show
                            when={
                              run.run.process === "ready" &&
                              runHasStoppableActivity(run)
                            }
                          >
                            <button
                              type="button"
                              onClick={() => void stopRunFromDashboard(run)}
                            >
                              Stop
                            </button>
                          </Show>
                          <Show when={canCloseRun(run)}>
                            <button
                              type="button"
                              disabled={Boolean(closingRunId())}
                              onClick={() => void closeRun(run)}
                            >
                              {closingRunId() === run.run.id ? "Closing" : "Close"}
                            </button>
                          </Show>
                          <Show when={isTerminalRun(run)}>
                            <button
                              type="button"
                              disabled={Boolean(dismissingRunId())}
                              onClick={() => void dismissRun(run)}
                            >
                              {dismissingRunId() === run.run.id ? "Dismissing" : "Dismiss"}
                            </button>
                          </Show>
                        </div>
                      </article>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={runActionError()}>
                {(error) => <p class="error">Run action failed: {error()}</p>}
              </Show>
            </section>

            <Show when={pendingDialogs().length > 0}>
              <section class="attention dashboard-attention" aria-label="Needs attention">
                <h2>Needs Attention</h2>
                <For each={pendingDialogs()}>
                  {(pending) => (
                    <article class="attention-summary">
                      <div>
                        <strong>{runById(pending.runId) ? runTitle(runById(pending.runId)!) : pending.runId.slice(0, 8)}</strong>
                        <span>{pending.dialog.request.kind.title}</span>
                      </div>
                      <button type="button" onClick={() => openRun(pending.runId)}>Open</button>
                    </article>
                  )}
                </For>
              </section>
            </Show>
          </Show>
        </main>
      </div>
    </>
  );
}
