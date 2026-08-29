export type ExtensionDiscoveryPolicy = "inherit" | "disabled";

export interface StartRunResult {
  runId: string;
  initialTaskSubmitted: boolean;
  initialTaskError: string | null;
}
