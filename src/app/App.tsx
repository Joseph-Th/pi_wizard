import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";

import { AutomationView } from "../features/automation/AutomationView";
import { ExtensionDialogCard } from "../features/attention/ExtensionDialogCard";
import { NeedsAttentionView } from "../features/attention/NeedsAttentionView";
import type {
  AutomationChangedSignal,
  AutomationExecutionSnapshot,
  DesktopAutomationSnapshot,
} from "../features/automation/types";
import { ModelPicker } from "../features/models/ModelPicker";
import type { ModelSelection } from "../features/models/types";
import { ProjectManager } from "../features/projects/ProjectManager";
import { ProjectLauncher } from "../features/projects/ProjectLauncher";
import { SessionCatalogBrowser } from "../features/sessions/SessionCatalogBrowser";
import { RecentSessionsView } from "../features/sessions/RecentSessionsView";
import { SupervisionView } from "../features/supervision/SupervisionView";
import type { SupervisionSnapshot } from "../features/supervision/types";
import { invokeDesktop } from "../lib/desktop";
import { pathLeaf } from "../lib/path";

import {
  AppView,
  StartRunResult,
  DesktopProjectRecord,
  RuntimeAttachmentLimits,
  RuntimeCapacitySnapshot,
  DesktopRuntimeDiagnostics,
  RuntimeStopResult,
  RuntimeCloseResult,
  RunHydration,
  GitReviewSummary,
  RuntimeHydration,
  RUNTIME_HYDRATION_SCHEMA_VERSION,
  runModelLabel,
  runThinkingLabel,
  runElapsedLabel,
  isTerminalRun,
  RuntimeManagerSignal,
  UiNotification,
  RuntimeUiDrain,
  PiProbeReport,
  formatBytes,
  createComposerState,
  ComposerState,
  ComposerCard,
  needsHydration,
  runTitle,
  runStateLabel,
  runQueuedCount,
  runActivityLabel,
  runHasStoppableActivity,
  canCloseRun,
  runDisplayPriority,
  ExtensionUiPanel,
  PiRuntimeNoticePanel,
} from "../features/runs/RunSurface";

export interface AppStartupSnapshot {
  runtime: RuntimeHydration;
  capacity: RuntimeCapacitySnapshot;
  attachmentLimits: RuntimeAttachmentLimits;
}

