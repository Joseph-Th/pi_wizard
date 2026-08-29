import { createEffect, createSignal, For, onMount, Show } from "solid-js";

import { invokeDesktop, pickDirectory } from "../../lib/desktop";
import type { ExtensionDiscoveryPolicy, StartRunResult } from "../../types/launch";
import { ModelPicker } from "../models/ModelPicker";
import type { ContextFilesPolicy, ModelSelection, ProjectTrustPolicy, ThinkingLevel } from "../models/types";
import { SessionCatalogBrowser } from "../sessions/SessionCatalogBrowser";
import type {
  ProjectResourcePreflight,
  WorktreeBaseSnapshot,
  WorktreeCleanupResult,
  WorktreeRecoveryInspection,
  WorktreeRecoveryPage,
  WorktreeRecoveryProbe,
  WorktreeRecoveryRecord,
} from "./types";

type ExecutionMode = "local" | "worktree";

function projectResourceLabels(preflight: ProjectResourcePreflight): string[] {
  const labels: string[] = [];
  if (preflight.piSettings) labels.push(".pi/settings.json");
  if (preflight.extensions) labels.push(".pi/extensions");
  if (preflight.skills) labels.push(".pi/skills");
  if (preflight.prompts) labels.push(".pi/prompts");
  if (preflight.themes) labels.push(".pi/themes");
  if (preflight.systemPrompt) labels.push(".pi/SYSTEM.md");
  if (preflight.appendSystemPrompt) labels.push(".pi/APPEND_SYSTEM.md");
  if (preflight.ancestorAgentSkills) labels.push("project/ancestor .agents/skills");
  return labels;
}

function suggestedWorktreeIdentity(
  repositoryRoot: string,
  task: string,
): { branch: string; path: string } {
  const normalized = repositoryRoot
    .replace(/^\\\\\?\\UNC\\/i, "\\\\")
    .replace(/^\\\\\?\\/, "")
    .replace(/[\\/]+$/, "");
  const separator = normalized.includes("\\") ? "\\" : "/";
  const parts = normalized.split(/[\/]/);
  const repositoryName = parts.pop() || "repo";
  const parent = parts.join(separator);
  const taskSlug = task
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 28);
  const leaf = `${taskSlug || "task"}-${Date.now().toString(36)}`;
  const branch = `pi-wizard/${leaf}`;
  const worktreeDirectory = `${repositoryName}-worktrees`;
  return {
    branch,
    path: parent
      ? `${parent}${separator}${worktreeDirectory}${separator}${leaf}`
      : `${worktreeDirectory}${separator}${leaf}`,
  };
}

