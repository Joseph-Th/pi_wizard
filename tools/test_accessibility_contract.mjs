import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const entry = readFileSync(resolve(root, "src", "index.tsx"), "utf8");
const app = readFileSync(resolve(root, "src", "app", "App.tsx"), "utf8");
const automation = readFileSync(resolve(root, "src", "features", "automation", "AutomationView.tsx"), "utf8");
const supervision = readFileSync(resolve(root, "src", "features", "supervision", "SupervisionView.tsx"), "utf8");
const models = [
  readFileSync(resolve(root, "src", "features", "models", "ModelPicker.tsx"), "utf8"),
  readFileSync(resolve(root, "src", "features", "models", "types.ts"), "utf8"),
].join("\n");
const projects = [
  readFileSync(resolve(root, "src", "features", "projects", "ProjectLauncher.tsx"), "utf8"),
].join("\n");
const sessions = [
  readFileSync(resolve(root, "src", "features", "sessions", "SessionCatalogBrowser.tsx"), "utf8"),
  readFileSync(resolve(root, "src", "features", "sessions", "RecentSessionsView.tsx"), "utf8"),
].join("\n");
const attention = [
  readFileSync(resolve(root, "src", "features", "attention", "ExtensionDialogCard.tsx"), "utf8"),
  readFileSync(resolve(root, "src", "features", "attention", "NeedsAttentionView.tsx"), "utf8"),
].join("\n");
const composer = readFileSync(resolve(root, "src", "features", "runs", "composer.tsx"), "utf8");
const runContextRail = readFileSync(
  resolve(root, "src", "features", "runs", "RunContextRail.tsx"),
  "utf8",
);
const runs = [
  readFileSync(resolve(root, "src", "features", "runs", "types.tsx"), "utf8"),
  readFileSync(resolve(root, "src", "features", "runs", "history.tsx"), "utf8"),
  composer,
  runContextRail,
  readFileSync(resolve(root, "src", "features", "runs", "presentation.tsx"), "utf8"),
  readFileSync(resolve(root, "src", "features", "runs", "MarkdownText.tsx"), "utf8"),
].join("\n");
const desktop = readFileSync(resolve(root, "src", "lib", "desktop.ts"), "utf8");
const mainCapability = JSON.parse(
  readFileSync(resolve(root, "src-tauri", "capabilities", "main.json"), "utf8"),
);
const desktopHost = [
  readFileSync(resolve(root, "src-tauri", "src", "app", "mod.rs"), "utf8"),
  readFileSync(resolve(root, "src-tauri", "src", "app", "desktop_commands.rs"), "utf8"),
  readFileSync(resolve(root, "src-tauri", "src", "commands", "automation.rs"), "utf8"),
].join("\n");
const automationStore = readFileSync(
  resolve(root, "crates", "pi-wizard-core", "src", "automation.rs"),
  "utf8",
);
const ui = [app, automation, supervision, models, projects, sessions, attention, runs].join("\n");
const styles = [
  readFileSync(resolve(root, "src", "styles", "app.css"), "utf8"),
  readFileSync(resolve(root, "src", "features", "models", "models.css"), "utf8"),
  readFileSync(resolve(root, "src", "features", "supervision", "supervision.css"), "utf8"),
].join("\n");

function requireContract(condition, detail) {
  if (!condition) throw new Error(`accessibility contract failed: ${detail}`);
}

