import { invoke } from "@tauri-apps/api/core";

const STARTUP_BACKEND_RETRY_DELAYS_MS = [
  0,
  25,
  50,
  100,
  200,
  400,
  800,
  1_600,
  3_200,
  5_000,
  5_000,
  5_000,
] as const;

export async function waitForDesktopBackend(): Promise<void> {
  let lastError: unknown;
  for (const delayMs of STARTUP_BACKEND_RETRY_DELAYS_MS) {
    if (delayMs > 0) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, delayMs));
    }
    try {
      const ready = await invokeDesktop<boolean>("runtime_backend_ready");
      if (ready) return;
      lastError = new Error("desktop backend reported that it is not ready");
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(String(lastError ?? "desktop backend did not become ready"));
}

export async function invokeDesktop<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>(command, args);
}

export async function pickDirectory(defaultPath?: string): Promise<string | undefined> {
  const selected = await invokeDesktop<string | null>("runtime_pick_directory", {
    request: { defaultPath: defaultPath?.trim() || null },
  });
  return selected ?? undefined;
}
