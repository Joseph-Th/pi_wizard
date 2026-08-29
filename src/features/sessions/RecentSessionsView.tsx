import { createEffect, createSignal, For, Show } from "solid-js";

import { pathLeaf } from "../../lib/path";
import type { ExtensionDiscoveryPolicy, StartRunResult } from "../../types/launch";
import type { DesktopProjectRecord } from "../../types/projects";
import type { ContextFilesPolicy, ProjectTrustPolicy } from "../models/types";
import { SessionCatalogBrowser } from "./SessionCatalogBrowser";

export function RecentSessionsView(props: {
  projects: DesktopProjectRecord[];
  preferredProjectPath: string;
  piReady: boolean;
  onStarted: (result: StartRunResult) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
  onNewRun: (projectPath: string) => void;
  activeRunIdForExecutionRoot: (path: string) => string | undefined;
  activeRunIdForSessionPath: (path: string) => string | undefined;
}) {
  const [projectPath, setProjectPath] = createSignal("");
  const [projectTrust, setProjectTrust] = createSignal<ProjectTrustPolicy>("inherit");
  const [contextFiles, setContextFiles] = createSignal<ContextFilesPolicy>("inherit");
  const [extensionDiscovery, setExtensionDiscovery] =
    createSignal<ExtensionDiscoveryPolicy>("inherit");

  const availableProjects = () => props.projects.filter((project) => project.status === "present");

  createEffect(() => {
    const available = availableProjects();
    const current = projectPath();
    if (available.some((project) => project.canonicalRoot === current)) return;
    const preferred = props.preferredProjectPath.trim();
    const next = available.find((project) => project.canonicalRoot === preferred) ?? available[0];
    setProjectPath(next?.canonicalRoot ?? "");
  });

  return (
    <section class="recent-sessions-surface" aria-label="Recent Pi sessions">
      <header class="surface-heading">
        <div>
          <h1>Recent sessions</h1>
          <p>
            Browse Pi’s authoritative session files on demand. Nothing is scanned while this view is
            closed.
          </p>
        </div>
        <button
          type="button"
          disabled={!projectPath()}
          onClick={() => props.onNewRun(projectPath())}
        >
          New run
        </button>
      </header>

      <Show
        when={availableProjects().length > 0}
        fallback={
          <div class="empty-state">
            <strong>No available project folders</strong>
            <span>Choose or browse a folder from New Run. Used folders are saved there as quick project presets.</span>
          </div>
        }
      >
        <div class="recent-session-controls">
          <label>
            <span>Project</span>
            <select
              value={projectPath()}
              onChange={(event) => setProjectPath(event.currentTarget.value)}
            >
              <For each={availableProjects()}>
                {(project) => (
                  <option value={project.canonicalRoot}>{pathLeaf(project.canonicalRoot)}</option>
                )}
              </For>
            </select>
            <small class="recent-project-path" title={projectPath()}>{projectPath()}</small>
          </label>
          <details class="session-launch-options">
            <summary>Resume launch options</summary>
            <div>
              <label>
                <span>Project resources</span>
                <select
                  value={projectTrust()}
                  onChange={(event) =>
                    setProjectTrust(event.currentTarget.value as ProjectTrustPolicy)
                  }
                >
                  <option value="inherit">Use Pi saved/default trust</option>
                  <option value="approve">Approve for this run</option>
                  <option value="ignore">Ignore protected project resources</option>
                </select>
              </label>
              <label>
                <span>Context instructions</span>
                <select
                  value={contextFiles()}
                  onChange={(event) =>
                    setContextFiles(event.currentTarget.value as ContextFilesPolicy)
                  }
                >
                  <option value="inherit">Load AGENTS.md / CLAUDE.md using Pi settings</option>
                  <option value="disabled">Disable context files for this launch</option>
                </select>
              </label>
              <label>
                <span>Extensions</span>
                <select
                  value={extensionDiscovery()}
                  onChange={(event) =>
                    setExtensionDiscovery(event.currentTarget.value as ExtensionDiscoveryPolicy)
                  }
                >
                  <option value="inherit">Load discovered Pi extensions</option>
                  <option value="disabled">Disable extensions for this launch</option>
                </select>
              </label>
            </div>
            <p>
              Trust controls protected Pi project resources. Context-file and extension loading are
              independent launch choices. Disable extensions for a one-run recovery when an
              installed Pi extension prevents startup.
            </p>
          </details>
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
      </Show>
    </section>
  );
}
