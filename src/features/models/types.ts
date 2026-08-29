export type ThinkingLevel =
  | "off"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max";

export type ProjectTrustPolicy = "inherit" | "approve" | "ignore";
export type ContextFilesPolicy = "inherit" | "disabled";

export interface ModelSummary {
  provider: string;
  id: string;
  name: string | null;
  supportsImages: boolean | null;
}

export interface ProjectLaunchOptions {
  currentModel: ModelSummary | null;
  currentThinkingLevel: ThinkingLevel;
  models: ModelSummary[];
  thinkingLevels: ThinkingLevel[];
  clearQueueSupported: boolean;
}

export interface CustomModelProfile {
  provider: string;
  model: string;
  name: string | null;
}

export interface ModelCatalogSnapshot {
  models: CustomModelProfile[];
  recoveryNotice: string | null;
}

export interface ModelSelection {
  provider: string;
  id: string;
}

export function encodeModelSelection(selection: ModelSelection): string {
  return JSON.stringify([selection.provider, selection.id]);
}

export function decodeModelSelection(value: string): ModelSelection | undefined {
  if (!value) return undefined;
  try {
    const parsed: unknown = JSON.parse(value);
    if (
      Array.isArray(parsed) &&
      parsed.length === 2 &&
      typeof parsed[0] === "string" &&
      typeof parsed[1] === "string"
    ) {
      return { provider: parsed[0], id: parsed[1] };
    }
  } catch {
    // Values come from our own option list. Malformed state falls back to Pi default.
  }
  return undefined;
}
