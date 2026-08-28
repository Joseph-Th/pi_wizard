import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";

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
type ExecutionMode = "local" | "worktree";
type ExecutionIsolation = "local_checkout" | "git_worktree";
type AppView = "dashboard" | "automation" | "attention" | "sessions" | "launcher" | "run";
type AutomationExecutionStatus =
  | "starting"
  | "running"
  | "completed"
  | "completed_with_errors"
  | "cancelled"
  | "failed";
type AutomationStepStatus =
  | "queued"
  | "starting"
  | "working"
  | "needs_attention"
  | "completed"
  | "failed"
  | "cancelled";

interface AutomationChain {
  id: string;
  name: string;
  prompts: string[];
}

interface AutomationStepSnapshot {
  index: number;
  promptPreview: string;
  promptTruncated: boolean;
  runId: string | null;
  status: AutomationStepStatus;
  error: string | null;
}

interface AutomationExecutionSnapshot {
  id: string;
  chainId: string;
  chainName: string;
  projectId: string;
  concurrency: number;
  worktrees: boolean;
  supervisorEnabled: boolean;
  supervisorRunId: string | null;
  supervisorError: string | null;
  supervisorCycles: number;
  error: string | null;
  status: AutomationExecutionStatus;
  steps: AutomationStepSnapshot[];
}

interface DesktopAutomationSnapshot {
  catalog: {
    chains: AutomationChain[];
    recoveryNotice: string | null;
  };
  executions: AutomationExecutionSnapshot[];
}

type AutomationChangedSignal = "catalog" | "executions";

interface StartRunResult {
  runId: string;
  initialTaskSubmitted: boolean;
  initialTaskError: string | null;
}

interface ProjectResourcePreflight {
  piSettings: boolean;
  extensions: boolean;
  skills: boolean;
  prompts: boolean;
  themes: boolean;
  systemPrompt: boolean;
  appendSystemPrompt: boolean;
  ancestorAgentSkills: boolean;
}

