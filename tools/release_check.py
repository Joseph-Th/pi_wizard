from __future__ import annotations

import json
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def require(condition: bool, detail: str) -> None:
    if not condition:
        raise SystemExit(f"release configuration check failed: {detail}")


def main() -> None:
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    tauri = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    main_capability_path = ROOT / "src-tauri" / "capabilities" / "main.json"
    require(main_capability_path.is_file(), "main-window Tauri capability is required")
    main_capability = json.loads(main_capability_path.read_text(encoding="utf-8"))
    desktop_main = (ROOT / "src-tauri" / "src" / "main.rs").read_text(encoding="utf-8")

    version = cargo["workspace"]["package"]["version"]
    require(package["version"] == version, "package.json version must match Cargo workspace")
    require(tauri["version"] == version, "Tauri version must match Cargo workspace")

    build = tauri["build"]
    require(build["frontendDist"] == "../dist", "production frontend must be static dist output")
    require(
        build["devUrl"].startswith("http://127.0.0.1:"),
        "development URL must remain loopback-only",
    )

    windows = tauri["app"].get("windows", [])
    require(
        len(windows) == 1 and windows[0].get("label") == "main",
        "the desktop window must keep the explicit 'main' label used by its Tauri capability",
    )

    security = tauri["app"]["security"]["csp"]
    require(security.get("default-src") == "'self'", "CSP default-src must be self-only")
    require(security.get("script-src") == "'self'", "production CSP script-src must be self-only")
    require(security.get("style-src") == "'self'", "production CSP style-src must be self-only")
    require("unsafe-inline" not in security.get("style-src", ""), "production CSP must forbid inline styles")
    require("unsafe-eval" not in security.get("script-src", ""), "production CSP must forbid script eval")
    require(security.get("object-src") == "'none'", "CSP object-src must be disabled")
    require(security.get("base-uri") == "'none'", "CSP base-uri must be disabled")
    require(security.get("form-action") == "'none'", "CSP form submission must be disabled")
    require(security.get("frame-ancestors") == "'none'", "CSP framing must be disabled")

    dev_security = tauri["app"]["security"].get("devCsp", {})
    require(
        "ws://127.0.0.1:1420" in dev_security.get("connect-src", ""),
        "development CSP must permit only the loopback Vite websocket needed for HMR",
    )
    require(
        "'unsafe-inline'" in dev_security.get("style-src", ""),
        "development-only CSP must contain the Vite inline-style allowance",
    )

    bundle = tauri["bundle"]
    require(bundle.get("active") is True, "Tauri bundling must be enabled")
    require(bundle.get("targets") == "all", "all native bundle targets must be configured")
    icons = bundle.get("icon", [])
    require(bool(icons), "at least one bundle icon is required")
    for relative in icons:
        require((ROOT / "src-tauri" / relative).is_file(), f"bundle icon is missing: {relative}")

    identifier = tauri.get("identifier", "")
    require(bool(identifier) and "." in identifier, "desktop application identifier is required")

    require(
        main_capability.get("windows") == ["main"],
        "main-window capability must stay scoped to the Tauri main window",
    )
    event_permissions = set(main_capability.get("permissions", []))
    require(
        "core:event:allow-listen" in event_permissions,
        "main window must be allowed to listen for backend invalidation events",
    )
    require(
        "core:event:allow-unlisten" in event_permissions,
        "main window must be allowed to unregister backend invalidation listeners",
    )

    scripts = package["scripts"]
    require("desktop:build" in scripts, "release desktop build command is required")
    require("desktop:bundle" in scripts, "installer bundle command is required")
    require(
        "@tauri-apps/cli" in package.get("devDependencies", {}),
        "Tauri CLI must be repository-pinned as a development dependency",
    )
    require(
        '#![cfg_attr(windows, windows_subsystem = "windows")]' in desktop_main,
        "Windows desktop executable must always use the GUI subsystem so app launch never opens a console window",
    )

    print("release configuration checks passed")


if __name__ == "__main__":
    main()
