import { createSignal, ErrorBoundary, onCleanup, onMount, Show } from "solid-js";
import { render } from "solid-js/web";

import { App } from "./app/App";
import { waitForDesktopBackend } from "./lib/desktop";
import {
  boundRendererErrorDetail,
  parseRendererCrashCount,
  rendererRecoveryPlan,
  STABLE_RENDERER_WINDOW_MS,
} from "./rendererRecoveryPolicy";
import "./styles/app.css";
import "./features/models/models.css";
import "./features/supervision/supervision.css";

const RENDERER_CRASH_COUNT_KEY = "pi-wizard:renderer-crash-count";

function readRendererCrashCount(): number | null {
  try {
    const value = window.sessionStorage.getItem(RENDERER_CRASH_COUNT_KEY);
    return parseRendererCrashCount(value);
  } catch {
    return null;
  }
}

function writeRendererCrashCount(count: number) {
  try {
    window.sessionStorage.setItem(RENDERER_CRASH_COUNT_KEY, String(count));
  } catch {
    // Storage failure disables automatic reloads rather than risking a loop.
  }
}

function clearRendererCrashCount() {
  try {
    window.sessionStorage.removeItem(RENDERER_CRASH_COUNT_KEY);
  } catch {
    // Recovery remains available through an explicit in-memory boundary reset.
  }
}

function StableApp() {
  let stableTimer: number | undefined;
  onMount(() => {
    stableTimer = window.setTimeout(clearRendererCrashCount, STABLE_RENDERER_WINDOW_MS);
  });
  onCleanup(() => {
    if (stableTimer !== undefined) window.clearTimeout(stableTimer);
  });
  return <App />;
}

function BackendGate() {
  const [ready, setReady] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let disposed = false;
  let attempt = 0;

  const connect = () => {
    const currentAttempt = ++attempt;
    setError(undefined);
    void waitForDesktopBackend()
      .then(() => {
        if (!disposed && currentAttempt === attempt) setReady(true);
      })
      .catch((startupError) => {
        if (!disposed && currentAttempt === attempt) setError(String(startupError));
      });
  };

  onMount(connect);
  onCleanup(() => {
    disposed = true;
    attempt += 1;
  });

  return (
    <Show
      when={ready()}
      fallback={
        <main class="renderer-recovery-screen">
          <section class="renderer-recovery-card" aria-label="Desktop startup">
            <h1>{error() ? "Pi Wizard could not start" : "Starting Pi Wizard"}</h1>
            <p>
              {error()
                ? "The desktop backend did not finish initializing. No Pi run was started or restarted."
                : "Waiting for the desktop backend to finish initializing."}
            </p>
            <Show when={error()}>
              {(detail) => (
                <>
                  <pre>{boundRendererErrorDetail(detail())}</pre>
                  <div class="renderer-recovery-actions">
                    <button type="button" onClick={connect}>
                      Retry startup
                    </button>
                  </div>
                </>
              )}
            </Show>
          </section>
        </main>
      }
    >
      <StableApp />
    </Show>
  );
}

function RendererRecovery(props: { error: unknown; reset: () => void }) {
  const previousCrashes = readRendererCrashCount();
  const plan = rendererRecoveryPlan(previousCrashes);
  const detail = boundRendererErrorDetail(
    props.error instanceof Error ? props.error.stack ?? props.error.message : String(props.error),
  );

  onMount(() => {
    if (plan.nextCrashCount !== null) writeRendererCrashCount(plan.nextCrashCount);
    if (plan.automaticReload) window.location.reload();
  });

  return (
    <main class="renderer-recovery-screen">
      <section class="renderer-recovery-card" aria-label="Renderer recovery">
        <h1>{plan.automaticReload ? "Recovering Pi Wizard" : "Pi Wizard renderer failed"}</h1>
        <p>
          {plan.automaticReload
            ? "The renderer failed and is being reloaded automatically. Backend-owned Pi runs remain supervised."
            : "Automatic renderer recovery stopped to prevent a reload loop. Backend-owned Pi runs remain supervised while this screen is open."}
        </p>
        <pre>{detail}</pre>
        <div class="renderer-recovery-actions">
          <button
            type="button"
            onClick={() => {
              clearRendererCrashCount();
              props.reset();
            }}
          >
            Try renderer again
          </button>
          <button
            type="button"
            onClick={() => {
              clearRendererCrashCount();
              window.location.reload();
            }}
          >
            Reload window
          </button>
        </div>
      </section>
    </main>
  );
}

const root = document.getElementById("root");

if (root === null) {
  throw new Error("Pi Wizard root element is missing");
}

render(
  () => (
    <ErrorBoundary fallback={(error, reset) => <RendererRecovery error={error} reset={reset} />}>
      <BackendGate />
    </ErrorBoundary>
  ),
  root,
);
