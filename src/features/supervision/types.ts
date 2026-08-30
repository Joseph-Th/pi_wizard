import type { ThinkingLevel } from "../models/types";

export type SupervisionStatus = "starting" | "running" | "completed" | "stopped" | "failed";

export interface SupervisionSnapshot {
  id: string;
  projectIds: string[];
  hostProjectId: string;
  supervisorRunId: string | null;
  provider: string | null;
  model: string | null;
  thinking: ThinkingLevel | null;
  cycles: number;
  watchedRuns: number;
  lastDecision: string | null;
  status: SupervisionStatus;
  error: string | null;
}
