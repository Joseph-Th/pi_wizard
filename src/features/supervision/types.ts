import type { ThinkingLevel } from "../models/types";

export type SupervisionStatus = "starting" | "running" | "completed" | "stopped" | "failed";

export interface SupervisionSnapshot {
  id: string;
  projectId: string;
  supervisorRunId: string | null;
  provider: string | null;
  model: string | null;
  thinking: ThinkingLevel | null;
  cycles: number;
  maxCycles: number;
  watchedRuns: number;
  status: SupervisionStatus;
  error: string | null;
}
