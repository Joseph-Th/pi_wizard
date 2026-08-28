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

    version = cargo["workspace"]["package"]["version"]
    require(package["version"] == version, "package.json version must match Cargo workspace")
    require(tauri["version"] == version, "Tauri version must match Cargo workspace")

    build = tauri["build"]
    require(build["frontendDist"] == "../dist", "production frontend must be static dist output")
    require(
        build["devUrl"].startswith("http://127.0.0.1:"),
        "development URL must remain loopback-only",
    )

    security = tauri["app"]["security"]["csp"]
    require(security.get("default-src") == "'self'", "CSP default-src must be self-only")
    require(security.get("object-src") == "'none'", "CSP object-src must be disabled")
    require(security.get("frame-ancestors") == "'none'", "CSP framing must be disabled")

    bundle = tauri["bundle"]
    require(bundle.get("active") is True, "Tauri bundling must be enabled")
    require(bundle.get("targets") == "all", "all native bundle targets must be configured")
    icons = bundle.get("icon", [])
    require(bool(icons), "at least one bundle icon is required")
    for relative in icons:
        require((ROOT / "src-tauri" / relative).is_file(), f"bundle icon is missing: {relative}")

    identifier = tauri.get("identifier", "")
    require(bool(identifier) and "." in identifier, "desktop application identifier is required")

    scripts = package["scripts"]
    require("desktop:build" in scripts, "release desktop build command is required")
    require("desktop:bundle" in scripts, "installer bundle command is required")
    require(
        "@tauri-apps/cli" in package.get("devDependencies", {}),
        "Tauri CLI must be repository-pinned as a development dependency",
    )

    print("release configuration checks passed")


if __name__ == "__main__":
    main()
