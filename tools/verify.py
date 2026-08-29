from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def tool(name: str) -> str:
    resolved = shutil.which(name)
    if resolved is None:
        raise SystemExit(f"required verification tool is unavailable: {name}")
    return resolved


def run(argv: list[str]) -> None:
    print("+", " ".join(argv), flush=True)
    completed = subprocess.run(argv, cwd=ROOT, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def quick() -> None:
    run([tool("cargo"), "fmt", "--all", "--", "--check"])
    run(
        [
            tool("cargo"),
            "test",
            "-p",
            "pi-wizard-core",
            "--locked",
            "--",
            "--test-threads=4",
        ]
    )
    run([sys.executable, "-B", str(ROOT / "tools" / "test_smoke_live_pi.py")])
    run([sys.executable, "-B", str(ROOT / "tools" / "test_tauri_surface_contract.py")])
    run([tool("npm"), "run", "test:renderer-recovery"])
    run([tool("npm"), "run", "test:accessibility"])
    run([tool("npm"), "run", "check"])


def standard() -> None:
    quick()
    run([tool("cargo"), "test", "-p", "pi-wizard-desktop", "--locked"])
    run(
        [
            tool("cargo"),
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ]
    )
    run([tool("npm"), "run", "build"])


def full() -> None:
    standard()
    run(
        [
            tool("cargo"),
            "test",
            "-p",
            "pi-wizard-core",
            "--locked",
            "--",
            "--ignored",
            "--test-threads=1",
        ]
    )
    run(
        [
            tool("cargo"),
            "test",
            "-p",
            "pi-wizard-desktop",
            "--locked",
            "--",
            "--ignored",
            "--test-threads=1",
        ]
    )
    run([sys.executable, str(ROOT / "tools" / "release_check.py")])
    run([tool("npm"), "run", "desktop:build"])
    run([sys.executable, str(ROOT / "tools" / "check_windows_gui_subsystem.py")])
    run([tool("node"), str(ROOT / "tools" / "smoke_packaged_desktop.mjs")])


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in {"quick", "standard", "full"}:
        raise SystemExit("usage: python tools/verify.py <quick|standard|full>")
    if sys.argv[1] == "quick":
        quick()
    elif sys.argv[1] == "standard":
        standard()
    else:
        full()


if __name__ == "__main__":
    main()
