export type ExtensionDialogKind =
  | { kind: "select"; title: string; options: string[] }
  | { kind: "confirm"; title: string; message: string }
  | { kind: "input"; title: string; placeholder: string | null }
  | { kind: "editor"; title: string; prefill: string | null };

export interface PendingExtensionDialog {
  request: {
    id: string;
    timeoutMs: number | null;
    kind: ExtensionDialogKind;
  };
  remainingTimeoutMs: number | null;
}

export type ExtensionDialogResponse =
  | { kind: "value"; id: string; value: string }
  | { kind: "confirmation"; id: string; confirmed: boolean }
  | { kind: "cancelled"; id: string };

export interface AttentionRun {
  run: {
    id: string;
    projectId: string;
    executionRoot: string;
    executionIsolation: "local_checkout" | "git_worktree";
    session: {
      sessionName: string | null;
      sessionId: string | null;
    };
  };
  rpc?: {
    pendingDialogs: PendingExtensionDialog[];
  } | null;
}
