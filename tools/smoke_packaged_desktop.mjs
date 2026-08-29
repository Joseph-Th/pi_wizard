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

function requireContract(condition, detail) {
  if (!condition) throw new Error(`packaged desktop smoke failed: ${detail}`);
}

function removeTree(path) {
  rmSync(path, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 });
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
  // npm exposes both an extensionless `pi` shim and `pi.cmd` on Windows.
  // The extensionless file is deliberately not a Win32 executable: this
  // packaged smoke must prove Pi Wizard resolves the package to node.exe.
  writeFileSync(resolve(npmRoot, "pi"), "#!/bin/sh\nexit 1\n");
  writeFileSync(resolve(npmRoot, "pi.cmd"), "@echo off\r\nexit /b 1\r\n");
  writeFileSync(
    cli,
    String.raw`if (process.argv.includes("--version")) {
  process.stdout.write("0.84.3\n");
  process.exit(0);
}

let buffer = "";
function respond(request, data) {
  process.stdout.write(JSON.stringify({
    id: request.id,
    type: "response",
    command: request.type,
    success: true,
    ...(data === undefined ? {} : { data })
  }) + "\n");
}
function reject(request, error) {
  process.stdout.write(JSON.stringify({
    id: request.id,
    type: "response",
    command: request.type,
    success: false,
    error
  }) + "\n");
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
        model: null,
        thinkingLevel: "medium",
        isStreaming: false,
        isCompacting: false,
        steeringMode: "all",
        followUpMode: "one-at-a-time",
        sessionFile: null,
        sessionId: "packaged-smoke",
        sessionName: null,
        autoCompactionEnabled: true,
        messageCount: 0,
        pendingMessageCount: 0
      });
    } else if (request.type === "get_available_thinking_levels") {
      respond(request, { levels: ["off", "medium", "high"] });
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
  const pathKey = Object.keys(childEnvironment).find((key) => key.toUpperCase() === "PATH") ?? "PATH";
  childEnvironment[pathKey] = `${fakeNpmPi};${childEnvironment[pathKey] ?? ""}`;
  childEnvironment.PATHEXT = ".COM;.EXE;.BAT;.CMD";
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
    removeTree(webviewData);
    throw error;
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
      client.close();
      await stopChild(child);
      removeTree(webviewData);
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
    removeTree(fakeNpmPi);
    removeTree(appRoot);
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

  async send(method, params = {}) {
    const id = this.nextId++;
    const response = new Promise((resolveResponse) => {
      this.pending.set(id, { resolve: resolveResponse });
    });
    this.socket.send(JSON.stringify({ id, method, params }));
    const result = await Promise.race([
      response,
      delay(10_000).then(() => {
        throw new Error(`CDP command timed out: ${method}`);
      }),
    ]);
    requireContract(!result.error, `CDP ${method} failed: ${JSON.stringify(result.error)}`);
    return result.result;
  }

  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    requireContract(
      !result.exceptionDetails,
      `WebView evaluation failed: ${JSON.stringify(result.exceptionDetails)}`,
    );
    return result.result?.value;
  }

  close() {
    this.socket.close();
  }
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  child.kill();
  await Promise.race([
    new Promise((resolveExit) => child.once("exit", resolveExit)),
    delay(2_000),
  ]);
  if (child.exitCode === null && child.pid) {
    spawnSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
      windowsHide: true,
      stdio: "ignore",
    });
  }
}

async function main() {
  requireContract(process.platform === "win32", "this release smoke requires Windows WebView2");
  requireContract(existsSync(executable), `release executable is missing: ${executable}`);

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
          catalog?.diagnostics?.directNpmNode === true &&
          String(catalog?.diagnostics?.invocationExecutable ?? "").toLowerCase().endsWith("node.exe")
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
    client?.close();
    await stopChild(child);
    removeTree(webviewData);
    removeTree(fakeNpmPi);
  }

  await smokeIsolatedModelPreferences();
  console.log("packaged desktop WebView smoke passed");
  console.log("verified: custom IPC, event listen/unlisten ACL, global model discovery, first-run Muse default, remembered New Run model, favorites-first persistence, project preset routing without sidebar manager, main navigation, no visible ACL/CSP/runtime-update failure");
}

await main();
