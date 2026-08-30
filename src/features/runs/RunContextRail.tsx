import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";

import { runElapsedLabel, runThinkingLabel } from "./history";
import { runActivityLabel, runStateLabel, runStatusTone, runTitle } from "./presentation";
import type { RunHydration, SessionStats } from "./types";

function formatTokenCount(value: number): string {
  return value.toLocaleString();
}

function formatSessionCost(value: number): string {
  if (!Number.isFinite(value)) return "Unknown";
  return `$${value.toFixed(value >= 1 ? 2 : 4)}`;
}

function isTerminal(run: RunHydration): boolean {
  return ["exited", "failed", "quarantined"].includes(run.run.process);
}

export function RunContextRail(props: {
  run: RunHydration | undefined;
  runs: RunHydration[];
  nowUnixMs: number;
  onOpenRun: (runId: string) => void;
}) {
  const [stats, setStats] = createSignal<SessionStats>();
  const [statsBusy, setStatsBusy] = createSignal(false);
  const [statsError, setStatsError] = createSignal<string>();
  let statsSequence = 0;
  let observedSessionId: string | undefined;
  let observedWorking = false;

  const refreshStats = async () => {
    const run = props.run;
    const sessionId = run?.run.session.sessionId;
    if (!run || !sessionId || run.run.process !== "ready") return;
    const sequence = ++statsSequence;
    setStatsBusy(true);
    setStatsError(undefined);
    try {
      const result = await invokeDesktop<SessionStats>("runtime_session_stats", {
        request: { runId: run.run.id },
      });
      if (
        sequence !== statsSequence ||
        props.run?.run.id !== run.run.id ||
        props.run?.run.session.sessionId !== sessionId
      )
        return;
      setStats(result);
    } catch (error) {
      if (sequence === statsSequence) setStatsError(String(error));
    } finally {
      if (sequence === statsSequence) setStatsBusy(false);
    }
  };

  createEffect(() => {
    const run = props.run;
    const sessionId = run?.run.session.sessionId ?? undefined;
    const working = Boolean(run?.run.agentWorking);
    const sessionChanged = sessionId !== observedSessionId;
    const justSettled = observedWorking && !working;
    observedWorking = working;

    if (sessionChanged) {
      observedSessionId = sessionId;
      statsSequence += 1;
      setStats(undefined);
      setStatsError(undefined);
      setStatsBusy(false);
    }
    if (run?.run.process === "ready" && sessionId && (sessionChanged || justSettled)) {
      void refreshStats();
    }
  });

  onCleanup(() => {
    statsSequence += 1;
  });

  const runCounts = () => {
    const counts = { active: 0, ready: 0, warning: 0, danger: 0, done: 0 };
    for (const run of props.runs) {
      if (run.run.process === "exited") {
        counts.done += 1;
        continue;
      }
      const tone = runStatusTone(run);
      if (tone === "active") counts.active += 1;
      else if (tone === "ready") counts.ready += 1;
      else if (tone === "warning") counts.warning += 1;
      else if (tone === "danger") counts.danger += 1;
    }
    return counts;
  };

  const liveRuns = () => props.runs.filter((run) => !isTerminal(run)).slice(0, 6);
  const contextPercent = () => {
    const percent = stats()?.contextUsage?.percent;
    return percent === null || percent === undefined
      ? undefined
      : Math.min(100, Math.max(0, percent));
  };

  return (
    <aside class="app-context-rail" aria-label="Run overview and usage">
      <Show
        when={props.run}
        fallback={
          <>
            <section class="context-rail-section">
              <header>
                <strong>Run overview</strong>
                <span>{props.runs.length} retained</span>
              </header>
              <div class="run-overview-counts">
                <div class="overview-active"><strong>{runCounts().active}</strong><span>Active</span></div>
                <div class="overview-ready"><strong>{runCounts().ready}</strong><span>Ready</span></div>
                <div class="overview-warning"><strong>{runCounts().warning}</strong><span>Attention</span></div>
                <div class="overview-danger"><strong>{runCounts().danger}</strong><span>Problems</span></div>
                <Show when={runCounts().done > 0}>
                  <div class="overview-neutral"><strong>{runCounts().done}</strong><span>Finished</span></div>
                </Show>
              </div>
            </section>
            <section class="context-rail-section">
              <header>
                <strong>Live now</strong>
                <span>{liveRuns().length}</span>
              </header>
              <div class="context-run-list">
                <For each={liveRuns()}>
                  {(run) => (
                    <button type="button" onClick={() => props.onOpenRun(run.run.id)}>
                      <span class={`status-dot tone-${runStatusTone(run)}`} aria-hidden="true" />
                      <span>
                        <strong>{runTitle(run)}</strong>
                        <small>{runActivityLabel(run)}</small>
                      </span>
                    </button>
                  )}
                </For>
                <Show when={liveRuns().length === 0}>
                  <p class="context-rail-note">No live Pi processes.</p>
                </Show>
              </div>
            </section>
          </>
        }
      >
        {(run) => (
          <>
            <section class="context-rail-section">
              <header>
                <strong>Current run</strong>
                <span class={`context-state tone-${runStatusTone(run())}`}>
                  {runStateLabel(run())}
                </span>
              </header>
              <div class="current-run-activity">
                <span class={`status-dot tone-${runStatusTone(run())}`} aria-hidden="true" />
                <div>
                  <strong>{runActivityLabel(run())}</strong>
                  <small>{run().run.session.model?.name ?? run().run.session.model?.id ?? "Model pending"}</small>
                </div>
              </div>
            </section>

            <section class="context-rail-section run-facts">
              <header><strong>Run facts</strong><span>Backend state</span></header>
              <dl class="usage-grid">
                <div><dt>Elapsed</dt><dd>{runElapsedLabel(run(), props.nowUnixMs)}</dd></div>
                <div><dt>Thinking</dt><dd>{runThinkingLabel(run())}</dd></div>
                <div><dt>Messages</dt><dd>{run().run.session.messageCount ?? "Unknown"}</dd></div>
                <div><dt>Queued</dt><dd>{run().run.queue.steering + run().run.queue.followUp}</dd></div>
                <div><dt>Active tools</dt><dd>{run().rpc?.live.activeTools.length ?? 0}</dd></div>
                <div><dt>Needs input</dt><dd>{run().rpc?.pendingDialogs.length ?? 0}</dd></div>
              </dl>
            </section>

            <section class="context-rail-section session-metrics">
              <header>
                <strong>Session usage</strong>
                <button
                  type="button"
                  disabled={statsBusy() || run().run.process !== "ready" || !run().run.session.sessionId}
                  onClick={() => void refreshStats()}
                >
                  {statsBusy() ? "Reading" : "Refresh"}
                </button>
              </header>
              <Show
                when={stats()}
                fallback={
                  <p class="context-rail-note">
                    {run().run.process === "ready"
                      ? "Pi session token and context metrics load on selection and after each settled turn."
                      : "Session metrics require a live Ready Pi process."}
                  </p>
                }
              >
                {(usage) => (
                  <>
                    <Show when={contextPercent() !== undefined}>
                      <div class="context-usage-meter">
                        <div>
                          <span>Context</span>
                          <strong>{contextPercent()!.toFixed(1)}%</strong>
                        </div>
                        <progress max="100" value={contextPercent()!} aria-label="Current Pi context usage" />
                        <small>
                          {usage().contextUsage?.tokens?.toLocaleString() ?? "Unknown"} / {usage().contextUsage?.contextWindow.toLocaleString()} tokens
                        </small>
                      </div>
                    </Show>
                    <dl class="usage-grid">
                      <div><dt>Total tokens</dt><dd>{formatTokenCount(usage().tokens.total)}</dd></div>
                      <div><dt>Input</dt><dd>{formatTokenCount(usage().tokens.input)}</dd></div>
                      <div><dt>Output</dt><dd>{formatTokenCount(usage().tokens.output)}</dd></div>
                      <div><dt>Cache read</dt><dd>{formatTokenCount(usage().tokens.cacheRead)}</dd></div>
                      <div><dt>Cache write</dt><dd>{formatTokenCount(usage().tokens.cacheWrite)}</dd></div>
                      <div><dt>Pi cost</dt><dd>{formatSessionCost(usage().cost)}</dd></div>
                      <div><dt>User turns</dt><dd>{usage().userMessages}</dd></div>
                      <div><dt>Tool calls</dt><dd>{usage().toolCalls}</dd></div>
                    </dl>
                  </>
                )}
              </Show>
              <Show when={statsError()}>{(error) => <p class="error">Usage failed: {error()}</p>}</Show>
            </section>

          </>
        )}
      </Show>
    </aside>
  );
}
