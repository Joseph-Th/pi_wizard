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
    run([tool("cargo"), "test", "-p", "pi-wizard-core", "--locked"])
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


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in {"quick", "standard"}:
        raise SystemExit("usage: python tools/verify.py <quick|standard>")
    if sys.argv[1] == "quick":
        quick()
    else:
        standard()


if __name__ == "__main__":
    main()