function pathLeaf(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function projectResourceLabels(preflight: ProjectResourcePreflight): string[] {
  const labels: string[] = [];
  if (preflight.piSettings) labels.push(".pi/settings.json");
  if (preflight.extensions) labels.push(".pi/extensions");
  if (preflight.skills) labels.push(".pi/skills");
  if (preflight.prompts) labels.push(".pi/prompts");
  if (preflight.themes) labels.push(".pi/themes");
  if (preflight.systemPrompt) labels.push(".pi/SYSTEM.md");
  if (preflight.appendSystemPrompt) labels.push(".pi/APPEND_SYSTEM.md");
  if (preflight.ancestorAgentSkills) labels.push("project/ancestor .agents/skills");
  return labels;
}

function dialogTimeoutLabel(dialog: PendingExtensionDialog): string {
  const remaining = dialog.remainingTimeoutMs;
  if (remaining === null) return "No Pi-side timeout";
  if (remaining < 1_000) return "Timed request · <1s remaining at last sync";
  if (remaining < 60_000) return `Timed request · ~${Math.ceil(remaining / 1_000)}s remaining at last sync`;
  return `Timed request · ~${Math.ceil(remaining / 60_000)}m remaining at last sync`;
}

interface ProjectLaunchOptions {
  currentModel: ModelSummary | null;
  currentThinkingLevel: ThinkingLevel;
  models: ModelSummary[];
  thinkingLevels: ThinkingLevel[];
  clearQueueSupported: boolean;
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

async function invokeDesktop<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

async function pickDirectory(defaultPath?: string): Promise<string | undefined> {
  const selected = await invokeDesktop<string | null>("runtime_pick_directory", {
    request: { defaultPath: defaultPath?.trim() || null },
  });
  return selected ?? undefined;
}

function suggestedWorktreeIdentity(
  repositoryRoot: string,
  task: string,
): { branch: string; path: string } {
  const normalized = repositoryRoot
    .replace(/^\\\\\?\\UNC\\/i, "\\\\")
    .replace(/^\\\\\?\\/, "")
    .replace(/[\\/]+$/, "");
  const separator = normalized.includes("\\") ? "\\" : "/";
  const parts = normalized.split(/[\\/]/);
  const repositoryName = parts.pop() || "repo";
  const parent = parts.join(separator);
  const taskSlug = task
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 28);
  const leaf = `${taskSlug || "task"}-${Date.now().toString(36)}`;
  const branch = `pi-wizard/${leaf}`;
  const worktreeDirectory = `${repositoryName}-worktrees`;
  return {
    branch,
    path: parent
      ? `${parent}${separator}${worktreeDirectory}${separator}${leaf}`
      : `${worktreeDirectory}${separator}${leaf}`,
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

function ExtensionDialogCard(props: {
  runId: string;
  dialog: PendingExtensionDialog;
  onResolved: () => Promise<unknown>;
}) {
  const [value, setValue] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let currentRequestId: string | undefined;

  createEffect(() => {
    const request = props.dialog.request;
    if (request.id === currentRequestId) return;
    currentRequestId = request.id;
    setValue(request.kind.kind === "editor" ? (request.kind.prefill ?? "") : "");
    setSubmitting(false);
    setError(undefined);
  });

  const respond = async (response: ExtensionDialogResponse) => {
    if (submitting()) return;
    setSubmitting(true);
    setError(undefined);
    try {
      await invokeDesktop<void>("runtime_respond_extension_ui", {
        request: { runId: props.runId, response },
      });
      await props.onResolved();
    } catch (responseError) {
      setSubmitting(false);
      setError(String(responseError));
      // A timeout can make a previously rendered request stale. Refreshing
      // authoritative state removes the zombie prompt instead of leaving it
      // actionable because the write was rejected.
      await props.onResolved();
    }
  };

  const request = () => props.dialog.request;
  const controls = () => {
    const kind = request().kind;
    switch (kind.kind) {
      case "select":
        return (
          <div class="dialog-options">
            <For each={kind.options}>
              {(option) => (
                <button
                  type="button"
                  disabled={submitting()}
                  onClick={() => void respond({ kind: "value", id: request().id, value: option })}
                >
                  {option}
                </button>
              )}
            </For>
          </div>
        );
      case "confirm":
        return (
          <>
            <p>{kind.message}</p>
            <div class="dialog-actions">
              <button
                type="button"
                disabled={submitting()}
                onClick={() =>
                  void respond({ kind: "confirmation", id: request().id, confirmed: true })
                }
              >
                Confirm
              </button>
              <button
                type="button"
                disabled={submitting()}
                onClick={() =>
                  void respond({ kind: "confirmation", id: request().id, confirmed: false })
                }
              >
                Decline
              </button>
            </div>
          </>
        );
      case "input":
        return (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void respond({ kind: "value", id: request().id, value: value() });
            }}
          >
            <input
              value={value()}
              placeholder={kind.placeholder ?? ""}
              aria-labelledby={`dialog-${request().id}`}
              disabled={submitting()}
              onInput={(event) => setValue(event.currentTarget.value)}
            />
            <button type="submit" disabled={submitting()}>
              Submit
            </button>
          </form>
        );
      case "editor":
        return (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void respond({ kind: "value", id: request().id, value: value() });
            }}
          >
            <textarea
              value={value()}
              aria-labelledby={`dialog-${request().id}`}
              disabled={submitting()}
              onInput={(event) => setValue(event.currentTarget.value)}
            />
            <button type="submit" disabled={submitting()}>
              Submit
            </button>
          </form>
        );
    }
  };

  return (
    <article class="dialog-card" aria-labelledby={`dialog-${request().id}`}>
      <header>
        <strong id={`dialog-${request().id}`}>{request().kind.title}</strong>
        <span>{dialogTimeoutLabel(props.dialog)}</span>
      </header>
      {controls()}
      <div class="dialog-actions">
        <button
          type="button"
          disabled={submitting()}
          onClick={() => void respond({ kind: "cancelled", id: request().id })}
        >
          Cancel
        </button>
      </div>
      <Show when={error()}>{(message) => <p class="error">{message()}</p>}</Show>
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

function ProjectManager(props: {
  refreshKey: number;
  onUse: (path: string) => void;
  onProjects: (projects: DesktopProjectRecord[]) => void;
}) {
  const [projects, setProjects] = createSignal<DesktopProjectRecord[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [busyId, setBusyId] = createSignal<string>();
  const [error, setError] = createSignal<string>();

  const load = async () => {
    if (loading()) return;
    setLoading(true);
    setError(undefined);
    try {
      const next = await invokeDesktop<DesktopProjectRecord[]>("runtime_list_projects");
      setProjects(next);
      props.onProjects(next);
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  };

  createEffect(() => {
    props.refreshKey;
    void load();
  });

  const relocate = async (project: DesktopProjectRecord) => {
    if (busyId()) return;
    let next: string | undefined;
    try {
      next = (await pickDirectory(project.canonicalRoot))?.trim();
    } catch (pickError) {
      setError(`Could not choose replacement folder: ${String(pickError)}`);
      return;
    }
    if (!next || next === project.canonicalRoot) return;
    setBusyId(project.id);
    setError(undefined);
    try {
      await invokeDesktop<DesktopProjectRecord>("runtime_relocate_project", {
        request: { id: project.id, newRoot: next },
      });
      await load();
    } catch (relocateError) {
      setError(String(relocateError));
    } finally {
      setBusyId(undefined);
    }
  };

  const remove = async (project: DesktopProjectRecord) => {
    if (busyId()) return;
    if (
      !window.confirm(
        `Remove ${project.canonicalRoot} from Pi Wizard? This only removes the app registration; it does not delete the folder or Git repository.`,
      )
    )
      return;
    setBusyId(project.id);
    setError(undefined);
    try {
      await invokeDesktop<void>("runtime_remove_project", { request: { id: project.id } });
      await load();
    } catch (removeError) {
      setError(String(removeError));
    } finally {
      setBusyId(undefined);
    }
  };

  return (
    <section class="project-manager" aria-label="Projects">
      <div class="sidebar-heading">
        <strong>Projects</strong>
        <button type="button" disabled={loading()} onClick={() => void load()}>
          {loading() ? "…" : "Refresh"}
        </button>
      </div>
      <For each={projects()}>
        {(project) => (
          <article class={`project-row project-${project.status}`}>
            <button
              type="button"
              class="project-use"
              disabled={project.status !== "present" || Boolean(busyId())}
              title={project.canonicalRoot}
              onClick={() => props.onUse(project.canonicalRoot)}
            >
              <strong>{pathLeaf(project.canonicalRoot)}</strong>
              <span>{project.status === "present" ? project.canonicalRoot : `Detached · ${project.canonicalRoot}`}</span>
            </button>
            <Show when={project.detail}>
              {(detail) => <small>{detail()}</small>}
            </Show>
            <div class="project-row-actions">
              <button type="button" disabled={Boolean(busyId())} onClick={() => void relocate(project)}>
                Relocate
              </button>
              <button type="button" disabled={Boolean(busyId())} onClick={() => void remove(project)}>
                Remove
              </button>
            </div>
          </article>
        )}
      </For>
      <Show when={!loading() && projects().length === 0}>
        <p class="sidebar-note">No registered projects yet.</p>
      </Show>
      <Show when={error()}>{(message) => <p class="error">Projects: {message()}</p>}</Show>
    </section>
  );
}

function SessionCatalogBrowser(props: {
  projectPath: string;
  piReady: boolean;
  projectTrust: ProjectTrustPolicy;
  contextFiles: ContextFilesPolicy;
  extensionDiscovery: ExtensionDiscoveryPolicy;
  onStarted: (result: StartRunResult) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
  activeRunIdForExecutionRoot: (path: string) => string | undefined;
  activeRunIdForSessionPath: (path: string) => string | undefined;
}) {
  const [sessionQuery, setSessionQuery] = createSignal("");
  const [sessionPage, setSessionPage] = createSignal<SessionCatalogPage>();
  const [sessionCursor, setSessionCursor] = createSignal<SessionCatalogCursor | null>(null);
  const [sessionBackCursors, setSessionBackCursors] = createSignal<
    Array<SessionCatalogCursor | null>
  >([]);
  const [sessionError, setSessionError] = createSignal<string>();
  const [sessionPagingNeedsRestart, setSessionPagingNeedsRestart] = createSignal(false);
  const [loadingSessions, setLoadingSessions] = createSignal(false);
  const [resumingPath, setResumingPath] = createSignal<string>();
  let catalogProjectPath = "";
  let catalogRequestSequence = 0;

  createEffect(() => {
    const path = props.projectPath.trim();
    if (path === catalogProjectPath) return;
    catalogProjectPath = path;
    catalogRequestSequence += 1;
    setLoadingSessions(false);
    setSessionPage(undefined);
    setSessionCursor(null);
    setSessionBackCursors([]);
    setSessionError(undefined);
    setSessionPagingNeedsRestart(false);
  });

  const loadSessionPage = async (
    cursor: SessionCatalogCursor | null,
    backCursors: Array<SessionCatalogCursor | null>,
  ) => {
    const path = props.projectPath.trim();
    if (!path || loadingSessions()) return;
    const requestSequence = ++catalogRequestSequence;
    setLoadingSessions(true);
    setSessionError(undefined);
    if (!cursor) setSessionPagingNeedsRestart(false);
    try {
      const page = await invokeDesktop<SessionCatalogPage>("runtime_list_project_sessions", {
        request: {
          projectPath: path,
          query: sessionQuery().trim() || null,
          cursor,
        },
      });
      if (requestSequence !== catalogRequestSequence || props.projectPath.trim() !== path) return;
      setSessionPage(page);
      setSessionCursor(cursor);
      setSessionBackCursors(backCursors);
      setSessionPagingNeedsRestart(false);
    } catch (catalogError) {
      if (requestSequence !== catalogRequestSequence || props.projectPath.trim() !== path) return;
      setSessionError(String(catalogError));
      if (cursor) setSessionPagingNeedsRestart(true);
    } finally {
      if (requestSequence === catalogRequestSequence) setLoadingSessions(false);
    }
  };

  const findSessions = async () => loadSessionPage(null, []);

  const nextSessionPage = async () => {
    const next = sessionPage()?.nextCursor;
    if (!next || loadingSessions()) return;
    await loadSessionPage(next, [...sessionBackCursors(), sessionCursor()]);
  };

  const previousSessionPage = async () => {
    const back = sessionBackCursors();
    if (back.length === 0 || loadingSessions()) return;
    const previous = back.at(-1) ?? null;
    await loadSessionPage(previous, back.slice(0, -1));
  };

  const resume = async (session: SessionCatalogEntry) => {
    const path = props.projectPath.trim();
    if (!path || resumingPath()) return;
    const activeSessionRunId = props.activeRunIdForSessionPath(session.path);
    if (activeSessionRunId) {
      props.onOpenRun(activeSessionRunId);
      return;
    }
    const checkoutOwnerRunId = props.activeRunIdForExecutionRoot(path);
    if (checkoutOwnerRunId) {
      setSessionError(
        "This checkout is already owned by a live run. Open that run and close it before resuming another session here.",
      );
      return;
    }
    setResumingPath(session.path);
    setSessionError(undefined);
    try {
      const runId = await invokeDesktop<string>("runtime_resume_project_session", {
        request: {
          projectPath: path,
          projectTrust: props.projectTrust,
          contextFiles: props.contextFiles,
          extensionDiscovery: props.extensionDiscovery,
          sessionPath: session.path,
        },
      });
      await props.onStarted({
        runId,
        initialTaskSubmitted: false,
        initialTaskError: null,
      });
    } catch (resumeError) {
      setSessionError(String(resumeError));
    } finally {
      setResumingPath(undefined);
    }
  };

  return (
    <div class="session-browser">
      <div class="session-search">
        <input
          value={sessionQuery()}
          disabled={loadingSessions() || !props.piReady || props.projectPath.trim().length === 0}
          placeholder="Search session name, prompt, or ID"
          aria-label="Search Pi sessions"
          onInput={(event) => {
            setSessionQuery(event.currentTarget.value);
            setSessionPage(undefined);
            setSessionCursor(null);
            setSessionBackCursors([]);
            setSessionError(undefined);
            setSessionPagingNeedsRestart(false);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void findSessions();
            }
          }}
        />
        <button
          type="button"
          disabled={loadingSessions() || !props.piReady || props.projectPath.trim().length === 0}
          onClick={() => void findSessions()}
        >
          {loadingSessions() ? "Searching" : "Find sessions"}
        </button>
      </div>
      <Show when={sessionPage()}>
        {(page) => (
          <div class="session-results">
            <div class="session-results-meta">
              <span>
                {page().sessions.length} session{page().sessions.length === 1 ? "" : "s"} · {page().directorySource}
                {` · scanned ${page().scannedFiles.toLocaleString()} detailed previews from ${page().candidateFiles.toLocaleString()} session files`}
              </span>
              <Show when={page().nextCursor}>
                <span>More older candidates are available.</span>
              </Show>
            </div>
            <For each={page().sessions}>
              {(session) => {
                const activeSessionRunId = () => props.activeRunIdForSessionPath(session.path);
                const checkoutOwnerRunId = () =>
                  props.activeRunIdForExecutionRoot(props.projectPath.trim());
                const owningRunId = () => activeSessionRunId() ?? checkoutOwnerRunId();
                return (
                  <article class="session-row">
                    <div>
                      <strong>{session.name ?? session.firstMessage ?? session.id}</strong>
                      <span>
                        {new Date(session.modifiedUnixMs).toLocaleString()} · {session.id.slice(0, 12)}
                        {session.previewIncomplete ? " · bounded preview" : ""}
                      </span>
                    </div>
                    <button
                      type="button"
                      disabled={Boolean(resumingPath())}
                      onClick={() => {
                        const runId = owningRunId();
                        if (runId) {
                          props.onOpenRun(runId);
                        } else {
                          void resume(session);
                        }
                      }}
                    >
                      {activeSessionRunId()
                        ? "Open"
                        : checkoutOwnerRunId()
                          ? "Open live run"
                          : resumingPath() === session.path
                            ? "Resuming"
                            : "Resume"}
                    </button>
                  </article>
                );
              }}
            </For>
            <Show when={page().sessions.length === 0}>
              <p class="launch-note">
                No matching Pi sessions were found on this bounded page.
                {page().nextCursor ? " Continue to older candidates to keep searching." : ""}
              </p>
            </Show>
            <div class="session-page-actions">
              <button
                type="button"
                disabled={loadingSessions() || sessionBackCursors().length === 0}
                onClick={() => void previousSessionPage()}
              >
                Previous
              </button>
              <span>Page {sessionBackCursors().length + 1}</span>
              <button
                type="button"
                disabled={loadingSessions() || !page().nextCursor}
                onClick={() => void nextSessionPage()}
              >
                Next older
              </button>
            </div>
          </div>
        )}
      </Show>
      <Show when={sessionError()}>
        {(message) => (
          <div class="session-lookup-error" role="alert">
            <p class="error">Session lookup failed: {message()}</p>
            <Show when={sessionPagingNeedsRestart()}>
              <button type="button" disabled={loadingSessions()} onClick={() => void findSessions()}>
                Restart from newest
              </button>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}

function ProjectLauncher(props: {
  piReady: boolean;
  preferredProjectPath: string;
  onStarted: (result: StartRunResult) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
  isExecutionRootActive: (path: string) => boolean;
  activeRunIdForExecutionRoot: (path: string) => string | undefined;
  activeRunIdForSessionPath: (path: string) => string | undefined;
}) {
  const [projectPath, setProjectPath] = createSignal("");
  const [initialTask, setInitialTask] = createSignal("");
  const [projectTrust, setProjectTrust] = createSignal<ProjectTrustPolicy>("inherit");
  const [contextFiles, setContextFiles] = createSignal<ContextFilesPolicy>("inherit");
  const [extensionDiscovery, setExtensionDiscovery] =
    createSignal<ExtensionDiscoveryPolicy>("inherit");
  const [launchOptions, setLaunchOptions] = createSignal<ProjectLaunchOptions>();
  const [launchOptionsLoading, setLaunchOptionsLoading] = createSignal(false);
  const [launchOptionsError, setLaunchOptionsError] = createSignal<string>();
  const [launchModelKey, setLaunchModelKey] = createSignal("");
  const [launchThinking, setLaunchThinking] = createSignal<ThinkingLevel | "">("");
  const [resourcePreflight, setResourcePreflight] = createSignal<ProjectResourcePreflight>();
  const [resourcePreflightPath, setResourcePreflightPath] = createSignal("");
  const [resourcePreflightLoading, setResourcePreflightLoading] = createSignal(false);
  const [resourcePreflightError, setResourcePreflightError] = createSignal<string>();
  const [executionMode, setExecutionMode] = createSignal<ExecutionMode>("local");
  const [starting, setStarting] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [worktreeBase, setWorktreeBase] = createSignal<WorktreeBaseSnapshot>();
  const [worktreeBranch, setWorktreeBranch] = createSignal("");
  const [worktreePath, setWorktreePath] = createSignal("");
  const [probingWorktree, setProbingWorktree] = createSignal(false);
  const [worktreeError, setWorktreeError] = createSignal<string>();
  const [recoveryPage, setRecoveryPage] = createSignal<WorktreeRecoveryPage>();
  const [recoveryInspections, setRecoveryInspections] = createSignal<
    Record<string, WorktreeRecoveryProbe>
  >({});
  const [loadingRecoveries, setLoadingRecoveries] = createSignal(false);
  const [recoveryError, setRecoveryError] = createSignal<string>();
  const [reconcilingRecovery, setReconcilingRecovery] = createSignal<string>();
  const [startingRecovery, setStartingRecovery] = createSignal<string>();
  const [cleaningRecovery, setCleaningRecovery] = createSignal<string>();

  const localCheckoutActive = () =>
    executionMode() === "local" &&
    projectPath().trim().length > 0 &&
    props.isExecutionRootActive(projectPath().trim());

  const resetLaunchOptions = () => {
    setLaunchOptions(undefined);
    setLaunchOptionsError(undefined);
    setLaunchModelKey("");
    setLaunchThinking("");
  };

  const resetResourcePreflight = () => {
    setResourcePreflight(undefined);
    setResourcePreflightPath("");
    setResourcePreflightError(undefined);
  };

  const probeProjectResources = async () => {
    const path = projectPath().trim();
    if (!path || resourcePreflightLoading()) return;
    setResourcePreflightLoading(true);
    setResourcePreflightError(undefined);
    try {
      const preflight = await invokeDesktop<ProjectResourcePreflight>(
        "runtime_probe_project_resources",
        { request: { projectPath: path } },
      );
      if (projectPath().trim() !== path) return;
      setResourcePreflight(preflight);
      setResourcePreflightPath(path);
    } catch (preflightError) {
      if (projectPath().trim() === path) setResourcePreflightError(String(preflightError));
    } finally {
      if (projectPath().trim() === path) setResourcePreflightLoading(false);
    }
  };

  const selectedLaunchModel = (): { provider: string; id: string } | undefined => {
    const value = launchModelKey();
    if (!value) return undefined;
    try {
      const parsed = JSON.parse(value);
      if (
        Array.isArray(parsed) &&
        parsed.length === 2 &&
        typeof parsed[0] === "string" &&
        typeof parsed[1] === "string"
      ) {
        return { provider: parsed[0], id: parsed[1] };
      }
    } catch {
      // The value comes from our own option list. Treat malformed state as no override.
    }
    return undefined;
  };

  createEffect(() => {
    const preferred = props.preferredProjectPath.trim();
    if (!preferred || preferred === projectPath()) return;
    setProjectPath(preferred);
    setWorktreeBase(undefined);
    setWorktreeError(undefined);
    resetLaunchOptions();
  });

  const loadRecoveries = async () => {
    if (loadingRecoveries()) return;
    setLoadingRecoveries(true);
    setRecoveryError(undefined);
    try {
      const page = await invokeDesktop<WorktreeRecoveryPage>("runtime_list_worktree_recoveries");
      setRecoveryPage(page);
    } catch (loadError) {
      setRecoveryError(String(loadError));
    } finally {
      setLoadingRecoveries(false);
    }
  };

  const cleanupRecovery = async (record: WorktreeRecoveryRecord) => {
    if (
      cleaningRecovery() ||
      reconcilingRecovery() ||
      startingRecovery() ||
      !record.created ||
      recoveryInspections()[record.id]?.kind !== "exact"
    )
      return;
    const confirmed = window.confirm(
      `Remove the unused worktree at ${record.created.worktreeRoot} and delete branch ${record.branch}? Pi Wizard will refuse if the worktree is dirty, has task commits, or is used by a live run.`,
    );
    if (!confirmed) return;
    setCleaningRecovery(record.id);
    setRecoveryError(undefined);
    try {
      const result = await invokeDesktop<WorktreeCleanupResult>(
        "runtime_cleanup_worktree_recovery",
        { request: { id: record.id } },
      );
      if (result.kind === "partial") {
        setRecoveryInspections((current) => ({ ...current, [record.id]: result }));
        setRecoveryError(
          `Cleanup stopped with recoverable Git state retained: ${result.detail}`,
        );
      } else {
        setRecoveryInspections((current) => {
          const next = { ...current };
          delete next[record.id];
          return next;
        });
      }
      await loadRecoveries();
    } catch (cleanupError) {
      setRecoveryError(String(cleanupError));
      await loadRecoveries();
    } finally {
      setCleaningRecovery(undefined);
    }
  };

  onMount(() => void loadRecoveries());

  const probeLaunchOptions = async (model?: { provider: string; id: string }) => {
    const path = projectPath().trim();
    if (!path || launchOptionsLoading() || !props.piReady) return;
    setLaunchOptionsLoading(true);
    setLaunchOptionsError(undefined);
    try {
      const options = await invokeDesktop<ProjectLaunchOptions>(
        "runtime_probe_project_launch_options",
        {
          request: {
            projectPath: path,
            projectTrust: projectTrust(),
            contextFiles: contextFiles(),
            provider: model?.provider ?? null,
            model: model?.id ?? null,
          },
        },
      );
      setLaunchOptions(options);
    } catch (probeError) {
      if (model) {
        setLaunchOptions((current) =>
          current ? { ...current, thinkingLevels: [] } : current,
        );
      }
      setLaunchOptionsError(String(probeError));
    } finally {
      setLaunchOptionsLoading(false);
    }
  };

  const start = async () => {
    const path = projectPath().trim();
    if (!path || starting()) return;
    if (executionMode() === "local" && props.isExecutionRootActive(path)) {
      setError(
        "This checkout is already owned by a live run. Open that run or use a Git worktree for parallel work.",
      );
      return;
    }
    if (executionMode() === "worktree") {
      if (!worktreeBase()) {
        setError("Inspect the exact Git base before creating a worktree");
        return;
      }
      if (!worktreeBranch().trim() || !worktreePath().trim()) {
        setError("New branch and absolute worktree path are required");
        return;
      }
    }
    setStarting(true);
    setError(undefined);
    try {
      const launchModel = selectedLaunchModel();
      let result: StartRunResult;
      if (executionMode() === "worktree") {
        result = await invokeDesktop<StartRunResult>("runtime_start_project_worktree", {
          request: {
            projectPath: path,
            projectTrust: projectTrust(),
            contextFiles: contextFiles(),
            extensionDiscovery: extensionDiscovery(),
            provider: launchModel?.provider ?? null,
            model: launchModel?.id ?? null,
            thinking: launchThinking() || null,
            base: worktreeBase(),
            branch: worktreeBranch().trim(),
            worktreePath: worktreePath().trim(),
            initialTask: initialTask().trim() || null,
          },
        });
      } else {
        result = await invokeDesktop<StartRunResult>("runtime_start_project", {
          request: {
            projectPath: path,
            projectTrust: projectTrust(),
            contextFiles: contextFiles(),
            extensionDiscovery: extensionDiscovery(),
            provider: launchModel?.provider ?? null,
            model: launchModel?.id ?? null,
            thinking: launchThinking() || null,
            initialTask: initialTask().trim() || null,
          },
        });
      }
      await props.onStarted(result);
      if (result.initialTaskError) {
        setError(`Run started, but the initial task was not sent automatically: ${result.initialTaskError}`);
      } else if (result.initialTaskSubmitted) {
        setInitialTask("");
      }
      if (executionMode() === "worktree") await loadRecoveries();
    } catch (startError) {
      setError(String(startError));
      if (executionMode() === "worktree") await loadRecoveries();
    } finally {
      setStarting(false);
    }
  };

  const reconcileRecovery = async (record: WorktreeRecoveryRecord) => {
    if (reconcilingRecovery()) return;
    setReconcilingRecovery(record.id);
    setRecoveryError(undefined);
    try {
      const inspection = await invokeDesktop<WorktreeRecoveryInspection>(
        "runtime_reconcile_worktree_recovery",
        { request: { id: record.id } },
      );
      setRecoveryInspections((current) => ({ ...current, [record.id]: inspection.probe }));
      await loadRecoveries();
    } catch (inspectError) {
      setRecoveryError(String(inspectError));
    } finally {
      setReconcilingRecovery(undefined);
    }
  };

  const startRecovered = async (record: WorktreeRecoveryRecord) => {
    if (startingRecovery() || !props.piReady) return;
    setStartingRecovery(record.id);
    setRecoveryError(undefined);
    try {
      const runId = await invokeDesktop<string>("runtime_start_recovered_worktree", {
        request: {
          id: record.id,
          projectTrust: projectTrust(),
          contextFiles: contextFiles(),
          extensionDiscovery: extensionDiscovery(),
          provider: selectedLaunchModel()?.provider ?? null,
          model: selectedLaunchModel()?.id ?? null,
          thinking: launchThinking() || null,
        },
      });
      await props.onStarted({
        runId,
        initialTaskSubmitted: false,
        initialTaskError: null,
      });
      await loadRecoveries();
    } catch (startError) {
      setRecoveryError(String(startError));
    } finally {
      setStartingRecovery(undefined);
    }
  };

  const inspectWorktree = async () => {
    const path = projectPath().trim();
    if (!path || probingWorktree()) return;
    setProbingWorktree(true);
    setWorktreeError(undefined);
    setWorktreeBase(undefined);
    try {
      const base = await invokeDesktop<WorktreeBaseSnapshot>("runtime_probe_project_worktree", {
        request: { projectPath: path },
      });
      setWorktreeBase(base);
      if (!worktreeBranch().trim() && !worktreePath().trim()) {
        const suggested = suggestedWorktreeIdentity(base.repositoryRoot, initialTask());
        setWorktreeBranch(suggested.branch);
        setWorktreePath(suggested.path);
      }
    } catch (probeError) {
      setWorktreeError(String(probeError));
    } finally {
      setProbingWorktree(false);
    }
  };

  const browseProject = async () => {
    try {
      const selected = await pickDirectory(projectPath());
      if (!selected) return;
      setProjectPath(selected);
      setWorktreeBase(undefined);
      setWorktreeError(undefined);
      resetLaunchOptions();
      resetResourcePreflight();
    } catch (browseError) {
      setError(`Could not choose project folder: ${String(browseError)}`);
    }
  };

  return (
    <section class="project-launcher" aria-label="Start Pi run">
      <h2>Start a run</h2>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void start();
        }}
      >
        <label class="path-field">
          <span>Project path</span>
          <div class="path-picker-row">
            <input
              value={projectPath()}
              disabled={starting()}
              placeholder="Absolute path to an existing project"
              onInput={(event) => {
                setProjectPath(event.currentTarget.value);
                setWorktreeBase(undefined);
                setWorktreeError(undefined);
                resetLaunchOptions();
                resetResourcePreflight();
              }}
            />
            <button type="button" disabled={starting()} onClick={() => void browseProject()}>
              Browse
            </button>
          </div>
        </label>
        <label>
          <span>Execution root</span>
          <select
            value={executionMode()}
            disabled={starting()}
            onChange={(event) => {
              setExecutionMode(event.currentTarget.value as ExecutionMode);
              setError(undefined);
            }}
          >
            <option value="local">Local checkout</option>
            <option value="worktree">New Git worktree</option>
          </select>
        </label>
        <div class="launch-resource-preflight">
          <button
            type="button"
            disabled={starting() || resourcePreflightLoading() || !projectPath().trim()}
            onClick={() => void probeProjectResources()}
          >
            {resourcePreflightLoading() ? "Checking project resources" : "Check project resources"}
          </button>
          <Show
            when={resourcePreflightPath() === projectPath().trim() ? resourcePreflight() : undefined}
          >
            {(preflight) => {
              const labels = () => projectResourceLabels(preflight());
              return (
                <p class="launch-note">
                  {labels().length > 0
                    ? `Protected Pi resources detected: ${labels().join(", ")}. `
                    : "No protected Pi project resources were detected in this snapshot. "}
                  {projectTrust() === "approve"
                    ? "Approve loads protected project resources for this run."
                    : projectTrust() === "ignore"
                      ? "Ignore skips protected resources for this run; context files remain a separate choice."
                      : "Use Pi saved/default trust leaves the final decision to Pi; RPC mode does not show an interactive trust prompt."}
                </p>
              );
            }}
          </Show>
          <Show when={resourcePreflightError()}>
            {(message) => <p class="error">Project-resource check: {message()}</p>}
          </Show>
        </div>
        <div class="launch-model-controls">
          <div class="launch-model-heading">
            <span>New-run model and thinking</span>
            <button
              type="button"
              disabled={
                launchOptionsLoading() ||
                starting() ||
                !props.piReady ||
                projectPath().trim().length === 0
              }
              onClick={() => void probeLaunchOptions(selectedLaunchModel())}
            >
              {launchOptionsLoading()
                ? "Reading Pi options"
                : launchOptions()
                  ? "Refresh Pi options"
                  : "Load Pi options"}
            </button>
          </div>
          <label>
            <span>Model</span>
            <select
              value={launchModelKey()}
              disabled={starting() || launchOptionsLoading()}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setLaunchModelKey(value);
                setLaunchThinking("");
                if (!value) {
                  void probeLaunchOptions();
                  return;
                }
                const model = selectedLaunchModel();
                if (model) void probeLaunchOptions(model);
              }}
            >
              <option value="">
                {launchOptions()?.currentModel
                  ? `Use Pi configured model (${launchOptions()!.currentModel!.provider}/${launchOptions()!.currentModel!.id})`
                  : "Use Pi configured model"}
              </option>
              <For each={launchOptions()?.models ?? []}>
                {(model) => (
                  <option value={JSON.stringify([model.provider, model.id])}>
                    {model.name ? `${model.name} · ` : ""}{model.provider}/{model.id}
                  </option>
                )}
              </For>
            </select>
          </label>
          <label>
            <span>Thinking</span>
            <select
              value={launchThinking()}
              disabled={starting() || launchOptionsLoading()}
              onChange={(event) =>
                setLaunchThinking(event.currentTarget.value as ThinkingLevel | "")
              }
            >
              <option value="">
                {launchOptions()
                  ? `Use Pi configured/default thinking (${launchOptions()!.currentThinkingLevel})`
                  : "Use Pi configured/default thinking"}
              </option>
              <For each={launchOptions()?.thinkingLevels ?? []}>
                {(level) => <option value={level}>{level}</option>}
              </For>
            </select>
          </label>
          <Show when={launchOptions() && !launchOptions()!.clearQueueSupported}>
            <p class="launch-note">
              This Pi build does not expose RPC queue clearing. Stop remains safe: Pi Wizard
              preserves the latest bounded user queue event and terminates the exact Pi process so
              queued or extension-created continuation work cannot keep running. Resume the Pi
              session afterward to continue.
            </p>
          </Show>
          <Show when={launchOptionsError()}>
            {(message) => <p class="error">Pi launch options: {message()}</p>}
          </Show>
        </div>
        <label class="initial-task-field">
          <span>Initial task</span>
          <textarea
            value={initialTask()}
            disabled={starting()}
            placeholder="What should Pi do? Leave blank to start an idle session."
            onInput={(event) => setInitialTask(event.currentTarget.value)}
          />
        </label>
        <label>
          <span>Project resources</span>
          <select
            value={projectTrust()}
            disabled={starting()}
            onChange={(event) => {
              setProjectTrust(event.currentTarget.value as ProjectTrustPolicy);
              resetLaunchOptions();
            }}
          >
            <option value="inherit">Use Pi saved/default trust</option>
            <option value="approve">Approve for this run</option>
            <option value="ignore">Ignore protected project resources</option>
          </select>
        </label>
        <details class="launch-advanced">
          <summary>Advanced launch options</summary>
          <label>
            <span>Context instructions</span>
            <select
              value={contextFiles()}
              disabled={starting()}
              onChange={(event) => {
                setContextFiles(event.currentTarget.value as ContextFilesPolicy);
                resetLaunchOptions();
              }}
            >
              <option value="inherit">Load AGENTS.md / CLAUDE.md using Pi settings</option>
              <option value="disabled">Disable context files for this launch</option>
            </select>
          </label>
          <p class="launch-note">
            This is independent from project-resource trust and applies to new, resumed, and
            recovered runs launched from this screen.
          </p>
          <label>
            <span>Extensions</span>
            <select
              value={extensionDiscovery()}
              disabled={starting()}
              onChange={(event) =>
                setExtensionDiscovery(event.currentTarget.value as ExtensionDiscoveryPolicy)
              }
            >
              <option value="inherit">Load discovered Pi extensions</option>
              <option value="disabled">Disable extensions for this launch</option>
            </select>
          </label>
          <p class="launch-note">
            Disabling extensions maps to Pi --no-extensions for this run only. It does not change
            Pi’s global configuration, project trust, or context-file loading. The model/thinking
            probe is always extension-free so a broken installed extension cannot block recovery.
          </p>
        </details>
        <Show when={executionMode() === "worktree"}>
          <div class="worktree-plan">
            <button
              type="button"
              disabled={probingWorktree() || starting() || !props.piReady || !projectPath().trim()}
              onClick={() => void inspectWorktree()}
            >
              {probingWorktree() ? "Inspecting Git base" : "Inspect Git base"}
            </button>
            <Show when={worktreeBase()}>
              {(base) => (
                <div class="worktree-base">
                  <strong>{base().sourceBranch ?? "Detached HEAD"}</strong>
                  <span title={base().baseCommit}>base {base().baseCommit.slice(0, 12)}</span>
                  <span title={base().repositoryRoot}>{base().repositoryRoot}</span>
                  <Show when={base().projectRelativePath}>
                    <span>project subdirectory: {base().projectRelativePath}</span>
                  </Show>
                  <Show when={base().dirty}>
                    <p>
                      Source checkout has uncommitted or untracked changes. The new worktree starts
                      from the captured commit only; those changes are not copied.
                    </p>
                  </Show>
                </div>
              )}
            </Show>
            <label>
              <span>New branch</span>
              <input
                value={worktreeBranch()}
                disabled={starting()}
                placeholder="Explicit new branch name"
                onInput={(event) => setWorktreeBranch(event.currentTarget.value)}
              />
            </label>
            <label>
              <span>Worktree path</span>
              <input
                value={worktreePath()}
                disabled={starting()}
                placeholder="Absolute path for the new worktree"
                onInput={(event) => setWorktreePath(event.currentTarget.value)}
              />
            </label>
            <p class="launch-note">
              The captured branch and commit are revalidated before Git mutates anything. A
              worktree isolates Git edits; it is not a filesystem, network, credential, or process sandbox.
            </p>
            <Show when={worktreeError()}>
              {(message) => <p class="error">Git base inspection failed: {message()}</p>}
            </Show>
          </div>
        </Show>
        <Show when={localCheckoutActive()}>
          <div class="launch-conflict" role="status">
            <span>This checkout is already in use by a live run.</span>
            <button
              type="button"
              disabled={starting() || probingWorktree() || !props.piReady}
              onClick={() => {
                setExecutionMode("worktree");
                setError(undefined);
                queueMicrotask(() => void inspectWorktree());
              }}
            >
              Use a worktree
            </button>
          </div>
        </Show>
        <button
          type="submit"
          disabled={
            starting() ||
            launchOptionsLoading() ||
            !props.piReady ||
            projectPath().trim().length === 0 ||
            localCheckoutActive() ||
            (executionMode() === "worktree" &&
              (!worktreeBase() || !worktreeBranch().trim() || !worktreePath().trim()))
          }
        >
          {starting()
            ? "Starting"
            : executionMode() === "worktree"
              ? "Create worktree and start Pi"
              : "Start Pi"}
        </button>
      </form>
      <p class="launch-note">
        Trust controls Pi project-resource loading. It is not an execution sandbox, and ignoring
        protected project resources does not disable context files such as AGENTS.md.
      </p>
      <Show when={error()}>{(message) => <p class="error">Could not start run: {message()}</p>}</Show>

      <div class="worktree-recoveries">
        <div class="recovery-heading">
          <div>
            <strong>Worktree recovery</strong>
            <span>Durable records are kept after Git creation so app or Pi launch failure cannot orphan context silently.</span>
          </div>
          <button type="button" disabled={loadingRecoveries()} onClick={() => void loadRecoveries()}>
            {loadingRecoveries() ? "Refreshing" : "Refresh"}
          </button>
        </div>
        <Show when={recoveryPage()?.recoveryNotice}>
          {(notice) => (
            <p class="error">
              Worktree recovery state was corrupt and quarantined: {notice()}
            </p>
          )}
        </Show>
        <Show when={recoveryPage()?.truncated}>
          <p class="launch-note">Showing a bounded recovery window; older records are omitted.</p>
        </Show>
        <For each={recoveryPage()?.records ?? []}>
          {(record) => {
            const probe = () => recoveryInspections()[record.id];
            const partialDetail = () => {
              const value = probe();
              return value?.kind === "partial" ? value.detail : undefined;
            };
            const activeRunId = () =>
              record.created
                ? props.activeRunIdForExecutionRoot(record.created.executionRoot)
                : undefined;
            return (
              <article class="recovery-row">
                <div>
                  <strong>{record.branch}</strong>
                  <span title={record.base.baseCommit}>
                    {record.created ? "Created" : "Needs inspection"} · base {record.base.baseCommit.slice(0, 12)}
                  </span>
                  <span title={record.created?.worktreeRoot ?? record.requestedPath}>
                    {record.created?.worktreeRoot ?? record.requestedPath}
                  </span>
                  <Show when={probe()?.kind === "notCreated"}>
                    <span>No branch or path remained; the stale intent was removed.</span>
                  </Show>
                  <Show when={probe()?.kind === "exact"}>
                    <span>Repository, branch and worktree path match; the captured base remains an ancestor of the current task HEAD.</span>
                  </Show>
                  <Show when={partialDetail()}>
                    {(detail) => (
                      <span>
                        Partial/conflicting Git state was retained. Pi Wizard will not delete it automatically. {detail()}
                      </span>
                    )}
                  </Show>
                </div>
                <div class="recovery-actions">
                  <button
                    type="button"
                    disabled={
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery()) ||
                      Boolean(cleaningRecovery())
                    }
                    onClick={() => void reconcileRecovery(record)}
                  >
                    {reconcilingRecovery() === record.id ? "Inspecting" : "Inspect"}
                  </button>
                  <button
                    type="button"
                    disabled={
                      !record.created ||
                      (!activeRunId() && !props.piReady) ||
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery()) ||
                      Boolean(cleaningRecovery())
                    }
                    onClick={() => {
                      const runId = activeRunId();
                      if (runId) {
                        props.onOpenRun(runId);
                      } else {
                        void startRecovered(record);
                      }
                    }}
                  >
                    {activeRunId()
                      ? "Open"
                      : startingRecovery() === record.id
                        ? "Starting"
                        : "Start Pi"}
                  </button>
                  <button
                    type="button"
                    disabled={
                      !record.created ||
                      Boolean(activeRunId()) ||
                      probe()?.kind !== "exact" ||
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery()) ||
                      Boolean(cleaningRecovery())
                    }
                    title="Only pristine worktrees whose branch is still at the captured base can be removed"
                    onClick={() => void cleanupRecovery(record)}
                  >
                    {cleaningRecovery() === record.id ? "Removing" : "Remove unused"}
                  </button>
                </div>
              </article>
            );
          }}
        </For>
        <Show when={(recoveryPage()?.records.length ?? 0) === 0 && !loadingRecoveries()}>
          <p class="launch-note">No retained worktree recovery records.</p>
        </Show>
        <Show when={recoveryError()}>
          {(message) => <p class="error">Worktree recovery: {message()}</p>}
        </Show>
      </div>

      <SessionCatalogBrowser
        projectPath={projectPath()}
        piReady={props.piReady}
        projectTrust={projectTrust()}
        contextFiles={contextFiles()}
        extensionDiscovery={extensionDiscovery()}
        onStarted={props.onStarted}
        onOpenRun={props.onOpenRun}
        activeRunIdForExecutionRoot={props.activeRunIdForExecutionRoot}
        activeRunIdForSessionPath={props.activeRunIdForSessionPath}
      />
    </section>
  );
}