export function App(props: { startup: AppStartupSnapshot }) {
  const [runtime, setRuntime] = createSignal<RuntimeHydration>(props.startup.runtime);
  const [runtimeError, setRuntimeError] = createSignal<string>();
  const [capacity, setCapacity] = createSignal<RuntimeCapacitySnapshot>(props.startup.capacity);
  const [capacityError, setCapacityError] = createSignal<string>();
  const [automation, setAutomation] = createSignal<DesktopAutomationSnapshot>();
  const [automationError, setAutomationError] = createSignal<string>();
  const [supervision, setSupervision] = createSignal<SupervisionSnapshot[]>([]);
  const [supervisionError, setSupervisionError] = createSignal<string>();
  const [capacityBusy, setCapacityBusy] = createSignal(false);
  const [liveRunLimitDraft, setLiveRunLimitDraft] = createSignal(
    String(props.startup.capacity.liveRunLimit),
  );
  const [piProbe, setPiProbe] = createSignal<PiProbeReport>();
  const [piProbeError, setPiProbeError] = createSignal<string>();
  const [attachmentLimits, setAttachmentLimits] = createSignal<RuntimeAttachmentLimits>(
    props.startup.attachmentLimits,
  );
  const [deliveredEvents, setDeliveredEvents] = createSignal(0);
  const [diagnostics, setDiagnostics] = createSignal<DesktopRuntimeDiagnostics>();
  const [diagnosticsBusy, setDiagnosticsBusy] = createSignal(false);
  const [diagnosticsError, setDiagnosticsError] = createSignal<string>();
  const [mountedTimelineRows, setMountedTimelineRows] = createSignal(0);
  const [longTaskMetrics, setLongTaskMetrics] = createSignal({
    count: 0,
    maxDurationMs: 0,
    lastDurationMs: 0,
  });
  const [view, setView] = createSignal<AppView>("dashboard");
  const [selectedRunId, setSelectedRunId] = createSignal<string>();
  const [preferredProjectPath, setPreferredProjectPath] = createSignal("");
  const [projects, setProjects] = createSignal<DesktopProjectRecord[]>([]);
  const [projectRefreshKey, setProjectRefreshKey] = createSignal(0);
  const [runActionError, setRunActionError] = createSignal<string>();
  const [closingRunId, setClosingRunId] = createSignal<string>();
  const [dismissingRunId, setDismissingRunId] = createSignal<string>();
  const [openingFolderRunId, setOpeningFolderRunId] = createSignal<string>();
  const [notifications, setNotifications] = createSignal<UiNotification[]>([]);
  const [elapsedClockUnixMs, setElapsedClockUnixMs] = createSignal(Date.now());
  const [knownChangeSummaries, setKnownChangeSummaries] = createSignal<
    Record<string, { fileCount: number; truncated: boolean; changeRevision: number }>
  >({});
  const drainingRuns = new Set<string>();
  const redrainRuns = new Set<string>();
  const hydrationNeededRuns = new Set<string>();
  const composerStates = new Map<string, ComposerState>();
  let hydrationRequestSequence = 0;
  let lastAppliedHydrationRequest = 0;
  let automationRequestSequence = 0;
  let lastAppliedAutomationRequest = 0;
  let supervisionRequestSequence = 0;
  let lastAppliedSupervisionRequest = 0;
  let notificationSequence = 0;
  let elapsedClockTimer: number | undefined;
  let longTaskObserver: PerformanceObserver | undefined;
  let disposed = false;
  let runtimeListenersReady = false;
  const runtimeUnlisteners: UnlistenFn[] = [];

  const applyHydration = (snapshot: RuntimeHydration, requestSequence: number) => {
    if (disposed || requestSequence < lastAppliedHydrationRequest) return;
    lastAppliedHydrationRequest = requestSequence;
    if (snapshot.schemaVersion !== RUNTIME_HYDRATION_SCHEMA_VERSION) {
      setRuntimeError(
        `Unsupported runtime hydration schema ${snapshot.schemaVersion}; this renderer requires schema ${RUNTIME_HYDRATION_SCHEMA_VERSION}. Reload the updated application instead of applying incompatible runtime state.`,
      );
      return;
    }
    setRuntime(snapshot);
    setRuntimeError(undefined);
  };

  const refreshDiagnostics = async () => {
    if (diagnosticsBusy()) return;
    setDiagnosticsBusy(true);
    setDiagnosticsError(undefined);
    try {
      const snapshot = await invokeDesktop<DesktopRuntimeDiagnostics>("runtime_diagnostics");
      if (!disposed) {
        setDiagnostics(snapshot);
        setMountedTimelineRows(document.querySelectorAll('[data-timeline-row="true"]').length);
      }
    } catch (error) {
      if (!disposed) setDiagnosticsError(String(error));
    } finally {
      if (!disposed) setDiagnosticsBusy(false);
    }
  };

  const rememberChangeSummary = (runId: string, summary: GitReviewSummary) => {
    const run = runById(runId);
    if (!run) return;
    setKnownChangeSummaries((current) => ({
      ...current,
      [runId]: {
        fileCount: summary.files.length,
        truncated: summary.truncated,
        changeRevision: run.run.changeRevision,
      },
    }));
  };

  const forgetChangeSummary = (runId: string) => {
    setKnownChangeSummaries((current) => {
      if (!(runId in current)) return current;
      const next = { ...current };
      delete next[runId];
      return next;
    });
  };

  const refreshCapacity = async () => {
    try {
      const snapshot = await invokeDesktop<RuntimeCapacitySnapshot>("runtime_capacity");
      if (!disposed) {
        setCapacity(snapshot);
        setLiveRunLimitDraft(String(snapshot.liveRunLimit));
        setCapacityError(undefined);
      }
      return snapshot;
    } catch (error) {
      if (!disposed) setCapacityError(String(error));
      return undefined;
    }
  };

  const refreshAutomation = async () => {
    const requestSequence = ++automationRequestSequence;
    try {
      const snapshot = await invokeDesktop<DesktopAutomationSnapshot>("runtime_automation_snapshot");
      if (!disposed && requestSequence >= lastAppliedAutomationRequest) {
        lastAppliedAutomationRequest = requestSequence;
        setAutomation(snapshot);
        setAutomationError(undefined);
      }
      return snapshot;
    } catch (error) {
      if (!disposed) setAutomationError(String(error));
      return undefined;
    }
  };

  const refreshAutomationExecutions = async () => {
    if (!automation()) return refreshAutomation();
    const requestSequence = ++automationRequestSequence;
    try {
      const executions = await invokeDesktop<AutomationExecutionSnapshot[]>("runtime_automation_executions");
      if (!disposed && requestSequence >= lastAppliedAutomationRequest) {
        lastAppliedAutomationRequest = requestSequence;
        setAutomation((current) => current ? { ...current, executions } : current);
        setAutomationError(undefined);
      }
      return executions;
    } catch (error) {
      if (!disposed) setAutomationError(String(error));
      return undefined;
    }
  };

  const refreshSupervision = async () => {
    const requestSequence = ++supervisionRequestSequence;
    try {
      const snapshots = await invokeDesktop<SupervisionSnapshot[]>("runtime_supervision_snapshot");
      if (!disposed && requestSequence >= lastAppliedSupervisionRequest) {
        lastAppliedSupervisionRequest = requestSequence;
        setSupervision(snapshots);
        setSupervisionError(undefined);
      }
      return snapshots;
    } catch (error) {
      if (!disposed) setSupervisionError(String(error));
      return undefined;
    }
  };

  createEffect(() => {
    if (view() === "automation") void refreshAutomation();
    if (view() === "supervision") void refreshSupervision();
  });

  const openRunFolder = async (run: RunHydration) => {
    if (openingFolderRunId()) return;
    setOpeningFolderRunId(run.run.id);
    setRunActionError(undefined);
    try {
      await invokeDesktop<void>("runtime_open_run_folder", {
        request: { runId: run.run.id },
      });
    } catch (error) {
      setRunActionError(String(error));
    } finally {
      setOpeningFolderRunId(undefined);
    }
  };

  const setLiveRunLimit = async () => {
    const next = Number.parseInt(liveRunLimitDraft(), 10);
    const current = capacity();
    if (
      capacityBusy() ||
      !current ||
      !Number.isInteger(next) ||
      next < 1 ||
      next > current.configuredMaxLiveRuns
    )
      return;
    setCapacityBusy(true);
    setCapacityError(undefined);
    try {
      const snapshot = await invokeDesktop<RuntimeCapacitySnapshot>("runtime_set_live_run_limit", {
        request: { limit: next },
      });
      if (!disposed) {
        setCapacity(snapshot);
        setLiveRunLimitDraft(String(snapshot.liveRunLimit));
      }
    } catch (error) {
      if (!disposed) setCapacityError(String(error));
    } finally {
      if (!disposed) setCapacityBusy(false);
    }
  };

  const refreshRuntimeState = async () => {
    const [snapshot] = await Promise.all([
      refreshHydration(),
      refreshCapacity(),
    ]);
    return snapshot;
  };

  const activeRunIdForExecutionRoot = (path: string) =>
    runtime()?.runs.find(
      (run) =>
        !["exited", "failed", "quarantined"].includes(run.run.process) &&
        run.run.executionRoot === path,
    )?.run.id;

  const activeRunIdForSessionPath = (path: string) =>
    runtime()?.runs.find(
      (run) =>
        !["exited", "failed", "quarantined"].includes(run.run.process) &&
        run.run.session.sessionFile === path,
    )?.run.id;

  const isExecutionRootActive = (path: string) =>
    Boolean(activeRunIdForExecutionRoot(path));

  const pendingDialogs = () =>
    runtime()?.runs.flatMap((run) =>
      (run.rpc?.pendingDialogs ?? []).map((dialog) => ({ runId: run.run.id, dialog })),
    ) ?? [];

  const sortedRuns = () =>
    [...(runtime()?.runs ?? [])].sort((left, right) => {
      const priority = runDisplayPriority(left) - runDisplayPriority(right);
      if (priority !== 0) return priority;
      // Run IDs are UUIDv7, so descending lexical order is descending creation time.
      return right.run.id.localeCompare(left.run.id);
    });

  const runById = (runId: string) => runtime()?.runs.find((run) => run.run.id === runId);

  const projectForRun = (run: RunHydration) =>
    projects().find((project) => project.id === run.run.projectId);

  const projectLabelForRun = (run: RunHydration) => {
    const project = projectForRun(run);
    return project ? pathLeaf(project.canonicalRoot) : `Project ${run.run.projectId.slice(0, 8)}`;
  };

  const selectedRun = () => {
    const id = selectedRunId();
    return id ? runById(id) : undefined;
  };

  const openRun = (runId: string) => {
    setRunActionError(undefined);
    setSelectedRunId(runId);
    setView("run");
  };

  const composerState = (run: RunHydration) => {
    let state = composerStates.get(run.run.id);
    if (!state) {
      state = createComposerState(run.run.id, run.draft?.text ?? "");
      composerStates.set(run.run.id, state);
    }
    return state;
  };

  const handleStarted = async (result: StartRunResult) => {
    await refreshRuntimeState();
    setProjectRefreshKey((value) => value + 1);
    openRun(result.runId);
    if (result.initialTaskError) {
      setNotifications((current) =>
        [
          ...current,
          {
            id: ++notificationSequence,
            runId: result.runId,
            message: `Run started, but the initial task was not sent automatically: ${result.initialTaskError}`,
            notifyType: "error" as const,
          },
        ].slice(-5),
      );
    }
  };

  const dismissRun = async (run: RunHydration) => {
    if (!isTerminalRun(run) || dismissingRunId()) return;
    setDismissingRunId(run.run.id);
    setRunActionError(undefined);
    try {
      await invokeDesktop<void>("runtime_dismiss_terminal_run", {
        request: { runId: run.run.id },
      });
      composerStates.delete(run.run.id);
      forgetChangeSummary(run.run.id);
      await Promise.all([refreshHydration(), refreshCapacity()]);
      if (selectedRunId() === run.run.id) {
        setSelectedRunId(undefined);
        setView("dashboard");
      }
    } catch (error) {
      setRunActionError(String(error));
      await refreshHydration();
    } finally {
      setDismissingRunId(undefined);
    }
  };

  const stopRunFromDashboard = async (run: RunHydration) => {
    if (run.run.process !== "ready" || !runHasStoppableActivity(run)) return;
    setRunActionError(undefined);
    try {
      await composerState(run).flush();
      const result = await invokeDesktop<RuntimeStopResult>("runtime_stop", {
        request: { runId: run.run.id },
      });
      await refreshHydration();
      if (result.quarantined) {
        setRunActionError(
          "Stop could not confirm Pi process termination. The run was quarantined for inspection.",
        );
      } else if (result.processTerminated) {
        setRunActionError(
          "Stop required terminating the Pi process. Its Pi session remains available from Recent Sessions.",
        );
      }
    } catch (error) {
      setRunActionError(String(error));
      await refreshHydration();
    }
  };

  const closeRun = async (run: RunHydration) => {
    if (!canCloseRun(run) || closingRunId()) return;
    setClosingRunId(run.run.id);
    setRunActionError(undefined);
    try {
      // Close is destructive to the renderer-owned unsynced editor value, so
      // unlike Stop it fails closed if the local draft cannot first reach the
      // backend. The backend then independently waits for durable persistence.
      await composerState(run).flush();
      const result = await invokeDesktop<RuntimeCloseResult>("runtime_close", {
        request: { runId: run.run.id },
      });
      await Promise.all([refreshHydration(), refreshCapacity()]);
      if (result.quarantined || !result.processTerminated) {
        setRunActionError(
          "Pi process termination could not be confirmed. The run was quarantined and remains visible for inspection.",
        );
        return;
      }
      if (selectedRunId() === run.run.id) {
        setSelectedRunId(undefined);
        setView("dashboard");
      }
    } catch (error) {
      setRunActionError(String(error));
      await Promise.all([refreshHydration(), refreshCapacity()]);
    } finally {
      setClosingRunId(undefined);
    }
  };

  createEffect(() => {
    if (view() !== "run") return;
    const id = selectedRunId();
    if (!id || runById(id)) return;
    setSelectedRunId(undefined);
    setView("dashboard");
  });

  createEffect(() => {
    const runs = runtime()?.runs ?? [];
    const retainedIds = new Set(runs.map((run) => run.run.id));
    setKnownChangeSummaries((current) => {
      const stale = Object.keys(current).filter((runId) => !retainedIds.has(runId));
      if (stale.length === 0) return current;
      const next = { ...current };
      for (const runId of stale) delete next[runId];
      return next;
    });

    const hasLiveRun = runs.some((run) => !isTerminalRun(run));
    if (hasLiveRun && elapsedClockTimer === undefined) {
      setElapsedClockUnixMs(Date.now());
      elapsedClockTimer = window.setInterval(() => setElapsedClockUnixMs(Date.now()), 60_000);
    } else if (!hasLiveRun && elapsedClockTimer !== undefined) {
      window.clearInterval(elapsedClockTimer);
      elapsedClockTimer = undefined;
    }
  });

  const refreshHydration = async () => {
    const requestSequence = ++hydrationRequestSequence;
    try {
      const snapshot = await invokeDesktop<RuntimeHydration>("runtime_hydrate");
      applyHydration(snapshot, requestSequence);
      return snapshot;
    } catch (error) {
      if (!disposed) setRuntimeError(String(error));
      return undefined;
    }
  };

  const recoverRun = async (runId: string) => {
    const requestSequence = ++hydrationRequestSequence;
    const snapshot = await invokeDesktop<RuntimeHydration>("runtime_recover_ui", {
      request: { runId },
    });
    applyHydration(snapshot, requestSequence);
  };

  const drainRun = async (runId: string) => {
    if (drainingRuns.has(runId)) {
      redrainRuns.add(runId);
      return;
    }
    drainingRuns.add(runId);
    let continueLater = false;
    try {
      // Bound work per browser task. `hasMore` continues the same backend
      // backlog without introducing an interval/polling loop.
      for (let batch = 0; batch < 8; batch += 1) {
        const drained = await invokeDesktop<RuntimeUiDrain>("runtime_drain", {
          request: { runId, maxEvents: 64 },
        });
        if (disposed) return;
        setDeliveredEvents((count) => count + drained.events.length);
        const incomingNotifications = drained.events.flatMap((event) =>
          event.kind === "extensionNotification" &&
          event.runId &&
          event.message &&
          event.notifyType
            ? [
                {
                  id: ++notificationSequence,
                  runId: event.runId,
                  message: event.message,
                  notifyType: event.notifyType,
                } satisfies UiNotification,
              ]
            : [],
        );
        if (incomingNotifications.length > 0) {
          setNotifications((current) => [...current, ...incomingNotifications].slice(-5));
        }
        if (needsHydration(drained.events)) hydrationNeededRuns.add(runId);
        if (drained.rehydrateRequired) {
          // Recovery is an explicit per-run transaction. Normal hydration is
          // non-destructive so it cannot erase another run's transient events.
          hydrationNeededRuns.delete(runId);
          await recoverRun(runId);
          return;
        }
        if (!drained.hasMore) return;
        if (batch === 7) continueLater = true;
      }
    } catch (error) {
      if (!disposed) setRuntimeError(`Runtime event drain failed: ${String(error)}`);
    } finally {
      drainingRuns.delete(runId);
      const wasRedirtied = redrainRuns.delete(runId);
      if (!disposed && (continueLater || wasRedirtied)) {
        queueMicrotask(() => void drainRun(runId));
      } else if (!disposed && hydrationNeededRuns.delete(runId)) {
        void refreshHydration();
      }
    }
  };

  const installRuntimeListeners = async () => {
    if (runtimeListenersReady) return;
    const pending: UnlistenFn[] = [];
    try {
      pending.push(
        await listen<RuntimeManagerSignal>("runtime://dirty", ({ payload }) => {
          void drainRun(payload.runId);
        }),
      );
      pending.push(
        await listen("runtime://rehydrate", () => {
          void refreshHydration();
        }),
      );
      pending.push(
        await listen<AutomationChangedSignal>("automation://changed", ({ payload }) => {
          if (view() !== "automation") return;
          if (payload === "catalog") void refreshAutomation();
          else void refreshAutomationExecutions();
        }),
      );
      pending.push(
        await listen("supervision://changed", () => {
          if (view() === "supervision") void refreshSupervision();
        }),
      );
      runtimeUnlisteners.push(...pending);
      runtimeListenersReady = true;
    } catch (error) {
      for (const unlisten of pending) unlisten();
      throw error;
    }
  };

  const connectBackend = async () => {
    try {
      await installRuntimeListeners();
      return await refreshRuntimeState();
    } catch (error) {
      if (!disposed) setRuntimeError(`Runtime listener setup failed: ${String(error)}`);
      return undefined;
    }
  };

  onMount(() => {
    // The root renderer does not mount App until runtime_backend_ready has
    // already returned a real RuntimeManager hydration plus capacity and
    // attachment limits. Subscribe before the reconciliation hydration so a
    // runtime change during the bootstrap-to-listener handoff cannot be lost.
    void connectBackend();

    void invokeDesktop<PiProbeReport>("probe_pi_environment")
      .then((report) => {
        if (!disposed) setPiProbe(report);
      })
      .catch((error) => {
        if (!disposed) setPiProbeError(String(error));
      });

    if (import.meta.env.DEV && typeof PerformanceObserver !== "undefined") {
      try {
        longTaskObserver = new PerformanceObserver((list) => {
          const entries = list.getEntries();
          if (entries.length === 0 || disposed) return;
          setLongTaskMetrics((current) => {
            let maxDurationMs = current.maxDurationMs;
            let lastDurationMs = current.lastDurationMs;
            for (const entry of entries) {
              maxDurationMs = Math.max(maxDurationMs, entry.duration);
              lastDurationMs = entry.duration;
            }
            return {
              count: current.count + entries.length,
              maxDurationMs,
              lastDurationMs,
            };
          });
        });
        longTaskObserver.observe({ entryTypes: ["longtask"] });
      } catch {
        longTaskObserver = undefined;
      }
    }

    onCleanup(() => {
      disposed = true;
      hydrationNeededRuns.clear();
      if (elapsedClockTimer !== undefined) window.clearInterval(elapsedClockTimer);
      longTaskObserver?.disconnect();
      for (const unlisten of runtimeUnlisteners) unlisten();
    });
  });

  return (
    <>
      <a class="skip-link" href="#main-content">Skip to main content</a>
      <div class="app-shell">
        <aside class="app-sidebar" aria-label="Pi Wizard navigation">
          <header class="app-brand">
            <strong>Pi Wizard</strong>
            <span>
              <Show when={piProbe()} fallback={"Pi …"}>
                {(report) =>
                  report().version
                    ? `Pi ${report().version!.display}`
                    : "Pi available"
                }
              </Show>
            </span>
          </header>

          <nav class="primary-nav" aria-label="Main views">
            <button
              type="button"
              class={view() === "dashboard" ? "active" : undefined}
              aria-current={view() === "dashboard" ? "page" : undefined}
              onClick={() => setView("dashboard")}
            >
              Dashboard
            </button>
            <button
              type="button"
              class={view() === "automation" ? "active" : undefined}
              aria-current={view() === "automation" ? "page" : undefined}
              onClick={() => setView("automation")}
            >
              Automation
            </button>
            <button
              type="button"
              class={view() === "supervision" ? "active" : undefined}
              aria-current={view() === "supervision" ? "page" : undefined}
              onClick={() => setView("supervision")}
            >
              Supervision
            </button>
            <button
              type="button"
              class={view() === "attention" ? "active" : undefined}
              aria-current={view() === "attention" ? "page" : undefined}
              onClick={() => setView("attention")}
            >
              <span>Needs attention</span>
              <strong class="nav-count">{pendingDialogs().length}</strong>
            </button>
            <button
              type="button"
              class={view() === "sessions" ? "active" : undefined}
              aria-current={view() === "sessions" ? "page" : undefined}
              onClick={() => setView("sessions")}
            >
              Recent sessions
            </button>
            <button
              type="button"
              class={view() === "launcher" ? "active" : undefined}
              aria-current={view() === "launcher" ? "page" : undefined}
              onClick={() => setView("launcher")}
            >
              New run
            </button>
          </nav>

          <section class="sidebar-runs" aria-label="Runs">
            <div class="sidebar-heading">
              <strong>Runs</strong>
              <span>{runtime()?.runs.length ?? 0}</span>
            </div>
            <For each={sortedRuns()}>
              {(run) => (
                <button
                  type="button"
                  class={`sidebar-run${selectedRunId() === run.run.id && view() === "run" ? " active" : ""}`}
                  aria-current={selectedRunId() === run.run.id && view() === "run" ? "page" : undefined}
                  onClick={() => openRun(run.run.id)}
                >
                  <strong>{runTitle(run)}</strong>
                  <span>{runStateLabel(run)}</span>
                  <small title={run.run.executionRoot}>
                    {projectLabelForRun(run)} · {run.run.executionIsolation === "git_worktree" ? "worktree" : "local"}
                  </small>
                </button>
              )}
            </For>
            <Show when={(runtime()?.runs.length ?? 0) === 0}>
              <p class="sidebar-note">No runs yet.</p>
            </Show>
          </section>

          <ProjectManager
            refreshKey={projectRefreshKey()}
            onProjects={setProjects}
            onUse={(path) => {
              setPreferredProjectPath(path);
              setView("launcher");
            }}
          />

          <details class="runtime-details">
            <summary>Runtime</summary>
            <div class="runtime-detail-grid" aria-live="polite">
              <span>Backend</span>
              <strong>{runtime() ? `ready · ${runtime()!.runs.length} runs` : "connecting"}</strong>
              <span>Events</span>
              <strong>{deliveredEvents()}</strong>
              <span>Pi</span>
              <strong>{piProbe()?.version?.display ?? (piProbe() ? "available" : "probing")}</strong>
              <Show when={piProbe()}>
                {(report) => (
                  <>
                    <span>Pi path source</span>
                    <strong>{report().environment.pathSource}</strong>
                    <span>Pi invocation</span>
                    <strong title={report().invocationExecutable}>
                      {report().invocationExecutable}
                      {report().directNpmNode ? " · direct npm Node" : ""}
                    </strong>
                  </>
                )}
              </Show>
            </div>
            <div class="runtime-diagnostics-actions">
              <button type="button" disabled={diagnosticsBusy()} onClick={() => void refreshDiagnostics()}>
                {diagnosticsBusy() ? "Reading diagnostics" : "Refresh diagnostics"}
              </button>
              <small>Explicit snapshot only. No diagnostic polling or logging.</small>
            </div>
            <Show when={diagnostics()}>
              {(snapshot) => (
                <>
                  <div class="runtime-detail-grid runtime-diagnostic-summary">
                    <span>Owned processes</span>
                    <strong>{snapshot().runtime.ownedProcesses}</strong>
                    <span>Git review jobs</span>
                    <strong>{snapshot().activeGitReviewJobs}</strong>
                    <span>Session catalog jobs</span>
                    <strong>{snapshot().activeSessionCatalogJobs}</strong>
                    <span>Mounted timeline rows</span>
                    <strong>{mountedTimelineRows()}</strong>
                    <span>Runtime revision</span>
                    <strong>{snapshot().runtime.runtimeRevision}</strong>
                    <Show when={import.meta.env.DEV}>
                      <span>Renderer long tasks</span>
                      <strong>
                        {longTaskMetrics().count} · last {longTaskMetrics().lastDurationMs.toFixed(0)} ms · max {longTaskMetrics().maxDurationMs.toFixed(0)} ms
                      </strong>
                    </Show>
                  </div>
                  <details class="runtime-diagnostic-runs">
                    <summary>Per-run counters</summary>
                    <For each={snapshot().runtime.runs}>
                      {(run) => (
                        <article class="runtime-diagnostic-run">
                          <strong title={run.runId}>{run.runId.slice(0, 8)}</strong>
                          <span>
                            {run.processOwned ? "process owned" : "no process"} · state {formatBytes(run.retainedRuntimeStateBytes)}
                          </span>
                          <span>
                            RPC {run.rpcEventsPerSecond}/s · {formatBytes(run.rpcEventBytesPerSecond)}/s · {run.pendingRpcRequests} pending · {run.activeRpcCommands} active
                          </span>
                          <span>
                            UI {formatBytes(run.uiBacklogBytes)} / {run.uiBacklogFrames} frames · {run.uiCoalescedFrames} coalesced · {run.uiDroppedDisplayFrames} dropped · {run.uiDeliveredEvents} delivered
                          </span>
                          <span>
                            Live {run.assistantBlocks} blocks · {run.activeTools} tools · {run.activeDirectBash} shell · {run.pendingExtensionDialogs} dialogs
                            {run.uiRehydrateRequired ? " · rehydration required" : ""}
                          </span>
                        </article>
                      )}
                    </For>
                  </details>
                </>
              )}
            </Show>
            <Show when={diagnosticsError()}>
              {(error) => <p class="error">Diagnostics: {error()}</p>}
            </Show>
            <Show when={capacity()}>
              {(current) => (
                <form
                  class="capacity-control"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void setLiveRunLimit();
                  }}
                >
                  <label>
                    <span>Concurrent run limit</span>
                    <input
                      type="number"
                      min="1"
                      max={current().configuredMaxLiveRuns}
                      value={liveRunLimitDraft()}
                      disabled={capacityBusy()}
                      aria-label="Live Pi run admission limit"
                      onInput={(event) => setLiveRunLimitDraft(event.currentTarget.value)}
                    />
                  </label>
                  <button type="submit" disabled={capacityBusy()}>
                    {capacityBusy() ? "Applying" : "Apply"}
                  </button>
                </form>
              )}
            </Show>
            <Show when={capacityError()}>{(error) => <p class="error">{error()}</p>}</Show>
            <Show when={capacity()?.preferenceRecoveryNotice}>
              {(notice) => <p class="error">Saved run limit was reset: {notice()}</p>}
            </Show>
            <Show when={piProbeError()}>{(error) => <p class="error">Pi: {error()}</p>}</Show>
            <Show when={piProbe()?.versionError}>
              {(error) => (
                <p class="model-picker-note">
                  Pi version diagnostic failed, but launch/model environment is available: {error()}
                </p>
              )}
            </Show>
          </details>
        </aside>

        <main class="app-main" id="main-content" tabIndex={-1}>
          <Show when={notifications().length > 0}>
            <section class="notification-stack" aria-label="Extension notifications" aria-live="polite">
              <For each={notifications()}>
                {(notification) => (
                  <article class={`extension-notification notification-${notification.notifyType}`}>
                    <div>
                      <strong>
                        {runById(notification.runId)
                          ? runTitle(runById(notification.runId)!)
                          : notification.runId.slice(0, 8)}
                      </strong>
                      <span>{notification.message}</span>
                    </div>
                    <button
                      type="button"
                      aria-label="Dismiss notification"
                      onClick={() =>
                        setNotifications((current) =>
                          current.filter((item) => item.id !== notification.id),
                        )
                      }
                    >
                      ×
                    </button>
                  </article>
                )}
              </For>
            </section>
          </Show>
          <Show when={runtimeError()}>
            {(error) => <p class="app-error">Runtime update failed: {error()}</p>}
          </Show>

          <Show when={view() === "automation"}>
            <Show when={automationError()}>
              {(message) => <p class="app-error">Automation state failed: {message()}</p>}
            </Show>
            <AutomationView
              snapshot={automation()}
              projects={projects()}
              capacity={capacity()}
              piReady={Boolean(piProbe())}
              onRefresh={refreshAutomation}
              onOpenRun={openRun}
            />
          </Show>

          <Show when={view() === "supervision"}>
            <Show when={supervisionError()}>
              {(message) => <p class="app-error">Supervision state failed: {message()}</p>}
            </Show>
            <SupervisionView
              snapshots={supervision()}
              projects={projects()}
              capacity={capacity()}
              piReady={Boolean(piProbe())}
              onRefresh={refreshSupervision}
              onOpenRun={openRun}
            />
          </Show>

          <Show when={view() === "launcher"}>
            <header class="surface-heading">
              <div>
                <h1>New run</h1>
                <p>Start Pi in a local checkout or an isolated Git worktree.</p>
              </div>
              <button type="button" onClick={() => setView("dashboard")}>Cancel</button>
            </header>
            <ProjectLauncher
              piReady={Boolean(piProbe())}
              preferredProjectPath={preferredProjectPath()}
              onStarted={handleStarted}
              onOpenRun={openRun}
              isExecutionRootActive={isExecutionRootActive}
              activeRunIdForExecutionRoot={activeRunIdForExecutionRoot}
              activeRunIdForSessionPath={activeRunIdForSessionPath}
            />
          </Show>

          <Show when={view() === "sessions"}>
            <RecentSessionsView
              projects={projects()}
              preferredProjectPath={preferredProjectPath()}
              piReady={Boolean(piProbe())}
              onStarted={handleStarted}
              onOpenRun={openRun}
              onNewRun={(path) => {
                setPreferredProjectPath(path);
                setView("launcher");
              }}
              activeRunIdForExecutionRoot={activeRunIdForExecutionRoot}
              activeRunIdForSessionPath={activeRunIdForSessionPath}
            />
          </Show>

          <Show when={view() === "attention"}>
            <NeedsAttentionView
              runs={runtime()?.runs ?? []}
              onOpenRun={openRun}
              onResolved={refreshHydration}
              projectLabel={(projectId) => {
                const project = projects().find((candidate) => candidate.id === projectId);
                return project ? pathLeaf(project.canonicalRoot) : `Project ${projectId.slice(0, 8)}`;
              }}
            />
          </Show>

          <Show when={view() === "run"}>
            <Show when={selectedRun()} fallback={<p class="empty-state">That run is no longer retained.</p>}>
              {(run) => (
                <section class="active-run-surface" aria-label={`Run ${runTitle(run())}`}>
                  <header class="surface-heading run-surface-heading">
                    <div>
                      <h1>{runTitle(run())}</h1>
                      <p title={run().run.executionRoot}>
                        {projectLabelForRun(run())} · {run().run.executionIsolation === "git_worktree" ? "Git-isolated worktree" : "Local checkout"} · {run().run.executionRoot}
                      </p>
                      <div class="run-identity-strip" aria-label="Run identity">
                        <span class={`run-state state-${runStateLabel(run()).replaceAll(" ", "-")}`}>
                          {runStateLabel(run())}
                        </span>
                        <span>{runModelLabel(run())}</span>
                        <span>{runThinkingLabel(run())}</span>
                        <span title={`Started ${new Date(run().run.startedUnixMs).toLocaleString()}`}>
                          {runElapsedLabel(run(), elapsedClockUnixMs())}
                        </span>
                        <Show when={run().run.worktree}>
                          {(worktree) => <span>Branch {worktree().branch}</span>}
                        </Show>
                        <Show when={runQueuedCount(run()) > 0}>
                          <span>{runQueuedCount(run())} queued</span>
                        </Show>
                      </div>
                    </div>
                    <div class="run-surface-actions">
                      <button
                        type="button"
                        disabled={Boolean(openingFolderRunId())}
                        onClick={() => void openRunFolder(run())}
                      >
                        {openingFolderRunId() === run().run.id ? "Opening" : "Open folder"}
                      </button>
                      <Show
                        when={isTerminalRun(run())}
                        fallback={
                          <button
                            type="button"
                            disabled={!canCloseRun(run()) || Boolean(closingRunId())}
                            onClick={() => void closeRun(run())}
                          >
                            {closingRunId() === run().run.id ? "Closing" : "Close run"}
                          </button>
                        }
                      >
                        <button
                          type="button"
                          disabled={Boolean(dismissingRunId())}
                          onClick={() => void dismissRun(run())}
                        >
                          {dismissingRunId() === run().run.id ? "Dismissing" : "Dismiss"}
                        </button>
                      </Show>
                      <button type="button" onClick={() => setView("dashboard")}>Dashboard</button>
                    </div>
                  </header>

                  <Show when={runActionError()}>
                    {(error) => <p class="app-error">Run action failed: {error()}</p>}
                  </Show>

                  <Show when={run().run.failure}>
                    {(failure) => (
                      <section class="run-failure-panel" role="alert" aria-label="Run failure">
                        <strong>
                          {run().run.process === "quarantined"
                            ? "Termination uncertain"
                            : `Run failed · ${failure().kind.replaceAll("_", " ")}`}
                        </strong>
                        <span>{failure().detail}</span>
                        <small>
                          {run().run.exitCode == null ? "No process exit code" : `Process exit code ${run().run.exitCode}`}
                          {failure().detailTruncated ? " · detail truncated by backend limit" : ""}
                        </small>
                      </section>
                    )}
                  </Show>

                  <Show when={(run().rpc?.pendingDialogs.length ?? 0) > 0}>
                    <section class="attention" aria-label="Needs attention">
                      <h2>Needs Attention</h2>
                      <For each={run().rpc?.pendingDialogs ?? []}>
                        {(dialog) => (
                          <ExtensionDialogCard
                            runId={run().run.id}
                            dialog={dialog}
                            onResolved={refreshHydration}
                          />
                        )}
                      </For>
                    </section>
                  </Show>

                  <PiRuntimeNoticePanel rpc={run().rpc} />

                  <ExtensionUiPanel snapshot={run().rpc?.extensionUi} />

                  <ComposerCard
                    run={run()}
                    state={composerState(run())}
                    attachmentLimits={attachmentLimits()}
                    onResolved={refreshHydration}
                    onReviewSummary={rememberChangeSummary}
                  />
                </section>
              )}
            </Show>
          </Show>

          <Show when={view() === "dashboard"}>
            <section class="dashboard" aria-label="Run dashboard">
              <header class="surface-heading">
                <div>
                  <h1>Runs</h1>
                  <p>Open a session for details. Runs keep working when you switch views.</p>
                </div>
                <button type="button" onClick={() => setView("launcher")}>New run</button>
              </header>

              <Show when={(runtime()?.runs.length ?? 0) > 0} fallback={
                <div class="empty-state">
                  <strong>No runs yet</strong>
                  <span>Start one and give Pi the task up front.</span>
                  <button type="button" onClick={() => setView("launcher")}>Start a run</button>
                </div>
              }>
                <div class="run-grid">
                  <For each={sortedRuns()}>
                    {(run) => (
                      <article class="run-card">
                        <div>
                          <strong>{runTitle(run)}</strong>
                          <span class={`run-state state-${runStateLabel(run).replaceAll(" ", "-")}`}>
                            {runStateLabel(run)}
                          </span>
                        </div>
                        <span title={run.run.executionRoot} class="run-path">
                          {projectLabelForRun(run)} · {run.run.executionIsolation === "git_worktree" ? "Git-isolated worktree" : "Local checkout"} · {run.run.executionRoot}
                        </span>
                        <Show when={run.run.worktree}>
                          {(worktree) => <small>{worktree().branch} · {worktree().baseCommit.slice(0, 12)}</small>}
                        </Show>
                        <small>{runModelLabel(run)} · {runThinkingLabel(run)}</small>
                        <small class="run-activity">{runActivityLabel(run)}</small>
                        <div class="run-card-meta">
                          <small title={`Started ${new Date(run.run.startedUnixMs).toLocaleString()}`}>
                            {runElapsedLabel(run, elapsedClockUnixMs())}
                          </small>
                          <Show when={knownChangeSummaries()[run.run.id]}>
                            {(known) => (
                              <small
                                class={known().changeRevision === run.run.changeRevision ? undefined : "review-stale"}
                                title={
                                  known().changeRevision === run.run.changeRevision
                                    ? "Last explicitly loaded Git review summary"
                                    : "Pi completed tool or shell activity after this Git review; open Changes to refresh"
                                }
                              >
                                {known().changeRevision === run.run.changeRevision ? "Last review" : "Review stale"} · {known().fileCount} changed file{known().fileCount === 1 ? "" : "s"}
                                {known().truncated ? "+" : ""}
                              </small>
                            )}
                          </Show>
                        </div>
                        <Show when={(run.rpc?.pendingDialogs.length ?? 0) > 0}>
                          <strong class="attention-count">{run.rpc!.pendingDialogs.length} request{run.rpc!.pendingDialogs.length === 1 ? "" : "s"} need attention</strong>
                        </Show>
                        <div class="run-card-actions">
                          <button type="button" onClick={() => openRun(run.run.id)}>Open</button>
                          <button
                            type="button"
                            disabled={Boolean(openingFolderRunId())}
                            onClick={() => void openRunFolder(run)}
                          >
                            {openingFolderRunId() === run.run.id ? "Opening" : "Folder"}
                          </button>
                          <Show
                            when={
                              run.run.process === "ready" &&
                              runHasStoppableActivity(run)
                            }
                          >
                            <button
                              type="button"
                              onClick={() => void stopRunFromDashboard(run)}
                            >
                              Stop
                            </button>
                          </Show>
                          <Show when={canCloseRun(run)}>
                            <button
                              type="button"
                              disabled={Boolean(closingRunId())}
                              onClick={() => void closeRun(run)}
                            >
                              {closingRunId() === run.run.id ? "Closing" : "Close"}
                            </button>
                          </Show>
                          <Show when={isTerminalRun(run)}>
                            <button
                              type="button"
                              disabled={Boolean(dismissingRunId())}
                              onClick={() => void dismissRun(run)}
                            >
                              {dismissingRunId() === run.run.id ? "Dismissing" : "Dismiss"}
                            </button>
                          </Show>
                        </div>
                      </article>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={runActionError()}>
                {(error) => <p class="error">Run action failed: {error()}</p>}
              </Show>
            </section>

            <Show when={pendingDialogs().length > 0}>
              <section class="attention dashboard-attention" aria-label="Needs attention">
                <h2>Needs Attention</h2>
                <For each={pendingDialogs()}>
                  {(pending) => (
                    <article class="attention-summary">
                      <div>
                        <strong>{runById(pending.runId) ? runTitle(runById(pending.runId)!) : pending.runId.slice(0, 8)}</strong>
                        <span>{pending.dialog.request.kind.title}</span>
                      </div>
                      <button type="button" onClick={() => openRun(pending.runId)}>Open</button>
                    </article>
                  )}
                </For>
              </section>
            </Show>
          </Show>
        </main>
      </div>
    </>
  );
}
