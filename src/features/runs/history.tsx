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
import { ProjectManager } from "../projects/ProjectManager";
import { ProjectLauncher } from "../projects/ProjectLauncher";
import { SessionCatalogBrowser } from "../sessions/SessionCatalogBrowser";
import { RecentSessionsView } from "../sessions/RecentSessionsView";
import { SupervisionView } from "../supervision/SupervisionView";
import type { SupervisionSnapshot } from "../supervision/types";
import { invokeDesktop } from "../../lib/desktop";
import { pathLeaf } from "../../lib/path";

import { formatBytes } from "./types";
import type { RunHydration, SessionHistoryCursor, SessionHistoryPage, SessionTimelineItem, SessionTreeNode, SessionTreeSnapshot } from "./types";

export function historyLabel(item: SessionTimelineItem): string {
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

export function historyTimestamp(timestamp: string | null): string | undefined {
  if (!timestamp) return undefined;
  const parsed = Date.parse(timestamp);
  return Number.isNaN(parsed) ? timestamp : new Date(parsed).toLocaleString();
}

export function HistoryTimeline(props: {
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

export function runModelLabel(run: RunHydration): string {
  const model = run.run.session.model;
  if (!model) return "Model pending";
  return model.name ? `${model.name} · ${model.provider}` : `${model.provider}/${model.id}`;
}

export function runThinkingLabel(run: RunHydration): string {
  return run.run.session.thinkingLevel ? `Thinking ${run.run.session.thinkingLevel}` : "Thinking pending";
}

export function formatElapsedDuration(elapsedMs: number): string {
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

export function runElapsedLabel(run: RunHydration, nowUnixMs: number): string {
  const end = run.run.terminalUnixMs ?? nowUnixMs;
  return `${formatElapsedDuration(end - run.run.startedUnixMs)} elapsed`;
}

export function isTerminalRun(run: RunHydration): boolean {
  return ["exited", "failed", "quarantined"].includes(run.run.process);
}

export function SessionTreeInspector(props: {
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

