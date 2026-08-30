export type AutomationExecutionStatus =
  | "starting"
  | "running"
  | "completed"
  | "completed_with_errors"
  | "cancelled"
  | "failed";

export type AutomationStepStatus =
  | "queued"
  | "starting"
  | "working"
  | "needs_attention"
  | "completed"
  | "failed"
  | "cancelled";

export interface AutomationChain {
  id: string;
  name: string;
  prompts: string[];
}

export interface AutomationStepSnapshot {
  index: number;
  promptPreview: string;
  promptTruncated: boolean;
  runId: string | null;
  status: AutomationStepStatus;
  error: string | null;
}

export interface AutomationExecutionSnapshot {
  id: string;
  chainId: string;
  chainName: string;
  projectId: string;
  error: string | null;
  status: AutomationExecutionStatus;
  steps: AutomationStepSnapshot[];
}

export interface DesktopAutomationSnapshot {
  catalog: {
    chains: AutomationChain[];
    recoveryNotice: string | null;
  };
  executions: AutomationExecutionSnapshot[];
}

export type AutomationChangedSignal = "catalog" | "executions";

export interface RuntimeCapacitySnapshot {
  activeRuns: number;
  liveRunLimit: number;
  configuredMaxLiveRuns: number;
  preferenceRecoveryNotice: string | null;
}
