import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import {
  decodeModelSelection,
  encodeModelSelection,
  type ContextFilesPolicy,
  type CustomModelProfile,
  type ModelCatalogSnapshot,
  type ModelSelection,
  type ModelSummary,
  type ProjectLaunchOptions,
  type ProjectModelCatalog,
  type ProjectTrustPolicy,
  type ThinkingLevel,
} from "./types";

interface ModelPickerProps {
  projectPath: string;
  disabled?: boolean;
  projectTrust?: ProjectTrustPolicy;
  contextFiles?: ContextFilesPolicy;
  model: ModelSelection | undefined;
  thinking: ThinkingLevel | "";
  onModelChange: (selection: ModelSelection | undefined) => void;
  onThinkingChange: (level: ThinkingLevel | "") => void;
  label?: string;
  description?: string;
}

interface PickerModel extends ModelSummary {
  custom: boolean;
}

function identity(provider: string, model: string): string {
  return `${provider}\u0000${model}`;
}

export function ModelPicker(props: ModelPickerProps) {
  const [discovery, setDiscovery] = createSignal<ProjectModelCatalog>();
  const [options, setOptions] = createSignal<ProjectLaunchOptions>();
  const [catalog, setCatalog] = createSignal<ModelCatalogSnapshot>();
  const [modelsLoading, setModelsLoading] = createSignal(false);
  const [selectionLoading, setSelectionLoading] = createSignal(false);
  const [catalogBusy, setCatalogBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [modelError, setModelError] = createSignal<string>();
  const [selectionError, setSelectionError] = createSignal<string>();
  const [providerDraft, setProviderDraft] = createSignal("");
  const [modelDraft, setModelDraft] = createSignal("");
  const [nameDraft, setNameDraft] = createSignal("");
  let modelSequence = 0;
  let probeSequence = 0;
  let autoLoadKey = "";

  const loadCatalog = async () => {
    try {
      const snapshot = await invokeDesktop<ModelCatalogSnapshot>("runtime_model_catalog");
      setCatalog(snapshot);
      return snapshot;
    } catch (caught) {
      setError(String(caught));
      return undefined;
    }
  };

  const loadModels = async () => {
    const path = props.projectPath.trim();
    const sequence = ++modelSequence;
    setModelsLoading(true);
    setModelError(undefined);
    try {
      const snapshot = await invokeDesktop<ProjectModelCatalog>("runtime_probe_project_models", {
        request: {
          projectPath: path || null,
          projectTrust: props.projectTrust ?? "inherit",
          contextFiles: props.contextFiles ?? "inherit",
        },
      });
      if (sequence === modelSequence && props.projectPath.trim() === path) {
        setDiscovery(snapshot);
      }
      return snapshot;
    } catch (caught) {
      if (sequence === modelSequence && props.projectPath.trim() === path) {
        setModelError(String(caught));
      }
      return undefined;
    } finally {
      if (sequence === modelSequence) setModelsLoading(false);
    }
  };

  const probeSelection = async (selection: ModelSelection | undefined) => {
    const path = props.projectPath.trim();
    if (!path) {
      setOptions(undefined);
      return undefined;
    }
    const sequence = ++probeSequence;
    setSelectionLoading(true);
    setSelectionError(undefined);
    try {
      const snapshot = await invokeDesktop<ProjectLaunchOptions>(
        "runtime_probe_project_launch_options",
        {
          request: {
            projectPath: path,
            projectTrust: props.projectTrust ?? "inherit",
            contextFiles: props.contextFiles ?? "inherit",
            provider: selection?.provider ?? null,
            model: selection?.id ?? null,
          },
        },
      );
      if (sequence === probeSequence && props.projectPath.trim() === path) {
        setOptions(snapshot);
      }
      return snapshot;
    } catch (caught) {
      if (sequence === probeSequence && props.projectPath.trim() === path) {
        setSelectionError(String(caught));
      }
      return undefined;
    } finally {
      if (sequence === probeSequence) setSelectionLoading(false);
    }
  };

  createEffect(() => {
    const path = props.projectPath.trim();
    const key = [
      path,
      props.projectTrust ?? "inherit",
      props.contextFiles ?? "inherit",
    ].join("|");
    if (key === autoLoadKey) return;
    autoLoadKey = key;
    setDiscovery(undefined);
    setOptions(undefined);
    setError(undefined);
    setModelError(undefined);
    setSelectionError(undefined);
    void loadCatalog();
    void loadModels();
    if (path) {
      void probeSelection(undefined);
    }
  });

  const models = (): PickerModel[] => {
    const merged = new Map<string, PickerModel>();
    for (const model of discovery()?.models ?? []) {
      merged.set(identity(model.provider, model.id), { ...model, custom: false });
    }
    for (const profile of catalog()?.models ?? []) {
      const key = identity(profile.provider, profile.model);
      const existing = merged.get(key);
      if (existing) {
        merged.set(key, {
          ...existing,
          name: existing.name ?? profile.name,
          custom: true,
        });
      } else {
        merged.set(key, {
          provider: profile.provider,
          id: profile.model,
          name: profile.name,
          supportsImages: null,
          custom: true,
        });
      }
    }
    return [...merged.values()].sort((left, right) => {
      const provider = left.provider.localeCompare(right.provider);
      return provider || left.id.localeCompare(right.id);
    });
  };

  const selectedKey = () => (props.model ? encodeModelSelection(props.model) : "");

  const saveCustomModel = async () => {
    if (catalogBusy()) return;
    const provider = providerDraft().trim();
    const model = modelDraft().trim();
    if (!provider || !model) {
      setError("Provider and model id are required for a custom model.");
      return;
    }
    setCatalogBusy(true);
    setError(undefined);
    try {
      const saved = await invokeDesktop<CustomModelProfile>("runtime_save_custom_model", {
        request: {
          provider,
          model,
          name: nameDraft().trim() || null,
        },
      });
      await loadCatalog();
      const next = { provider: saved.provider, id: saved.model };
      props.onModelChange(next);
      props.onThinkingChange("");
      setProviderDraft("");
      setModelDraft("");
      setNameDraft("");
      void probeSelection(next);
    } catch (caught) {
      setError(String(caught));
    } finally {
      setCatalogBusy(false);
    }
  };

  const deleteCustomModel = async (profile: CustomModelProfile) => {
    if (catalogBusy()) return;
    setCatalogBusy(true);
    setError(undefined);
    try {
      await invokeDesktop<boolean>("runtime_delete_custom_model", {
        request: { provider: profile.provider, model: profile.model },
      });
      if (props.model?.provider === profile.provider && props.model.id === profile.model) {
        props.onModelChange(undefined);
        props.onThinkingChange("");
        void probeSelection(undefined);
      }
      await loadCatalog();
    } catch (caught) {
      setError(String(caught));
    } finally {
      setCatalogBusy(false);
    }
  };

  return (
    <section class="model-picker" aria-label={props.label ?? "Model selection"}>
      <div class="model-picker-heading">
        <div>
          <strong>{props.label ?? "Model and thinking"}</strong>
          <Show when={props.description}>
            {(description) => <span>{description()}</span>}
          </Show>
        </div>
        <button
          type="button"
          disabled={
            Boolean(props.disabled) ||
            modelsLoading() ||
            selectionLoading()
          }
          onClick={() =>
            void Promise.all([loadCatalog(), loadModels(), probeSelection(props.model)])
          }
        >
          {modelsLoading() ? "Reading Pi models" : selectionLoading() ? "Checking model" : "Refresh models"}
        </button>
      </div>

      <div class="model-picker-grid">
        <label>
          <span>Model</span>
          <select
            value={selectedKey()}
            disabled={Boolean(props.disabled) || modelsLoading()}
            onChange={(event) => {
              const next = decodeModelSelection(event.currentTarget.value);
              props.onModelChange(next);
              props.onThinkingChange("");
              void probeSelection(next);
            }}
          >
            <option value="">
              {options()?.currentModel
                ? `Pi default · ${options()!.currentModel!.provider}/${options()!.currentModel!.id}`
                : "Pi default"}
            </option>
            <For each={models()}>
              {(model) => (
                <option value={encodeModelSelection({ provider: model.provider, id: model.id })}>
                  {model.name ? `${model.name} · ` : ""}{model.provider}/{model.id}
                  {model.custom ? " · saved" : ""}
                </option>
              )}
            </For>
          </select>
        </label>
        <label>
          <span>Thinking</span>
          <select
            value={props.thinking}
            disabled={Boolean(props.disabled) || selectionLoading()}
            onChange={(event) =>
              props.onThinkingChange(event.currentTarget.value as ThinkingLevel | "")
            }
          >
            <option value="">
              {options()
                ? `Pi default · ${options()!.currentThinkingLevel}`
                : "Pi default"}
            </option>
            <For each={options()?.thinkingLevels ?? []}>
              {(level) => <option value={level}>{level}</option>}
            </For>
          </select>
        </label>
      </div>

      <Show when={discovery()}>
        {(snapshot) => (
          <>
            <p class="model-picker-note">
              {snapshot().models.length} model{snapshot().models.length === 1 ? "" : "s"} available from Pi
              {snapshot().diagnostics.scope === "global" ? " without project context" : " for this project"}.
            </p>
            <details class="model-catalog-editor">
              <summary>Model diagnostics</summary>
              <div class="model-picker-note">
                <div>Scope: {snapshot().diagnostics.scope}</div>
                <div title={snapshot().diagnostics.probeRoot}>Probe root: {snapshot().diagnostics.probeRoot}</div>
                <div>Environment: {snapshot().diagnostics.pathSource}</div>
                <div title={snapshot().diagnostics.logicalPi}>Logical Pi: {snapshot().diagnostics.logicalPi}</div>
                <div title={snapshot().diagnostics.invocationExecutable}>
                  Invocation: {snapshot().diagnostics.invocationExecutable}
                  {snapshot().diagnostics.directNpmNode ? " · direct npm Node" : ""}
                </div>
              </div>
            </details>
          </>
        )}
      </Show>

      <Show when={catalog()?.recoveryNotice}>
        {(notice) => <p class="error">Saved model catalog was reset: {notice()}</p>}
      </Show>
      <Show when={options() && !options()!.clearQueueSupported}>
        <p class="model-picker-note">
          This Pi build does not expose RPC queue clearing. Stop preserves bounded queued user text
          and terminates the exact owned Pi process when a reusable abort is not possible.
        </p>
      </Show>
      <Show when={modelError()}>{(message) => <p class="error">Pi model discovery: {message()}</p>}</Show>
      <Show when={selectionError()}>{(message) => <p class="error">Model options: {message()}</p>}</Show>
      <Show when={error()}>{(message) => <p class="error">Models: {message()}</p>}</Show>

      <details class="model-catalog-editor">
        <summary>Add or remove model identities</summary>
        <p>
          Add a provider/model identity when Pi can launch it but does not enumerate it. The next
          model probe validates that Pi accepts the identity for the selected project.
        </p>
        <div class="model-catalog-form">
          <label>
            <span>Provider</span>
            <input
              value={providerDraft()}
              disabled={catalogBusy()}
              placeholder="provider-id"
              onInput={(event) => setProviderDraft(event.currentTarget.value)}
            />
          </label>
          <label>
            <span>Model id</span>
            <input
              value={modelDraft()}
              disabled={catalogBusy()}
              placeholder="model-id"
              onInput={(event) => setModelDraft(event.currentTarget.value)}
            />
          </label>
          <label>
            <span>Name (optional)</span>
            <input
              value={nameDraft()}
              disabled={catalogBusy()}
              placeholder="Display name"
              onInput={(event) => setNameDraft(event.currentTarget.value)}
            />
          </label>
          <button type="button" disabled={catalogBusy()} onClick={() => void saveCustomModel()}>
            {catalogBusy() ? "Saving" : "Add model"}
          </button>
        </div>
        <div class="model-catalog-list" aria-label="Saved custom models">
          <For each={catalog()?.models ?? []}>
            {(profile) => (
              <div>
                <span>
                  <strong>{profile.name ?? profile.model}</strong> · {profile.provider}/{profile.model}
                </span>
                <button
                  type="button"
                  disabled={catalogBusy()}
                  onClick={() => void deleteCustomModel(profile)}
                >
                  Remove
                </button>
              </div>
            )}
          </For>
          <Show when={(catalog()?.models.length ?? 0) === 0}>
            <span class="sidebar-note">No custom models saved.</span>
          </Show>
        </div>
      </details>
    </section>
  );
}
