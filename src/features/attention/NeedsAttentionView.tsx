import { For, Show } from "solid-js";

import { ExtensionDialogCard } from "./ExtensionDialogCard";
import type { AttentionRun } from "./types";

function runTitle(run: AttentionRun): string {
  return (
    run.run.session.sessionName ??
    run.run.session.sessionId?.slice(0, 12) ??
    `Run ${run.run.id.slice(0, 8)}`
  );
}

export function NeedsAttentionView(props: {
  runs: AttentionRun[];
  onOpenRun: (runId: string) => void;
  onResolved: () => Promise<unknown>;
  projectLabel: (projectId: string) => string;
}) {
  const pending = () =>
    props.runs
      .flatMap((run) =>
        (run.rpc?.pendingDialogs ?? []).map((dialog) => ({ run, dialog })),
      )
      .sort((left, right) => {
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
                      {props.projectLabel(pendingRequest.run.run.projectId)} · {pendingRequest.run.run.executionIsolation === "git_worktree"
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
