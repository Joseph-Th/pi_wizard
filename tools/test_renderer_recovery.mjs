import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = path.join(root, "src", "rendererRecoveryPolicy.ts");
const tscPath = path.join(root, "node_modules", "typescript", "bin", "tsc");
const outputRoot = await mkdtemp(path.join(tmpdir(), "pi-wizard-renderer-recovery-"));

try {
  const compile = spawnSync(
    process.execPath,
    [
      tscPath,
      sourcePath,
      "--ignoreConfig",
      "--target",
      "ES2022",
      "--module",
      "ES2022",
      "--skipLibCheck",
      "--outDir",
      outputRoot,
    ],
    { cwd: root, encoding: "utf8" },
  );
  assert.equal(
    compile.status,
    0,
    `renderer recovery policy compilation failed:\n${compile.stdout}${compile.stderr}`,
  );

  const policy = await import(pathToFileURL(path.join(outputRoot, "rendererRecoveryPolicy.js")).href);

  assert.equal(policy.parseRendererCrashCount(null), 0);
  assert.equal(policy.parseRendererCrashCount("not-a-number"), 0);
  assert.equal(policy.parseRendererCrashCount("-4"), 0);
  assert.equal(policy.parseRendererCrashCount("1"), 1);

  assert.deepEqual(policy.rendererRecoveryPlan(0), {
    automaticReload: true,
    nextCrashCount: 1,
  });
  assert.deepEqual(policy.rendererRecoveryPlan(1), {
    automaticReload: true,
    nextCrashCount: 2,
  });
  assert.deepEqual(policy.rendererRecoveryPlan(2), {
    automaticReload: false,
    nextCrashCount: 3,
  });
  assert.deepEqual(policy.rendererRecoveryPlan(null), {
    automaticReload: false,
    nextCrashCount: null,
  });

  const oversizedDetail = "x".repeat(policy.MAX_RENDERER_ERROR_DETAIL_CHARS + 100);
  assert.equal(
    policy.boundRendererErrorDetail(oversizedDetail).length,
    policy.MAX_RENDERER_ERROR_DETAIL_CHARS,
  );

  console.log("renderer recovery policy tests passed");
} finally {
  await rm(outputRoot, { recursive: true, force: true });
}
