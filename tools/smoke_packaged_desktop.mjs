import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:net";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const configuredExecutable = process.env.PI_WIZARD_SMOKE_EXECUTABLE?.trim();
const executable = configuredExecutable
  ? resolve(configuredExecutable)
  : resolve(root, "target", "release", "pi-wizard-desktop.exe");
const deferredWebViewCleanup = new Set();

function requireContract(condition, detail) {
  if (!condition) throw new Error(`packaged desktop smoke failed: ${detail}`);
}

async function removeTree(path, deferTransientLock = false) {
  // WebView2 can hold its disposable profile briefly while CDP and browser
  // processes unwind. A synchronous retry loop blocks Node's event loop and
  // can therefore prevent that shutdown from completing. Retry only the
  // transient Windows lock errors while yielding between attempts; all other
  // filesystem errors still fail immediately.
  const deadline = Date.now() + (deferTransientLock ? 3_000 : 15_000);
  while (true) {
    try {
      rmSync(path, { recursive: true, force: true });
      return;
    } catch (error) {
      if (!["EPERM", "EBUSY", "EACCES", "ENOTEMPTY"].includes(error?.code)) throw error;
      const nativeRemoval = spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-NonInteractive",
          "-Command",
          "Remove-Item -LiteralPath $env:PI_WIZARD_REMOVE_TREE -Recurse -Force -ErrorAction Stop",
        ],
        {
          env: { ...process.env, PI_WIZARD_REMOVE_TREE: path },
          windowsHide: true,
          stdio: "ignore",
        },
      );
      if (nativeRemoval.status === 0) return;
      if (Date.now() >= deadline) {
        if (deferTransientLock) {
          deferredWebViewCleanup.add(path);
          return;
        }
        throw error;
      }
      await delay(125);
    }
  }
}

function scheduleDeferredWebViewCleanup() {
  if (deferredWebViewCleanup.size === 0) return;
  const helper = spawn(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "$parentId=[int]$env:PI_WIZARD_CLEANUP_PARENT; " +
        "$paths=ConvertFrom-Json $env:PI_WIZARD_CLEANUP_PATHS; " +
        "$parentDeadline=(Get-Date).AddSeconds(30); " +
        "while ((Get-Date) -lt $parentDeadline -and (Get-Process -Id $parentId -ErrorAction SilentlyContinue)) { Start-Sleep -Milliseconds 100 }; " +
        "foreach ($path in $paths) { $deadline=(Get-Date).AddSeconds(60); while ((Get-Date) -lt $deadline) { try { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop; break } catch { Start-Sleep -Milliseconds 250 } } }",
    ],
    {
      detached: true,
      env: {
        ...process.env,
        PI_WIZARD_CLEANUP_PARENT: String(process.pid),
        PI_WIZARD_CLEANUP_PATHS: JSON.stringify([...deferredWebViewCleanup]),
      },
      stdio: "ignore",
      windowsHide: true,
    },
  );
  helper.on("error", () => {});
  helper.unref();
}

function createFakeNpmPi() {
  const npmRoot = mkdtempSync(resolve(tmpdir(), "pi-wizard-packaged-pi-"));
  const cli = resolve(
    npmRoot,
    "node_modules",
    "@earendil-works",
    "pi-coding-agent",
    "dist",
    "bundle",
    "cli.js",
  );
  mkdirSync(dirname(cli), { recursive: true });
  // npm exposes both an extensionless POSIX shim and `pi.cmd` on Windows.
  // The extensionless file is deliberately unusable by Win32; the .cmd shim
  // is the public launcher contract Pi Wizard must wrap without inspecting
  // npm's internal package layout.
  writeFileSync(resolve(npmRoot, "pi"), "#!/bin/sh\nexit 1\n");
  writeFileSync(
    resolve(npmRoot, "pi.cmd"),
    "@echo off\r\nnode \"%~dp0node_modules\\@earendil-works\\pi-coding-agent\\dist\\bundle\\cli.js\" %*\r\n",
  );
  writeFileSync(
    cli,
    String.raw`if (process.argv.includes("--version")) {
  process.stdout.write("0.84.3\n");
  process.exit(0);
}

const fs = require("node:fs");
const path = require("node:path");
const sessionArg = process.argv.indexOf("--session-id");
const persistedSession = sessionArg >= 0 && Boolean(process.argv[sessionArg + 1]);
const sessionId = persistedSession ? String(process.argv[sessionArg + 1]) : "packaged-smoke";
const providerArg = process.argv.indexOf("--provider");
const modelArg = process.argv.indexOf("--model");
const selectedProvider = providerArg >= 0 ? String(process.argv[providerArg + 1] ?? "") : "";
const selectedModel = modelArg >= 0 ? String(process.argv[modelArg + 1] ?? "") : "";
const sessionFile = persistedSession
  ? path.join(process.cwd(), "pi-wizard-packaged-" + sessionId + ".jsonl")
  : null;
const entries = [];
let leafId = null;
let turn = 0;
let working = false;
let activeBash = null;
const fence = String.fromCharCode(96, 96, 96);
const finalAnswer =
  "## Packaged handoff\n\n- **Persisted** answer\n\n" +
  fence + "js\nconst answer = 7;\n" + fence +
  "\n\n<script>window.__PI_WIZARD_SMOKE_SCRIPTED__=true</script>";

let buffer = "";
function emit(value) {
  process.stdout.write(JSON.stringify(value) + "\n");
}
function respond(request, data) {
  emit({
    id: request.id,
    type: "response",
    command: request.type,
    success: true,
    ...(data === undefined ? {} : { data })
  });
}
function reject(request, error) {
  emit({
    id: request.id,
    type: "response",
    command: request.type,
    success: false,
    error
  });
}
function ensureSessionFile() {
  if (!sessionFile || fs.existsSync(sessionFile)) return;
  fs.writeFileSync(
    sessionFile,
    JSON.stringify({
      type: "session",
      version: 3,
      id: sessionId,
      timestamp: "2026-08-29T00:00:00.000Z",
      cwd: process.cwd()
    }) + "\n"
  );
}
function appendEntry(entry) {
  ensureSessionFile();
  entries.push(entry);
  leafId = entry.id;
  if (sessionFile) fs.appendFileSync(sessionFile, JSON.stringify(entry) + "\n");
}
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  while (true) {
    const newline = buffer.indexOf("\n");
    if (newline < 0) break;
    const line = buffer.slice(0, newline).replace(/\r$/, "");
    buffer = buffer.slice(newline + 1);
    if (!line) continue;
    const request = JSON.parse(line);
    if (request.type === "get_available_models") {
      respond(request, { models: [
        { provider: "opencode-go", id: "muse-spark-1.2-contributor", name: "Muse Spark 1.2 Contributor", input: ["text", "image"] },
        { provider: "fake", id: "alpha", name: "Alpha", input: ["text"] },
        { provider: "fake", id: "vision", name: "Vision", input: ["text", "image"] },
        { provider: "other", id: "beta", name: "Beta", input: ["text"] }
      ] });
    } else if (request.type === "get_state") {
      respond(request, {
        model: selectedProvider && selectedModel
          ? {
              provider: selectedProvider,
              id: selectedModel,
              name: selectedModel === "muse-spark-1.2-contributor" ? "Muse Spark 1.2 Contributor" : selectedModel,
              input: ["text", "image"]
            }
          : null,
        thinkingLevel: "medium",
        isStreaming: working,
        isCompacting: false,
        steeringMode: "all",
        followUpMode: "one-at-a-time",
        sessionFile,
        sessionId,
        sessionName: null,
        autoCompactionEnabled: true,
        // Deliberately stale for persisted runs. The packaged transcript smoke
        // proves final-answer handoff follows session-sync revision rather than
        // depending on another get_state reconciliation.
        messageCount: 0,
        pendingMessageCount: 0
      });
    } else if (request.type === "get_entries") {
      const since = request.since == null ? null : String(request.since);
      const index = since == null ? -1 : entries.findIndex((entry) => entry.id === since);
      const start = since == null ? 0 : index >= 0 ? index + 1 : entries.length;
      respond(request, { entries: entries.slice(start), leafId });
    } else if (request.type === "get_session_stats" && persistedSession) {
      respond(request, {
        sessionFile,
        sessionId,
        userMessages: 1,
        assistantMessages: 1,
        toolCalls: 0,
        toolResults: 0,
        totalMessages: 2,
        tokens: {
          input: 50000,
          output: 10000,
          cacheRead: 40000,
          cacheWrite: 5000,
          total: 105000
        },
        cost: 0.1234,
        contextUsage: { tokens: 42000, contextWindow: 200000, percent: 21 }
      });
    } else if (request.type === "get_available_thinking_levels") {
      respond(request, { levels: ["off", "medium", "high"] });
    } else if (request.type === "get_commands") {
      respond(request, { commands: [] });
    } else if (request.type === "export_html" && persistedSession) {
      const exported = path.join(process.cwd(), "packaged-session-export.html");
      fs.writeFileSync(exported, "<!doctype html><title>Packaged session export</title>\n");
      respond(request, { path: exported });
    } else if (request.type === "bash" && persistedSession) {
      if (request.excludeFromContext !== true) {
        reject(request, "packaged smoke requires excludeFromContext=true");
        continue;
      }
      emit({
        type: "bash_execution_update",
        id: request.id,
        delta: "packaged bash stream"
      });
      const timer = setTimeout(() => {
        activeBash = null;
        respond(request, {
          output: "packaged bash result",
          exitCode: 0,
          cancelled: false,
          truncated: false,
          fullOutputPath: null
        });
      }, request.command === "reload-owned" ? 5_000 : 400);
      activeBash = { request, timer };
    } else if (request.type === "abort_bash" && persistedSession) {
      respond(request);
      if (activeBash) {
        const bash = activeBash;
        activeBash = null;
        clearTimeout(bash.timer);
        respond(bash.request, {
          output: "packaged bash cancelled",
          exitCode: 130,
          cancelled: true,
          truncated: false,
          fullOutputPath: null
        });
      }
    } else if (request.type === "prompt" && persistedSession) {
      turn += 1;
      const userId = "packaged-u" + turn;
      const assistantId = "packaged-a" + turn;
      appendEntry({
        type: "message",
        id: userId,
        parentId: leafId,
        timestamp: "2026-08-29T00:00:01.000Z",
        message: { role: "user", content: String(request.message) }
      });
      respond(request);
      working = true;
      emit({ type: "agent_start" });
      emit({ type: "message_start", message: { role: "assistant", content: [] } });
      emit({
        type: "message_update",
        assistantMessageEvent: { type: "thinking_start", contentIndex: 0 }
      });
      emit({
        type: "message_update",
        assistantMessageEvent: {
          type: "thinking_delta",
          contentIndex: 0,
          delta: "verify packaged handoff"
        }
      });
      emit({
        type: "message_update",
        assistantMessageEvent: { type: "text_start", contentIndex: 1 }
      });
      emit({
        type: "message_update",
        assistantMessageEvent: {
          type: "text_delta",
          contentIndex: 1,
          delta: "## Packaged handoff\n\nStreaming"
        }
      });
      setTimeout(() => {
        emit({
          type: "message_update",
          assistantMessageEvent: {
            type: "thinking_end",
            contentIndex: 0,
            content: "verify packaged handoff"
          }
        });
        emit({
          type: "message_update",
          assistantMessageEvent: {
            type: "text_end",
            contentIndex: 1,
            content: finalAnswer
          }
        });
        appendEntry({
          type: "message",
          id: assistantId,
          parentId: userId,
          timestamp: "2026-08-29T00:00:02.000Z",
          message: {
            role: "assistant",
            content: [
              { type: "thinking", thinking: "verify packaged handoff" },
              { type: "text", text: finalAnswer }
            ],
            model: "packaged-smoke",
            stopReason: "stop"
          }
        });
        // This persisted custom entry deliberately makes the successful
        // get_entries response exceed Pi Wizard's 512 KiB hot-page ceiling.
        // The packaged release must recover through cold JSONL resync while
        // keeping the Pi process Ready instead of converting a local bound
        // into a fatal protocol failure.
        appendEntry({
          type: "custom",
          id: "packaged-large-" + turn,
          parentId: assistantId,
          customType: "packaged-sync-overflow",
          data: { payload: "x".repeat(524500) }
        });
        emit({
          type: "message_end",
          message: {
            role: "assistant",
            content: [
              { type: "thinking", thinking: "verify packaged handoff" },
              { type: "text", text: finalAnswer }
            ]
          }
        });
        working = false;
        emit({ type: "agent_settled" });
      }, 1_500);
    } else if (request.type === "clear_queue") {
      reject(request, "Unknown command: clear_queue");
    } else {
      respond(request, {});
    }
  }
});
`,
  );
  return npmRoot;
}

