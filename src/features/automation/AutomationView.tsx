import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import { pathLeaf } from "../../lib/path";
import { ModelPicker } from "../models/ModelPicker";
import type { ModelSelection, ThinkingLevel } from "../models/types";
import type { DesktopProjectRecord } from "../../types/projects";
import type {
  AutomationChain,
  DesktopAutomationSnapshot,
} from "./types";

interface AutomationViewProps {
  snapshot: DesktopAutomationSnapshot | undefined;
  projects: DesktopProjectRecord[];
  onRefresh: (projectId?: string) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
}

interface PromptChainViewDraft {
  chainId: string | undefined;
  name: string;
  prompts: string[];
  projectId: string;
  model: ModelSelection | undefined;
  thinking: ThinkingLevel | "";
  autoOpenSavedChain: boolean;
}

let promptChainViewDraft: PromptChainViewDraft = {
  chainId: undefined,
  name: "",
  prompts: [""],
  projectId: "",
  model: undefined,
  thinking: "",
  autoOpenSavedChain: true,
};

export function AutomationView(props: AutomationViewProps) {
  const [chainId, setChainId] = createSignal<string | undefined>(promptChainViewDraft.chainId);
  const [name, setName] = createSignal(promptChainViewDraft.name);
  const [prompts, setPrompts] = createSignal<string[]>([...promptChainViewDraft.prompts]);
  const [projectId, setProjectId] = createSignal(promptChainViewDraft.projectId);
  const [model, setModel] = createSignal<ModelSelection | undefined>(promptChainViewDraft.model);
  const [thinking, setThinking] = createSignal<ThinkingLevel | "">(promptChainViewDraft.thinking);
  const [autoOpenSavedChain, setAutoOpenSavedChain] = createSignal(
    promptChainViewDraft.autoOpenSavedChain,
  );
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let chainNameInput: HTMLInputElement | undefined;

  const loadChain = (chain: AutomationChain) => {
    setChainId(chain.id);
    setName(chain.name);
    setPrompts([...chain.prompts]);
    setAutoOpenSavedChain(false);
    setError(undefined);
    queueMicrotask(() => chainNameInput?.focus());
  };

  createEffect(() => {
    promptChainViewDraft = {
      chainId: chainId(),
      name: name(),
      prompts: [...prompts()],
      projectId: projectId(),
      model: model(),
      thinking: thinking(),
      autoOpenSavedChain: autoOpenSavedChain(),
    };
  });

  createEffect(() => {
    const currentProjectId = projectId();
    if (currentProjectId && props.snapshot?.projectId !== currentProjectId) {
      void props.onRefresh(currentProjectId);
    }
  });

  createEffect(() => {
    if (!projectId()) {
      const project = props.projects.find((candidate) => candidate.status === "present");
      if (project) setProjectId(project.id);
    }
  });

  const selectedProject = () =>
    props.projects.find((project) => project.id === projectId() && project.status === "present");

  const visibleCatalog = () =>
    props.snapshot?.projectId === projectId() ? props.snapshot.catalog : undefined;

  createEffect(() => {
    const catalog = visibleCatalog();
    if (!catalog || !autoOpenSavedChain() || chainId()) return;
    const saved = catalog.chains[0];
    if (saved) loadChain(saved);
    else setAutoOpenSavedChain(false);
  });

  const newChain = () => {
    setChainId(undefined);
    setName("");
    setPrompts([""]);
    setAutoOpenSavedChain(false);
    setError(undefined);
  };

  const switchProject = (nextProjectId: string) => {
    setChainId(undefined);
    setName("");
    setPrompts([""]);
    setAutoOpenSavedChain(true);
    setError(undefined);
    setProjectId(nextProjectId);
    setThinking("");
  };

  const updatePrompt = (index: number, value: string) =>
    setPrompts((current) =>
      current.map((prompt, currentIndex) => (currentIndex === index ? value : prompt)),
    );

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
    const project = projectId();
    if (!project) {
      setError("Select a project before saving this chain.");
      return;
    }
    setBusy(true);
    setError(undefined);
    try {
      const saved = await invokeDesktop<AutomationChain>("runtime_save_automation_chain", {
        request: {
          projectId: project,
          id: chainId() ?? null,
          name: name(),
          prompts: prompts(),
        },
      });
      setChainId(saved.id);
      setName(saved.name);
      setPrompts([...saved.prompts]);
      await props.onRefresh(project);
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const deleteChain = async () => {
    const id = chainId();
    const project = projectId();
    if (!id || !project || busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<boolean>("runtime_delete_automation_chain", {
        request: { projectId: project, id },
      });
      newChain();
      setAutoOpenSavedChain(true);
      await props.onRefresh(project);
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const startChain = async () => {
    const id = chainId();
    const project = projectId();
    if (!id) {
      setError("Save the chain before starting it.");
      return;
    }
    if (!project) return;
    setBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<string>("runtime_start_automation", {
        request: {
          chainId: id,
          projectId: project,
          provider: model()?.provider ?? null,
          model: model()?.id ?? null,
          thinking: thinking() || null,
        },
      });
      await props.onRefresh(project);
    } catch (caught) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const cancelExecution = async (id: string) => {
    try {
      await invokeDesktop<void>("runtime_cancel_automation", { request: { id } });
      await props.onRefresh(projectId() || undefined);
    } catch (caught) {
      setError(String(caught));
    }
  };

  return (
    <section class="automation-surface" aria-label="Prompt chains">
      <header class="surface-heading">
        <h1>Prompt chains</h1>
        <button type="button" onClick={newChain}>New chain</button>
      </header>

      <Show when={visibleCatalog()?.recoveryNotice}>
        {(notice) => <p class="app-error">Saved prompt chains were reset: {notice()}</p>}
      </Show>
      <Show when={error()}>{(message) => <p class="app-error">Prompt chains: {message()}</p>}</Show>

      <div class="automation-layout">
        <aside class="automation-library" aria-label="Saved chains">
          <strong>Saved chains</strong>
          <For each={visibleCatalog()?.chains ?? []}>
            {(chain) => (
              <button
                type="button"
                class={chainId() === chain.id ? "active" : undefined}
                aria-pressed={chainId() === chain.id}
                aria-controls="prompt-chain-editor"
                onClick={() => loadChain(chain)}
              >
                <strong>{chain.name}</strong>
                <span>{chain.prompts.length} prompt{chain.prompts.length === 1 ? "" : "s"}</span>
              </button>
            )}
          </For>
          <Show when={(visibleCatalog()?.chains.length ?? 0) === 0}>
            <span class="sidebar-note">No saved chains.</span>
          </Show>
        </aside>

        <div class="automation-builder" id="prompt-chain-editor">
          <label class="automation-name">
            <span>Chain name</span>
            <input
              ref={chainNameInput}
              value={name()}
              onInput={(event) => setName(event.currentTarget.value)}
            />
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
                    aria-label={`Prompt chain prompt ${index() + 1}`}
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
              <select
                value={projectId()}
                onChange={(event) => switchProject(event.currentTarget.value)}
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
          </div>

          <ModelPicker
            projectPath={selectedProject()?.canonicalRoot ?? ""}
            disabled={busy()}
            model={model()}
            thinking={thinking()}
            onModelChange={setModel}
            onThinkingChange={setThinking}
            rememberNewRunSelection
            label="Model and thinking"
          />

          <div class="automation-builder-actions">
            <button type="button" disabled={busy()} onClick={() => void saveChain()}>{busy() ? "Working" : "Save"}</button>
            <Show when={chainId()}>
              <button type="button" disabled={busy()} onClick={() => void deleteChain()}>Delete</button>
              <button type="button" class="primary-action" disabled={busy() || !projectId()} onClick={() => void startChain()}>
                Run chain
              </button>
            </Show>
          </div>
        </div>
      </div>

      <section class="automation-executions" aria-label="Prompt chain runs">
        <h2>Chain runs</h2>
        <For each={props.snapshot?.executions ?? []}>
          {(execution) => (
            <article class="automation-execution">
              <header>
                <div>
                  <strong>{execution.chainName}</strong>
                  <span>
                    {execution.status.replaceAll("_", " ")} · {execution.steps.length} prompt{execution.steps.length === 1 ? "" : "s"}
                  </span>
                </div>
                <Show when={!['completed', 'completed_with_errors', 'cancelled', 'failed'].includes(execution.status)}>
                  <button type="button" onClick={() => void cancelExecution(execution.id)}>Cancel chain</button>
                </Show>
              </header>
              <Show when={execution.error}>
                {(message) => <p class="error">Execution failed: {message()}</p>}
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
          <p class="empty-state">No prompt chains run yet.</p>
        </Show>
      </section>
    </section>
  );
}