export function ProjectLauncher(props: {
  piReady: boolean;
  preferredProjectPath: string;
  onStarted: (result: StartRunResult) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
  isExecutionRootActive: (path: string) => boolean;
  activeRunIdForExecutionRoot: (path: string) => string | undefined;
  activeRunIdForSessionPath: (path: string) => string | undefined;
}) {
  const [projectPath, setProjectPath] = createSignal("");
  const [initialTask, setInitialTask] = createSignal("");
  const [projectTrust, setProjectTrust] = createSignal<ProjectTrustPolicy>("inherit");
  const [contextFiles, setContextFiles] = createSignal<ContextFilesPolicy>("inherit");
  const [extensionDiscovery, setExtensionDiscovery] =
    createSignal<ExtensionDiscoveryPolicy>("inherit");
  const [launchModelKey, setLaunchModelKey] = createSignal("");
  const [launchThinking, setLaunchThinking] = createSignal<ThinkingLevel | "">("");
  const [resourcePreflight, setResourcePreflight] = createSignal<ProjectResourcePreflight>();
  const [resourcePreflightPath, setResourcePreflightPath] = createSignal("");
  const [resourcePreflightLoading, setResourcePreflightLoading] = createSignal(false);
  const [resourcePreflightError, setResourcePreflightError] = createSignal<string>();
  const [executionMode, setExecutionMode] = createSignal<ExecutionMode>("local");
  const [starting, setStarting] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [worktreeBase, setWorktreeBase] = createSignal<WorktreeBaseSnapshot>();
  const [worktreeBranch, setWorktreeBranch] = createSignal("");
  const [worktreePath, setWorktreePath] = createSignal("");
  const [probingWorktree, setProbingWorktree] = createSignal(false);
  const [worktreeError, setWorktreeError] = createSignal<string>();
  const [recoveryPage, setRecoveryPage] = createSignal<WorktreeRecoveryPage>();
  const [recoveryInspections, setRecoveryInspections] = createSignal<
    Record<string, WorktreeRecoveryProbe>
  >({});
  const [loadingRecoveries, setLoadingRecoveries] = createSignal(false);
  const [recoveryError, setRecoveryError] = createSignal<string>();
  const [reconcilingRecovery, setReconcilingRecovery] = createSignal<string>();
  const [startingRecovery, setStartingRecovery] = createSignal<string>();
  const [cleaningRecovery, setCleaningRecovery] = createSignal<string>();

  const localCheckoutActive = () =>
    executionMode() === "local" &&
    projectPath().trim().length > 0 &&
    props.isExecutionRootActive(projectPath().trim());

  const resetLaunchOptions = () => {
    setLaunchModelKey("");
    setLaunchThinking("");
  };

  const resetResourcePreflight = () => {
    setResourcePreflight(undefined);
    setResourcePreflightPath("");
    setResourcePreflightError(undefined);
  };

  const probeProjectResources = async () => {
    const path = projectPath().trim();
    if (!path || resourcePreflightLoading()) return;
    setResourcePreflightLoading(true);
    setResourcePreflightError(undefined);
    try {
      const preflight = await invokeDesktop<ProjectResourcePreflight>(
        "runtime_probe_project_resources",
        { request: { projectPath: path } },
      );
      if (projectPath().trim() !== path) return;
      setResourcePreflight(preflight);
      setResourcePreflightPath(path);
    } catch (preflightError) {
      if (projectPath().trim() === path) setResourcePreflightError(String(preflightError));
    } finally {
      if (projectPath().trim() === path) setResourcePreflightLoading(false);
    }
  };

  const selectedLaunchModel = (): { provider: string; id: string } | undefined => {
    const value = launchModelKey();
    if (!value) return undefined;
    try {
      const parsed = JSON.parse(value);
      if (
        Array.isArray(parsed) &&
        parsed.length === 2 &&
        typeof parsed[0] === "string" &&
        typeof parsed[1] === "string"
      ) {
        return { provider: parsed[0], id: parsed[1] };
      }
    } catch {
      // The value comes from our own option list. Treat malformed state as no override.
    }
    return undefined;
  };

  createEffect(() => {
    const preferred = props.preferredProjectPath.trim();
    if (!preferred || preferred === projectPath()) return;
    setProjectPath(preferred);
    setWorktreeBase(undefined);
    setWorktreeError(undefined);
    resetLaunchOptions();
  });

  const loadRecoveries = async () => {
    if (loadingRecoveries()) return;
    setLoadingRecoveries(true);
    setRecoveryError(undefined);
    try {
      const page = await invokeDesktop<WorktreeRecoveryPage>("runtime_list_worktree_recoveries");
      setRecoveryPage(page);
    } catch (loadError) {
      setRecoveryError(String(loadError));
    } finally {
      setLoadingRecoveries(false);
    }
  };

  const cleanupRecovery = async (record: WorktreeRecoveryRecord) => {
    if (
      cleaningRecovery() ||
      reconcilingRecovery() ||
      startingRecovery() ||
      !record.created ||
      recoveryInspections()[record.id]?.kind !== "exact"
    )
      return;
    const confirmed = window.confirm(
      `Remove the unused worktree at ${record.created.worktreeRoot} and delete branch ${record.branch}? Pi Wizard will refuse if the worktree is dirty, has task commits, or is used by a live run.`,
    );
    if (!confirmed) return;
    setCleaningRecovery(record.id);
    setRecoveryError(undefined);
    try {
      const result = await invokeDesktop<WorktreeCleanupResult>(
        "runtime_cleanup_worktree_recovery",
        { request: { id: record.id } },
      );
      if (result.kind === "partial") {
        setRecoveryInspections((current) => ({ ...current, [record.id]: result }));
        setRecoveryError(
          `Cleanup stopped with recoverable Git state retained: ${result.detail}`,
        );
      } else {
        setRecoveryInspections((current) => {
          const next = { ...current };
          delete next[record.id];
          return next;
        });
      }
      await loadRecoveries();
    } catch (cleanupError) {
      setRecoveryError(String(cleanupError));
      await loadRecoveries();
    } finally {
      setCleaningRecovery(undefined);
    }
  };

  onMount(() => void loadRecoveries());

  const start = async () => {
    const path = projectPath().trim();
    if (!path || starting()) return;
    if (executionMode() === "local" && props.isExecutionRootActive(path)) {
      setError(
        "This checkout is already owned by a live run. Open that run or use a Git worktree for parallel work.",
      );
      return;
    }
    if (executionMode() === "worktree") {
      if (!worktreeBase()) {
        setError("Inspect the exact Git base before creating a worktree");
        return;
      }
      if (!worktreeBranch().trim() || !worktreePath().trim()) {
        setError("New branch and absolute worktree path are required");
        return;
      }
    }
    setStarting(true);
    setError(undefined);
    try {
      const launchModel = selectedLaunchModel();
      let result: StartRunResult;
      if (executionMode() === "worktree") {
        result = await invokeDesktop<StartRunResult>("runtime_start_project_worktree", {
          request: {
            projectPath: path,
            projectTrust: projectTrust(),
            contextFiles: contextFiles(),
            extensionDiscovery: extensionDiscovery(),
            provider: launchModel?.provider ?? null,
            model: launchModel?.id ?? null,
            thinking: launchThinking() || null,
            base: worktreeBase(),
            branch: worktreeBranch().trim(),
            worktreePath: worktreePath().trim(),
            initialTask: initialTask().trim() || null,
          },
        });
      } else {
        result = await invokeDesktop<StartRunResult>("runtime_start_project", {
          request: {
            projectPath: path,
            projectTrust: projectTrust(),
            contextFiles: contextFiles(),
            extensionDiscovery: extensionDiscovery(),
            provider: launchModel?.provider ?? null,
            model: launchModel?.id ?? null,
            thinking: launchThinking() || null,
            initialTask: initialTask().trim() || null,
          },
        });
      }
      await props.onStarted(result);
      if (result.initialTaskError) {
        setError(`Run started, but the initial task was not sent automatically: ${result.initialTaskError}`);
      } else if (result.initialTaskSubmitted) {
        setInitialTask("");
      }
      if (executionMode() === "worktree") await loadRecoveries();
    } catch (startError) {
      setError(String(startError));
      if (executionMode() === "worktree") await loadRecoveries();
    } finally {
      setStarting(false);
    }
  };

  const reconcileRecovery = async (record: WorktreeRecoveryRecord) => {
    if (reconcilingRecovery()) return;
    setReconcilingRecovery(record.id);
    setRecoveryError(undefined);
    try {
      const inspection = await invokeDesktop<WorktreeRecoveryInspection>(
        "runtime_reconcile_worktree_recovery",
        { request: { id: record.id } },
      );
      setRecoveryInspections((current) => ({ ...current, [record.id]: inspection.probe }));
      await loadRecoveries();
    } catch (inspectError) {
      setRecoveryError(String(inspectError));
    } finally {
      setReconcilingRecovery(undefined);
    }
  };

  const startRecovered = async (record: WorktreeRecoveryRecord) => {
    if (startingRecovery() || !props.piReady) return;
    setStartingRecovery(record.id);
    setRecoveryError(undefined);
    try {
      const runId = await invokeDesktop<string>("runtime_start_recovered_worktree", {
        request: {
          id: record.id,
          projectTrust: projectTrust(),
          contextFiles: contextFiles(),
          extensionDiscovery: extensionDiscovery(),
          provider: selectedLaunchModel()?.provider ?? null,
          model: selectedLaunchModel()?.id ?? null,
          thinking: launchThinking() || null,
        },
      });
      await props.onStarted({
        runId,
        initialTaskSubmitted: false,
        initialTaskError: null,
      });
      await loadRecoveries();
    } catch (startError) {
      setRecoveryError(String(startError));
    } finally {
      setStartingRecovery(undefined);
    }
  };

  const inspectWorktree = async () => {
    const path = projectPath().trim();
    if (!path || probingWorktree()) return;
    setProbingWorktree(true);
    setWorktreeError(undefined);
    setWorktreeBase(undefined);
    try {
      const base = await invokeDesktop<WorktreeBaseSnapshot>("runtime_probe_project_worktree", {
        request: { projectPath: path },
      });
      setWorktreeBase(base);
      if (!worktreeBranch().trim() && !worktreePath().trim()) {
        const suggested = suggestedWorktreeIdentity(base.repositoryRoot, initialTask());
        setWorktreeBranch(suggested.branch);
        setWorktreePath(suggested.path);
      }
    } catch (probeError) {
      setWorktreeError(String(probeError));
    } finally {
      setProbingWorktree(false);
    }
  };

  const browseProject = async () => {
    try {
      const selected = await pickDirectory(projectPath());
      if (!selected) return;
      setProjectPath(selected);
      setWorktreeBase(undefined);
      setWorktreeError(undefined);
      resetLaunchOptions();
      resetResourcePreflight();
    } catch (browseError) {
      setError(`Could not choose project folder: ${String(browseError)}`);
    }
  };

  return (
    <section class="project-launcher" aria-label="Start Pi run">
      <h2>Start a run</h2>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void start();
        }}
      >
        <label class="path-field">
          <span>Project path</span>
          <div class="path-picker-row">
            <input
              value={projectPath()}
              disabled={starting()}
              placeholder="Absolute path to an existing project"
              onInput={(event) => {
                setProjectPath(event.currentTarget.value);
                setWorktreeBase(undefined);
                setWorktreeError(undefined);
                resetLaunchOptions();
                resetResourcePreflight();
              }}
            />
            <button type="button" disabled={starting()} onClick={() => void browseProject()}>
              Browse
            </button>
          </div>
        </label>
        <label>
          <span>Execution root</span>
          <select
            value={executionMode()}
            disabled={starting()}
            onChange={(event) => {
              setExecutionMode(event.currentTarget.value as ExecutionMode);
              setError(undefined);
            }}
          >
            <option value="local">Local checkout</option>
            <option value="worktree">New Git worktree</option>
          </select>
        </label>
        <div class="launch-resource-preflight">
          <button
            type="button"
            disabled={starting() || resourcePreflightLoading() || !projectPath().trim()}
            onClick={() => void probeProjectResources()}
          >
            {resourcePreflightLoading() ? "Checking project resources" : "Check project resources"}
          </button>
          <Show
            when={resourcePreflightPath() === projectPath().trim() ? resourcePreflight() : undefined}
          >
            {(preflight) => {
              const labels = () => projectResourceLabels(preflight());
              return (
                <p class="launch-note">
                  {labels().length > 0
                    ? `Protected Pi resources detected: ${labels().join(", ")}. `
                    : "No protected Pi project resources were detected in this snapshot. "}
                  {projectTrust() === "approve"
                    ? "Approve loads protected project resources for this run."
                    : projectTrust() === "ignore"
                      ? "Ignore skips protected resources for this run; context files remain a separate choice."
                      : "Use Pi saved/default trust leaves the final decision to Pi; RPC mode does not show an interactive trust prompt."}
                </p>
              );
            }}
          </Show>
          <Show when={resourcePreflightError()}>
            {(message) => <p class="error">Project-resource check: {message()}</p>}
          </Show>
        </div>
        <ModelPicker
          projectPath={projectPath()}
          piReady={props.piReady}
          disabled={starting()}
          projectTrust={projectTrust()}
          contextFiles={contextFiles()}
          model={selectedLaunchModel()}
          thinking={launchThinking()}
          onModelChange={(selection: ModelSelection | undefined) =>
            setLaunchModelKey(
              selection ? JSON.stringify([selection.provider, selection.id]) : "",
            )
          }
          onThinkingChange={setLaunchThinking}
          label="New-run model and thinking"
          description="Pi models load automatically for the selected project; saved custom identities are merged into the same picker."
        />
        <label class="initial-task-field">
          <span>Initial task</span>
          <textarea
            value={initialTask()}
            disabled={starting()}
            placeholder="What should Pi do? Leave blank to start an idle session."
            onInput={(event) => setInitialTask(event.currentTarget.value)}
          />
        </label>
        <label>
          <span>Project resources</span>
          <select
            value={projectTrust()}
            disabled={starting()}
            onChange={(event) => {
              setProjectTrust(event.currentTarget.value as ProjectTrustPolicy);
              resetLaunchOptions();
            }}
          >
            <option value="inherit">Use Pi saved/default trust</option>
            <option value="approve">Approve for this run</option>
            <option value="ignore">Ignore protected project resources</option>
          </select>
        </label>
        <details class="launch-advanced">
          <summary>Advanced launch options</summary>
          <label>
            <span>Context instructions</span>
            <select
              value={contextFiles()}
              disabled={starting()}
              onChange={(event) => {
                setContextFiles(event.currentTarget.value as ContextFilesPolicy);
                resetLaunchOptions();
              }}
            >
              <option value="inherit">Load AGENTS.md / CLAUDE.md using Pi settings</option>
              <option value="disabled">Disable context files for this launch</option>
            </select>
          </label>
          <p class="launch-note">
            This is independent from project-resource trust and applies to new, resumed, and
            recovered runs launched from this screen.
          </p>
          <label>
            <span>Extensions</span>
            <select
              value={extensionDiscovery()}
              disabled={starting()}
              onChange={(event) =>
                setExtensionDiscovery(event.currentTarget.value as ExtensionDiscoveryPolicy)
              }
            >
              <option value="inherit">Load discovered Pi extensions</option>
              <option value="disabled">Disable extensions for this launch</option>
            </select>
          </label>
          <p class="launch-note">
            Disabling extensions maps to Pi --no-extensions for this run only. It does not change
            Pi’s global configuration, project trust, or context-file loading. The model/thinking
            probe is always extension-free so a broken installed extension cannot block recovery.
          </p>
        </details>
        <Show when={executionMode() === "worktree"}>
          <div class="worktree-plan">
            <button
              type="button"
              disabled={probingWorktree() || starting() || !props.piReady || !projectPath().trim()}
              onClick={() => void inspectWorktree()}
            >
              {probingWorktree() ? "Inspecting Git base" : "Inspect Git base"}
            </button>
            <Show when={worktreeBase()}>
              {(base) => (
                <div class="worktree-base">
                  <strong>{base().sourceBranch ?? "Detached HEAD"}</strong>
                  <span title={base().baseCommit}>base {base().baseCommit.slice(0, 12)}</span>
                  <span title={base().repositoryRoot}>{base().repositoryRoot}</span>
                  <Show when={base().projectRelativePath}>
                    <span>project subdirectory: {base().projectRelativePath}</span>
                  </Show>
                  <Show when={base().dirty}>
                    <p>
                      Source checkout has uncommitted or untracked changes. The new worktree starts
                      from the captured commit only; those changes are not copied.
                    </p>
                  </Show>
                </div>
              )}
            </Show>
            <label>
              <span>New branch</span>
              <input
                value={worktreeBranch()}
                disabled={starting()}
                placeholder="Explicit new branch name"
                onInput={(event) => setWorktreeBranch(event.currentTarget.value)}
              />
            </label>
            <label>
              <span>Worktree path</span>
              <input
                value={worktreePath()}
                disabled={starting()}
                placeholder="Absolute path for the new worktree"
                onInput={(event) => setWorktreePath(event.currentTarget.value)}
              />
            </label>
            <p class="launch-note">
              The captured branch and commit are revalidated before Git mutates anything. A
              worktree isolates Git edits; it is not a filesystem, network, credential, or process sandbox.
            </p>
            <Show when={worktreeError()}>
              {(message) => <p class="error">Git base inspection failed: {message()}</p>}
            </Show>
          </div>
        </Show>
        <Show when={localCheckoutActive()}>
          <div class="launch-conflict" role="status">
            <span>This checkout is already in use by a live run.</span>
            <button
              type="button"
              disabled={starting() || probingWorktree() || !props.piReady}
              onClick={() => {
                setExecutionMode("worktree");
                setError(undefined);
                queueMicrotask(() => void inspectWorktree());
              }}
            >
              Use a worktree
            </button>
          </div>
        </Show>
        <button
          type="submit"
          disabled={
            starting() ||
            !props.piReady ||
            projectPath().trim().length === 0 ||
            localCheckoutActive() ||
            (executionMode() === "worktree" &&
              (!worktreeBase() || !worktreeBranch().trim() || !worktreePath().trim()))
          }
        >
          {starting()
            ? "Starting"
            : executionMode() === "worktree"
              ? "Create worktree and start Pi"
              : "Start Pi"}
        </button>
      </form>
      <p class="launch-note">
        Trust controls Pi project-resource loading. It is not an execution sandbox, and ignoring
        protected project resources does not disable context files such as AGENTS.md.
      </p>
      <Show when={error()}>{(message) => <p class="error">Could not start run: {message()}</p>}</Show>

      <div class="worktree-recoveries">
        <div class="recovery-heading">
          <div>
            <strong>Worktree recovery</strong>
            <span>Durable records are kept after Git creation so app or Pi launch failure cannot orphan context silently.</span>
          </div>
          <button type="button" disabled={loadingRecoveries()} onClick={() => void loadRecoveries()}>
            {loadingRecoveries() ? "Refreshing" : "Refresh"}
          </button>
        </div>
        <Show when={recoveryPage()?.recoveryNotice}>
          {(notice) => (
            <p class="error">
              Worktree recovery state was corrupt and quarantined: {notice()}
            </p>
          )}
        </Show>
        <Show when={recoveryPage()?.truncated}>
          <p class="launch-note">Showing a bounded recovery window; older records are omitted.</p>
        </Show>
        <For each={recoveryPage()?.records ?? []}>
          {(record) => {
            const probe = () => recoveryInspections()[record.id];
            const partialDetail = () => {
              const value = probe();
              return value?.kind === "partial" ? value.detail : undefined;
            };
            const activeRunId = () =>
              record.created
                ? props.activeRunIdForExecutionRoot(record.created.executionRoot)
                : undefined;
            return (
              <article class="recovery-row">
                <div>
                  <strong>{record.branch}</strong>
                  <span title={record.base.baseCommit}>
                    {record.created ? "Created" : "Needs inspection"} · base {record.base.baseCommit.slice(0, 12)}
                  </span>
                  <span title={record.created?.worktreeRoot ?? record.requestedPath}>
                    {record.created?.worktreeRoot ?? record.requestedPath}
                  </span>
                  <Show when={probe()?.kind === "notCreated"}>
                    <span>No branch or path remained; the stale intent was removed.</span>
                  </Show>
                  <Show when={probe()?.kind === "exact"}>
                    <span>Repository, branch and worktree path match; the captured base remains an ancestor of the current task HEAD.</span>
                  </Show>
                  <Show when={partialDetail()}>
                    {(detail) => (
                      <span>
                        Partial/conflicting Git state was retained. Pi Wizard will not delete it automatically. {detail()}
                      </span>
                    )}
                  </Show>
                </div>
                <div class="recovery-actions">
                  <button
                    type="button"
                    disabled={
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery()) ||
                      Boolean(cleaningRecovery())
                    }
                    onClick={() => void reconcileRecovery(record)}
                  >
                    {reconcilingRecovery() === record.id ? "Inspecting" : "Inspect"}
                  </button>
                  <button
                    type="button"
                    disabled={
                      !record.created ||
                      (!activeRunId() && !props.piReady) ||
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery()) ||
                      Boolean(cleaningRecovery())
                    }
                    onClick={() => {
                      const runId = activeRunId();
                      if (runId) {
                        props.onOpenRun(runId);
                      } else {
                        void startRecovered(record);
                      }
                    }}
                  >
                    {activeRunId()
                      ? "Open"
                      : startingRecovery() === record.id
                        ? "Starting"
                        : "Start Pi"}
                  </button>
                  <button
                    type="button"
                    disabled={
                      !record.created ||
                      Boolean(activeRunId()) ||
                      probe()?.kind !== "exact" ||
                      Boolean(reconcilingRecovery()) ||
                      Boolean(startingRecovery()) ||
                      Boolean(cleaningRecovery())
                    }
                    title="Only pristine worktrees whose branch is still at the captured base can be removed"
                    onClick={() => void cleanupRecovery(record)}
                  >
                    {cleaningRecovery() === record.id ? "Removing" : "Remove unused"}
                  </button>
                </div>
              </article>
            );
          }}
        </For>
        <Show when={(recoveryPage()?.records.length ?? 0) === 0 && !loadingRecoveries()}>
          <p class="launch-note">No retained worktree recovery records.</p>
        </Show>
        <Show when={recoveryError()}>
          {(message) => <p class="error">Worktree recovery: {message()}</p>}
        </Show>
      </div>

      <SessionCatalogBrowser
        projectPath={projectPath()}
        piReady={props.piReady}
        projectTrust={projectTrust()}
        contextFiles={contextFiles()}
        extensionDiscovery={extensionDiscovery()}
        onStarted={props.onStarted}
        onOpenRun={props.onOpenRun}
        activeRunIdForExecutionRoot={props.activeRunIdForExecutionRoot}
        activeRunIdForSessionPath={props.activeRunIdForSessionPath}
      />
    </section>
  );
}