async function launchDesktop(executablePath, fakeNpmPi) {
  const port = await freeLoopbackPort();
  const webviewData = mkdtempSync(resolve(tmpdir(), "pi-wizard-packaged-webview-"));
  const existingBrowserArgs = process.env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS?.trim();
  const browserArgs = `--remote-debugging-port=${port} --remote-allow-origins=*`;
  const childEnvironment = { ...process.env };
  if (fakeNpmPi) {
    const pathKey = Object.keys(childEnvironment).find((key) => key.toUpperCase() === "PATH") ?? "PATH";
    childEnvironment[pathKey] = `${fakeNpmPi};${childEnvironment[pathKey] ?? ""}`;
    childEnvironment.PATHEXT = ".COM;.EXE;.BAT;.CMD";
  }
  const child = spawn(executablePath, [], {
    env: {
      ...childEnvironment,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: existingBrowserArgs
        ? `${existingBrowserArgs} ${browserArgs}`
        : browserArgs,
      WEBVIEW2_USER_DATA_FOLDER: webviewData,
    },
    stdio: "ignore",
    windowsHide: true,
  });
  try {
    const page = await waitForCdp(port, child);
    const client = new CdpClient(page.webSocketDebuggerUrl);
    await client.open();
    await delay(1_000);
    return { child, client, webviewData };
  } catch (error) {
    await stopChild(child);
    await removeTree(webviewData, true);
    throw error;
  }
}

