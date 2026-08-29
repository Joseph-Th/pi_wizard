import { invoke } from "@tauri-apps/api/core";

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