function RecentSessionsView(props: {
  projects: DesktopProjectRecord[];
  preferredProjectPath: string;
  piReady: boolean;
  onStarted: (result: StartRunResult) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
  onNewRun: (projectPath: string) => void;
  activeRunIdForExecutionRoot: (path: string) => string | undefined;
  activeRunIdForSessionPath: (path: string) => string | undefined;
}) {
  const [projectPath, setProjectPath] = createSignal("");
  const [projectTrust, setProjectTrust] = createSignal<ProjectTrustPolicy>("inherit");
  const [contextFiles, setContextFiles] = createSignal<ContextFilesPolicy>("inherit");
  const [extensionDiscovery, setExtensionDiscovery] =
    createSignal<ExtensionDiscoveryPolicy>("inherit");

  const availableProjects = () => props.projects.filter((project) => project.status === "present");

  createEffect(() => {
    const available = availableProjects();
    const current = projectPath();
    if (available.some((project) => project.canonicalRoot === current)) return;
    const preferred = props.preferredProjectPath.trim();
    const next = available.find((project) => project.canonicalRoot === preferred) ?? available[0];
    setProjectPath(next?.canonicalRoot ?? "");
  });

  return (
    <section class="recent-sessions-surface" aria-label="Recent Pi sessions">
      <header class="surface-heading">
        <div>
          <h1>Recent sessions</h1>
          <p>
            Browse Pi’s authoritative session files on demand. Nothing is scanned while this view is
            closed.
          </p>
        </div>
        <button
          type="button"
          disabled={!projectPath()}
          onClick={() => props.onNewRun(projectPath())}
        >
          New run
        </button>
      </header>

      <Show
        when={availableProjects().length > 0}
        fallback={
          <div class="empty-state">
            <strong>No available project folders</strong>
            <span>Register a project from New Run, or relocate a detached project from the sidebar.</span>
          </div>
        }
      >
        <div class="recent-session-controls">
          <label>
            <span>Project</span>
            <select
              value={projectPath()}
              onChange={(event) => setProjectPath(event.currentTarget.value)}
            >
              <For each={availableProjects()}>
                {(project) => (
                  <option value={project.canonicalRoot}>
                    {pathLeaf(project.canonicalRoot)}
                  </option>
                )}
              </For>
            </select>
            <small class="recent-project-path" title={projectPath()}>{projectPath()}</small>
          </label>
          <details class="session-launch-options">
            <summary>Resume launch options</summary>
            <div>
              <label>
                <span>Project resources</span>
                <select
                  value={projectTrust()}
                  onChange={(event) =>
                    setProjectTrust(event.currentTarget.value as ProjectTrustPolicy)
                  }
                >
                  <option value="inherit">Use Pi saved/default trust</option>
                  <option value="approve">Approve for this run</option>
                  <option value="ignore">Ignore protected project resources</option>
                </select>
              </label>
              <label>
                <span>Context instructions</span>
                <select
                  value={contextFiles()}
                  onChange={(event) =>
                    setContextFiles(event.currentTarget.value as ContextFilesPolicy)
                  }
                >
                  <option value="inherit">Load AGENTS.md / CLAUDE.md using Pi settings</option>
                  <option value="disabled">Disable context files for this launch</option>
                </select>
              </label>
              <label>
                <span>Extensions</span>
                <select
                  value={extensionDiscovery()}
                  onChange={(event) =>
                    setExtensionDiscovery(event.currentTarget.value as ExtensionDiscoveryPolicy)
                  }
                >
                  <option value="inherit">Load discovered Pi extensions</option>
                  <option value="disabled">Disable extensions for this launch</option>
                </select>
              </label>
            </div>
            <p>
              Trust controls protected Pi project resources. Context-file and extension loading are
              independent launch choices. Disable extensions for a one-run recovery when an
              installed Pi extension prevents startup.
            </p>
          </details>
        </div>

        <SessionCatalogBrowser
          projectPath={projectPath()}
          piReady={props.piReady}
          projectTrust={projectTrust()}
          contextFiles={contextFiles()}
          extensionDiscovery={extensionDiscovery()}
          onStarted={props.onStarted}
          onOpenRun={props.onOpenRun}
          activeRunIdForExecutionRoot={props.activeRunIdForExecutionRoot}
          activeRunIdForSessionPath={props.activeRunIdForSessionPath}
        />
      </Show>
    </section>
  );
}