async function smokeRealInstalledPi() {
  const useCurrentState = process.env.PI_WIZARD_REAL_PI_USE_CURRENT_STATE === "1";
  const appRoot = mkdtempSync(resolve(tmpdir(), "pi-wizard-real-pi-app-"));
  const defaultProjectRoot = mkdtempSync(resolve(tmpdir(), "pi-wizard-real-pi-project-"));
  const projectRoot = process.env.PI_WIZARD_REAL_PI_PROJECT?.trim() || defaultProjectRoot;
  const prompt = process.env.PI_WIZARD_REAL_PI_PROMPT?.trim() || null;
  const isolatedExecutable = resolve(appRoot, "pi-wizard-desktop.exe");
  const executablePath = useCurrentState ? executable : isolatedExecutable;
  if (!useCurrentState) {
    copyFileSync(executable, isolatedExecutable);
    mkdirSync(resolve(appRoot, "pi-wizard-data"), { recursive: true });
  }
  writeFileSync(resolve(defaultProjectRoot, "seed.txt"), "real Pi packaged smoke\n");

  const { child, client, webviewData } = await launchDesktop(executablePath, null);
  try {
    const result = await client.evaluate(String.raw`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const projectRoot = ${JSON.stringify(projectRoot)};
      const probe = await invoke("probe_pi_environment", {});
      const started = await invoke("runtime_start_project", {
        request: {
          projectPath: projectRoot,
          projectTrust: "inherit",
          contextFiles: "inherit",
          extensionDiscovery: "inherit",
          provider: "opencode-go",
          model: "muse-spark-1.2-contributor",
          thinking: null,
          initialTask: null
        }
      });
      const deadline = Date.now() + 12_000;
      let run;
      while (Date.now() < deadline) {
        const hydration = await invoke("runtime_hydrate", {});
        run = hydration?.runs?.find((candidate) => candidate.run?.id === started.runId);
        if (run && run.run?.process !== "starting") break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
      }
      if (run?.run?.process === "ready" && ${JSON.stringify(Boolean(process.env.PI_WIZARD_REAL_PI_PROMPT?.trim()))}) {
        const prompt = ${JSON.stringify(process.env.PI_WIZARD_REAL_PI_PROMPT?.trim() || "")};
        await invoke("runtime_edit_draft", { request: { runId: started.runId, text: prompt } });
        const submitted = await invoke("runtime_submit_draft", {
          request: { runId: started.runId, action: "send" }
        });
        let sawWorking = false;
        const turnDeadline = Date.now() + 45_000;
        while (Date.now() < turnDeadline) {
          const hydration = await invoke("runtime_hydrate", {});
          run = hydration?.runs?.find((candidate) => candidate.run?.id === started.runId);
          if (!run) break;
          if (run.run?.process !== "ready") break;
          if (run.run?.agentWorking === true) sawWorking = true;
          if (submitted?.accepted && sawWorking && run.run?.agentWorking === false) break;
          await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 100));
        }
        const diagnostics = await invoke("runtime_diagnostics", {});
        return { probe, started, submitted, sawWorking, run, diagnostics };
      }
      const diagnostics = await invoke("runtime_diagnostics", {});
      return { probe, started, run, diagnostics };
    })()`, 55_000);
    requireContract(Boolean(result?.probe), `real Pi environment probe failed: ${JSON.stringify(result)}`);
    requireContract(
      result?.probe?.windowsCommandWrapper === true &&
        String(result?.probe?.invocationExecutable ?? "").toLowerCase().endsWith("cmd.exe"),
      `real Pi did not resolve through the Windows command wrapper: ${JSON.stringify(result)}`,
    );
    requireContract(Boolean(result?.started?.runId), `real Pi run did not start: ${JSON.stringify(result)}`);
    requireContract(
      result?.run?.run?.process === "ready",
      `real Pi process did not reach Ready: ${JSON.stringify(result)}`,
    );
    if (prompt) {
      requireContract(result?.submitted?.accepted === true, `real Pi prompt was not accepted: ${JSON.stringify(result)}`);
      requireContract(result?.sawWorking === true, `real Pi prompt never entered active work: ${JSON.stringify(result)}`);
      requireContract(result?.run?.run?.agentWorking === false, `real Pi prompt did not settle: ${JSON.stringify(result)}`);
    }
    console.log("packaged real Pi smoke passed");
    console.log(JSON.stringify({ probe: result.probe, run: result.run?.run, diagnostics: result.diagnostics }, null, 2));
  } finally {
    await client?.close();
    await stopChild(child);
    try {
      await removeTree(webviewData, true);
    } catch (error) {
      console.warn(`real Pi smoke WebView cleanup warning: ${String(error)}`);
    }
    await removeTree(appRoot);
    await removeTree(defaultProjectRoot);
  }
}

async function smokeIsolatedModelPreferences() {
  const appRoot = mkdtempSync(resolve(tmpdir(), "pi-wizard-packaged-preferences-"));
  const isolatedExecutable = resolve(appRoot, "pi-wizard-desktop.exe");
  const fakeNpmPi = createFakeNpmPi();
  copyFileSync(executable, isolatedExecutable);
  // An existing empty portable root prevents legacy-state migration from
  // contaminating this disposable fresh-preference fixture.
  mkdirSync(resolve(appRoot, "pi-wizard-data"), { recursive: true });

  const runOnce = async (expectedSelection, mutate) => {
    const { child, client, webviewData } = await launchDesktop(isolatedExecutable, fakeNpmPi);
    try {
      const result = await client.evaluate(String.raw`(async () => {
        const invoke = window.__TAURI_INTERNALS__.invoke;
        const newRun = [...document.querySelectorAll("button")].find(
          (candidate) => candidate.textContent.trim() === "New run"
        );
        if (!newRun) return { error: "New run navigation missing" };
        newRun.click();
        const deadline = Date.now() + 8_000;
        let modelSelect;
        while (Date.now() < deadline) {
          modelSelect = [...document.querySelectorAll("select")].find((candidate) =>
            [...candidate.options].some((option) => option.textContent.includes("Muse Spark 1.2 Contributor"))
          );
          if (modelSelect && document.body.innerText.includes("4 models available from Pi without project context")) break;
          await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 100));
        }
        if (!modelSelect) return { error: "model select did not load" };
        const picker = modelSelect.closest(".model-picker");
        const modelField = modelSelect.closest(".model-picker-field");
        const favorite = picker?.querySelector(".model-favorite-toggle");
        const selectRect = modelSelect.getBoundingClientRect();
        const fieldRect = modelField?.getBoundingClientRect();
        const favoriteRect = favorite?.getBoundingClientRect();
        const favoriteOverlapsSelect = Boolean(
          favoriteRect &&
          selectRect.left < favoriteRect.right &&
          selectRect.right > favoriteRect.left &&
          selectRect.top < favoriteRect.bottom &&
          selectRect.bottom > favoriteRect.top
        );
        const modelControlUsable = Boolean(
          fieldRect &&
          selectRect.width >= 160 &&
          selectRect.width >= fieldRect.width * 0.9 &&
          !favoriteOverlapsSelect
        );
        const initialSelection = modelSelect.value;
        if (${JSON.stringify(mutate)}) {
          const next = JSON.stringify(["fake", "vision"]);
          modelSelect.value = next;
          modelSelect.dispatchEvent(new Event("change", { bubbles: true }));
          if (!favorite) return { error: "favorite control missing", initialSelection };
          const favoriteDeadline = Date.now() + 5_000;
          while (favorite.disabled && Date.now() < favoriteDeadline) {
            await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
          }
          favorite.click();
          const persistDeadline = Date.now() + 5_000;
          let persisted;
          while (Date.now() < persistDeadline) {
            persisted = await invoke("runtime_model_preferences", {});
            const remembered =
              persisted?.newRunModel?.provider === "fake" &&
              persisted?.newRunModel?.model === "vision";
            const favorited = persisted?.favoriteModels?.some(
              (model) => model.provider === "fake" && model.model === "vision"
            );
            if (remembered && favorited) break;
            await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
          }
          const favoriteGroup = modelSelect.querySelector('optgroup[label="Favorites"]');
          const favoriteFirst = favoriteGroup?.querySelector("option")?.value ?? null;
          return {
            initialSelection,
            modelControlUsable,
            selectWidth: selectRect.width,
            fieldWidth: fieldRect?.width ?? null,
            favoriteOverlapsSelect,
            selectedAfterChange: modelSelect.value,
            favoritePressed: favorite.getAttribute("aria-pressed"),
            favoriteFirst,
            firstGroupLabel: modelSelect.querySelector("optgroup")?.label ?? null,
            persisted
          };
        }
        const favoriteGroup = modelSelect.querySelector('optgroup[label="Favorites"]');
        return {
          initialSelection,
          modelControlUsable,
          selectWidth: selectRect.width,
          fieldWidth: fieldRect?.width ?? null,
          favoriteOverlapsSelect,
          favoriteFirst: favoriteGroup?.querySelector("option")?.value ?? null,
          firstGroupLabel: modelSelect.querySelector("optgroup")?.label ?? null,
          preferences: await invoke("runtime_model_preferences", {})
        };
      })()`);
      requireContract(!result.error, result.error ?? "isolated model preference smoke failed");
      requireContract(
        result.initialSelection === expectedSelection,
        `unexpected New Run model selection: ${JSON.stringify(result)}`,
      );
    requireContract(
      result.modelControlUsable === true,
      `New Run model selector is squeezed or overlapped: ${JSON.stringify(result)}`,
    );
      return result;
    } finally {
      await client.close();
      await stopChild(child);
      await removeTree(webviewData, true);
    }
  };

  try {
    const museKey = JSON.stringify(["opencode-go", "muse-spark-1.2-contributor"]);
    const visionKey = JSON.stringify(["fake", "vision"]);
    const first = await runOnce(museKey, true);
    requireContract(
      first.selectedAfterChange === visionKey,
      `New Run model did not change to the selected model: ${JSON.stringify(first)}`,
    );
    requireContract(first.favoritePressed === "true", "favorite control did not become pressed");
    requireContract(first.firstGroupLabel === "Favorites", "Favorites group is not first in the selector");
    requireContract(first.favoriteFirst === visionKey, "favorited model was not grouped first");
    const second = await runOnce(visionKey, false);
    requireContract(second.firstGroupLabel === "Favorites", "Favorites group order did not survive desktop restart");
    requireContract(second.favoriteFirst === visionKey, "favorite ordering did not survive desktop restart");
    requireContract(
      second.preferences?.newRunModel?.provider === "fake" &&
        second.preferences?.newRunModel?.model === "vision",
      "last New Run model did not survive desktop restart",
    );
  } finally {
    await removeTree(fakeNpmPi);
    await removeTree(appRoot);
  }
}

