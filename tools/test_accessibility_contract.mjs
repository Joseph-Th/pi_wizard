import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const app = readFileSync(resolve(root, "src", "App.tsx"), "utf8");
const styles = readFileSync(resolve(root, "src", "styles.css"), "utf8");

function requireContract(condition, detail) {
  if (!condition) throw new Error(`accessibility contract failed: ${detail}`);
}

requireContract(app.includes('href="#main-content"'), "keyboard skip link is required");
requireContract(app.includes('id="main-content"'), "skip-link destination is required");
requireContract(
  app.includes('aria-live="polite"') || app.includes('class="runtime-details"'),
  "runtime status must remain available from the shell",
);
requireContract(
  app.includes('aria-keyshortcuts="Control+Enter Meta+Enter ArrowUp ArrowDown"'),
  "composer send and command-palette keyboard shortcuts must be discoverable",
);
requireContract(
  app.includes("commandSuggestionIndex") &&
    app.includes('event.key === "ArrowDown" || event.key === "ArrowUp"') &&
    app.includes("stageCommandSuggestion(selected)") &&
    app.includes('class={index() === commandSuggestionIndex() ? "selected" : undefined}') &&
    app.includes('data-command-index={index()}') &&
    app.includes('scrollIntoView({ block: "nearest" })'),
  "bounded Pi slash-command suggestions must support keyboard navigation, keep the active row visible, and avoid a separate command-history store",
);
requireContract(
  app.includes('aria-label="Choose draft images"'),
  "hidden draft image picker must have an accessible name",
);
requireContract(
  app.split('aria-labelledby={`dialog-${request().id}`}').length - 1 >= 2,
  "extension input and editor controls must inherit the dialog title",
);
requireContract(styles.includes(":focus-visible"), "visible keyboard focus style is required");
requireContract(
  styles.includes("prefers-reduced-motion: reduce"),
  "reduced-motion preference must be honored",
);
requireContract(app.includes('class="app-shell"'), "desktop shell must expose sidebar/main navigation");
requireContract(app.includes('class="run-grid"'), "dashboard must render compact run cards");
requireContract(
  app.includes('view() === "automation"') &&
    app.includes("function AutomationView") &&
    app.includes('aria-label="Automation chains"') &&
    app.includes('aria-label="Ordered prompts"') &&
    app.includes('"runtime_save_automation_chain"') &&
    app.includes('"runtime_start_automation"') &&
    app.includes('"runtime_cancel_automation"') &&
    app.includes('"runtime_automation_executions"') &&
    app.includes('"automation://changed"') &&
    app.includes('if (view() !== "automation") return;') &&
    app.includes('if (payload === "catalog") void refreshAutomation();') &&
    app.includes("else void refreshAutomationExecutions();") &&
    app.includes('if (view() === "automation") void refreshAutomation();') &&
    app.includes("promptPreview") &&
    app.includes("promptTruncated") &&
    app.includes("const refreshRuntimeState = async () =>") &&
    app.includes("refreshHydration(),\n      refreshCapacity(),\n    ]);") &&
    app.includes("Git-isolated workers") &&
    app.includes("LLM supervisor · uses one live slot") &&
    styles.includes(".automation-layout") &&
    styles.includes(".automation-step"),
  "finite prompt chains and supervised multi-run orchestration must be first-class keyboard-accessible navigation with on-demand catalog hydration and execution-only event refreshes rather than polling",
);
requireContract(
  app.includes('when={view() === "run"}') && app.includes('when={selectedRun()}'),
  "only the selected run should mount the detailed session surface",
);
requireContract(
  app.includes('"runtime_list_projects"') &&
    app.includes('"runtime_relocate_project"') &&
    app.includes('"runtime_remove_project"'),
  "project list, relocation, and registration removal must be reachable from the renderer",
);
requireContract(
  app.includes("initialTask: initialTask().trim() || null"),
  "new runs must be able to submit their initial task during launch",
);
requireContract(
  app.includes('"runtime_probe_project_launch_options"') &&
    app.includes("New-run model and thinking") &&
    app.includes("provider: launchModel?.provider ?? null") &&
    app.includes("thinking: launchThinking() || null") &&
    app.includes("clearQueueSupported: boolean") &&
    app.includes("does not expose RPC queue clearing") &&
    app.includes("terminates the exact Pi process"),
  "new-run model/thinking discovery must also surface the current Pi build's reusable Stop capability before the initial task is submitted",
);
requireContract(
  app.includes("type ContextFilesPolicy") &&
    app.includes("Disable context files for this launch") &&
    app.includes("contextFiles: contextFiles()"),
  "context-file loading must be an explicit launch policy separate from project-resource trust",
);
requireContract(
  app.includes('"runtime_probe_project_resources"') &&
    app.includes("Check project resources") &&
    app.includes("Protected Pi resources detected") &&
    app.includes("Use Pi saved/default trust leaves the final decision to Pi") &&
    app.includes("context files remain a separate choice"),
  "launch trust must offer a bounded protected-resource preflight without pretending to own Pi saved/default trust resolution",
);
requireContract(
  app.includes("nextCursor: SessionCatalogCursor | null") &&
    app.includes("nextSessionPage") &&
    app.includes("previousSessionPage") &&
    app.includes("Next older") &&
    app.includes("sessionPagingNeedsRestart") &&
    app.includes("Restart from newest"),
  "bounded project-session search must page to older candidates instead of permanently omitting them",
);
requireContract(
  app.includes("function SessionCatalogBrowser") &&
    app.split("<SessionCatalogBrowser").length - 1 >= 2 &&
    app.includes('view() === "sessions"') &&
    app.includes("Recent sessions") &&
    app.includes("Nothing is scanned while this view is") &&
    app.includes("Resume launch options"),
  "historical sessions must have a first-class on-demand navigation surface that reuses the bounded resume browser",
);
requireContract(
  app.includes('view() === "attention"') &&
    app.includes("function NeedsAttentionView") &&
    app.includes('class="nav-count"') &&
    app.includes('class="attention-queue"') &&
    app.includes("<ExtensionDialogCard") &&
    app.includes("Answer extension requests across live Pi runs") &&
    app.includes("remainingTimeoutMs ?? Number.POSITIVE_INFINITY"),
  "Needs Attention must be a first-class global queue whose actions keep exact backend request ownership",
);
requireContract(
  app.includes("function dialogTimeoutLabel") &&
    app.includes("remaining at last sync") &&
    app.includes("No Pi-side timeout"),
  "extension request timeout metadata must be visible without introducing a renderer countdown loop",
);
requireContract(
  app.includes('"runtime_diagnostics"') &&
    app.includes("Refresh diagnostics") &&
    app.includes("No diagnostic polling or logging") &&
    app.includes('data-timeline-row="true"') &&
    app.includes("uiDroppedDisplayFrames") &&
    app.includes("activeSessionCatalogJobs") &&
    app.includes("PerformanceObserver") &&
    app.includes("import.meta.env.DEV"),
  "runtime diagnostics must be explicit pull-based bounded counters with mounted-row and development long-task measurements, not another polling loop",
);
requireContract(
  app.includes('class="run-identity-strip"') &&
    app.includes("runModelLabel(run())") &&
    app.includes("runThinkingLabel(run())") &&
    app.includes("runModelLabel(run)}") &&
    app.includes("runThinkingLabel(run)}"),
  "run and dashboard identity surfaces must keep model and thinking state visible without opening secondary controls",
);
requireContract(
  app.includes("compacting: boolean") &&
    app.includes("followUp: number") &&
    app.includes('return "compacting"') &&
    app.includes('return "queued"') &&
    app.includes("runActivityLabel(run)") &&
    app.includes("function runHasStoppableActivity") &&
    app.includes("runHasStoppableActivity(run)"),
  "dashboard orchestration state must project bounded compaction/queue/retry state and keep Stop reachable across stoppable Pi activity",
);
requireContract(
  app.includes("startedUnixMs: number") &&
    app.includes("terminalUnixMs: number | null") &&
    app.includes("runElapsedLabel(run, elapsedClockUnixMs())") &&
    app.includes("elapsedClockTimer = window.setInterval") &&
    app.includes('known().changeRevision === run.run.changeRevision ? "Last review" : "Review stale"') &&
    app.includes("onReviewSummary={rememberChangeSummary}"),
  "dashboard cards must show backend-owned elapsed time and reuse already-known change summaries without polling Git",
);
requireContract(
  app.includes("const RUNTIME_HYDRATION_SCHEMA_VERSION = 9") &&
    app.includes("snapshot.schemaVersion !== RUNTIME_HYDRATION_SCHEMA_VERSION") &&
    app.includes("Unsupported runtime hydration schema"),
  "renderer hydration must reject an unsupported backend schema instead of applying structurally incompatible state",
);
requireContract(
  app.includes("collapseCompletedOutput") &&
    app.includes('class={`${itemClass} history-collapsible`}') &&
    app.includes("VERBOSE_THINKING_BYTES = 480") &&
    app.includes("collapseThinking") &&
    app.includes("show reasoning"),
  "completed successful tool output and verbose completed thinking must collapse by default while failed or active output remains visible",
);
requireContract(
  app.includes('type InspectorKind = "details" | "changes" | "tree"') &&
    app.includes("const [activeInspector, setActiveInspector]") &&
    app.includes('activeInspector() === "details"') &&
    app.includes('activeInspector() === "changes"') &&
    app.includes('activeInspector() === "tree"') &&
    app.includes('aria-label="Run inspectors"') &&
    app.includes('next === "changes"') &&
    app.includes('reviewChangeRevision() !== props.run.run.changeRevision') &&
    app.includes('void refreshReview();'),
  "run details, Changes, and Session Tree must be mutually exclusive inspectors that are closed by default and load expensive state only on demand",
);
requireContract(
  app.includes("changeRevision: number") &&
    app.includes("reviewChangeRevision() !== props.run.run.changeRevision") &&
    app.includes("Review stale") &&
    app.includes("open Changes to refresh") &&
    app.includes("!reviewSummary() || reviewChangeRevision() !== props.run.run.changeRevision"),
  "known Git review results must become visibly stale after backend tool/Bash invalidation without launching passive Git work",
);
requireContract(
  app.includes('if (run.run.process === "exited") return "done"') &&
    app.includes('if (run.run.process === "quarantined") return "termination uncertain"') &&
    app.includes('return "ready"') &&
    app.includes('"Git-isolated worktree" : "Local checkout"') &&
    app.includes('if (run.run.process === "exited") return "Run finished"') &&
    app.includes('if (run.run.process === "quarantined") return "Process termination is uncertain"'),
  "run lifecycle and execution-isolation labels must use user-facing ready/done/failure/uncertainty wording and literal Git-isolation language instead of leaking backend enums or implying sandboxing",
);
requireContract(
  app.includes("const depthClass = `tree-depth-${Math.min(node.depth, 24)}`") &&
    !app.includes("style={`--tree-depth:") &&
    styles.includes(".tree-depth-24 { --tree-depth: 24; }"),
  "session-tree indentation must use bounded static classes so production CSP does not require inline styles",
);
requireContract(
  app.split('aria-current={view() ===').length - 1 >= 4 &&
    app.includes('aria-current={selectedRunId() === run.run.id && view() === "run" ? "page" : undefined}') &&
    app.includes("projectLabelForRun(run())") &&
    app.includes("projectLabelForRun(run)"),
  "main navigation and run identity must expose current-page state and registered project identity to assistive technology",
);
requireContract(
  app.includes('"runtime_pick_directory"') && app.includes("Browse"),
  "project selection must expose a native folder picker",
);
requireContract(
  app.includes("extensionUi: ExtensionUiSnapshot") && app.includes('class="extension-ui-panel"'),
  "retained extension status/widget/title state must be visible for the selected run",
);
requireContract(
  app.includes('type ExtensionDiscoveryPolicy = "inherit" | "disabled"') &&
    app.includes("Disable extensions for this launch") &&
    app.includes("extensionDiscovery: extensionDiscovery()") &&
    app.includes("--no-extensions"),
  "new and resumed runs must expose Pi's one-run extension-discovery recovery policy independently from trust/context loading",
);
requireContract(
  app.includes("function PiRuntimeNoticePanel") &&
    app.includes("Provider retry scheduled") &&
    app.includes("Summarization retry") &&
    app.includes("Last extension error") &&
    app.includes("abort_retry"),
  "selected runs must surface bounded Pi retry/summarization/extension errors and explain Stop semantics without inventing local retry authority",
);
requireContract(
  app.includes("interface RunCompactionSnapshot") &&
    app.includes("willRetry: boolean") &&
    app.includes("prompt retry pending") &&
    app.includes("does not resubmit it"),
  "Pi compaction reason/abort/overflow-retry outcomes must remain visible without client-side prompt replay",
);
requireContract(
  app.includes("streamStalled: boolean") &&
    app.includes("Pi stream quiet") &&
    app.includes("not probe, retry, or resubmit anything automatically"),
  "quiet working streams must be presented as a non-authoritative one-shot advisory rather than automatic recovery",
);
requireContract(
  app.includes('"runtime_set_auto_retry"') &&
    app.includes("Automatic retry") &&
    app.includes("Pi RPC does not report the current retry-enabled flag in get_state"),
  "automatic provider retry must use Pi's native command without fabricating a recoverable enabled-state mirror",
);
requireContract(
  app.includes("Stop required terminating this Pi process") &&
    app.includes("Pi session history and any recovered queued draft text remain available for Resume") &&
    app.includes("Stop required terminating the Pi process. Its Pi session remains available from Recent Sessions."),
  "hard Stop must be visibly distinguished from a reusable Pi RPC abort in both selected-run and dashboard surfaces",
);
requireContract(
  app.includes('event.kind === "extensionNotification"') && app.includes('class="notification-stack"'),
  "transient extension notifications must be surfaced instead of discarded",
);
requireContract(
  app.includes("function suggestedWorktreeIdentity") &&
    app.includes("setWorktreeBranch(suggested.branch)") &&
    app.includes("setWorktreePath(suggested.path)"),
  "worktree launch should generate usable branch/path defaults after Git inspection",
);
requireContract(
  app.includes("Run started, but the initial task was not sent automatically"),
  "initial-task launch failures must remain visible after the launcher closes",
);
requireContract(
  app.includes("localCheckoutActive") &&
    app.includes("This checkout is already in use by a live run") &&
    app.includes("Use a worktree"),
  "the launcher must block known same-checkout parallel starts and offer Git isolation",
);
requireContract(
  app.includes("MAX_HISTORY_RENDER_PAGES = 4") &&
    app.includes('class="history-window"') &&
    app.includes("historyViewport.scrollTop += historyViewport.scrollHeight - previousHeight"),
  "history navigation must retain a fixed multi-page window and preserve position when older pages prepend",
);
requireContract(
  app.includes('"runtime_close"') &&
    app.includes("function canCloseRun") &&
    app.includes("Close run"),
  "idle runs must expose a backend-owned Close action instead of consuming a live slot forever",
);
requireContract(
  app.includes("activeRunIdForExecutionRoot") &&
    app.includes("props.onOpenRun(runId)") &&
    app.includes('activeRunId()\n                      ? "Open"'),
  "active worktree recovery rows must open their existing live run instead of presenting a disabled Open button",
);
requireContract(
  app.includes('"runtime_dismiss_terminal_run"') &&
    app.includes("function isTerminalRun") &&
    app.includes('dismissingRunId() === run.run.id ? "Dismissing" : "Dismiss"'),
  "terminal runs must be explicitly dismissible without deleting their Pi session or draft",
);
requireContract(
  app.includes("activeRunIdForSessionPath") &&
    app.includes('"Open live run"') &&
    app.includes("close it before resuming another session here"),
  "session Resume must navigate to existing owners instead of offering a start the backend will reject",
);
requireContract(
  app.includes('"runtime_open_run_folder"') &&
    app.includes('"Open folder"') &&
    app.includes('openingFolderRunId() === run.run.id ? "Opening" : "Folder"'),
  "run surfaces must expose the backend-derived checkout/worktree folder without renderer-supplied paths",
);
requireContract(
  app.includes("function runDisplayPriority") &&
    app.split("<For each={sortedRuns()}>").length - 1 >= 2 &&
    app.includes("return right.run.id.localeCompare(left.run.id)"),
  "sidebar and dashboard runs must prioritize attention/working/live state and newest runs over stale terminal rows",
);
requireContract(
  app.includes("interface RunFailureSnapshot") &&
    app.includes('class="run-failure-panel"') &&
    app.includes('role="alert"') &&
    app.includes('"Termination uncertain"') &&
    app.includes("detailTruncated"),
  "failed and quarantined runs must surface backend-bounded failure detail and termination uncertainty",
);

console.log("accessibility structure contract tests passed");
