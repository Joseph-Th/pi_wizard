from __future__ import annotations

import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ALLOWED_RENDERER_TAURI_IMPORTS = {
    "@tauri-apps/api/core",
    "@tauri-apps/api/event",
}


def require(condition: bool, detail: str) -> None:
    if not condition:
        raise SystemExit(f"Tauri surface contract failed: {detail}")


def renderer_source() -> str:
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "src").rglob("*"))
        if path.suffix in {".ts", ".tsx"}
    )


def main() -> None:
    renderer = renderer_source()
    host = (ROOT / "src-tauri" / "src" / "app" / "mod.rs").read_text(encoding="utf-8")
    config = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    capability = json.loads(
        (ROOT / "src-tauri" / "capabilities" / "main.json").read_text(encoding="utf-8")
    )

    tauri_imports = set(
        re.findall(r'''from\s+["'](@tauri-apps/[^"']+)["']''', renderer)
    )
    unexpected_imports = sorted(tauri_imports - ALLOWED_RENDERER_TAURI_IMPORTS)
    require(
        not unexpected_imports,
        f"new renderer Tauri APIs require an explicit ACL/release review: {unexpected_imports}",
    )
    require(
        "@tauri-apps/api/core" in tauri_imports,
        "renderer custom-command IPC must remain visible to this audit",
    )
    require(
        "@tauri-apps/api/event" in tauri_imports,
        "renderer event IPC must remain visible to this audit",
    )

    handler_match = re.search(
        r"\.invoke_handler\(tauri::generate_handler!\[(.*?)\]\)",
        host,
        re.DOTALL,
    )
    require(handler_match is not None, "Tauri generate_handler list is missing")
    handlers = {
        entry.strip().split("::")[-1]
        for entry in handler_match.group(1).split(",")
        if entry.strip()
    }
    renderer_commands = set(re.findall(r'''["'](runtime_[a-z0-9_]+)["']''', renderer))
    missing_handlers = sorted(renderer_commands - handlers)
    require(
        not missing_handlers,
        f"renderer references commands not registered by the host: {missing_handlers}",
    )

    windows = config["app"].get("windows", [])
    require(
        len(windows) == 1 and windows[0].get("label") == "main",
        "renderer capability contract requires the explicit main window label",
    )
    require(
        capability.get("windows") == ["main"],
        "main capability must target the explicit main window",
    )
    permissions = set(capability.get("permissions", []))
    require(
        {"core:event:allow-listen", "core:event:allow-unlisten"} <= permissions,
        "renderer event imports require listen and unlisten capability permissions",
    )

    print(
        "Tauri surface contract passed: "
        f"{len(renderer_commands)} renderer runtime commands, "
        f"{len(handlers)} registered handlers, imports={sorted(tauri_imports)}"
    )


if __name__ == "__main__":
    main()
