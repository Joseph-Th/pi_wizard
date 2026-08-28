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
  app.includes('aria-keyshortcuts="Control+Enter Meta+Enter"'),
  "composer keyboard shortcut must be discoverable",
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
  app.includes('"runtime_pick_directory"') && app.includes("Browse"),
  "project selection must expose a native folder picker",
);
requireContract(
  app.includes("extensionUi: ExtensionUiSnapshot") && app.includes('class="extension-ui-panel"'),
  "retained extension status/widget/title state must be visible for the selected run",
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
