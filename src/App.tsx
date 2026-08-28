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
type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
type ExecutionMode = "local" | "worktree";
type ExecutionIsolation = "local_checkout" | "git_worktree";

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

interface GitWorktreeIdentity {
  repositoryRoot: string;
  worktreeRoot: string;
  branch: string;
  baseCommit: string;
}

interface RunHydration {
  run: {
    id: string;
    projectId: string;
    executionRoot: string;
    executionIsolation: ExecutionIsolation;
    worktree: GitWorktreeIdentity | null;
    projectTrust: ProjectTrustPolicy;
    agentWorking: boolean;
    process: "starting" | "ready" | "stopping" | "exited" | "failed" | "quarantined";
    revision: number;
    session: {
      sessionFile: string | null;
      sessionId: string | null;
      sessionName: string | null;
      model: ModelSummary | null;
      thinkingLevel: ThinkingLevel | null;
      autoCompactionEnabled: boolean | null;
      messageCount: number | null;
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

interface GitFileDiff {
  path: string;
  diff: string;
  truncated: boolean;
  untracked: boolean;
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
  directorySource: "environment" | "settings" | "default";
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

interface RuntimeHydration {
  schemaVersion: number;
  runtimeRevision: number;
  runs: RunHydration[];
}

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
  const [page, setPage] = createSignal<SessionHistoryPage>();
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [currentCursor, setCurrentCursor] = createSignal<SessionHistoryCursor | null>(null);
  const [newerCursors, setNewerCursors] = createSignal<(SessionHistoryCursor | null)[]>([]);
  const [loadedMessageCount, setLoadedMessageCount] = createSignal<number | null>(null);
  let requestSequence = 0;
  let loadedSessionId: string | undefined;

  const load = async (
    cursor: SessionHistoryCursor | null,
    expectedSessionId: string,
    clearNewer = false,
  ) => {
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
        return false;
      setPage(result);
      setCurrentCursor(cursor);
      setLoadedMessageCount(props.run.run.session.messageCount);
      if (clearNewer) setNewerCursors([]);
      return true;
    } catch (historyError) {
      if (sequence === requestSequence) setError(String(historyError));
      return false;
    } finally {
      if (sequence === requestSequence) setLoading(false);
    }
  };

  createEffect(() => {
    const sessionId = props.run.run.session.sessionId;
    const sessionFile = props.run.run.session.sessionFile;
    if (!sessionId || !sessionFile) {
      requestSequence += 1;
      loadedSessionId = undefined;
      setPage(undefined);
      setError(undefined);
      setLoading(false);
      setCurrentCursor(null);
      setNewerCursors([]);
      setLoadedMessageCount(null);
      return;
    }
    if (sessionId === loadedSessionId) return;
    loadedSessionId = sessionId;
    setPage(undefined);
    setCurrentCursor(null);
    setNewerCursors([]);
    setLoadedMessageCount(null);
    void load(null, sessionId, true);
  });

  const loadOlder = async () => {
    const cursor = page()?.nextCursor;
    const sessionId = props.run.run.session.sessionId;
    if (!cursor || !sessionId || loading()) return;
    const previous = currentCursor();
    if (await load(cursor, sessionId)) {
      const history = newerCursors();
      setNewerCursors(
        history.length >= 64
          ? [...history.slice(history.length - 63), previous]
          : [...history, previous],
      );
    }
  };

  const loadNewer = async () => {
    const history = newerCursors();
    const cursor = history.at(-1);
    const sessionId = props.run.run.session.sessionId;
    if (cursor === undefined || !sessionId || loading()) return;
    if (await load(cursor, sessionId)) setNewerCursors(history.slice(0, -1));
  };

  const loadLatest = () => {
    const sessionId = props.run.run.session.sessionId;
    if (!sessionId || loading()) return;
    void load(null, sessionId, true);
  };

  const hasNewActivity = () => {
    const current = props.run.run.session.messageCount;
    const loaded = loadedMessageCount();
    return current !== null && loaded !== null && current > loaded;
  };

  return (
    <Show when={props.run.run.session.sessionFile && props.run.run.session.sessionId}>
      <section class="history-timeline" aria-label="Persisted Pi session history">
        <div class="history-toolbar">
          <strong>Session history</strong>
          <span>Bounded active-branch page</span>
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
              disabled={loading() || !page()?.nextCursor}
              onClick={() => void loadOlder()}
            >
              Older
            </button>
            <button
              type="button"
              disabled={loading() || (currentCursor() === null && !hasNewActivity())}
              onClick={loadLatest}
            >
              Latest
            </button>
          </div>
        </div>
        <Show when={hasNewActivity()}>
          <p class="history-note">New persisted activity is available. Latest refreshes this page.</p>
        </Show>
        <Show when={page()}>
          {(history) => (
            <div class="history-page">
              <For each={history().items}>
                {(item) => (
                  <article
                    class={`history-item history-${item.kind}${item.isError ? " history-error" : ""}`}
                  >
                    <header>
                      <strong>{historyLabel(item)}</strong>
                      <div>
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
                    </header>
                    <Show when={item.text}>
                      <pre>{item.text}</pre>
                    </Show>
                    <Show when={item.textTruncated}>
                      <span class="truncation-note">History preview truncated</span>
                    </Show>
                  </article>
                )}
              </For>
              <Show when={history().items.length === 0}>
                <p class="history-note">
                  {history().nextCursor
                    ? "No displayable entries in this bounded scan window. Continue with Older to scan farther back on the active branch."
                    : "No persisted displayable history is available for this session yet."}
                </p>
              </Show>
              <span class="history-footnote">
                Read {history().scannedBytes.toLocaleString()} session bytes for this page.
              </span>
            </div>
          )}
        </Show>
        <Show when={loading() && !page()}>
          <p class="history-note">Loading bounded session history.</p>
        </Show>
        <Show when={error()}>
          {(message) => <p class="error">Session history failed: {message()}</p>}
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
          {(block) => (
            <article class={`live-block live-${block.kind}`}>
              <header>
                <strong>
                  {block.kind === "thinking"
                    ? "Thinking"
                    : block.kind === "tool_call"
                      ? "Tool call"
                      : "Assistant"}
                </strong>
                <span>{block.complete ? "Complete" : "Streaming"}</span>
              </header>
              <pre>{block.text}</pre>
              <DroppedBytes count={block.droppedBytes} />
            </article>
          )}
        </For>
        <For each={props.live?.activeTools ?? []}>
          {(tool) => (
            <article class="live-block live-tool">
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
            <article class="live-block live-bash">
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
}) {
  const [submitError, setSubmitError] = createSignal<string>();
  const [submitting, setSubmitting] = createSignal(false);
  const [stopping, setStopping] = createSignal(false);
  const [stopError, setStopError] = createSignal<string>();
  const [controlBusy, setControlBusy] = createSignal(false);
  const [controlError, setControlError] = createSignal<string>();
  const [attaching, setAttaching] = createSignal(false);
  const [attachmentError, setAttachmentError] = createSignal<string>();
  const [sessionStats, setSessionStats] = createSignal<SessionStats>();
  const [statsBusy, setStatsBusy] = createSignal(false);
  const [statsError, setStatsError] = createSignal<string>();
  const [lastCompaction, setLastCompaction] = createSignal<CompactionResult>();
  const [reviewSummary, setReviewSummary] = createSignal<GitReviewSummary>();
  const [reviewBusy, setReviewBusy] = createSignal(false);
  const [reviewError, setReviewError] = createSignal<string>();
  const [reviewDiff, setReviewDiff] = createSignal<GitFileDiff>();
  const [reviewDiffPath, setReviewDiffPath] = createSignal<string>();
  const [sessionNameDraft, setSessionNameDraft] = createSignal(
    props.run.run.session.sessionName ?? "",
  );
  const [sessionNameDirty, setSessionNameDirty] = createSignal(false);
  let fileInput!: HTMLInputElement;

  createEffect(() => props.state.applyBackend(props.run.draft));
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
    if (reviewBusy()) return;
    setReviewBusy(true);
    setReviewError(undefined);
    setReviewDiff(undefined);
    setReviewDiffPath(undefined);
    try {
      const summary = await invokeDesktop<GitReviewSummary>("runtime_git_review_summary", {
        request: { runId: props.run.run.id },
      });
      setReviewSummary(summary);
    } catch (error) {
      setReviewError(String(error));
    } finally {
      setReviewBusy(false);
    }
  };

  const loadReviewDiff = async (path: string) => {
    if (reviewDiffPath()) return;
    setReviewDiffPath(path);
    setReviewError(undefined);
    try {
      const diff = await invokeDesktop<GitFileDiff>("runtime_git_review_file", {
        request: { runId: props.run.run.id, path },
      });
      setReviewDiff(diff);
    } catch (error) {
      setReviewError(String(error));
    } finally {
      setReviewDiffPath(undefined);
    }
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
            {props.run.run.executionIsolation === "git_worktree" ? "Git worktree" : "Local checkout"}
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
      <Show when={controlError()}>{(error) => <p class="error">Session control: {error()}</p>}</Show>
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
      <div class="git-review">
        <div class="git-review-toolbar">
          <button type="button" disabled={reviewBusy()} onClick={() => void refreshReview()}>
            {reviewBusy() ? "Reading changes" : reviewSummary() ? "Refresh changes" : "Changes"}
          </button>
          <Show when={reviewSummary()}>
            {(summary) => (
              <span>
                {summary().files.length} changed file{summary().files.length === 1 ? "" : "s"}
                {summary().truncated ? " · bounded list" : ""}
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
                    disabled={Boolean(reviewDiffPath())}
                    class={reviewDiff()?.path === file.path ? "selected" : undefined}
                    onClick={() => void loadReviewDiff(file.path)}
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
                    : diff().truncated
                      ? "diff truncated at backend limit"
                      : "current diff"}
                </span>
              </div>
              <pre>{diff().diff || "No current tracked diff for this file."}</pre>
            </div>
          )}
        </Show>
        <Show when={reviewError()}>{(error) => <p class="error">Change review: {error()}</p>}</Show>
      </div>
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
          onInput={(event) => props.state.edit(event.currentTarget.value)}
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
        <div class="command-suggestions" aria-label="Pi commands">
          <For each={commandSuggestions()}>
            {(command) => (
              <button
                type="button"
                disabled={composerDisabled()}
                onClick={() => props.state.edit(`/${command.name} `)}
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
          when={
            props.run.composerAvailability === "agent_working" ||
            props.run.composerAvailability === "blocked_by_compaction"
          }
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
        <span>Extension request</span>
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

function ProjectLauncher(props: {
  piReady: boolean;
  onStarted: () => Promise<unknown>;
  isSessionActive: (path: string) => boolean;
  isExecutionRootActive: (path: string) => boolean;
}) {
  const [projectPath, setProjectPath] = createSignal("");
  const [projectTrust, setProjectTrust] = createSignal<ProjectTrustPolicy>("inherit");
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
  const [sessionQuery, setSessionQuery] = createSignal("");
  const [sessionPage, setSessionPage] = createSignal<SessionCatalogPage>();
  const [sessionError, setSessionError] = createSignal<string>();
  const [loadingSessions, setLoadingSessions] = createSignal(false);
  const [resumingPath, setResumingPath] = createSignal<string>();

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

  onMount(() => void loadRecoveries());

  const start = async () => {
    const path = projectPath().trim();
    if (!path || starting()) return;
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
      if (executionMode() === "worktree") {
        await invokeDesktop<string>("runtime_start_project_worktree", {
          request: {
            projectPath: path,
            projectTrust: projectTrust(),
            base: worktreeBase(),
            branch: worktreeBranch().trim(),
            worktreePath: worktreePath().trim(),
          },
        });
      } else {
        await invokeDesktop<string>("runtime_start_project", {
          request: { projectPath: path, projectTrust: projectTrust() },
        });
      }
      await props.onStarted();
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
      await invokeDesktop<string>("runtime_start_recovered_worktree", {
        request: { id: record.id, projectTrust: projectTrust() },
      });
      await props.onStarted();
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
    } catch (probeError) {
      setWorktreeError(String(probeError));
    } finally {
      setProbingWorktree(false);
    }
  };

  const findSessions = async () => {
    const path = projectPath().trim();
    if (!path || loadingSessions()) return;
    setLoadingSessions(true);
    setSessionError(undefined);
    try {
      const page = await invokeDesktop<SessionCatalogPage>("runtime_list_project_sessions", {
        request: { projectPath: path, query: sessionQuery().trim() || null },
      });
      setSessionPage(page);
    } catch (catalogError) {
      setSessionError(String(catalogError));
    } finally {
      setLoadingSessions(false);
    }
  };

  const resume = async (session: SessionCatalogEntry) => {
    const path = projectPath().trim();
    if (!path || resumingPath() || props.isSessionActive(session.path)) return;
    setResumingPath(session.path);
    setSessionError(undefined);
    try {
      await invokeDesktop<string>("runtime_resume_project_session", {
        request: {
          projectPath: path,
          projectTrust: projectTrust(),
          sessionPath: session.path,
        },
      });
      await props.onStarted();
    } catch (resumeError) {
      setSessionError(String(resumeError));
    } finally {
      setResumingPath(undefined);
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
        <label>
          <span>Project path</span>
          <input
            value={projectPath()}
            disabled={starting()}
            placeholder="Absolute path to an existing project"
            onInput={(event) => {
              setProjectPath(event.currentTarget.value);
              setSessionPage(undefined);
              setSessionError(undefined);
              setWorktreeBase(undefined);
              setWorktreeError(undefined);
            }}
          />
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
        <label>
          <span>Project resources</span>
          <select
            value={projectTrust()}
            disabled={starting()}
            onChange={(event) => setProjectTrust(event.currentTarget.value as ProjectTrustPolicy)}
          >
            <option value="inherit">Use Pi saved/default trust</option>
            <option value="approve">Approve for this run</option>
            <option value="ignore">Ignore protected project resources</option>
          </select>
        </label>
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
        <button
          type="submit"
          disabled={
            starting() ||
            !props.piReady ||
            projectPath().trim().length === 0 ||
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
            const active = () =>
              Boolean(record.created && props.isExecutionRootActive(record.created.executionRoot));
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
                    disabled={Boolean(reconcilingRecovery()) || Boolean(startingRecovery())}
                    onClick={() => void reconcileRecovery(record)}
                  >
                    {reconcilingRecovery() === record.id ? "Inspecting" : "Inspect"}
                  </button>
                  <button
                    type="button"
                    disabled={
                      !record.created ||
                      active() ||
                      !props.piReady ||
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery())
                    }
                    onClick={() => void startRecovered(record)}
                  >
                    {active()
                      ? "Open"
                      : startingRecovery() === record.id
                        ? "Starting"
                        : "Start Pi"}
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

      <div class="session-browser">
        <div class="session-search">
          <input
            value={sessionQuery()}
            disabled={loadingSessions() || !props.piReady}
            placeholder="Search session name, prompt, or ID"
            aria-label="Search Pi sessions"
            onInput={(event) => setSessionQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void findSessions();
              }
            }}
          />
          <button
            type="button"
            disabled={loadingSessions() || !props.piReady || projectPath().trim().length === 0}
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
                </span>
                <Show when={page().truncated}>
                  <span>Bounded catalog window; some sessions may be omitted.</span>
                </Show>
              </div>
              <For each={page().sessions}>
                {(session) => {
                  const active = () => props.isSessionActive(session.path);
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
                        disabled={Boolean(resumingPath()) || active()}
                        onClick={() => void resume(session)}
                      >
                        {active()
                          ? "Open"
                          : resumingPath() === session.path
                            ? "Resuming"
                            : "Resume"}
                      </button>
                    </article>
                  );
                }}
              </For>
              <Show when={page().sessions.length === 0}>
                <p class="launch-note">No matching Pi sessions were found for this project.</p>
              </Show>
            </div>
          )}
        </Show>
        <Show when={sessionError()}>
          {(message) => <p class="error">Session lookup failed: {message()}</p>}
        </Show>
      </div>
    </section>
  );
}

export function App() {
  const [runtime, setRuntime] = createSignal<RuntimeHydration>();
  const [runtimeError, setRuntimeError] = createSignal<string>();
  const [piProbe, setPiProbe] = createSignal<PiProbeReport>();
  const [piProbeError, setPiProbeError] = createSignal<string>();
  const [attachmentLimits, setAttachmentLimits] = createSignal<RuntimeAttachmentLimits>();
  const [deliveredEvents, setDeliveredEvents] = createSignal(0);
  const drainingRuns = new Set<string>();
  const redrainRuns = new Set<string>();
  const hydrationNeededRuns = new Set<string>();
  const composerStates = new Map<string, ComposerState>();
  let hydrationRequestSequence = 0;
  let lastAppliedHydrationRequest = 0;
  let disposed = false;

  const applyHydration = (snapshot: RuntimeHydration, requestSequence: number) => {
    if (disposed || requestSequence < lastAppliedHydrationRequest) return;
    lastAppliedHydrationRequest = requestSequence;
    setRuntime(snapshot);
    setRuntimeError(undefined);
  };

  const isSessionActive = (path: string) =>
    Boolean(
      runtime()?.runs.some(
        (run) =>
          !["exited", "failed", "quarantined"].includes(run.run.process) &&
          run.run.session.sessionFile === path,
      ),
    );

  const isExecutionRootActive = (path: string) =>
    Boolean(
      runtime()?.runs.some(
        (run) =>
          !["exited", "failed", "quarantined"].includes(run.run.process) &&
          run.run.executionRoot === path,
      ),
    );

  const pendingDialogs = () =>
    runtime()?.runs.flatMap((run) =>
      (run.rpc?.pendingDialogs ?? []).map((dialog) => ({ runId: run.run.id, dialog })),
    ) ?? [];

  const runIds = () => runtime()?.runs.map((run) => run.run.id) ?? [];

  const runById = (runId: string) => runtime()?.runs.find((run) => run.run.id === runId);

  const composerState = (run: RunHydration) => {
    let state = composerStates.get(run.run.id);
    if (!state) {
      state = createComposerState(run.run.id, run.draft?.text ?? "");
      composerStates.set(run.run.id, state);
    }
    return state;
  };

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
        await refreshHydration();
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

    onCleanup(() => {
      disposed = true;
      hydrationNeededRuns.clear();
      for (const unlisten of unlisteners) unlisten();
    });
  });

  return (
    <main class="foundation-shell">
      <h1>Pi Wizard</h1>
      <p class="subtitle">Runtime integration</p>

      <section class="runtime-status" aria-label="Runtime status">
        <div>
          <span>Backend</span>
          <Show when={runtime()} fallback={<strong>Connecting</strong>}>
            {(snapshot) => (
              <strong>
                Ready · schema {snapshot().schemaVersion} · revision {snapshot().runtimeRevision} · {snapshot().runs.length} runs
              </strong>
            )}
          </Show>
        </div>
        <Show when={runtimeError()}>
          {(error) => <p class="error">Backend runtime failed: {error()}</p>}
        </Show>

        <div>
          <span>Delivery</span>
          <strong>{deliveredEvents()} normalized runtime events</strong>
        </div>

        <div>
          <span>Pi</span>
          <Show when={piProbe()} fallback={<strong>Probing</strong>}>
            {(report) => (
              <strong>
                {report().version.display} · {report().environment.pathSource}
              </strong>
            )}
          </Show>
        </div>
        <Show when={piProbeError()}>
          {(error) => <p class="error">Pi discovery failed: {error()}</p>}
        </Show>
      </section>

      <ProjectLauncher
        piReady={Boolean(piProbe())}
        onStarted={refreshHydration}
        isSessionActive={isSessionActive}
        isExecutionRootActive={isExecutionRootActive}
      />

      <Show when={pendingDialogs().length > 0}>
        <section class="attention" aria-label="Needs attention">
          <h2>Needs Attention</h2>
          <For each={pendingDialogs()}>
            {(pending) => (
              <ExtensionDialogCard
                runId={pending.runId}
                dialog={pending.dialog}
                onResolved={refreshHydration}
              />
            )}
          </For>
        </section>
      </Show>

      <Show when={(runtime()?.runs.length ?? 0) > 0}>
        <section class="composers" aria-label="Run composers">
          <h2>Composers</h2>
          <For each={runIds()}>
            {(runId) => (
              <Show when={runById(runId)}>
                {(run) => (
                  <ComposerCard
                    run={run()}
                    state={composerState(run())}
                    attachmentLimits={attachmentLimits()}
                    onResolved={refreshHydration}
                  />
                )}
              </Show>
            )}
          </For>
        </section>
      </Show>
    </main>
  );
}
