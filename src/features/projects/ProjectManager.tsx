import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop, pickDirectory } from "../../lib/desktop";
import { pathLeaf } from "../../lib/path";
import type { DesktopProjectRecord } from "../../types/projects";

export function ProjectManager(props: {
  refreshKey: number;
  onUse: (path: string) => void;
  onProjects: (projects: DesktopProjectRecord[]) => void;
}) {
  const [projects, setProjects] = createSignal<DesktopProjectRecord[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [busyId, setBusyId] = createSignal<string>();
  const [error, setError] = createSignal<string>();

  const load = async () => {
    if (loading()) return;
    setLoading(true);
    setError(undefined);
    try {
      const next = await invokeDesktop<DesktopProjectRecord[]>("runtime_list_projects");
      setProjects(next);
      props.onProjects(next);
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  };

  createEffect(() => {
    props.refreshKey;
    void load();
  });

  const relocate = async (project: DesktopProjectRecord) => {
    if (busyId()) return;
    let next: string | undefined;
    try {
      next = (await pickDirectory(project.canonicalRoot))?.trim();
    } catch (pickError) {
      setError(`Could not choose replacement folder: ${String(pickError)}`);
      return;
    }
    if (!next || next === project.canonicalRoot) return;
    setBusyId(project.id);
    setError(undefined);
    try {
      await invokeDesktop<DesktopProjectRecord>("runtime_relocate_project", {
        request: { id: project.id, newRoot: next },
      });
      await load();
    } catch (relocateError) {
      setError(String(relocateError));
    } finally {
      setBusyId(undefined);
    }
  };

  const remove = async (project: DesktopProjectRecord) => {
    if (busyId()) return;
    if (
      !window.confirm(
        `Remove ${project.canonicalRoot} from Pi Wizard? This only removes the app registration; it does not delete the folder or Git repository.`,
      )
    )
      return;
    setBusyId(project.id);
    setError(undefined);
    try {
      await invokeDesktop<void>("runtime_remove_project", { request: { id: project.id } });
      await load();
    } catch (removeError) {
      setError(String(removeError));
    } finally {
      setBusyId(undefined);
    }
  };

  return (
    <section class="project-manager" aria-label="Projects">
      <div class="sidebar-heading">
        <strong>Projects</strong>
        <button type="button" disabled={loading()} onClick={() => void load()}>
          {loading() ? "…" : "Refresh"}
        </button>
      </div>
      <For each={projects()}>
        {(project) => (
          <article class={`project-row project-${project.status}`}>
            <button
              type="button"
              class="project-use"
              disabled={project.status !== "present" || Boolean(busyId())}
              title={project.canonicalRoot}
              onClick={() => props.onUse(project.canonicalRoot)}
            >
              <strong>{pathLeaf(project.canonicalRoot)}</strong>
              <span>{project.status === "present" ? project.canonicalRoot : `Detached · ${project.canonicalRoot}`}</span>
            </button>
            <Show when={project.detail}>
              {(detail) => <small>{detail()}</small>}
            </Show>
            <div class="project-row-actions">
              <button type="button" disabled={Boolean(busyId())} onClick={() => void relocate(project)}>
                Relocate
              </button>
              <button type="button" disabled={Boolean(busyId())} onClick={() => void remove(project)}>
                Remove
              </button>
            </div>
          </article>
        )}
      </For>
      <Show when={!loading() && projects().length === 0}>
        <p class="sidebar-note">No registered projects yet.</p>
      </Show>
      <Show when={error()}>{(message) => <p class="error">Projects: {message()}</p>}</Show>
    </section>
  );
}
