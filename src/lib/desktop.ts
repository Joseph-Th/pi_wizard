import { invoke } from "@tauri-apps/api/core";

const STARTUP_BACKEND_RETRY_DELAYS_MS = [0, 50, 100, 200, 400, 800] as const;

export async function retryStartupOperation<T>(operation: () => Promise<T>): Promise<T> {
  let lastError: unknown;
  for (const delayMs of STARTUP_BACKEND_RETRY_DELAYS_MS) {
    if (delayMs > 0) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, delayMs));
    }
    try {
      return await operation();
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

export async function invokeDesktop<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>(command, args);
}

export function invokeDesktopAtStartup<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return retryStartupOperation(() => invokeDesktop<T>(command, args));
}

export async function pickDirectory(defaultPath?: string): Promise<string | undefined> {
  const selected = await invokeDesktop<string | null>("runtime_pick_directory", {
    request: { defaultPath: defaultPath?.trim() || null },
  });
  return selected ?? undefined;
}
