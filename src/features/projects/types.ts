export interface ProjectResourcePreflight {
  piSettings: boolean;
  extensions: boolean;
  skills: boolean;
  prompts: boolean;
  themes: boolean;
  systemPrompt: boolean;
  appendSystemPrompt: boolean;
  ancestorAgentSkills: boolean;
}

export interface WorktreeBaseSnapshot {
  repositoryRoot: string;
  projectRoot: string;
  projectRelativePath: string;
  sourceBranch: string | null;
  baseCommit: string;
  dirty: boolean;
}

export interface CreatedWorktree {
  repositoryRoot: string;
  worktreeRoot: string;
  executionRoot: string;
  branch: string;
  baseCommit: string;
}

export interface WorktreeRecoveryRecord {
  id: string;
  projectId: string;
  base: WorktreeBaseSnapshot;
  branch: string;
  requestedPath: string;
  created: CreatedWorktree | null;
}

export interface WorktreeRecoveryPage {
  records: WorktreeRecoveryRecord[];
  truncated: boolean;
  recoveryNotice: string | null;
}

export type WorktreeRecoveryProbe =
  | { kind: "notCreated" }
  | { kind: "exact"; created: CreatedWorktree }
  | { kind: "partial"; branchExists: boolean; pathExists: boolean; detail: string };

export type WorktreeCleanupResult =
  | { kind: "removed" }
  | { kind: "partial"; branchExists: boolean; pathExists: boolean; detail: string };

export interface WorktreeRecoveryInspection {
  record: WorktreeRecoveryRecord | null;
  probe: WorktreeRecoveryProbe;
}