requireContract(ui.includes('href="#main-content"'), "keyboard skip link is required");
requireContract(ui.includes('id="main-content"'), "skip-link destination is required");
requireContract(
  ui.includes('aria-live="polite"') || ui.includes('class="runtime-details"'),
  "runtime status must remain available from the shell",
);
requireContract(
  ui.includes('aria-keyshortcuts="Control+Enter Meta+Enter ArrowUp ArrowDown"'),
  "composer send and command-palette keyboard shortcuts must be discoverable",
);
requireContract(
  ui.includes("commandSuggestionIndex") &&
    ui.includes('event.key === "ArrowDown" || event.key === "ArrowUp"') &&
    ui.includes("stageCommandSuggestion(selected)") &&
    ui.includes('class={index() === commandSuggestionIndex() ? "selected" : undefined}') &&
    ui.includes('data-command-index={index()}') &&
    ui.includes('scrollIntoView({ block: "nearest" })'),
  "bounded Pi slash-command suggestions must support keyboard navigation, keep the active row visible, and avoid a separate command-history store",
);
requireContract(
  ui.includes('aria-label="Choose draft images"'),
  "hidden draft image picker must have an accessible name",
);
requireContract(
  ui.split('aria-labelledby={`dialog-${request().id}`}').length - 1 >= 2,
  "extension input and editor controls must inherit the dialog title",
);
requireContract(styles.includes(":focus-visible"), "visible keyboard focus style is required");
requireContract(
  styles.includes("prefers-reduced-motion: reduce"),
  "reduced-motion preference must be honored",
);
requireContract(
  app.includes('class={`app-shell sidebar-width-${sidebarWidth()}${contextRailVisible() ? " context-rail-visible" : ""}`}'),
  "desktop shell must expose sidebar/main navigation",
);
requireContract(
  app.includes('role="separator"') &&
    app.includes('aria-label="Resize navigation sidebar"') &&
    app.includes("setPointerCapture") &&
    app.includes('event.key !== "ArrowLeft" && event.key !== "ArrowRight"') &&
    app.includes("Math.round((bounded - 208) / 16) * 16") &&
    app.includes('contextRailVisible() ? " context-rail-visible" : ""') &&
    !app.includes('style={`--sidebar-width') &&
    styles.includes("--sidebar-width") &&
    styles.includes(".sidebar-width-208") &&
    styles.includes(".sidebar-width-480") &&
    styles.includes(".attention") &&
    styles.includes(".sidebar-resizer"),
  "desktop navigation width must be pointer- and keyboard-adjustable through CSP-safe bounded static classes without constraining the main surface width",
);
requireContract(ui.includes('class="run-grid"'), "dashboard must render compact run cards");
requireContract(
  runContextRail.includes('aria-label="Run overview and usage"') &&
    runContextRail.includes('"runtime_session_stats"') &&
    runContextRail.includes("justSettled") &&
    runContextRail.includes("sessionChanged || justSettled") &&
    runContextRail.includes("Total tokens") &&
    runContextRail.includes("Cache read") &&
    runContextRail.includes("Context") &&
    runContextRail.includes("Run facts") &&
    runContextRail.includes("Active tools") &&
    !runContextRail.includes("setInterval") &&
    app.includes("runStatusTone(run)") &&
    styles.includes(".app-context-rail") &&
    styles.includes(".tone-danger") &&
    styles.includes(".tone-ready"),
  "wide desktop run surfaces must expose event-driven Pi session usage and semantic run health without passive metrics polling",
);
requireContract(
  mainCapability.windows?.length === 1 &&
    mainCapability.windows[0] === "main" &&
    mainCapability.permissions?.includes("core:event:allow-listen") &&
    mainCapability.permissions?.includes("core:event:allow-unlisten"),
  "the packaged main window must have Tauri ACL permission to install and remove backend event listeners",
);
requireContract(
  ui.includes('view() === "automation"') &&
    ui.includes("function AutomationView") &&
    automation.includes('aria-label="Prompt chains"') &&
    automation.includes("<h1>Prompt chains</h1>") &&
    ui.includes('aria-label="Ordered prompts"') &&
    ui.includes('"runtime_save_automation_chain"') &&
    ui.includes('"runtime_start_automation"') &&
    ui.includes('"runtime_cancel_automation"') &&
    ui.includes('"runtime_automation_executions"') &&
    ui.includes('"automation://changed"') &&
    ui.includes('if (view() !== "automation") return;') &&
    ui.includes('if (payload === "catalog") void refreshAutomation();') &&
    ui.includes("else void refreshAutomationExecutions();") &&
    ui.includes('if (view() === "automation") void refreshAutomation();') &&
    ui.includes("promptPreview") &&
    ui.includes("promptTruncated") &&
    ui.includes("const refreshRuntimeState = async () =>") &&
    ui.includes("const [snapshot] = await Promise.all([") &&
    ui.includes("refreshHydration(),") &&
    ui.includes("refreshCapacity(),") &&
    ui.includes("const installRuntimeListeners = async () =>") &&
    ui.includes("void connectBackend();") &&
    !automation.includes("concurrency") &&
    !automation.includes("worktrees") &&
    automation.includes("promptChainViewDraft") &&
    automation.includes("projectId: project") &&
    automation.includes("visibleCatalog") &&
    automation.includes('aria-pressed={chainId() === chain.id}') &&
    automation.includes('aria-controls="prompt-chain-editor"') &&
    automation.includes("queueMicrotask(() => chainNameInput?.focus())") &&
    app.includes("request: { projectId: requestedProjectId }") &&
    desktopHost.includes('const PROJECT_AUTOMATION_DIRECTORY: &str = ".pi-wizard"') &&
    desktopHost.includes("project.canonical_root().join(PROJECT_AUTOMATION_DIRECTORY)") &&
    automationStore.includes('root.join("prompt-chains.json")') &&
    !desktopHost.includes("AutomationStore::open(root, limits)") &&
    !automation.includes("Git-isolated workers") &&
    !automation.includes("<span>Workers</span>") &&
    !automation.includes("supervisor") &&
    ui.includes('view() === "supervision"') &&
    supervision.includes('"runtime_start_supervision"') &&
    supervision.includes('"runtime_stop_supervision"') &&
    supervision.includes("projectIds") &&
    !supervision.includes("promptTemplates") &&
    !supervision.includes("maxCycles") &&
    supervision.includes("modelProject()?.canonicalRoot") &&
    !supervision.includes("Reusable prompt playbook") &&
    !app.includes("chains={automation()?.catalog.chains ?? []}") &&
    app.includes('"supervision://changed"') &&
    styles.includes(".automation-layout") &&
    styles.includes(".automation-step") &&
    styles.includes(".supervision-surface"),
  "sequential project-local Prompt chains and continuous multi-project Supervision must remain separate first-class keyboard-accessible surfaces driven by backend invalidation rather than polling; prompt-chain definitions must be scoped to the selected project directory rather than the portable/AppData state root",
);
requireContract(
  entry.includes("waitForDesktopBackend") &&
    entry.includes("<BackendGate />") &&
    entry.includes("Starting Pi Wizard") &&
    desktop.includes('invokeDesktop<T>("runtime_backend_ready")') &&
    desktopHost.includes("async fn runtime_backend_ready") &&
    desktopHost.includes("DesktopStartupSnapshot") &&
    desktopHost.includes(".manager") &&
    desktopHost.includes(".hydrate()") &&
    desktopHost.includes("desktop_commands::runtime_backend_ready") &&
    entry.includes("waitForDesktopBackend<AppStartupSnapshot>()") &&
    app.includes("props.startup.runtime") &&
    app.includes("props.startup.capacity") &&
    app.includes("props.startup.attachmentLimits") &&
    !app.includes('invokeDesktop<RuntimeAttachmentLimits>("runtime_attachment_limits")') &&
    app.includes("void connectBackend();") &&
    !app.includes("Runtime state unavailable") &&
    !app.includes("invokeDesktopAtStartup") &&
    !app.includes("retryStartupOperation") &&
    !app.includes("Backend connection failed"),
  "the main runtime UI must mount only after an explicit backend-ready handshake; ordinary App listeners/hydration must not race Tauri startup or surface the old false backend-connection banner",
);
requireContract(
  models.includes('"runtime_probe_project_models"') &&
    models.includes('projectPath: path || null') &&
    !models.includes("!props.piReady") &&
    !models.includes("!props.projectPath.trim()") &&
    models.includes("Model diagnostics") &&
    models.includes('"runtime_probe_project_launch_options"') &&
    models.includes('"runtime_model_catalog"') &&
    models.includes('"runtime_model_preferences"') &&
    models.includes('"runtime_set_new_run_model_preference"') &&
    models.includes('"runtime_set_model_favorite"') &&
    models.includes('"runtime_save_custom_model"') &&
    models.includes('"runtime_delete_custom_model"') &&
    projects.includes("<ModelPicker") &&
    automation.includes("<ModelPicker") &&
    supervision.includes("<ModelPicker") &&
    automation.includes("rememberNewRunSelection") &&
    !automation.includes("setModel(undefined)") &&
    automation.includes("provider: model()?.provider ?? null") &&
    supervision.includes("provider: model()?.provider ?? null") &&
    supervision.includes('label="Model and thinking"') &&
    models.includes("for (const model of discovery()?.models ?? [])") &&
    models.includes('optgroup label="Favorites"') &&
    models.includes("modelSelectElement.value = key") &&
    models.includes('class="model-picker-model-control"') &&
    styles.includes(".model-picker-model-control") &&
    !styles.includes(".model-select-row") &&
    models.includes('aria-pressed={props.model ? selectedIsFavorite() : false}') &&
    models.includes('"★ Favorited" : "☆ Favorite"') &&
    projects.includes("rememberNewRunSelection") &&
    !projects.includes('setLaunchModelKey("");\n    setLaunchThinking("");') &&
    models.includes("Pi model discovery:"),
  "New Run, Prompt chains, and Supervision must use the shared Pi model catalog and favorites-first selector, with Supervision probing against a selected project and New Run keeping its durable preference",
);
requireContract(
  ui.includes('when={view() === "run"}') && ui.includes('when={selectedRun()}'),
  "only the selected run should mount the detailed session surface",
);
requireContract(
  app.includes('"runtime_list_projects"') &&
    app.includes("void refreshProjects();") &&
    ui.includes('"runtime_relocate_project"') &&
    ui.includes('"runtime_remove_project"') &&
    projects.includes("projects: DesktopProjectRecord[]") &&
    projects.includes("Choose a saved project") &&
    projects.includes("Manage saved projects") &&
    !app.includes("<ProjectManager"),
  "durable project registrations must act as quick directory presets inside New Run, with relocation/removal available on demand rather than occupying the sidebar",
);
requireContract(
  ui.includes("initialTask: initialTask().trim() || null"),
  "new runs must be able to submit their initial task during launch",
);
requireContract(
  ui.includes('"runtime_probe_project_launch_options"') &&
    ui.includes("Model and thinking") &&
    ui.includes("provider: launchModel?.provider ?? null") &&
    ui.includes("thinking: launchThinking() || null") &&
    ui.includes("clearQueueSupported: boolean") &&
    ui.includes("Stop compatibility:") &&
    ui.includes("may terminate the") &&
    ui.includes("owned Pi process instead of reusing it"),
  "new-run model/thinking discovery must also surface the current Pi build's reusable Stop capability before the initial task is submitted",
);
requireContract(
  ui.includes("type ContextFilesPolicy") &&
    ui.includes("Disable context files for this launch") &&
    ui.includes("contextFiles: contextFiles()"),
  "context-file loading must be an explicit launch policy separate from project-resource trust",
);
requireContract(
  ui.includes('"runtime_probe_project_resources"') &&
    ui.includes("Check project resources") &&
    ui.includes("Protected Pi resources detected") &&
    ui.includes("Use Pi saved/default trust leaves the final decision to Pi") &&
    ui.includes("context files remain a separate choice"),
  "launch trust must offer a bounded protected-resource preflight without pretending to own Pi saved/default trust resolution",
);
requireContract(
  ui.includes("nextCursor: SessionCatalogCursor | null") &&
    ui.includes("nextSessionPage") &&
    ui.includes("previousSessionPage") &&
    ui.includes("Next older") &&
    ui.includes("sessionPagingNeedsRestart") &&
    ui.includes("Restart from newest"),
  "bounded project-session search must page to older candidates instead of permanently omitting them",
);
requireContract(
  ui.includes("function SessionCatalogBrowser") &&
    ui.split("<SessionCatalogBrowser").length - 1 >= 2 &&
    ui.includes('view() === "sessions"') &&
    ui.includes("Recent sessions") &&
    sessions.includes("autoLoad?: boolean") &&
    sessions.includes("autoLoadProjectPath") &&
    sessions.includes("autoLoad") &&
    ui.includes("Resume launch options"),
  "Recent Sessions must automatically load the newest bounded page while New Run reuses the same browser without enabling automatic catalog work",
);
requireContract(
  ui.includes('view() === "attention"') &&
    ui.includes("function NeedsAttentionView") &&
    ui.includes('class="nav-count"') &&
    ui.includes('class="attention-queue"') &&
    ui.includes("<ExtensionDialogCard") &&
    ui.includes("Answer extension requests across live Pi runs") &&
    ui.includes("remainingTimeoutMs ?? Number.POSITIVE_INFINITY"),
  "Needs Attention must be a first-class global queue whose actions keep exact backend request ownership",
);
requireContract(
  ui.includes("function dialogTimeoutLabel") &&
    ui.includes("remaining at last sync") &&
    ui.includes("No Pi-side timeout"),
  "extension request timeout metadata must be visible without introducing a renderer countdown loop",
);
requireContract(
  ui.includes('"runtime_diagnostics"') &&
    ui.includes("Refresh diagnostics") &&
    ui.includes("No diagnostic polling or logging") &&
    ui.includes('data-timeline-row="true"') &&
    ui.includes("uiDroppedDisplayFrames") &&
    ui.includes("activeSessionCatalogJobs") &&
    ui.includes("PerformanceObserver") &&
    ui.includes("import.meta.env.DEV"),
  "runtime diagnostics must be explicit pull-based bounded counters with mounted-row and development long-task measurements, not another polling loop",
);
requireContract(
  ui.includes('"runtime_export_session_html"') &&
    ui.includes("Export session HTML") &&
    ui.includes('"runtime_run_bash"') &&
    ui.includes('"runtime_abort_bash"') &&
    ui.includes("One-shot command") &&
    composer.includes("const hasActiveDirectBash = () =>") &&
    composer.includes("composerDisabled = () => disabled() || props.run.draftRestorePending || hasActiveDirectBash()") &&
    composer.includes("commandBusy() || hasActiveDirectBash()") &&
    composer.includes("Direct Bash owns this execution root") &&
    ui.includes('return "command running"') &&
    ui.includes("(run.rpc?.live.directBash.length ?? 0) === 0") &&
    ui.includes("and is excluded") &&
    ui.includes("from model context.") &&
    ui.includes("Live output appears in Live activity") &&
    ui.includes("bounded output"),
  "run details must expose Pi-native session HTML export and a bounded cancellable one-shot command surface whose backend-projected ownership survives renderer reload, blocks conflicting model/session controls and Close, and never adds command output to model context",
);
requireContract(
  ui.includes('class="run-identity-strip"') &&
    ui.includes("runModelLabel(run())") &&
    ui.includes("runThinkingLabel(run())") &&
    ui.includes("runModelLabel(run)}") &&
    ui.includes("runThinkingLabel(run)}"),
  "run and dashboard identity surfaces must keep model and thinking state visible without opening secondary controls",
);
requireContract(
  ui.includes("compacting: boolean") &&
    ui.includes("followUp: number") &&
    ui.includes('return "compacting"') &&
    ui.includes('return "queued"') &&
    ui.includes("runActivityLabel(run)") &&
    ui.includes("function runHasStoppableActivity") &&
    ui.includes("runHasStoppableActivity(run)"),
  "dashboard orchestration state must project bounded compaction/queue/retry state and keep Stop reachable across stoppable Pi activity",
);
requireContract(
  app.includes("function DashboardRunMetrics") &&
    app.includes('invokeDesktop<SessionStats>("runtime_session_stats"') &&
    app.includes("sessionChanged || justSettled") &&
    app.includes("Context ${Math.min(100, Math.max(0, contextPercent)).toFixed(1)}%") &&
    app.includes("usage.tokens.total.toLocaleString()") &&
    app.includes("run-card-live-facts"),
  "dashboard run cards must show lightweight event-driven Pi context/token facts without polling",
);
requireContract(
  ui.includes("startedUnixMs: number") &&
    ui.includes("terminalUnixMs: number | null") &&
    ui.includes("runElapsedLabel(run, elapsedClockUnixMs())") &&
    ui.includes("elapsedClockTimer = window.setInterval") &&
    ui.includes('known().changeRevision === run.run.changeRevision ? "Last review" : "Review stale"') &&
    ui.includes("onReviewSummary={rememberChangeSummary}"),
  "dashboard cards must show backend-owned elapsed time and reuse already-known change summaries without polling Git",
);
requireContract(
  ui.includes("const RUNTIME_HYDRATION_SCHEMA_VERSION = 10") &&
    ui.includes("snapshot.schemaVersion !== RUNTIME_HYDRATION_SCHEMA_VERSION") &&
    ui.includes("Unsupported runtime hydration schema"),
  "renderer hydration must reject an unsupported backend schema instead of applying structurally incompatible state",
);
requireContract(
  ui.includes("const live = () => props.run.rpc?.live") &&
    ui.includes('class="live-block live-thinking live-reasoning"') &&
    !composer.includes('class="live-block live-tool"') &&
    ui.includes('class="live-block live-command"') &&
    ui.includes("Model turn active · thinking / generating") &&
    ui.includes("Pi is idle and ready") &&
    ui.includes("Pi still reports this turn active") &&
    ui.includes("streamStalled") &&
    ui.includes("Reading project files") &&
    ui.includes("Searching the codebase") &&
    ui.includes("Editing files") &&
    ui.includes("Checking repository state") &&
    ui.includes("toolActivityLabel(tool.toolName)") &&
    ui.includes("pinnedToBottom") &&
    ui.includes("viewport.scrollTop = viewport.scrollHeight") &&
    ui.includes("props.run.run.agentWorking") &&
    ui.includes("const visibleReasoning = () =>") &&
    ui.includes('loaded.page.items.filter((item) => item.kind === "user" || item.kind === "assistant")') &&
    ui.includes("historyPinnedToBottom") &&
    ui.includes("loadLatest()") &&
    ui.includes("const observedMessageCount = cursor === null ? props.run.run.session.messageCount : null") &&
    ui.includes("const observedSessionSyncRevision = cursor === null ? (props.run.rpc?.sessionSync.revision ?? null) : null") &&
    ui.includes("sessionSync: SessionSyncState") &&
    ui.includes("currentSyncRevision > loadedSyncRevision") &&
    desktopHost.includes("let latest_read = request.cursor.is_none();") &&
    desktopHost.includes("SessionHistoryReadSource::AwaitingFirstEntry") &&
    desktopHost.includes("message_count == Some(0)") &&
    desktopHost.includes("observed_persisted_cursor") &&
    desktopHost.includes("bootstrap_session_sync(") &&
    desktopHost.includes("page.append_cursor.clone()") &&
    ui.includes('<pre class="history-prompt-text">{item.text}</pre>') &&
    ui.includes("<MarkdownText text={item.text} />") &&
    ui.includes("DOMPurify.sanitize") &&
    ui.includes("markedHighlight") &&
    ui.includes("return escapeHtml(code)") &&
    ui.includes("html({ text })") &&
    ui.includes("return escapeHtml(text)") &&
    ui.includes('ALLOWED_ATTR: ["class"]') &&
    !ui.includes("USE_PROFILES: { html: true }") &&
    styles.includes(".live-activity-idle") &&
    styles.includes(".history-prompt-text") &&
    !composer.includes("const hasContent = () =>") &&
    !ui.includes("Running tool:") &&
    !ui.includes("Running shell command") &&
    !ui.includes("VERBOSE_THINKING_BYTES") &&
    !ui.includes("collapseThinking"),
  "the top conversation must preserve prompts verbatim and render only final answers as sanitized rich text, keep rapid tool calls represented only by stable activity status, treat Pi's advertised-but-not-yet-created zero-message session file as a valid empty latest page, seed Pi get_entries synchronization even from that null cursor, and follow session-sync revisions so final output cannot disappear at settlement while the lower activity pane drops transient content",
);
requireContract(
  ui.includes('type InspectorKind = "details" | "changes" | "tree"') &&
    ui.includes("const [activeInspector, setActiveInspector]") &&
    ui.includes('activeInspector() === "details"') &&
    ui.includes('activeInspector() === "changes"') &&
    ui.includes('activeInspector() === "tree"') &&
    ui.includes('aria-label="Run inspectors"') &&
    ui.includes('next === "changes"') &&
    ui.includes('reviewChangeRevision() !== props.run.run.changeRevision') &&
    ui.includes('void refreshReview();'),
  "run details, Changes, and Session Tree must be mutually exclusive inspectors that are closed by default and load expensive state only on demand",
);
requireContract(
  ui.includes("changeRevision: number") &&
    ui.includes("reviewChangeRevision() !== props.run.run.changeRevision") &&
    ui.includes("Review stale") &&
    ui.includes("open Changes to refresh") &&
    ui.includes("!reviewSummary() || reviewChangeRevision() !== props.run.run.changeRevision"),
  "known Git review results must become visibly stale after backend tool/Bash invalidation without launching passive Git work",
);
requireContract(
  ui.includes('if (run.run.process === "exited") return "done"') &&
    ui.includes('if (run.run.process === "quarantined") return "termination uncertain"') &&
    ui.includes('return "ready"') &&
    ui.includes('"Git-isolated worktree" : "Local checkout"') &&
    ui.includes('if (run.run.process === "exited") return "Run finished"') &&
    ui.includes('if (run.run.process === "quarantined") return "Process termination is uncertain"'),
  "run lifecycle and execution-isolation labels must use user-facing ready/done/failure/uncertainty wording and literal Git-isolation language instead of leaking backend enums or implying sandboxing",
);
requireContract(
  ui.includes("const depthClass = `tree-depth-${Math.min(node.depth, 24)}`") &&
    !ui.includes("style={`--tree-depth:") &&
    styles.includes(".tree-depth-24 { --tree-depth: 24; }"),
  "session-tree indentation must use bounded static classes so production CSP does not require inline styles",
);
requireContract(
  ui.split('aria-current={view() ===').length - 1 >= 4 &&
    ui.includes('aria-current={selectedRunId() === run.run.id && view() === "run" ? "page" : undefined}') &&
    ui.includes("projectLabelForRun(run())") &&
    ui.includes("projectLabelForRun(run)"),
  "main navigation and run identity must expose current-page state and registered project identity to assistive technology",
);
requireContract(
  desktop.includes('"runtime_pick_directory"') && ui.includes("pickDirectory") && ui.includes("Browse"),
  "project selection must expose a native folder picker through the shared desktop adapter",
);
requireContract(
  ui.includes("extensionUi: ExtensionUiSnapshot") && ui.includes('class="extension-ui-panel"'),
  "retained extension status/widget/title state must be visible for the selected run",
);
requireContract(
  ui.includes('type ExtensionDiscoveryPolicy = "inherit" | "disabled"') &&
    ui.includes("Disable extensions for this launch") &&
    ui.includes("extensionDiscovery: extensionDiscovery()") &&
    ui.includes("--no-extensions"),
  "new and resumed runs must expose Pi's one-run extension-discovery recovery policy independently from trust/context loading",
);
requireContract(
  ui.includes("function PiRuntimeNoticePanel") &&
    ui.includes("Provider retry scheduled") &&
    ui.includes("Summarization retry") &&
    ui.includes("Last extension error") &&
    ui.includes("abort_retry"),
  "selected runs must surface bounded Pi retry/summarization/extension errors and explain Stop semantics without inventing local retry authority",
);
requireContract(
  ui.includes("interface RunCompactionSnapshot") &&
    ui.includes("willRetry: boolean") &&
    ui.includes("prompt retry pending") &&
    ui.includes("does not resubmit it"),
  "Pi compaction reason/abort/overflow-retry outcomes must remain visible without client-side prompt replay",
);
requireContract(
  ui.includes("streamStalled: boolean") &&
    ui.includes("Pi stream quiet") &&
    ui.includes("not probe, retry, or resubmit anything automatically"),
  "quiet working streams must be presented as a non-authoritative one-shot advisory rather than automatic recovery",
);
requireContract(
  ui.includes('"runtime_set_auto_retry"') &&
    ui.includes("Automatic retry") &&
    ui.includes("Pi RPC does not report the current retry-enabled flag in get_state"),
  "automatic provider retry must use Pi's native command without fabricating a recoverable enabled-state mirror",
);
requireContract(
  ui.includes("Stop required terminating this Pi process") &&
    ui.includes("Pi session history and any recovered queued draft text remain available for Resume") &&
    ui.includes("Stop required terminating the Pi process. Its Pi session remains available from Recent Sessions."),
  "hard Stop must be visibly distinguished from a reusable Pi RPC abort in both selected-run and dashboard surfaces",
);
requireContract(
  ui.includes('event.kind === "extensionNotification"') && ui.includes('class="notification-stack"'),
  "transient extension notifications must be surfaced instead of discarded",
);
requireContract(
  ui.includes("function suggestedWorktreeIdentity") &&
    ui.includes("setWorktreeBranch(suggested.branch)") &&
    ui.includes("setWorktreePath(suggested.path)"),
  "worktree launch should generate usable branch/path defaults after Git inspection",
);
requireContract(
  ui.includes("Run started, but the initial task was not sent automatically"),
  "initial-task launch failures must remain visible after the launcher closes",
);
requireContract(
  ui.includes("localCheckoutActive") &&
    ui.includes("This checkout is already in use by a live run") &&
    ui.includes("Use a worktree"),
  "the launcher must block known same-checkout parallel starts and offer Git isolation",
);
requireContract(
  ui.includes("MAX_HISTORY_RENDER_PAGES = 4") &&
    ui.includes('class="history-window"') &&
    ui.includes("historyViewport.scrollTop += historyViewport.scrollHeight - previousHeight"),
  "history navigation must retain a fixed multi-page window and preserve position when older pages prepend",
);
requireContract(
  ui.includes('"runtime_close"') &&
    ui.includes("function canCloseRun") &&
    ui.includes("Close run"),
  "idle runs must expose a backend-owned Close action instead of consuming a live slot forever",
);
requireContract(
  ui.includes("activeRunIdForExecutionRoot") &&
    ui.includes("props.onOpenRun(runId)") &&
    ui.includes('activeRunId()\n                      ? "Open"'),
  "active worktree recovery rows must open their existing live run instead of presenting a disabled Open button",
);
requireContract(
  ui.includes('"runtime_dismiss_terminal_run"') &&
    ui.includes("function isTerminalRun") &&
    ui.includes('dismissingRunId() === run.run.id ? "Dismissing" : "Dismiss"'),
  "terminal runs must be explicitly dismissible without deleting their Pi session or draft",
);
requireContract(
  ui.includes("activeRunIdForSessionPath") &&
    ui.includes('"Open live run"') &&
    ui.includes("close it before resuming another session here"),
  "session Resume must navigate to existing owners instead of offering a start the backend will reject",
);
requireContract(
  ui.includes('"runtime_open_run_folder"') &&
    ui.includes('"Open folder"') &&
    ui.includes('openingFolderRunId() === run.run.id ? "Opening" : "Folder"'),
  "run surfaces must expose the backend-derived checkout/worktree folder without renderer-supplied paths",
);
requireContract(
  ui.includes("function runDisplayPriority") &&
    ui.split("<For each={sortedRuns()}>").length - 1 >= 2 &&
    ui.includes("return right.run.id.localeCompare(left.run.id)"),
  "sidebar and dashboard runs must prioritize attention/working/live state and newest runs over stale terminal rows",
);
requireContract(
  ui.includes("interface RunFailureSnapshot") &&
    ui.includes('class="run-failure-panel"') &&
    ui.includes('role="alert"') &&
    ui.includes('"Termination uncertain"') &&
    ui.includes("detailTruncated"),
  "failed and quarantined runs must surface backend-bounded failure detail and termination uncertainty",
);

console.log("accessibility structure contract tests passed");
