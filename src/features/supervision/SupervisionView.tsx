import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import { pathLeaf } from "../../lib/path";
import type { DesktopProjectRecord } from "../../types/projects";
import type { RuntimeCapacitySnapshot } from "../automation/types";
import { ModelPicker } from "../models/ModelPicker";
import type { ModelSelection, ThinkingLevel } from "../models/types";
import type { SupervisionSnapshot } from "./types";

interface SupervisionViewProps {
  snapshots: SupervisionSnapshot[];
  projects: DesktopProjectRecord[];
  capacity: RuntimeCapacitySnapshot | undefined;
  piReady: boolean;
  onRefresh: () => Promise<unknown>;
  onOpenRun: (runId: string) => void;
}

export function SupervisionView(props: SupervisionViewProps) {
  const [projectId, setProjectId] = createSignal("");
  const [model, setModel] = createSignal<ModelSelection>();
  const [thinking, setThinking] = createSignal<ThinkingLevel | "">("");
  const [maxCycles, setMaxCycles] = createSignal("12");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();

  createEffect(() => {
    if (!projectId()) {
      const project = props.projects.find((candidate) => candidate.status === "present");
      if (project) setProjectId(project.id);
    }
  });

  const selectedProject = () =>
    props.projects.find((project) => project.id === projectId() && project.status === "present");

  const activeForSelectedProject = () =>
    props.snapshots.find(
      (snapshot) =>
        snapshot.projectId === projectId() &&
        !["completed", "stopped", "failed"].includes(snapshot.status),
    );

  const start = async () => {
    const project = projectId();
    const cycles = Number.parseInt(maxCycles(), 10);
    if (!project || !Number.isInteger(cycles) || cycles < 1 || busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<string>("runtime_start_supervision", {
        request: {
          projectId: project,
          provider: model()?.provider ?? null,
          model: model()?.id ?? null,
          thinking: thinking() || null,
          maxCycles: cycles,
        },
      });
      await props.onRefresh();
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const stop = async (id: string) => {
    if (busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<void>("runtime_stop_supervision", { request: { id } });
      await props.onRefresh();
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section class="supervision-surface" aria-label="Supervision">
      <header class="surface-heading">
        <div>
          <h1>Supervision</h1>
          <p>
            Run one independent Pi supervisor over live sessions for a project. It is not part of
            an automation chain and consumes one normal live-run slot while active.
          </p>
        </div>
      </header>

      <Show when={error()}>{(message) => <p class="app-error">Supervision: {message()}</p>}</Show>

      <section class="supervision-config" aria-label="Start supervision">
        <div class="supervision-grid">
          <label>
            <span>Project</span>
            <select
              value={projectId()}
              disabled={busy()}
              onChange={(event) => {
                setProjectId(event.currentTarget.value);
                setModel(undefined);
                setThinking("");
              }}
            >
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
            <span>Maximum supervisor cycles</span>
            <input
              type="number"
              min="1"
              value={maxCycles()}
              disabled={busy()}
              onInput={(event) => setMaxCycles(event.currentTarget.value)}
            />
          </label>
        </div>

        <ModelPicker
          projectPath={selectedProject()?.canonicalRoot ?? ""}
          disabled={busy()}
          contextFiles="disabled"
          model={model()}
          thinking={thinking()}
          onModelChange={setModel}
          onThinkingChange={setThinking}
          label="Supervisor model and thinking"
          description="The supervisor has context files and extensions disabled and works through bounded directives only."
        />

        <div class="supervision-actions">
          <button
            type="button"
            class="primary-action"
            disabled={
              busy() ||
              !projectId() ||
              Boolean(activeForSelectedProject()) ||
              Boolean(props.capacity && props.capacity.activeRuns >= props.capacity.liveRunLimit)
            }
            onClick={() => void start()}
          >
            {busy() ? "Working" : "Start supervision"}
          </button>
          <Show when={activeForSelectedProject()}>
            {(active) => (
              <span class="supervision-note">
                This project already has active supervision {active().id.slice(0, 8)}.
              </span>
            )}
          </Show>
        </div>
      </section>

      <section class="supervision-history" aria-label="Supervision sessions">
        <h2>Sessions</h2>
        <For each={props.snapshots}>
          {(snapshot) => {
            const project = () => props.projects.find((item) => item.id === snapshot.projectId);
            const terminal = () => ["completed", "stopped", "failed"].includes(snapshot.status);
            return (
              <article class="supervision-session">
                <header>
                  <div>
                    <strong>{project() ? pathLeaf(project()!.canonicalRoot) : snapshot.projectId.slice(0, 8)}</strong>
                    <span>
                      {snapshot.status} · {snapshot.cycles}/{snapshot.maxCycles} cycles · {snapshot.watchedRuns} watched run{snapshot.watchedRuns === 1 ? "" : "s"}
                    </span>
                    <small>
                      {snapshot.provider && snapshot.model
                        ? `${snapshot.provider}/${snapshot.model}`
                        : "Pi default model"}
                      {snapshot.thinking ? ` · ${snapshot.thinking} thinking` : ""}
                    </small>
                  </div>
                  <div>
                    <Show when={snapshot.supervisorRunId}>
                      {(runId) => <button type="button" onClick={() => props.onOpenRun(runId())}>Open supervisor</button>}
                    </Show>
                    <Show when={!terminal()}>
                      <button type="button" disabled={busy()} onClick={() => void stop(snapshot.id)}>Stop supervision</button>
                    </Show>
                  </div>
                </header>
                <Show when={snapshot.error}>
                  {(message) => <p class="error">Supervisor failed: {message()}</p>}
                </Show>
              </article>
            );
          }}
        </For>
        <Show when={props.snapshots.length === 0}>
          <p class="empty-state">No supervision sessions yet.</p>
        </Show>
      </section>
    </section>
  );
}