async function smokeIsolatedTranscriptHandoff() {
  const appRoot = mkdtempSync(resolve(tmpdir(), "pi-wizard-packaged-transcript-app-"));
  const projectRoot = mkdtempSync(resolve(tmpdir(), "pi-wizard-packaged-transcript-project-"));
  const isolatedExecutable = resolve(appRoot, "pi-wizard-desktop.exe");
  const fakeNpmPi = createFakeNpmPi();
  copyFileSync(executable, isolatedExecutable);
  mkdirSync(resolve(appRoot, "pi-wizard-data"), { recursive: true });
  writeFileSync(resolve(projectRoot, "seed.txt"), "packaged transcript fixture\n");

  const { child, client, webviewData } = await launchDesktop(isolatedExecutable, fakeNpmPi);
  try {
    const result = await client.evaluate(String.raw`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const projectRoot = ${JSON.stringify(projectRoot)};
      const initialTask = "Keep **these prompt markers** verbatim";
      const started = await invoke("runtime_start_project", {
        request: {
          projectPath: projectRoot,
          projectTrust: "inherit",
          contextFiles: "disabled",
          extensionDiscovery: "disabled",
          provider: "opencode-go",
          model: "muse-spark-1.2-contributor",
          thinking: null,
          initialTask: null
        }
      });
      if (!started?.runId || started.initialTaskSubmitted || started.initialTaskError) {
        return { error: "packaged empty run did not start cleanly", started };
      }

      const dashboard = [...document.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "Dashboard"
      );
      dashboard?.click();
      const cardDeadline = Date.now() + 5_000;
      let card;
      while (Date.now() < cardDeadline) {
        card = [...document.querySelectorAll(".run-card")].find((candidate) =>
          candidate.textContent.includes(projectRoot)
        );
        if (card) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
      }
      if (!card) return { error: "started packaged run never appeared on dashboard", started };
      const open = [...card.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "Open"
      );
      if (!open) return { error: "started packaged run has no Open action", started };
      open.click();

      const surfaceDeadline = Date.now() + 3_000;
      while (!document.querySelector(".active-run-surface") && Date.now() < surfaceDeadline) {
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
      }
      if (!document.querySelector(".active-run-surface")) {
        return { error: "packaged run surface did not mount", started };
      }

      const sessionReadyDeadline = Date.now() + 5_000;
      let beforeRun;
      while (Date.now() < sessionReadyDeadline) {
        const beforeHydration = await invoke("runtime_hydrate", {});
        beforeRun = beforeHydration?.runs?.find(
          (candidate) => candidate.run?.id === started.runId
        );
        if (
          beforeRun?.run?.session?.sessionId &&
          beforeRun?.run?.session?.sessionFile &&
          beforeRun?.run?.session?.messageCount === 0
        ) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
      }
      if (
        !beforeRun?.run?.session?.sessionId ||
        !beforeRun?.run?.session?.sessionFile ||
        beforeRun?.run?.session?.messageCount !== 0
      ) {
        return { error: "brand-new Pi session identity/path was not advertised before history read", started, beforeRun };
      }
      const emptyHistory = await invoke("runtime_read_session_history", {
        request: { runId: started.runId, cursor: null }
      });
      const afterHistoryHydration = await invoke("runtime_hydrate", {});
      const afterHistoryRun = afterHistoryHydration?.runs?.find(
        (candidate) => candidate.run?.id === started.runId
      );
      const emptySnapshot = {
        items: emptyHistory?.items?.length ?? null,
        sessionId: emptyHistory?.sessionId ?? null,
        advertisedSessionFile: beforeRun?.run?.session?.sessionFile ?? null,
        messageCount: beforeRun?.run?.session?.messageCount ?? null,
        sessionSyncInitialized: afterHistoryRun?.rpc?.sessionSync?.initialized ?? false,
        sessionSyncCursor: afterHistoryRun?.rpc?.sessionSync?.cursor ?? null,
        visibleHistoryError: document.body.innerText.includes("Session history failed:")
      };
      if (
        emptySnapshot.items !== 0 ||
        !emptySnapshot.advertisedSessionFile ||
        emptySnapshot.messageCount !== 0 ||
        !emptySnapshot.sessionSyncInitialized ||
        emptySnapshot.sessionSyncCursor !== null ||
        emptySnapshot.visibleHistoryError
      ) {
        return { error: "brand-new advertised session path was not treated as empty history", started, emptySnapshot };
      }

      await invoke("runtime_edit_draft", {
        request: { runId: started.runId, text: initialTask }
      });
      const submitted = await invoke("runtime_submit_draft", {
        request: { runId: started.runId, action: "send" }
      });
      if (!submitted?.accepted) {
        return { error: "packaged first prompt was not accepted", started, emptySnapshot, submitted };
      }

      let sawLiveAnswer = false;
      let sawLiveReasoning = false;
      let sawActiveStatus = false;
      const liveDeadline = Date.now() + 1_250;
      while (Date.now() < liveDeadline) {
        const live = document.querySelector(".live-timeline");
        const liveAnswer = live?.querySelector(".live-text pre")?.textContent ?? "";
        const reasoning = live?.querySelector(".live-reasoning pre")?.textContent ?? "";
        const status = live?.querySelector('[role="status"]')?.textContent ?? "";
        sawLiveAnswer ||= liveAnswer.includes("## Packaged handoff") && liveAnswer.includes("Streaming");
        sawLiveReasoning ||= reasoning.includes("verify packaged handoff");
        sawActiveStatus ||= status.includes("Model turn active");
        if (sawLiveAnswer && sawLiveReasoning && sawActiveStatus) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
      }

      const finalDeadline = Date.now() + 7_000;
      let finalSnapshot;
      while (Date.now() < finalDeadline) {
        const assistant = [...document.querySelectorAll(".history-assistant")].find((candidate) =>
          candidate.textContent.includes("Packaged handoff")
        );
        const markdown = assistant?.querySelector(".markdown-body");
        const prompt = document.querySelector(".history-user .history-prompt-text")?.textContent ?? null;
        const live = document.querySelector(".live-timeline");
        const liveAnswerPresent = Boolean(live?.querySelector(".live-text"));
        const liveReasoningPresent = Boolean(live?.querySelector(".live-reasoning"));
        const status = live?.querySelector('[role="status"]')?.textContent ?? "";
        const heading = markdown?.querySelector("h2")?.textContent ?? null;
        const strong = markdown?.querySelector("strong")?.textContent ?? null;
        const code = markdown?.querySelector("pre code");
        const codeText = code?.textContent ?? null;
        const codeHighlighted = Boolean(code?.classList.contains("hljs"));
        const rawHtmlVisible = Boolean(
          markdown?.textContent.includes("<script>window.__PI_WIZARD_SMOKE_SCRIPTED__=true</script>")
        );
        const rawHtmlExecuted = Boolean(
          markdown?.querySelector("script") || window.__PI_WIZARD_SMOKE_SCRIPTED__
        );
        const railText = document.querySelector(".app-context-rail")?.textContent ?? "";
        if (
          prompt === initialTask &&
          heading === "Packaged handoff" &&
          strong === "Persisted" &&
          codeText?.includes("const answer = 7;") &&
          codeHighlighted &&
          rawHtmlVisible &&
          !rawHtmlExecuted &&
          !liveAnswerPresent &&
          !liveReasoningPresent &&
          status.includes("Pi is idle and ready") &&
          railText.includes("105,000") &&
          railText.includes("21.0%") &&
          railText.includes("Tool calls")
        ) {
          const hydration = await invoke("runtime_hydrate", {});
          const run = hydration?.runs?.find((candidate) => candidate.run?.id === started.runId);
          finalSnapshot = {
            prompt,
            heading,
            strong,
            codeText,
            codeHighlighted,
            rawHtmlVisible,
            rawHtmlExecuted,
            liveAnswerPresent,
            liveReasoningPresent,
            status,
            process: run?.run?.process ?? null,
            messageCount: run?.run?.session?.messageCount ?? null,
            sessionSyncInitialized: run?.rpc?.sessionSync?.initialized ?? false,
            sessionSyncRevision: run?.rpc?.sessionSync?.revision ?? null,
            sessionSyncCursor: run?.rpc?.sessionSync?.cursor ?? null,
            railText,
            visibleError:
              document.body.innerText.includes("Runtime update failed:") ||
              document.body.innerText.includes("Session history failed:") ||
              document.body.innerText.includes("not allowed by CSP")
          };
          break;
        }
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
      }

      let utilitySnapshot;
      if (finalSnapshot) {
        const detailsButton = [...document.querySelectorAll(".inspector-tabs button")].find(
          (candidate) => candidate.textContent.trim() === "Run details"
        );
        detailsButton?.click();
        const detailsDeadline = Date.now() + 3_000;
        while (!document.querySelector(".run-details-inspector") && Date.now() < detailsDeadline) {
          await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
        }
        const details = document.querySelector(".run-details-inspector");
        if (!details) {
          return { error: "Run details did not open for packaged utility smoke", started, finalSnapshot };
        }

        const exportButton = [...details.querySelectorAll("button")].find(
          (candidate) => candidate.textContent.trim() === "Export session HTML"
        );
        if (!exportButton) {
          return { error: "Export session HTML action is missing", started, finalSnapshot };
        }
        exportButton.click();
        const exportDeadline = Date.now() + 3_000;
        let exportText = "";
        while (Date.now() < exportDeadline) {
          exportText = details.textContent ?? "";
          if (
            exportText.includes("Session HTML exported to") &&
            exportText.includes("packaged-session-export.html")
          ) break;
          await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
        }

        const commandForm = details.querySelector(".direct-command-control");
        const commandInput = commandForm?.querySelector("input");
        const commandButton = [...(commandForm?.querySelectorAll("button") ?? [])].find(
          (candidate) => candidate.textContent.trim() === "Run command"
        );
        if (!commandInput || !commandButton) {
          return { error: "One-shot command controls are missing", started, finalSnapshot, exportText };
        }
        commandInput.value = "git status --short";
        commandInput.dispatchEvent(new Event("input", { bubbles: true }));
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
        commandButton.click();

        let sawLiveCommand = false;
        const commandLiveDeadline = Date.now() + 1_500;
        while (Date.now() < commandLiveDeadline) {
          const output = document.querySelector(".live-command pre")?.textContent ?? "";
          sawLiveCommand ||= output.includes("packaged bash stream");
          if (sawLiveCommand) break;
          await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 20));
        }

        const commandDeadline = Date.now() + 4_000;
        let commandResult;
        while (Date.now() < commandDeadline) {
          const resultNode = details.querySelector(".direct-command-result");
          const output = resultNode?.querySelector("pre")?.textContent ?? "";
          const text = resultNode?.textContent ?? "";
          if (output.includes("packaged bash result") && text.includes("Last command · exit 0")) {
            commandResult = { output, text };
            break;
          }
          await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
        }

        utilitySnapshot = {
          exportText,
          sawLiveCommand,
          commandResult,
          userRows: document.querySelectorAll(".history-user").length,
          assistantRows: document.querySelectorAll(".history-assistant").length,
          conversationText: document.querySelector(".history-timeline")?.textContent ?? "",
          commandError: [...details.querySelectorAll(".error")]
            .map((node) => node.textContent ?? "")
            .filter(Boolean),
          visibleError:
            document.body.innerText.includes("Runtime update failed:") ||
            document.body.innerText.includes("not allowed by CSP")
        };
      }
      return {
        started,
        emptySnapshot,
        sawLiveAnswer,
        sawLiveReasoning,
        sawActiveStatus,
        finalSnapshot,
        utilitySnapshot,
        debugText: document.querySelector(".active-run-surface")?.textContent?.slice(0, 4_000) ?? ""
      };
    })()`, 20_000);

    requireContract(!result.error, result.error ?? "packaged transcript handoff failed");
    requireContract(
      result.started.initialTaskSubmitted === false &&
        result.emptySnapshot?.items === 0 &&
        Boolean(result.emptySnapshot?.advertisedSessionFile) &&
        result.emptySnapshot?.messageCount === 0 &&
        result.emptySnapshot?.sessionSyncInitialized === true &&
        result.emptySnapshot?.sessionSyncCursor === null &&
        result.emptySnapshot?.visibleHistoryError === false,
      `brand-new Pi session path did not remain a healthy empty conversation before first entry: ${JSON.stringify(result)}`,
    );
    requireContract(result.sawLiveAnswer === true, `streaming answer never appeared in Live activity: ${JSON.stringify(result)}`);
    requireContract(result.sawLiveReasoning === true, `streaming reasoning never appeared in Live activity: ${JSON.stringify(result)}`);
    requireContract(result.sawActiveStatus === true, `active model status never appeared in Live activity: ${JSON.stringify(result)}`);
    requireContract(Boolean(result.finalSnapshot), `persisted final answer never replaced the live projection: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.prompt === "Keep **these prompt markers** verbatim", `user prompt was reformatted or changed: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.heading === "Packaged handoff", `Markdown heading did not render: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.strong === "Persisted", `Markdown emphasis did not render: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.codeHighlighted === true, `fenced code was not syntax-highlighted: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.rawHtmlVisible === true && result.finalSnapshot.rawHtmlExecuted === false, `raw assistant HTML was not escaped safely: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.liveAnswerPresent === false && result.finalSnapshot.liveReasoningPresent === false, `settled answer/reasoning remained duplicated in Live activity: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.messageCount === 0, `fixture no longer proves stale get_state messageCount: ${JSON.stringify(result)}`);
    requireContract(result.finalSnapshot.process === "ready", `oversized get_entries page killed the packaged Pi process: ${JSON.stringify(result)}`);
    requireContract(
      result.finalSnapshot.sessionSyncInitialized === true &&
        result.finalSnapshot.sessionSyncRevision > 0 &&
        result.finalSnapshot.sessionSyncCursor === "packaged-large-1",
      `oversized live page did not recover through persisted session resynchronization: ${JSON.stringify(result)}`,
    );
    requireContract(
      result.finalSnapshot.railText.includes("105,000") &&
        result.finalSnapshot.railText.includes("21.0%") &&
        result.finalSnapshot.railText.includes("Tool calls"),
      `selected-run context rail did not expose Pi usage and run facts: ${JSON.stringify(result)}`,
    );
    requireContract(result.finalSnapshot.visibleError === false, `packaged transcript exposed a runtime/history/CSP error: ${JSON.stringify(result)}`);
    requireContract(Boolean(result.utilitySnapshot), `packaged run utilities were not exercised: ${JSON.stringify(result)}`);
    requireContract(
      result.utilitySnapshot.exportText.includes("Session HTML exported to") &&
        result.utilitySnapshot.exportText.includes("packaged-session-export.html"),
      `session HTML export did not complete through the packaged UI: ${JSON.stringify(result)}`,
    );
    requireContract(result.utilitySnapshot.sawLiveCommand === true, `one-shot Bash streaming never appeared in Live activity: ${JSON.stringify(result)}`);
    requireContract(
      result.utilitySnapshot.commandResult?.output.includes("packaged bash result") &&
        result.utilitySnapshot.commandResult?.text.includes("Last command · exit 0"),
      `one-shot Bash final result did not render: ${JSON.stringify(result)}`,
    );
    requireContract(
      result.utilitySnapshot.userRows === 1 && result.utilitySnapshot.assistantRows === 1,
      `one-shot Bash leaked into the model conversation instead of remaining excluded from context: ${JSON.stringify(result)}`,
    );
    requireContract(
      !result.utilitySnapshot.conversationText.includes("packaged bash stream") &&
        !result.utilitySnapshot.conversationText.includes("packaged bash result"),
      `one-shot Bash output leaked into persisted conversation rows: ${JSON.stringify(result)}`,
    );
    requireContract(
      result.utilitySnapshot.commandError.length === 0 && result.utilitySnapshot.visibleError === false,
      `packaged Pi-native utilities exposed a command/export/runtime error: ${JSON.stringify(result)}`,
    );

    const reloadStart = await client.evaluate(String.raw`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const runId = ${JSON.stringify(result.started.runId)};
      void invoke("runtime_run_bash", {
        request: { runId, command: "reload-owned" }
      }).catch((error) => { window.__PI_WIZARD_RELOAD_BASH_ERROR__ = String(error); });
      const deadline = Date.now() + 2_000;
      while (Date.now() < deadline) {
        const hydration = await invoke("runtime_hydrate", {});
        const run = hydration?.runs?.find((candidate) => candidate.run?.id === runId);
        if ((run?.rpc?.live?.directBash?.length ?? 0) > 0) {
          return { active: true, error: window.__PI_WIZARD_RELOAD_BASH_ERROR__ ?? null };
        }
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 20));
      }
      return { active: false, error: window.__PI_WIZARD_RELOAD_BASH_ERROR__ ?? null };
    })()`);
    requireContract(
      reloadStart.active === true && reloadStart.error == null,
      `could not establish active direct Bash before renderer reload: ${JSON.stringify(reloadStart)}`,
    );

    await client.send("Page.reload", { ignoreCache: true }, 5_000);
    await delay(250);
    const reloadOwnership = await client.evaluate(String.raw`(async () => {
      const runId = ${JSON.stringify(result.started.runId)};
      const deadline = Date.now() + 8_000;
      let card;
      while (Date.now() < deadline) {
        const dashboard = [...document.querySelectorAll("button")].find(
          (candidate) => candidate.textContent.trim() === "Dashboard"
        );
        if (dashboard) dashboard.click();
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
        const cards = [...document.querySelectorAll(".run-card")];
        card = cards.length === 1 ? cards[0] : undefined;
        if (card?.textContent.includes("command running")) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
      }
      if (!card) return { error: "run card did not recover after renderer reload" };
      const dashboardCommandRunning = card.textContent.includes("command running");
      const closeVisible = [...card.querySelectorAll("button")].some(
        (candidate) => candidate.textContent.trim() === "Close"
      );
      const open = [...card.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "Open"
      );
      if (!open) return { error: "reloaded active run has no Open action", dashboardCommandRunning, closeVisible };
      open.click();
      const surfaceDeadline = Date.now() + 3_000;
      while (!document.querySelector(".active-run-surface") && Date.now() < surfaceDeadline) {
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
      }
      const detailsButton = [...document.querySelectorAll(".inspector-tabs button")].find(
        (candidate) => candidate.textContent.trim() === "Run details"
      );
      detailsButton?.click();
      const detailsDeadline = Date.now() + 3_000;
      while (!document.querySelector(".run-details-inspector") && Date.now() < detailsDeadline) {
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 25));
      }
      const details = document.querySelector(".run-details-inspector");
      if (!details) return { error: "run details did not recover after renderer reload" };
      const cancel = [...details.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "Cancel command"
      );
      const commandInput = details.querySelector(".direct-command-control input");
      const send = [...document.querySelectorAll(".composer-actions button")].find(
        (candidate) => candidate.textContent.trim() === "Send"
      );
      if (!send) return { error: "reloaded composer Send action is missing" };
      const statusText = document.querySelector(".live-timeline [role=\"status\"]")?.textContent ?? "";
      const ownerText = details.textContent ?? "";
      const sessionMutatorsDisabled = [...details.querySelectorAll(".run-controls select, .run-controls button")]
        .filter((element) => !element.textContent.includes("Session usage") && !element.textContent.includes("Export session HTML"))
        .every((element) => element.disabled);
      const snapshot = {
        dashboardCommandRunning,
        closeVisible,
        cancelVisible: Boolean(cancel),
        cancelDisabled: Boolean(cancel?.disabled),
        commandInputDisabled: Boolean(commandInput?.disabled),
        sendDisabled: Boolean(send?.disabled),
        ownerTextVisible: ownerText.includes("Direct Bash owns this execution root"),
        statusShowsCommand: statusText.includes("Running a command"),
        sessionMutatorsDisabled,
        visibleError:
          document.body.innerText.includes("Runtime update failed:") ||
          document.body.innerText.includes("not allowed by CSP")
      };
      cancel?.click();
      return snapshot;
    })()`, 15_000);
    requireContract(!reloadOwnership.error, reloadOwnership.error ?? "renderer reload ownership smoke failed");
    requireContract(
      reloadOwnership.dashboardCommandRunning === true && reloadOwnership.closeVisible === false,
      `dashboard did not preserve direct-Bash ownership across renderer reload: ${JSON.stringify(reloadOwnership)}`,
    );
    requireContract(
      reloadOwnership.cancelVisible === true &&
        reloadOwnership.cancelDisabled === false &&
        reloadOwnership.commandInputDisabled === true &&
        reloadOwnership.sendDisabled === true &&
        reloadOwnership.sessionMutatorsDisabled === true,
      `reloaded run controls did not mirror authoritative direct-Bash ownership: ${JSON.stringify(reloadOwnership)}`,
    );
    requireContract(
      reloadOwnership.ownerTextVisible === true &&
        reloadOwnership.statusShowsCommand === true &&
        reloadOwnership.visibleError === false,
      `reloaded direct-Bash status/cancellation surface was not explicit and healthy: ${JSON.stringify(reloadOwnership)}`,
    );
  } finally {
    await client.close();
    await stopChild(child);
    await removeTree(webviewData, true);
    await removeTree(fakeNpmPi);
    await removeTree(projectRoot);
    await removeTree(appRoot);
  }
}

async function freeLoopbackPort() {
  const server = createServer();
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  requireContract(address && typeof address === "object", "could not reserve a loopback port");
  const port = address.port;
  await new Promise((resolveClose) => server.close(resolveClose));
  return port;
}

async function waitForCdp(port, child) {
  const deadline = Date.now() + 12_000;
  while (Date.now() < deadline) {
    requireContract(child.exitCode === null, `desktop exited during startup with code ${child.exitCode}`);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json`);
      if (response.ok) {
        const targets = await response.json();
        const page = targets.find(
          (target) => target.type === "page" && typeof target.webSocketDebuggerUrl === "string",
        );
        if (page) return page;
      }
    } catch {
      // WebView2 has not opened the debugging endpoint yet.
    }
    await delay(100);
  }
  throw new Error("packaged desktop smoke failed: WebView2 CDP endpoint did not become available");
}