function NeedsAttentionView(props: {
  runs: RunHydration[];
  onOpenRun: (runId: string) => void;
  onResolved: () => Promise<unknown>;
  projectLabel: (run: RunHydration) => string;
}) {
  const pending = () =>
    props.runs.flatMap((run) =>
      (run.rpc?.pendingDialogs ?? []).map((dialog) => ({ run, dialog })),
    ).sort((left, right) => {
      const leftDeadline = left.dialog.remainingTimeoutMs ?? Number.POSITIVE_INFINITY;
      const rightDeadline = right.dialog.remainingTimeoutMs ?? Number.POSITIVE_INFINITY;
      if (leftDeadline !== rightDeadline) return leftDeadline - rightDeadline;
      const runOrder = right.run.run.id.localeCompare(left.run.run.id);
      if (runOrder !== 0) return runOrder;
      return left.dialog.request.id.localeCompare(right.dialog.request.id);
    });

  return (
    <section class="attention-queue-surface" aria-label="Needs attention queue">
      <header class="surface-heading">
        <div>
          <h1>Needs attention</h1>
          <p>Answer extension requests across live Pi runs without changing process ownership.</p>
        </div>
        <strong class="attention-total">
          {pending().length} request{pending().length === 1 ? "" : "s"}
        </strong>
      </header>
      <Show
        when={pending().length > 0}
        fallback={
          <div class="empty-state">
            <strong>Nothing needs attention</strong>
            <span>Working runs continue independently while you use other views.</span>
          </div>
        }
      >
        <div class="attention-queue">
          <For each={pending()}>
            {(pendingRequest) => (
              <section class="attention-queue-item">
                <header>
                  <div>
                    <strong>{runTitle(pendingRequest.run)}</strong>
                    <span title={pendingRequest.run.run.executionRoot}>
                      {props.projectLabel(pendingRequest.run)} · {pendingRequest.run.run.executionIsolation === "git_worktree"
                        ? "Git worktree"
                        : "Local checkout"}
                      {" · "}{pendingRequest.run.run.executionRoot}
                    </span>
                  </div>
                  <button type="button" onClick={() => props.onOpenRun(pendingRequest.run.run.id)}>
                    Open run
                  </button>
                </header>
                <ExtensionDialogCard
                  runId={pendingRequest.run.run.id}
                  dialog={pendingRequest.dialog}
                  onResolved={props.onResolved}
                />
              </section>
            )}
          </For>
        </div>
      </Show>
    </section>
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

function AutomationView(props: {
  snapshot: DesktopAutomationSnapshot | undefined;
  projects: DesktopProjectRecord[];
  capacity: RuntimeCapacitySnapshot | undefined;
  onRefresh: () => Promise<unknown>;
  onOpenRun: (runId: string) => void;
}) {
  const [chainId, setChainId] = createSignal<string>();
  const [name, setName] = createSignal("");
  const [prompts, setPrompts] = createSignal<string[]>([""]);
  const [projectId, setProjectId] = createSignal("");
  const [concurrency, setConcurrency] = createSignal("4");
  const [worktrees, setWorktrees] = createSignal(true);
  const [supervisor, setSupervisor] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();

  createEffect(() => {
    if (!projectId()) {
      const project = props.projects.find((candidate) => candidate.status === "present");
      if (project) setProjectId(project.id);
    }
  });

  const loadChain = (chain: AutomationChain) => {
    setChainId(chain.id);
    setName(chain.name);
    setPrompts([...chain.prompts]);
    setError(undefined);
  };

  const newChain = () => {
    setChainId(undefined);
    setName("");
    setPrompts([""]);
    setError(undefined);
  };

  const updatePrompt = (index: number, value: string) =>
    setPrompts((current) => current.map((prompt, currentIndex) => (currentIndex === index ? value : prompt)));

  const removePrompt = (index: number) =>
    setPrompts((current) => {
      const next = current.filter((_, currentIndex) => currentIndex !== index);
      return next.length > 0 ? next : [""];
    });

  const movePrompt = (index: number, direction: -1 | 1) =>
    setPrompts((current) => {
      const target = index + direction;
      if (target < 0 || target >= current.length) return current;
      const next = [...current];
      const selected = next[index];
      const adjacent = next[target];
      if (selected === undefined || adjacent === undefined) return current;
      next[index] = adjacent;
      next[target] = selected;
      return next;
    });

  const saveChain = async () => {
    if (busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      const saved = await invokeDesktop<AutomationChain>("runtime_save_automation_chain", {
        request: {
          id: chainId() ?? null,
          name: name(),
          prompts: prompts(),
        },
      });
      setChainId(saved.id);
      setName(saved.name);
      setPrompts([...saved.prompts]);
      await props.onRefresh();
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const deleteChain = async () => {
    const id = chainId();
    if (!id || busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<boolean>("runtime_delete_automation_chain", { request: { id } });
      newChain();
      await props.onRefresh();
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const startChain = async () => {
    const id = chainId();
    const project = projectId();
    const workers = Number.parseInt(concurrency(), 10);
    if (!id) {
      setError("Save the chain before starting it.");
      return;
    }
    if (!project || !Number.isInteger(workers)) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<string>("runtime_start_automation", {
        request: {
          chainId: id,
          projectId: project,
          concurrency: workers,
          worktrees: worktrees(),
          supervisor: supervisor(),
        },
      });
      await props.onRefresh();
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const cancelExecution = async (id: string) => {
    try {
      await invokeDesktop<void>("runtime_cancel_automation", { request: { id } });
      await props.onRefresh();
    } catch (caught) {
      setError(String(caught));
    }
  };

  const maxWorkers = () => {
    const live = props.capacity?.liveRunLimit ?? 8;
    return Math.max(1, live - (supervisor() ? 1 : 0));
  };

  createEffect(() => {
    const maximum = maxWorkers();
    const current = Number.parseInt(concurrency(), 10);
    if (Number.isInteger(current) && current > maximum) {
      setConcurrency(String(maximum));
    }
  });

  return (
    <section class="automation-surface" aria-label="Automation chains">
      <header class="surface-heading">
        <div>
          <h1>Automation</h1>
          <p>Run a finite prompt list through new Pi sessions. Parallel workers use isolated Git worktrees.</p>
        </div>
        <button type="button" onClick={newChain}>New chain</button>
      </header>

      <Show when={props.snapshot?.catalog.recoveryNotice}>
        {(notice) => <p class="app-error">Saved automation definitions were reset: {notice()}</p>}
      </Show>
      <Show when={error()}>{(message) => <p class="app-error">Automation: {message()}</p>}</Show>

      <div class="automation-layout">
        <aside class="automation-library" aria-label="Saved chains">
          <strong>Saved chains</strong>
          <For each={props.snapshot?.catalog.chains ?? []}>
            {(chain) => (
              <button
                type="button"
                class={chainId() === chain.id ? "active" : undefined}
                onClick={() => loadChain(chain)}
              >
                <strong>{chain.name}</strong>
                <span>{chain.prompts.length} prompt{chain.prompts.length === 1 ? "" : "s"}</span>
              </button>
            )}
          </For>
          <Show when={(props.snapshot?.catalog.chains.length ?? 0) === 0}>
            <span class="sidebar-note">No saved chains.</span>
          </Show>
        </aside>

        <div class="automation-builder">
          <label class="automation-name">
            <span>Chain name</span>
            <input value={name()} onInput={(event) => setName(event.currentTarget.value)} />
          </label>

          <div class="automation-prompts" aria-label="Ordered prompts">
            <For each={prompts()}>
              {(prompt, index) => (
                <article class="automation-prompt-card">
                  <header>
                    <strong>Prompt {index() + 1}</strong>
                    <div>
                      <button type="button" disabled={index() === 0} onClick={() => movePrompt(index(), -1)}>Up</button>
                      <button type="button" disabled={index() === prompts().length - 1} onClick={() => movePrompt(index(), 1)}>Down</button>
                      <button type="button" onClick={() => removePrompt(index())}>Remove</button>
                    </div>
                  </header>
                  <textarea
                    rows="5"
                    value={prompt}
                    aria-label={`Automation prompt ${index() + 1}`}
                    onInput={(event) => updatePrompt(index(), event.currentTarget.value)}
                  />
                </article>
              )}
            </For>
            <button type="button" class="automation-add-prompt" onClick={() => setPrompts((current) => [...current, ""])}>
              Add prompt
            </button>
          </div>

          <div class="automation-launch-grid">
            <label>
              <span>Project</span>
              <select value={projectId()} onChange={(event) => setProjectId(event.currentTarget.value)}>
                <option value="">Select project</option>
                <For each={props.projects}>
                  {(project) => (
                    <option value={project.id} disabled={project.status !== "present"}>
                      {pathLeaf(project.canonicalRoot)}{project.status === "present" ? "" : ` · ${project.status}`}
                    </option>
                  )}
                </For>
              </select>
            </label>
            <label>
              <span>Workers</span>
              <input
                type="number"
                min="1"
                max={maxWorkers()}
                value={concurrency()}
                onInput={(event) => setConcurrency(event.currentTarget.value)}
              />
            </label>
            <label class="automation-checkbox">
              <input
                type="checkbox"
                checked={worktrees()}
                onChange={(event) => {
                  setWorktrees(event.currentTarget.checked);
                  if (!event.currentTarget.checked) setConcurrency("1");
                }}
              />
              <span>Git-isolated workers</span>
            </label>
            <label class="automation-checkbox">
              <input type="checkbox" checked={supervisor()} onChange={(event) => setSupervisor(event.currentTarget.checked)} />
              <span>LLM supervisor · uses one live slot</span>
            </label>
          </div>

          <div class="automation-builder-actions">
            <button type="button" disabled={busy()} onClick={() => void saveChain()}>{busy() ? "Working" : "Save"}</button>
            <Show when={chainId()}>
              <button type="button" disabled={busy()} onClick={() => void deleteChain()}>Delete</button>
              <button type="button" class="primary-action" disabled={busy() || !projectId()} onClick={() => void startChain()}>
                Start chain
              </button>
            </Show>
          </div>
        </div>
      </div>

      <section class="automation-executions" aria-label="Automation executions">
        <h2>Executions</h2>
        <For each={props.snapshot?.executions ?? []}>
          {(execution) => (
            <article class="automation-execution">
              <header>
                <div>
                  <strong>{execution.chainName}</strong>
                  <span>
                    {execution.status.replaceAll("_", " ")} · {execution.concurrency} worker{execution.concurrency === 1 ? "" : "s"}
                    {execution.supervisorEnabled ? " · supervised" : ""}
                    {execution.supervisorCycles > 0 ? ` · ${execution.supervisorCycles} supervisor cycle${execution.supervisorCycles === 1 ? "" : "s"}` : ""}
                  </span>
                </div>
                <Show when={!['completed', 'completed_with_errors', 'cancelled', 'failed'].includes(execution.status)}>
                  <button type="button" onClick={() => void cancelExecution(execution.id)}>Cancel chain</button>
                </Show>
              </header>
              <Show when={execution.error}>
                {(message) => <p class="error">Execution failed: {message()}</p>}
              </Show>
              <Show when={execution.supervisorError}>
                {(message) => <p class="error">Supervisor disabled: {message()}</p>}
              </Show>
              <div class="automation-step-list">
                <For each={execution.steps}>
                  {(step) => (
                    <div class={`automation-step step-${step.status}`}>
                      <strong>{step.index + 1}</strong>
                      <span title={step.promptTruncated ? "Prompt preview truncated" : step.promptPreview}>
                        {step.promptPreview}{step.promptTruncated ? "…" : ""}
                      </span>
                      <small>{step.status.replaceAll("_", " ")}</small>
                      <Show when={step.runId}>
                        {(runId) => <button type="button" onClick={() => props.onOpenRun(runId())}>Open</button>}
                      </Show>
                      <Show when={step.error}>{(message) => <small class="error">{message()}</small>}</Show>
                    </div>
                  )}
                </For>
              </div>
            </article>
          )}
        </For>
        <Show when={(props.snapshot?.executions.length ?? 0) === 0}>
          <p class="empty-state">No automation executions yet.</p>
        </Show>
      </section>
    </section>
  );
}

export function App() {
  const [runtime, setRuntime] = createSignal<RuntimeHydration>();
  const [runtimeError, setRuntimeError] = createSignal<string>();
  const [capacity, setCapacity] = createSignal<RuntimeCapacitySnapshot>();
  const [capacityError, setCapacityError] = createSignal<string>();
  const [automation, setAutomation] = createSignal<DesktopAutomationSnapshot>();
  const [automationError, setAutomationError] = createSignal<string>();
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

  createEffect(() => {
    if (view() === "automation") void refreshAutomation();
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
              onRefresh={refreshAutomation}
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
              projectLabel={projectLabelForRun}
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
