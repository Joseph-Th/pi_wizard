import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:net";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const executable = resolve(root, "target", "release", "pi-wizard-desktop.exe");

function requireContract(condition, detail) {
  if (!condition) throw new Error(`packaged desktop smoke failed: ${detail}`);
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

  const port = await freeLoopbackPort();
  const existingBrowserArgs = process.env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS?.trim();
  const browserArgs = `--remote-debugging-port=${port} --remote-allow-origins=*`;
  const child = spawn(executable, [], {
    env: {
      ...process.env,
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
    console.log("verified: custom IPC, event listen/unlisten ACL, main navigation, no visible ACL/CSP/runtime-update failure");
  } finally {
    client?.close();
    await stopChild(child);
  }
}

await main();