class CdpClient {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      pending.resolve(message);
    });
  }

  async open() {
    if (this.socket.readyState === WebSocket.OPEN) return;
    await new Promise((resolveOpen, reject) => {
      this.socket.addEventListener("open", resolveOpen, { once: true });
      this.socket.addEventListener("error", () => reject(new Error("CDP WebSocket open failed")), {
        once: true,
      });
    });
  }

  async send(method, params = {}, timeoutMs = 10_000) {
    const id = this.nextId++;
    const response = new Promise((resolveResponse) => {
      this.pending.set(id, { resolve: resolveResponse });
    });
    this.socket.send(JSON.stringify({ id, method, params }));
    const result = await Promise.race([
      response,
      delay(timeoutMs).then(() => {
        throw new Error(`CDP command timed out: ${method}`);
      }),
    ]);
    requireContract(!result.error, `CDP ${method} failed: ${JSON.stringify(result.error)}`);
    return result.result;
  }

  async evaluate(expression, timeoutMs = 10_000) {
    const result = await this.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    }, timeoutMs);
    requireContract(
      !result.exceptionDetails,
      `WebView evaluation failed: ${JSON.stringify(result.exceptionDetails)}`,
    );
    return result.result?.value;
  }

  async close() {
    if (this.socket.readyState === WebSocket.CLOSED) return;
    const closed = new Promise((resolveClose) => {
      this.socket.addEventListener("close", resolveClose, { once: true });
    });
    this.socket.close();
    await Promise.race([closed, delay(1_000)]);
  }
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  const pid = child.pid;
  if (pid) {
    // Terminate the exact desktop tree while the parent PID is still alive.
    // Killing the parent first can let WebView2 descendants outlive it long
    // enough to retain the disposable profile and make strict cleanup flaky.
    spawnSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      windowsHide: true,
      stdio: "ignore",
    });
    await Promise.race([
      new Promise((resolveExit) => child.once("exit", resolveExit)),
      delay(5_000),
    ]);
  }
  if (child.exitCode === null) {
    child.kill();
    await Promise.race([
      new Promise((resolveExit) => child.once("exit", resolveExit)),
      delay(2_000),
    ]);
  }
  requireContract(child.exitCode !== null, `desktop process ${pid ?? "unknown"} did not terminate`);
}

