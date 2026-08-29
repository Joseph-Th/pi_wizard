import { createEffect, createSignal, For, Show } from "solid-js";

import { invokeDesktop } from "../../lib/desktop";
import type { ExtensionDialogResponse, PendingExtensionDialog } from "./types";

function dialogTimeoutLabel(dialog: PendingExtensionDialog): string {
  const remaining = dialog.remainingTimeoutMs;
  if (remaining === null) return "No Pi-side timeout";
  if (remaining < 1_000) return "Timed request · <1s remaining at last sync";
  if (remaining < 60_000) return `Timed request · ~${Math.ceil(remaining / 1_000)}s remaining at last sync`;
  return `Timed request · ~${Math.ceil(remaining / 60_000)}m remaining at last sync`;
}

export function ExtensionDialogCard(props: {
  runId: string;
  dialog: PendingExtensionDialog;
  onResolved: () => Promise<unknown>;
}) {
  const [value, setValue] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let currentRequestId: string | undefined;

  createEffect(() => {
    const request = props.dialog.request;
    if (request.id === currentRequestId) return;
    currentRequestId = request.id;
    setValue(request.kind.kind === "editor" ? (request.kind.prefill ?? "") : "");
    setSubmitting(false);
    setError(undefined);
  });

  const respond = async (response: ExtensionDialogResponse) => {
    if (submitting()) return;
    setSubmitting(true);
    setError(undefined);
    try {
      await invokeDesktop<void>("runtime_respond_extension_ui", {
        request: { runId: props.runId, response },
      });
      await props.onResolved();
    } catch (responseError) {
      setSubmitting(false);
      setError(String(responseError));
      await props.onResolved();
    }
  };

  const request = () => props.dialog.request;
  const controls = () => {
    const kind = request().kind;
    switch (kind.kind) {
      case "select":
        return (
          <div class="dialog-options">
            <For each={kind.options}>
              {(option) => (
                <button
                  type="button"
                  disabled={submitting()}
                  onClick={() => void respond({ kind: "value", id: request().id, value: option })}
                >
                  {option}
                </button>
              )}
            </For>
          </div>
        );
      case "confirm":
        return (
          <>
            <p>{kind.message}</p>
            <div class="dialog-actions">
              <button
                type="button"
                disabled={submitting()}
                onClick={() =>
                  void respond({ kind: "confirmation", id: request().id, confirmed: true })
                }
              >
                Confirm
              </button>
              <button
                type="button"
                disabled={submitting()}
                onClick={() =>
                  void respond({ kind: "confirmation", id: request().id, confirmed: false })
                }
              >
                Decline
              </button>
            </div>
          </>
        );
      case "input":
        return (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void respond({ kind: "value", id: request().id, value: value() });
            }}
          >
            <input
              value={value()}
              placeholder={kind.placeholder ?? ""}
              aria-labelledby={`dialog-${request().id}`}
              disabled={submitting()}
              onInput={(event) => setValue(event.currentTarget.value)}
            />
            <button type="submit" disabled={submitting()}>Submit</button>
          </form>
        );
      case "editor":
        return (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void respond({ kind: "value", id: request().id, value: value() });
            }}
          >
            <textarea
              value={value()}
              aria-labelledby={`dialog-${request().id}`}
              disabled={submitting()}
              onInput={(event) => setValue(event.currentTarget.value)}
            />
            <button type="submit" disabled={submitting()}>Submit</button>
          </form>
        );
    }
  };

  return (
    <article class="dialog-card" aria-labelledby={`dialog-${request().id}`}>
      <header>
        <strong id={`dialog-${request().id}`}>{request().kind.title}</strong>
        <span>{dialogTimeoutLabel(props.dialog)}</span>
      </header>
      {controls()}
      <div class="dialog-actions">
        <button
          type="button"
          disabled={submitting()}
          onClick={() => void respond({ kind: "cancelled", id: request().id })}
        >
          Cancel
        </button>
      </div>
      <Show when={error()}>{(message) => <p class="error">{message()}</p>}</Show>
    </article>
  );
}
