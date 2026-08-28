export const MAX_AUTOMATIC_RENDER_RELOADS = 2;
export const MAX_RENDERER_ERROR_DETAIL_CHARS = 2048;
export const STABLE_RENDERER_WINDOW_MS = 10_000;

export function parseRendererCrashCount(value: string | null): number {
  if (value === null) return 0;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
}

export interface RendererRecoveryPlan {
  automaticReload: boolean;
  nextCrashCount: number | null;
}

export function rendererRecoveryPlan(previousCrashes: number | null): RendererRecoveryPlan {
  return {
    automaticReload:
      previousCrashes !== null && previousCrashes < MAX_AUTOMATIC_RENDER_RELOADS,
    nextCrashCount: previousCrashes === null ? null : previousCrashes + 1,
  };
}

export function boundRendererErrorDetail(detail: string): string {
  return detail.slice(0, MAX_RENDERER_ERROR_DETAIL_CHARS);
}
