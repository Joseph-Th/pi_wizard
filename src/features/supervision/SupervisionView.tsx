import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import { pathLeaf } from "../../lib/path";
import type { DesktopProjectRecord } from "../../types/projects";
import type { AutomationChain, RuntimeCapacitySnapshot } from "../automation/types";
import { ModelPicker } from "../models/ModelPicker";
import type { ModelSelection, ThinkingLevel } from "../models/types";
import type { SupervisionSnapshot } from "./types";

interface SupervisionViewProps {
  snapshots: SupervisionSnapshot[];
  projects: DesktopProjectRecord[];
  chains: AutomationChain[];
  capacity: RuntimeCapacitySnapshot | undefined;
  piReady: boolean;
  onRefresh: () => Promise<unknown>;
  onOpenRun: (runId: string) => void;
}

export function SupervisionView(props: SupervisionViewProps) {
  const [projectIds, setProjectIds] = createSignal<string[]>([]);
  const [playbookChainId, setPlaybookChainId] = createSignal("");
  const [model, setModel] = createSignal<ModelSelection>();
  const [thinking, setThinking] = createSignal<ThinkingLevel | "">("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let selectionInitialized = false;

  createEffect(() => {
    const available = props.projects
      .filter((project) => project.status === "present")
      .map((project) => project.id);
    const availableSet = new Set(available);
    setProjectIds((current) => {
      if (!selectionInitialized) {
        selectionInitialized = true;
        return available;
      }
      return current.filter((projectId) => availableSet.has(projectId));
    });
  });

  const selectedPlaybook = () =>
    props.chains.find((chain) => chain.id === playbookChainId());

  const overlappingSupervision = () => {
    const selected = new Set(projectIds());
    if (selected.size === 0) return undefined;
    return props.snapshots.find(
      (snapshot) =>
        !["completed", "stopped", "failed"].includes(snapshot.status) &&
        snapshot.projectIds.some((projectId) => selected.has(projectId)),
    );
  };

  const projectNames = (ids: string[]) => {
    const names = ids.map((id) => {
      const project = props.projects.find((candidate) => candidate.id === id);
      return project ? pathLeaf(project.canonicalRoot) : id.slice(0, 8);
    });
    if (names.length <= 4) return names.join(", ");
    return `${names.slice(0, 4).join(", ")} +${names.length - 4}`;
  };

  const toggleProject = (projectId: string, selected: boolean) => {
    setProjectIds((current) =>
      selected
        ? current.includes(projectId)
          ? current
          : [...current, projectId]
        : current.filter((id) => id !== projectId),
    );
  };

  const start = async () => {
    const projects = projectIds();
    if (projects.length === 0 || busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<string>("runtime_start_supervision", {
        request: {
          projectIds: projects,
          provider: model()?.provider ?? null,
          model: model()?.id ?? null,
          thinking: thinking() || null,
          promptTemplates: selectedPlaybook()?.prompts ?? [],
          maxCycles: null,
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
            Keep live runs moving across multiple projects. The supervisor wakes when a selected
            run becomes idle, reviews its last result, and chooses the next task or stops a run
            that should not continue autonomously.
          </p>
        </div>
      </header>

      <Show when={error()}>{(message) => <p class="app-error">Supervision: {message()}</p>}</Show>

      <section class="supervision-config" aria-label="Start supervision">
        <div class="supervision-projects">
          <header>
            <div>
              <strong>Projects</strong>
              <span>{projectIds().length} selected</span>
            </div>
            <div>
              <button
                type="button"
                disabled={busy()}
                onClick={() =>
                  setProjectIds(
                    props.projects
                      .filter((project) => project.status === "present")
                      .map((project) => project.id),
                  )
                }
              >
                Select all
              </button>
              <button type="button" disabled={busy()} onClick={() => setProjectIds([])}>
                Clear
              </button>
            </div>
          </header>
          <div class="supervision-project-list" aria-label="Supervised projects">
            <For each={props.projects}>
              {(project) => (
                <label class={project.status === "present" ? undefined : "project-unavailable"}>
                  <input
                    type="checkbox"
                    checked={projectIds().includes(project.id)}
                    disabled={busy() || project.status !== "present"}
                    onChange={(event) => toggleProject(project.id, event.currentTarget.checked)}
                  />
                  <span>
                    <strong>{pathLeaf(project.canonicalRoot)}</strong>
                    <small title={project.canonicalRoot}>
                      {project.status === "present" ? project.canonicalRoot : `${project.status} · ${project.canonicalRoot}`}
                    </small>
                  </span>
                </label>
              )}
            </For>
          </div>
        </div>

        <label class="supervision-playbook">
          <span>Reusable prompt playbook</span>
          <select
            value={playbookChainId()}
            disabled={busy()}
            onChange={(event) => setPlaybookChainId(event.currentTarget.value)}
          >
            <option value="">No saved playbook · choose tasks from project state</option>
            <For each={props.chains}>
              {(chain) => (
                <option value={chain.id}>
                  {chain.name} · {chain.prompts.length} prompt{chain.prompts.length === 1 ? "" : "s"}
                </option>
              )}
            </For>
          </select>
          <small>
            Saved Automation prompts are guidance, not a fixed sequence. The supervisor chooses,
            adapts, or skips them based on each run's current result.
          </small>
        </label>

        <ModelPicker
          projectPath=""
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
              projectIds().length === 0 ||
              Boolean(overlappingSupervision()) ||
              Boolean(props.capacity && props.capacity.activeRuns >= props.capacity.liveRunLimit)
            }
            onClick={() => void start()}
          >
            {busy() ? "Working" : "Start supervision"}
          </button>
          <Show when={overlappingSupervision()}>
            {(active) => (
              <span class="supervision-note">
                At least one selected project is already covered by supervision {active().id.slice(0, 8)}.
              </span>
            )}
          </Show>
          <Show when={!overlappingSupervision() && projectIds().length > 0}>
            <span class="supervision-note">
              Continuous until stopped · {projectNames(projectIds())}
            </span>
          </Show>
        </div>
      </section>

      <section class="supervision-history" aria-label="Supervision sessions">
        <h2>Sessions</h2>
        <For each={props.snapshots}>
          {(snapshot) => {
            const terminal = () => ["completed", "stopped", "failed"].includes(snapshot.status);
            return (
              <article class="supervision-session">
                <header>
                  <div>
                    <strong>{projectNames(snapshot.projectIds)}</strong>
                    <span>
                      {snapshot.status} · {snapshot.cycles}{snapshot.maxCycles == null ? " decisions · continuous" : `/${snapshot.maxCycles} decisions`} · {snapshot.watchedRuns} watched run{snapshot.watchedRuns === 1 ? "" : "s"}
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
                <Show when={snapshot.lastDecision}>
                  {(decision) => (
                    <p class="supervision-last-decision">
                      <strong>Last decision</strong>
                      <span>{decision()}</span>
                    </p>
                  )}
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
