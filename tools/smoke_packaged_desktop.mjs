import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:net";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const executable = resolve(root, "target", "release", "pi-wizard-desktop.exe");

function requireContract(condition, detail) {
  if (!condition) throw new Error(`packaged desktop smoke failed: ${detail}`);
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
  const port = await freeLoopbackPort();
  const existingBrowserArgs = process.env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS?.trim();
  const browserArgs = `--remote-debugging-port=${port} --remote-allow-origins=*`;
  const childEnvironment = { ...process.env };
  const pathKey = Object.keys(childEnvironment).find((key) => key.toUpperCase() === "PATH") ?? "PATH";
  childEnvironment[pathKey] = `${fakeNpmPi};${childEnvironment[pathKey] ?? ""}`;
  childEnvironment.PATHEXT = ".COM;.EXE;.BAT;.CMD";
  const child = spawn(executable, [], {
    env: {
      ...childEnvironment,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: existingBrowserArgs
        ? `${existingBrowserArgs} ${browserArgs}`
        : browserArgs,
    },
    stdio: "ignore",
    windowsHide: true,
  });

  let client;
  try {
    const page = await waitForCdp(port, child);
    client = new CdpClient(page.webSocketDebuggerUrl);
    await client.open();
    await delay(1_000);

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
          catalog?.models?.length === 3 &&
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
      const newRun = [...document.querySelectorAll("button")].find(
        (candidate) => candidate.textContent.trim() === "New run"
      );
      if (!newRun) return { error: "New run navigation missing" };
      newRun.click();
      const deadline = Date.now() + 8_000;
      let refresh;
      while (Date.now() < deadline) {
        refresh = [...document.querySelectorAll("button")].find((candidate) =>
          candidate.textContent.includes("Refresh models") ||
          candidate.textContent.includes("Reading Pi models")
        );
        const text = document.body.innerText;
        if (refresh && text.includes("3 models available from Pi without project context")) break;
        await new Promise((resolveDelay) => window.setTimeout(resolveDelay, 100));
      }
      const text = document.body.innerText;
      const modelSelect = [...document.querySelectorAll("select")].find((candidate) =>
        [...candidate.options].some((option) => option.textContent.includes("Alpha"))
      );
      return {
        refreshFound: Boolean(refresh),
        refreshDisabled: refresh?.disabled ?? null,
        globalModelsVisible: text.includes("3 models available from Pi without project context"),
        diagnosticsVisible: text.includes("Model diagnostics"),
        selectableModels: modelSelect?.options.length ?? 0,
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
    requireContract(modelPicker.selectableModels >= 4, "Pi model choices are not selectable in the packaged renderer");
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

    console.log("packaged desktop WebView smoke passed");
    console.log("verified: custom IPC, event listen/unlisten ACL, global model discovery with enabled refresh, main navigation, no visible ACL/CSP/runtime-update failure");
  } finally {
    client?.close();
    await stopChild(child);
    rmSync(fakeNpmPi, { recursive: true, force: true });
  }
}

await main();