async function main() {
  requireContract(process.platform === "win32", "this release smoke requires Windows WebView2");
  requireContract(existsSync(executable), `release executable is missing: ${executable}`);

  if (process.env.PI_WIZARD_SMOKE_REAL_PI === "1") {
    await smokeRealInstalledPi();
    return;
  }

  const fakeNpmPi = createFakeNpmPi();
  const {
    child,
    client: launchedClient,
    webviewData,
  } = await launchDesktop(executable, fakeNpmPi);

  let client = launchedClient;
  try {
    const ipc = await client.evaluate(String.raw`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const results = {};
      const calls = [
        ["runtime_backend_ready", {}],
        ["runtime_capacity", {}],
        ["runtime_list_projects", {}],
        ["runtime_model_catalog", {}],
        ["runtime_automation_snapshot", {}],
        ["runtime_supervision_snapshot", {}],
        ["runtime_diagnostics", {}]
      ];
      for (const [command, args] of calls) {
        try {
          await invoke(command, args);
          results[command] = "ok";
        } catch (error) {
          results[command] = "error: " + String(error);
        }
      }
      try {
        const catalog = await invoke("runtime_probe_project_models", {
          request: {
            projectPath: null,
            projectTrust: "inherit",
            contextFiles: "inherit"
          }
        });
        results["runtime_probe_project_models"] =
          catalog?.models?.length === 4 &&
          catalog?.diagnostics?.scope === "global" &&
          catalog?.diagnostics?.windowsCommandWrapper === true &&
          String(catalog?.diagnostics?.invocationExecutable ?? "").toLowerCase().endsWith("cmd.exe")
            ? "ok"
            : "unexpected catalog: " + JSON.stringify(catalog);
      } catch (error) {
        results["runtime_probe_project_models"] = "error: " + String(error);
      }
      try {
        const event = "pi-wizard-packaged-acl-smoke";
        const handler = window.__TAURI_INTERNALS__.transformCallback(() => {});
        const eventId = await invoke("plugin:event|listen", {
          event,
          target: { kind: "Any" },
          handler
        });
        results["plugin:event|listen"] = "ok";
        try {
          window.__TAURI_EVENT_PLUGIN_INTERNALS__?.unregisterListener(event, eventId);
        } catch {}
        await invoke("plugin:event|unlisten", { event, eventId });
        results["plugin:event|unlisten"] = "ok";
      } catch (error) {
        results["event-acl"] = "error: " + String(error);
      }
      return results;
    })()`);

    const failedIpc = Object.entries(ipc).filter(([, value]) => value !== "ok");
    requireContract(
      failedIpc.length === 0,
      `packaged IPC failed: ${JSON.stringify(Object.fromEntries(failedIpc))}`,
    );

    const modelPicker = await client.evaluate(String.raw`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const projects = await invoke("runtime_list_projects", {});
      const oldProjectManager = document.querySelector(".project-manager");
      const sidebar = document.querySelector(".app-sidebar");
      const projectsNavButton = [...sidebar.querySelectorAll("button")].some(
        (candidate) => candidate.textContent.trim() === "Projects"
      );
      const newRun = [...document.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "New run"
      );
      if (!newRun) return { error: "New run navigation missing" };
      newRun.click();
      const deadline = Date.now() + 8_000;
      let refresh;
      while (Date.now() < deadline) {
        refresh = [...document.querySelectorAll("button")].find((candidate) =>
          candidate.closest(".model-picker-heading") &&
          ["Refresh", "Loading…", "Checking…"].includes(candidate.textContent.trim())
        );
        const text = document.body.innerText;
        if (refresh && text.includes("4 models available from Pi without project context")) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 100));
      }
      const text = document.body.innerText;
      const modelSelect = [...document.querySelectorAll("select")].find((candidate) =>
        [...candidate.options].some((option) => option.textContent.includes("Alpha"))
      );
      const refreshDisabledBeforeProjectSelection = refresh?.disabled ?? null;
      const selectableModelsBeforeProjectSelection = modelSelect?.options.length ?? 0;
      const modelField = modelSelect?.closest(".model-picker-field");
      const favorite = modelSelect?.closest(".model-picker")?.querySelector(".model-favorite-toggle");
      const selectRect = modelSelect?.getBoundingClientRect();
      const fieldRect = modelField?.getBoundingClientRect();
      const favoriteRect = favorite?.getBoundingClientRect();
      const favoriteOverlapsSelect = Boolean(
        selectRect &&
        favoriteRect &&
        selectRect.left < favoriteRect.right &&
        selectRect.right > favoriteRect.left &&
        selectRect.top < favoriteRect.bottom &&
        selectRect.bottom > favoriteRect.top
      );
      const modelControlUsable = Boolean(
        selectRect &&
        fieldRect &&
        selectRect.width >= 160 &&
        selectRect.width >= fieldRect.width * 0.9 &&
        !favoriteOverlapsSelect
      );
      const projectSelect = document.querySelector(".project-preset-row select");
      const presentProjects = projects.filter((project) => project.status === "present");
      const projectOptionValues = projectSelect
        ? [...projectSelect.options].map((option) => option.value)
        : [];
      let routedProjectPath = null;
      if (projectSelect && presentProjects.length > 0) {
        projectSelect.value = presentProjects[0].canonicalRoot;
        projectSelect.dispatchEvent(new Event("change", { bubbles: true }));
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 150));
        routedProjectPath = document.querySelector(".project-preset-path")?.textContent.trim() ?? null;
      }
      return {
        refreshFound: Boolean(refresh),
        refreshDisabled: refreshDisabledBeforeProjectSelection,
        globalModelsVisible: text.includes("4 models available from Pi without project context"),
        diagnosticsVisible: text.includes("Model diagnostics"),
        selectableModels: selectableModelsBeforeProjectSelection,
        modelControlUsable,
        selectWidth: selectRect?.width ?? null,
        fieldWidth: fieldRect?.width ?? null,
        favoriteOverlapsSelect,
        oldProjectManager: Boolean(oldProjectManager),
        projectsNavButton,
        projectPresetSelect: Boolean(projectSelect),
        canonicalProjectOptions: presentProjects.every((project) =>
          projectOptionValues.includes(project.canonicalRoot)
        ),
        routedProjectPath,
        expectedProjectPath: presentProjects[0]?.canonicalRoot ?? null,
        manageSavedProjects:
          presentProjects.length === 0 || Boolean(document.querySelector(".project-preset-manager")),
        visibleError:
          text.includes("Pi model discovery:") ||
          text.includes("not allowed by ACL") ||
          text.includes("Runtime update failed:")
      };
    })()`);
    requireContract(!modelPicker.error, modelPicker.error ?? "model picker failed");
    requireContract(modelPicker.refreshFound === true, "Refresh models button is missing");
    requireContract(modelPicker.refreshDisabled === false, "Refresh models button is disabled without a project");
    requireContract(modelPicker.globalModelsVisible === true, "global Pi models did not load before project selection");
    requireContract(modelPicker.diagnosticsVisible === true, "model probe diagnostics are missing");
    requireContract(modelPicker.selectableModels >= 5, "Pi model choices are not selectable in the packaged renderer");
    requireContract(
      modelPicker.modelControlUsable === true,
      `model selector is squeezed or overlapped in packaged New Run: ${JSON.stringify(modelPicker)}`,
    );
    requireContract(modelPicker.oldProjectManager === false, "obsolete Projects sidebar manager is still mounted");
    requireContract(modelPicker.projectsNavButton === false, "obsolete Projects sidebar navigation is still visible");
    requireContract(modelPicker.projectPresetSelect === true, "New Run project preset dropdown is missing");
    requireContract(modelPicker.canonicalProjectOptions === true, "saved project presets do not use canonical registered paths");
    if (modelPicker.expectedProjectPath) {
      requireContract(
        modelPicker.routedProjectPath === modelPicker.expectedProjectPath,
        "selecting a saved project preset did not route New Run to its canonical directory",
      );
    }
    requireContract(modelPicker.manageSavedProjects === true, "saved project management is not reachable from New Run");
    requireContract(modelPicker.visibleError === false, "model picker shows a packaged discovery/runtime error");

    const layout = await client.evaluate(String.raw`(async () => {
      const contentWidth = (element) => {
        if (!element) return null;
        const style = getComputedStyle(element);
        return element.clientWidth - parseFloat(style.paddingLeft || "0") - parseFloat(style.paddingRight || "0");
      };
      const main = document.querySelector(".app-main");
      const launcher = document.querySelector(".project-launcher");
      const sidebar = document.querySelector(".app-sidebar");
      const shell = document.querySelector(".app-shell");
      const resizer = document.querySelector('.sidebar-resizer[role="separator"]');
      if (!main || !launcher || !sidebar || !shell || !resizer) {
        return { error: "desktop layout controls missing" };
      }
      const mainContentWidth = contentWidth(main);
      const launcherWidth = launcher.getBoundingClientRect().width;
      const beforeAria = Number(resizer.getAttribute("aria-valuenow"));
      const beforeSidebarWidth = sidebar.getBoundingClientRect().width;
      resizer.focus();
      resizer.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
      await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 80));
      const expandedAria = Number(resizer.getAttribute("aria-valuenow"));
      const expandedSidebarWidth = sidebar.getBoundingClientRect().width;
      const expandedShellClass = shell.className;
      resizer.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
      await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 80));
      const restoredAria = Number(resizer.getAttribute("aria-valuenow"));
      const restoredSidebarWidth = sidebar.getBoundingClientRect().width;

      const supervisionButton = [...document.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "Supervision"
      );
      supervisionButton?.click();
      const deadline = Date.now() + 5_000;
      let supervision;
      while (Date.now() < deadline) {
        supervision = document.querySelector(".supervision-surface");
        if (supervision) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 50));
      }
      const supervisionWidth = supervision?.getBoundingClientRect().width ?? null;
      const supervisionContentWidth = contentWidth(main);
      const text = document.body.innerText;
      return {
        mainContentWidth,
        launcherWidth,
        launcherUsesWorkspace: mainContentWidth !== null && launcherWidth >= mainContentWidth - 4,
        beforeAria,
        expandedAria,
        restoredAria,
        beforeSidebarWidth,
        expandedSidebarWidth,
        restoredSidebarWidth,
        expandedShellClass,
        resizerVisible: getComputedStyle(resizer).display !== "none",
        supervisionWidth,
        supervisionContentWidth,
        supervisionUsesWorkspace:
          supervisionWidth !== null &&
          supervisionContentWidth !== null &&
          supervisionWidth >= supervisionContentWidth - 4,
        visibleError:
          text.includes("not allowed by ACL") ||
          text.includes("Runtime update failed:") ||
          text.includes("not allowed by CSP")
      };
    })()`);
    requireContract(!layout.error, layout.error ?? "desktop layout smoke failed");
    requireContract(layout.resizerVisible === true, `sidebar resizer is hidden at release window size: ${JSON.stringify(layout)}`);
    requireContract(
      layout.expandedAria === layout.beforeAria + 16 &&
        layout.restoredAria === layout.beforeAria,
      `keyboard sidebar resize did not update its bounded accessible value: ${JSON.stringify(layout)}`,
    );
    requireContract(
      layout.expandedSidebarWidth >= layout.beforeSidebarWidth + 15 &&
        Math.abs(layout.restoredSidebarWidth - layout.beforeSidebarWidth) <= 1,
      `packaged CSP-safe sidebar width classes did not resize and restore the real sidebar: ${JSON.stringify(layout)}`,
    );
    requireContract(
      String(layout.expandedShellClass).includes(`sidebar-width-${layout.expandedAria}`),
      `expanded sidebar did not use the expected static shell class: ${JSON.stringify(layout)}`,
    );
    requireContract(layout.launcherUsesWorkspace === true, `New Run does not consume the available workspace width: ${JSON.stringify(layout)}`);
    requireContract(layout.supervisionUsesWorkspace === true, `Supervision does not consume the available workspace width: ${JSON.stringify(layout)}`);
    requireContract(layout.visibleError === false, `layout exercise exposed a packaged ACL/CSP/runtime error: ${JSON.stringify(layout)}`);

    const navigation = await client.evaluate(String.raw`(async () => {
      const labels = [
        "Dashboard",
        "Automation",
        "Supervision",
        "Needs attention",
        "Recent sessions",
        "New run"
      ];
      const results = {};
      for (const label of labels) {
        const button = [...document.querySelectorAll("button")].find((candidate) =>
          candidate.textContent.trim().startsWith(label)
        );
        if (!button) {
          results[label] = "missing";
          continue;
        }
        button.click();
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 200));
        const text = document.body.innerText;
        results[label] =
          text.includes("not allowed by ACL") ||
          text.includes("Runtime update failed:") ||
          text.includes("not allowed by CSP")
            ? "error-visible"
            : "ok";
      }
      return {
        results,
        shellLoaded: document.body.innerText.includes("Pi Wizard")
      };
    })()`);

    requireContract(navigation.shellLoaded === true, "main application shell did not render");
    const failedNavigation = Object.entries(navigation.results).filter(([, value]) => value !== "ok");
    requireContract(
      failedNavigation.length === 0,
      `packaged navigation failed: ${JSON.stringify(Object.fromEntries(failedNavigation))}`,
    );

  } finally {
    await client?.close();
    await stopChild(child);
    await removeTree(webviewData, true);
    await removeTree(fakeNpmPi);
  }

  await smokeIsolatedModelPreferences();
  await smokeIsolatedTranscriptHandoff();
  console.log("packaged desktop WebView smoke passed");
  console.log("verified: custom IPC, event listen/unlisten ACL, global model discovery, first-run Muse default, remembered New Run model, favorites-first persistence, project preset routing without sidebar manager, CSP-safe keyboard sidebar resizing, full-width New Run/Supervision surfaces, dashboard/right-rail run status, Pi session usage metrics and run facts, oversized get_entries cold-resync recovery without process failure, real packaged run streaming-to-persisted transcript handoff with stale messageCount, sanitized rich final Markdown, native session HTML export, one-shot Bash streaming/result with context exclusion, direct-Bash execution-root ownership across renderer reload with cancellation and mutation gating, main navigation, no visible ACL/CSP/runtime-update failure");
}

try {
  await main();
} finally {
  scheduleDeferredWebViewCleanup();
}
