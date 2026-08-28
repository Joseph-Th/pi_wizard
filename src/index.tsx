import { ErrorBoundary, onCleanup, onMount } from "solid-js";
import { render } from "solid-js/web";

import { App } from "./App";
import {
  boundRendererErrorDetail,
  parseRendererCrashCount,
  rendererRecoveryPlan,
  STABLE_RENDERER_WINDOW_MS,
} from "./rendererRecoveryPolicy";
import "./styles.css";

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
      <StableApp />
    </ErrorBoundary>
  ),
  root,
);
