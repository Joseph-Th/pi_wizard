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

import type { ExtensionUiSnapshot, RunHydration } from "./types";
import type { RuntimeUiEvent } from "./composer";

export function needsHydration(events: RuntimeUiEvent[]): boolean {
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

export function runTitle(run: RunHydration): string {
  return (
    run.run.session.sessionName ??
    run.run.session.sessionId?.slice(0, 12) ??
    `Run ${run.run.id.slice(0, 8)}`
  );
}

export function runStateLabel(run: RunHydration): string {
  if (run.run.process === "exited") return "done";
  if (run.run.process === "failed") return "failed";
  if (run.run.process === "quarantined") return "termination uncertain";
  if (run.run.process !== "ready") return run.run.process.replaceAll("_", " ");
  if ((run.rpc?.pendingDialogs.length ?? 0) > 0) return "needs attention";
  if (run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished) return "summary retry";
  if (run.run.compacting) return "compacting";
  if (run.rpc?.retry && !run.rpc.retry.finished) return "retrying";
  if (run.rpc?.streamStalled) return "possibly stalled";
  if ((run.rpc?.live.directBash.length ?? 0) > 0) return "command running";
  if (run.run.agentWorking) return "working";
  if (run.run.queue.steering + run.run.queue.followUp > 0) return "queued";
  return "ready";
}

export type RunStatusTone = "active" | "ready" | "warning" | "danger" | "neutral";

export function runStatusTone(run: RunHydration): RunStatusTone {
  if (run.run.process === "failed" || run.run.process === "quarantined") return "danger";
  if (run.run.process === "exited") return "neutral";
  if (
    run.run.process === "stopping" ||
    (run.rpc?.pendingDialogs.length ?? 0) > 0 ||
    Boolean(run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished) ||
    Boolean(run.rpc?.retry && !run.rpc.retry.finished) ||
    Boolean(run.rpc?.streamStalled)
  )
    return "warning";
  if (
    run.run.process === "starting" ||
    run.run.compacting ||
    run.run.agentWorking ||
    (run.rpc?.live.directBash.length ?? 0) > 0 ||
    run.run.queue.steering + run.run.queue.followUp > 0
  )
    return "active";
  if (run.run.process === "ready") return "ready";
  return "neutral";
}

export function runQueuedCount(run: RunHydration): number {
  return run.run.queue.steering + run.run.queue.followUp;
}

export function toolActivityLabel(toolName: string): string {
  const name = toolName.toLowerCase();
  if (name.includes("read")) return "Reading project files";
  if (name.includes("search") || name.includes("grep") || name.includes("find"))
    return "Searching the codebase";
  if (name.includes("edit") || name.includes("write") || name.includes("patch"))
    return "Editing files";
  if (name.includes("git")) return "Checking repository state";
  if (
    name.includes("bash") ||
    name.includes("shell") ||
    name.includes("exec") ||
    name.includes("run")
  )
    return "Running a command";
  return "Working with a project tool";
}

export function runActivityLabel(run: RunHydration): string {
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
  const tool = run.rpc?.live.activeTools.at(-1);
  if (tool) return toolActivityLabel(tool.toolName);
  if ((run.rpc?.live.directBash.length ?? 0) > 0) return "Running a command";
  if (run.run.agentWorking) return "Pi is working";
  const queued = runQueuedCount(run);
  if (queued > 0) return `${queued} queued message${queued === 1 ? "" : "s"}`;
  if (run.run.process === "ready") return "Ready for input";
  if (run.run.process === "exited") return "Run finished";
  if (run.run.process === "quarantined") return "Process termination is uncertain";
  if (run.run.failure) return `${run.run.failure.kind.replaceAll("_", " ")} failure`;
  return run.run.process.replaceAll("_", " ");
}

export function runHasStoppableActivity(run: RunHydration): boolean {
  return (
    run.run.agentWorking ||
    run.run.compacting ||
    Boolean(run.rpc?.retry && !run.rpc.retry.finished) ||
    Boolean(run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished)
  );
}

export function canCloseRun(run: RunHydration): boolean {
  if (run.run.process === "starting") return true;
  if (run.run.process !== "ready") return false;
  return (
    !run.run.agentWorking &&
    (run.rpc?.live.directBash.length ?? 0) === 0 &&
    run.composerAvailability === "ready" &&
    !run.composerSubmissionPending &&
    !run.draftRestorePending &&
    (run.rpc?.pendingDialogs.length ?? 0) === 0
  );
}

export function runDisplayPriority(run: RunHydration): number {
  if ((run.rpc?.pendingDialogs.length ?? 0) > 0) return 0;
  if (
    run.run.process === "ready" &&
    (run.run.agentWorking ||
      (run.rpc?.live.directBash.length ?? 0) > 0 ||
      Boolean(run.rpc?.retry && !run.rpc.retry.finished) ||
      Boolean(run.rpc?.summarizationRetry && !run.rpc.summarizationRetry.finished))
  )
    return 1;
  if (!["exited", "failed", "quarantined"].includes(run.run.process)) return 2;
  return 3;
}

export function ExtensionUiPanel(props: { snapshot: ExtensionUiSnapshot | undefined }) {
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

export function PiRuntimeNoticePanel(props: { rpc: RunHydration["rpc"] | undefined }) {
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

