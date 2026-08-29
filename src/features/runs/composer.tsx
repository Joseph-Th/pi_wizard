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

import { diffPageSegments, formatBytes } from "./types";
import type { CommandSummary, CompactionResult, ComposerAction, ComposerAvailability, ComposerSubmitResult, DraftSnapshot, ExtensionUiSnapshot, GitDiffCursor, GitFileDiffPage, GitReviewSummary, LiveProjectionSnapshot, ModelSummary, PendingExtensionDialog, RunHydration, RuntimeAttachmentLimits, RuntimeStopResult, SessionStats, ThinkingLevel } from "./types";
import { HistoryTimeline, SessionTreeInspector } from "./history";
import { runHasStoppableActivity } from "./presentation";

export interface RuntimeManagerSignal {
  kind: "runDirty";
  runId: string;
}

export interface RuntimeUiEvent {
  kind: string;
  runId?: string;
  message?: string;
  notifyType?: "info" | "warning" | "error";
}

export interface UiNotification {
  id: number;
  runId: string;
  message: string;
  notifyType: "info" | "warning" | "error";
}

export interface RuntimeUiDrain {
  events: RuntimeUiEvent[];
  rehydrateRequired: boolean;
  pendingEditorText: string | null;
  hasMore: boolean;
}

export interface PiProbeReport {
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

export function fileToBase64(file: File): Promise<string> {
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

export type ExtensionDialogResponse =
  | { kind: "value"; id: string; value: string }
  | { kind: "confirmation"; id: string; confirmed: boolean }
  | { kind: "cancelled"; id: string };

export function createComposerState(runId: string, initialText: string) {
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

export type ComposerState = ReturnType<typeof createComposerState>;

export function DroppedBytes(props: { count: number }) {
  return (
    <Show when={props.count > 0}>
      <span class="truncation-note">Oldest {props.count} bytes omitted</span>
    </Show>
  );
}

export function LiveTimeline(props: { live: LiveProjectionSnapshot | undefined }) {
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

export function modelKey(model: ModelSummary): string {
  return `${encodeURIComponent(model.provider)}:${encodeURIComponent(model.id)}`;
}

export function parseModelKey(value: string): { provider: string; modelId: string } | undefined {
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

export function slashCommandName(text: string): string | undefined {
  const trimmed = text.trimStart();
  if (!trimmed.startsWith("/")) return undefined;
  const command = trimmed.slice(1).split(/\s/, 1)[0];
  return command && command.length > 0 ? command : undefined;
}

export function ComposerCard(props: {
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

