import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import {
  decodeModelSelection,
  encodeModelSelection,
  type ContextFilesPolicy,
  type CustomModelProfile,
  type ModelCatalogSnapshot,
  type ModelPreference,
  type ModelPreferencesSnapshot,
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
  rememberNewRunSelection?: boolean;
  label?: string;
}

interface PickerModel extends ModelSummary {
  custom: boolean;
  favorite: boolean;
}

function identity(provider: string, model: string): string {
  return `${provider}\u0000${model}`;
}

export function ModelPicker(props: ModelPickerProps) {
  const [discovery, setDiscovery] = createSignal<ProjectModelCatalog>();
  const [options, setOptions] = createSignal<ProjectLaunchOptions>();
  const [catalog, setCatalog] = createSignal<ModelCatalogSnapshot>();
  const [preferences, setPreferences] = createSignal<ModelPreferencesSnapshot>();
  const [modelsLoading, setModelsLoading] = createSignal(false);
  const [modelsLoaded, setModelsLoaded] = createSignal(false);
  const [catalogLoaded, setCatalogLoaded] = createSignal(false);
  const [selectionLoading, setSelectionLoading] = createSignal(false);
  const [catalogBusy, setCatalogBusy] = createSignal(false);
  const [favoriteBusy, setFavoriteBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [modelError, setModelError] = createSignal<string>();
  const [selectionError, setSelectionError] = createSignal<string>();
  const [preferenceError, setPreferenceError] = createSignal<string>();
  const [providerDraft, setProviderDraft] = createSignal("");
  const [modelDraft, setModelDraft] = createSignal("");
  const [nameDraft, setNameDraft] = createSignal("");
  let modelSequence = 0;
  let probeSequence = 0;
  let autoLoadKey = "";
  let preferencesLoadStarted = false;
  let preferredSelectionApplied = false;
  let userSelectedModel = false;
  let preferenceWrite: Promise<void> = Promise.resolve();
  let modelSelectElement: HTMLSelectElement | undefined;

  const loadCatalog = async () => {
    try {
      const snapshot = await invokeDesktop<ModelCatalogSnapshot>("runtime_model_catalog");
      setCatalog(snapshot);
      return snapshot;
    } catch (caught) {
      setError(String(caught));
      return undefined;
    } finally {
      setCatalogLoaded(true);
    }
  };

  // Native select state can reset when a selected option moves between
  // optgroups. Reassert the controlled value after regrouping so presentation
  // matches the durable selection.
  createEffect(() => {
    const key = selectedKey();
    const groupRevision = [
      ...favoriteModels().map((model) => identity(model.provider, model.id)),
      "|",
      ...otherModels().map((model) => identity(model.provider, model.id)),
    ].join("\u0001");
    void groupRevision;
    queueMicrotask(() => {
      if (modelSelectElement && modelSelectElement.value !== key) {
        modelSelectElement.value = key;
      }
    });
  });

  const loadPreferences = async () => {
    try {
      const snapshot = await invokeDesktop<ModelPreferencesSnapshot>("runtime_model_preferences");
      setPreferences(snapshot);
      return snapshot;
    } catch (caught) {
      setPreferenceError(String(caught));
      return undefined;
    }
  };

  const loadModels = async () => {
    const path = props.projectPath.trim();
    const sequence = ++modelSequence;
    setModelsLoading(true);
    setModelsLoaded(false);
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
      if (sequence === modelSequence) {
        setModelsLoading(false);
        setModelsLoaded(true);
      }
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
    setPreferenceError(undefined);
    if (!preferencesLoadStarted) {
      preferencesLoadStarted = true;
      void loadPreferences();
    }
    setCatalogLoaded(false);
    void loadCatalog();
    void loadModels();
    if (path) {
      void probeSelection(undefined);
    }
  });

  const models = (): PickerModel[] => {
    const merged = new Map<string, PickerModel>();
    const favorites = new Set(
      (preferences()?.favoriteModels ?? []).map((model) => identity(model.provider, model.model)),
    );
    for (const model of discovery()?.models ?? []) {
      const key = identity(model.provider, model.id);
      merged.set(key, { ...model, custom: false, favorite: favorites.has(key) });
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
          favorite: favorites.has(key),
        });
      }
    }
    return [...merged.values()].sort((left, right) => {
      const label = (left.name ?? left.id).localeCompare(right.name ?? right.id);
      const provider = left.provider.localeCompare(right.provider);
      return label || provider || left.id.localeCompare(right.id);
    });
  };

  const favoriteModels = () => models().filter((model) => model.favorite);
  const otherModels = () => models().filter((model) => !model.favorite);

  const selectedKey = () => (props.model ? encodeModelSelection(props.model) : "");
  const selectedIsFavorite = () => {
    const selected = props.model;
    if (!selected) return false;
    return (preferences()?.favoriteModels ?? []).some(
      (model) => model.provider === selected.provider && model.model === selected.id,
    );
  };

  const persistNewRunSelection = (selection: ModelSelection | undefined) => {
    if (!props.rememberNewRunSelection) return;
    const model: ModelPreference | null = selection
      ? { provider: selection.provider, model: selection.id }
      : null;
    preferenceWrite = preferenceWrite.then(async () => {
      try {
        const snapshot = await invokeDesktop<ModelPreferencesSnapshot>(
          "runtime_set_new_run_model_preference",
          { request: { model } },
        );
        setPreferences(snapshot);
        setPreferenceError(undefined);
      } catch (caught) {
        setPreferenceError(`Could not remember New Run model: ${String(caught)}`);
      }
    });
  };

  const applySelection = (
    selection: ModelSelection | undefined,
    options: { remember: boolean; user: boolean } = { remember: true, user: true },
  ) => {
    if (options.user) userSelectedModel = true;
    props.onModelChange(selection);
    props.onThinkingChange("");
    if (options.remember) persistNewRunSelection(selection);
    void probeSelection(selection);
  };

  createEffect(() => {
    if (
      !props.rememberNewRunSelection ||
      preferredSelectionApplied ||
      userSelectedModel ||
      !preferences() ||
      !modelsLoaded() ||
      !catalogLoaded()
    )
      return;
    preferredSelectionApplied = true;
    const preferred = preferences()!.newRunModel;
    if (!preferred) return;
    const available = models().some(
      (model) => model.provider === preferred.provider && model.id === preferred.model,
    );
    if (!available) {
      setPreferenceError(
        `Saved New Run model ${preferred.provider}/${preferred.model} is not currently available; using Pi default.`,
      );
      return;
    }
    const selection = { provider: preferred.provider, id: preferred.model };
    props.onModelChange(selection);
    props.onThinkingChange("");
    void probeSelection(selection);
  });

  const toggleFavorite = async () => {
    const selected = props.model;
    if (!selected || favoriteBusy()) return;
    setFavoriteBusy(true);
    setPreferenceError(undefined);
    try {
      const snapshot = await invokeDesktop<ModelPreferencesSnapshot>("runtime_set_model_favorite", {
        request: {
          provider: selected.provider,
          model: selected.id,
          favorite: !selectedIsFavorite(),
        },
      });
      setPreferences(snapshot);
    } catch (caught) {
      setPreferenceError(`Could not update model favorite: ${String(caught)}`);
    } finally {
      setFavoriteBusy(false);
    }
  };

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
      applySelection(next);
      setProviderDraft("");
      setModelDraft("");
      setNameDraft("");
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
        applySelection(undefined);
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
          {modelsLoading() ? "Loading…" : selectionLoading() ? "Checking…" : "Refresh"}
        </button>
      </div>

      <div class="model-picker-grid">
        <div class="model-picker-model-control">
          <label class="model-picker-field">
            <span>Model</span>
            <select
              ref={modelSelectElement}
              value={selectedKey()}
              disabled={Boolean(props.disabled) || modelsLoading()}
              onChange={(event) => applySelection(decodeModelSelection(event.currentTarget.value))}
            >
              <Show when={favoriteModels().length > 0}>
                <optgroup label="Favorites">
                  <For each={favoriteModels()}>
                    {(model) => (
                      <option value={encodeModelSelection({ provider: model.provider, id: model.id })}>
                        ★ {model.name ? `${model.name} · ` : ""}{model.provider}/{model.id}
                        {model.custom ? " · saved" : ""}
                      </option>
                    )}
                  </For>
                </optgroup>
              </Show>
              <optgroup label="Models">
                <option value="">
                  {options()?.currentModel
                    ? `Pi default · ${options()!.currentModel!.provider}/${options()!.currentModel!.id}`
                    : "Pi default"}
                </option>
                <For each={otherModels()}>
                  {(model) => (
                    <option value={encodeModelSelection({ provider: model.provider, id: model.id })}>
                      {model.name ? `${model.name} · ` : ""}{model.provider}/{model.id}
                      {model.custom ? " · saved" : ""}
                    </option>
                  )}
                </For>
              </optgroup>
            </select>
          </label>
          <button
            type="button"
            class="model-favorite-toggle"
            disabled={Boolean(props.disabled) || favoriteBusy() || !props.model}
            aria-pressed={props.model ? selectedIsFavorite() : false}
            title={
              props.model
                ? selectedIsFavorite()
                  ? "Remove selected model from favorites"
                  : "Add selected model to favorites"
                : "Select a model to favorite it"
            }
            onClick={() => void toggleFavorite()}
          >
            {selectedIsFavorite() ? "★ Favorited" : "☆ Favorite"}
          </button>
        </div>
        <label class="model-picker-field">
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
          <details class="model-catalog-editor">
            <summary>Model diagnostics</summary>
            <div class="model-picker-note">
              <div>Scope: {snapshot().diagnostics.scope}</div>
              <div title={snapshot().diagnostics.probeRoot}>Probe root: {snapshot().diagnostics.probeRoot}</div>
              <div>Environment: {snapshot().diagnostics.pathSource}</div>
              <div title={snapshot().diagnostics.logicalPi}>Logical Pi: {snapshot().diagnostics.logicalPi}</div>
              <div title={snapshot().diagnostics.invocationExecutable}>
                Invocation: {snapshot().diagnostics.invocationExecutable}
                {snapshot().diagnostics.windowsCommandWrapper ? " · Windows Pi wrapper" : ""}
              </div>
            </div>
          </details>
        )}
      </Show>

      <Show when={catalog()?.recoveryNotice}>
        {(notice) => <p class="error">Saved model catalog was reset: {notice()}</p>}
      </Show>
      <Show when={options() && !options()!.clearQueueSupported}>
        <p class="model-picker-note model-picker-compatibility-note">
          Stop compatibility: this Pi build cannot clear queued RPC work, so Stop may terminate the
          owned Pi process instead of reusing it.
        </p>
      </Show>
      <Show when={modelError()}>{(message) => <p class="error">Pi model discovery: {message()}</p>}</Show>
      <Show when={selectionError()}>{(message) => <p class="error">Model options: {message()}</p>}</Show>
      <Show when={preferenceError()}>{(message) => <p class="error">Model preference: {message()}</p>}</Show>
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
