import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import type { ExtensionDiscoveryPolicy, StartRunResult } from "../../types/launch";
import type { ContextFilesPolicy, ProjectTrustPolicy } from "../models/types";
import type { SessionCatalogCursor, SessionCatalogEntry, SessionCatalogPage } from "./types";

export function SessionCatalogBrowser(props: {
  projectPath: string;
  piReady: boolean;
  projectTrust: ProjectTrustPolicy;
  contextFiles: ContextFilesPolicy;
  extensionDiscovery: ExtensionDiscoveryPolicy;
  onStarted: (result: StartRunResult) => Promise<unknown>;
  onOpenRun: (runId: string) => void;
  activeRunIdForExecutionRoot: (path: string) => string | undefined;
  activeRunIdForSessionPath: (path: string) => string | undefined;
}) {
  const [sessionQuery, setSessionQuery] = createSignal("");
  const [sessionPage, setSessionPage] = createSignal<SessionCatalogPage>();
  const [sessionCursor, setSessionCursor] = createSignal<SessionCatalogCursor | null>(null);
  const [sessionBackCursors, setSessionBackCursors] = createSignal<
    Array<SessionCatalogCursor | null>
  >([]);
  const [sessionError, setSessionError] = createSignal<string>();
  const [sessionPagingNeedsRestart, setSessionPagingNeedsRestart] = createSignal(false);
  const [loadingSessions, setLoadingSessions] = createSignal(false);
  const [resumingPath, setResumingPath] = createSignal<string>();
  let catalogProjectPath = "";
  let catalogRequestSequence = 0;

  createEffect(() => {
    const path = props.projectPath.trim();
    if (path === catalogProjectPath) return;
    catalogProjectPath = path;
    catalogRequestSequence += 1;
    setLoadingSessions(false);
    setSessionPage(undefined);
    setSessionCursor(null);
    setSessionBackCursors([]);
    setSessionError(undefined);
    setSessionPagingNeedsRestart(false);
  });

  const loadSessionPage = async (
    cursor: SessionCatalogCursor | null,
    backCursors: Array<SessionCatalogCursor | null>,
  ) => {
    const path = props.projectPath.trim();
    if (!path || loadingSessions()) return;
    const requestSequence = ++catalogRequestSequence;
    setLoadingSessions(true);
    setSessionError(undefined);
    if (!cursor) setSessionPagingNeedsRestart(false);
    try {
      const page = await invokeDesktop<SessionCatalogPage>("runtime_list_project_sessions", {
        request: {
          projectPath: path,
          query: sessionQuery().trim() || null,
          cursor,
        },
      });
      if (requestSequence !== catalogRequestSequence || props.projectPath.trim() !== path) return;
      setSessionPage(page);
      setSessionCursor(cursor);
      setSessionBackCursors(backCursors);
      setSessionPagingNeedsRestart(false);
    } catch (catalogError) {
      if (requestSequence !== catalogRequestSequence || props.projectPath.trim() !== path) return;
      setSessionError(String(catalogError));
      if (cursor) setSessionPagingNeedsRestart(true);
    } finally {
      if (requestSequence === catalogRequestSequence) setLoadingSessions(false);
    }
  };

  const findSessions = async () => loadSessionPage(null, []);

  const nextSessionPage = async () => {
    const next = sessionPage()?.nextCursor;
    if (!next || loadingSessions()) return;
    await loadSessionPage(next, [...sessionBackCursors(), sessionCursor()]);
  };

  const previousSessionPage = async () => {
    const back = sessionBackCursors();
    if (back.length === 0 || loadingSessions()) return;
    const previous = back.at(-1) ?? null;
    await loadSessionPage(previous, back.slice(0, -1));
  };

  const resume = async (session: SessionCatalogEntry) => {
    const path = props.projectPath.trim();
    if (!path || resumingPath()) return;
    const activeSessionRunId = props.activeRunIdForSessionPath(session.path);
    if (activeSessionRunId) {
      props.onOpenRun(activeSessionRunId);
      return;
    }
    const checkoutOwnerRunId = props.activeRunIdForExecutionRoot(path);
    if (checkoutOwnerRunId) {
      setSessionError(
        "This checkout is already owned by a live run. Open that run and close it before resuming another session here.",
      );
      return;
    }
    setResumingPath(session.path);
    setSessionError(undefined);
    try {
      const runId = await invokeDesktop<string>("runtime_resume_project_session", {
        request: {
          projectPath: path,
          projectTrust: props.projectTrust,
          contextFiles: props.contextFiles,
          extensionDiscovery: props.extensionDiscovery,
          sessionPath: session.path,
        },
      });
      await props.onStarted({
        runId,
        initialTaskSubmitted: false,
        initialTaskError: null,
      });
    } catch (resumeError) {
      setSessionError(String(resumeError));
    } finally {
      setResumingPath(undefined);
    }
  };

  return (
    <div class="session-browser">
      <div class="session-search">
        <input
          value={sessionQuery()}
          disabled={loadingSessions() || !props.piReady || props.projectPath.trim().length === 0}
          placeholder="Search session name, prompt, or ID"
          aria-label="Search Pi sessions"
          onInput={(event) => {
            setSessionQuery(event.currentTarget.value);
            setSessionPage(undefined);
            setSessionCursor(null);
            setSessionBackCursors([]);
            setSessionError(undefined);
            setSessionPagingNeedsRestart(false);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void findSessions();
            }
          }}
        />
        <button
          type="button"
          disabled={loadingSessions() || !props.piReady || props.projectPath.trim().length === 0}
          onClick={() => void findSessions()}
        >
          {loadingSessions() ? "Searching" : "Find sessions"}
        </button>
      </div>
      <Show when={sessionPage()}>
        {(page) => (
          <div class="session-results">
            <div class="session-results-meta">
              <span>
                {page().sessions.length} session{page().sessions.length === 1 ? "" : "s"} · {page().directorySource}
                {` · scanned ${page().scannedFiles.toLocaleString()} detailed previews from ${page().candidateFiles.toLocaleString()} session files`}
              </span>
              <Show when={page().nextCursor}>
                <span>More older candidates are available.</span>
              </Show>
            </div>
            <For each={page().sessions}>
              {(session) => {
                const activeSessionRunId = () => props.activeRunIdForSessionPath(session.path);
                const checkoutOwnerRunId = () =>
                  props.activeRunIdForExecutionRoot(props.projectPath.trim());
                const owningRunId = () => activeSessionRunId() ?? checkoutOwnerRunId();
                return (
                  <article class="session-row">
                    <div>
                      <strong>{session.name ?? session.firstMessage ?? session.id}</strong>
                      <span>
                        {new Date(session.modifiedUnixMs).toLocaleString()} · {session.id.slice(0, 12)}
                        {session.previewIncomplete ? " · bounded preview" : ""}
                      </span>
                    </div>
                    <button
                      type="button"
                      disabled={Boolean(resumingPath())}
                      onClick={() => {
                        const runId = owningRunId();
                        if (runId) props.onOpenRun(runId);
                        else void resume(session);
                      }}
                    >
                      {activeSessionRunId()
                        ? "Open"
                        : checkoutOwnerRunId()
                          ? "Open live run"
                          : resumingPath() === session.path
                            ? "Resuming"
                            : "Resume"}
                    </button>
                  </article>
                );
              }}
            </For>
            <Show when={page().sessions.length === 0}>
              <p class="launch-note">
                No matching Pi sessions were found on this bounded page.
                {page().nextCursor ? " Continue to older candidates to keep searching." : ""}
              </p>
            </Show>
            <div class="session-page-actions">
              <button
                type="button"
                disabled={loadingSessions() || sessionBackCursors().length === 0}
                onClick={() => void previousSessionPage()}
              >
                Previous
              </button>
              <span>Page {sessionBackCursors().length + 1}</span>
              <button
                type="button"
                disabled={loadingSessions() || !page().nextCursor}
                onClick={() => void nextSessionPage()}
              >
                Next older
              </button>
            </div>
          </div>
        )}
      </Show>
      <Show when={sessionError()}>
        {(message) => (
          <div class="session-lookup-error" role="alert">
            <p class="error">Session lookup failed: {message()}</p>
            <Show when={sessionPagingNeedsRestart()}>
              <button type="button" disabled={loadingSessions()} onClick={() => void findSessions()}>
                Restart from newest
              </button>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
