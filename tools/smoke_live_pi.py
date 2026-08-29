from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO


ROOT = Path(__file__).resolve().parents[1]
MAX_CAPTURE_BYTES = 2 * 1024 * 1024
VERSION_TIMEOUT_SECONDS = 5
RPC_TIMEOUT_SECONDS = 15


@dataclass(frozen=True)
class BoundedCommandResult:
    returncode: int
    stdout: bytes
    stderr: bytes


class BoundedPipeCapture:
    def __init__(self, label: str, stream: BinaryIO) -> None:
        self.label = label
        self.stream = stream
        self.buffer = bytearray()
        self.total_bytes = 0
        self.exceeded = threading.Event()
        self.error: Exception | None = None
        self.thread = threading.Thread(target=self._drain, name=f"pi-smoke-{label}", daemon=True)

    def start(self) -> None:
        self.thread.start()

    def join(self) -> None:
        self.thread.join(timeout=2)
        if self.thread.is_alive():
            raise SystemExit(f"live Pi smoke {self.label} drain did not reach EOF")
        if self.error is not None:
            raise SystemExit(f"live Pi smoke could not read {self.label}: {self.error}")

    def _drain(self) -> None:
        try:
            while True:
                chunk = self.stream.read(16 * 1024)
                if not chunk:
                    return
                self.total_bytes += len(chunk)
                remaining = MAX_CAPTURE_BYTES - len(self.buffer)
                if remaining > 0:
                    self.buffer.extend(chunk[:remaining])
                if self.total_bytes > MAX_CAPTURE_BYTES:
                    self.exceeded.set()
        except Exception as error:  # pragma: no cover - OS pipe failures are platform-specific
            self.error = error
        finally:
            self.stream.close()


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description=(
            "Run a no-prompt compatibility smoke against an installed Pi CLI. "
            "The smoke creates no session and sends no model request."
        )
    )
    result.add_argument("--pi", help="Explicit Pi executable; defaults to PATH lookup")
    result.add_argument("--provider", help="Optional provider to verify at launch")
    result.add_argument("--model", help="Optional model id to verify at launch")
    result.add_argument(
        "--thinking",
        choices=["off", "minimal", "low", "medium", "high", "xhigh", "max"],
        help="Optional thinking level to verify at launch",
    )
    return result


def resolve_pi(value: str | None) -> str:
    if value:
        path = Path(value).expanduser()
        if not path.exists():
            raise SystemExit(f"Pi executable does not exist: {path}")
        return str(path)
    resolved = shutil.which("pi")
    if resolved is None:
        raise SystemExit("Pi executable is unavailable on PATH; pass --pi explicitly")
    return resolved


def bounded_run(argv: list[str], *, input_bytes: bytes | None, timeout: int) -> BoundedCommandResult:
    process = subprocess.Popen(
        argv,
        cwd=ROOT,
        stdin=subprocess.PIPE if input_bytes is not None else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert process.stdout is not None
    assert process.stderr is not None
    stdout = BoundedPipeCapture("stdout", process.stdout)
    stderr = BoundedPipeCapture("stderr", process.stderr)
    stdout.start()
    stderr.start()

    if input_bytes is not None:
        assert process.stdin is not None
        try:
            process.stdin.write(input_bytes)
            process.stdin.flush()
        except BrokenPipeError:
            pass
        finally:
            process.stdin.close()

    deadline = time.monotonic() + timeout
    failure: str | None = None
    while process.poll() is None:
        if stdout.exceeded.is_set() or stderr.exceeded.is_set():
            failure = "live Pi smoke output exceeded the bounded capture limit"
            process.kill()
            break
        if time.monotonic() >= deadline:
            failure = f"command timed out after {timeout}s: {argv[0]}"
            process.kill()
            break
        time.sleep(0.01)

    try:
        returncode = process.wait(timeout=2)
    except subprocess.TimeoutExpired as error:
        process.kill()
        process.wait()
        raise SystemExit(f"could not terminate timed-out command: {argv[0]}") from error
    stdout.join()
    stderr.join()
    if failure is not None:
        raise SystemExit(failure)
    for capture in (stdout, stderr):
        if capture.exceeded.is_set():
            raise SystemExit(
                f"live Pi smoke {capture.label} exceeded {MAX_CAPTURE_BYTES} bytes"
            )
    stdout_bytes = bytes(stdout.buffer)
    stderr_bytes = bytes(stderr.buffer)
    if returncode != 0:
        stderr_text = stderr_bytes.decode("utf-8", errors="replace").strip()
        detail = stderr_text[-2_000:] if stderr_text else "no stderr"
        raise SystemExit(f"Pi exited with code {returncode}: {detail}")
    return BoundedCommandResult(returncode, stdout_bytes, stderr_bytes)


def parse_responses(stdout: bytes) -> dict[str, dict[str, object]]:
    responses: dict[str, dict[str, object]] = {}
    for raw_line in stdout.splitlines():
        if not raw_line.strip():
            continue
        try:
            value = json.loads(raw_line)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise SystemExit(f"Pi emitted non-JSON stdout during RPC smoke: {error}") from error
        if not isinstance(value, dict):
            continue
        response_id = value.get("id")
        if value.get("type") == "response" and isinstance(response_id, str):
            responses[response_id] = value
    return responses


def require_response(
    responses: dict[str, dict[str, object]], response_id: str, command: str
) -> dict[str, object]:
    response = responses.get(response_id)
    if response is None:
        raise SystemExit(f"Pi did not return RPC response {response_id!r}")
    if response.get("command") != command:
        raise SystemExit(
            f"Pi response {response_id!r} reported command {response.get('command')!r}, expected {command!r}"
        )
    if response.get("success") is not True:
        raise SystemExit(f"Pi rejected {command}: {response.get('error', 'unknown error')}")
    data = response.get("data")
    if data is None:
        return {}
    if not isinstance(data, dict):
        raise SystemExit(f"Pi {command} response data is not an object")
    return data


def optional_command_supported(
    responses: dict[str, dict[str, object]], response_id: str, command: str
) -> bool:
    response = responses.get(response_id)
    if response is None:
        raise SystemExit(f"Pi did not return RPC response {response_id!r}")
    if response.get("command") != command:
        raise SystemExit(
            f"Pi response {response_id!r} reported command {response.get('command')!r}, expected {command!r}"
        )
    if response.get("success") is True:
        return True
    error = str(response.get("error", "unknown error"))
    if "unknown command" in error.lower():
        return False
    raise SystemExit(f"Pi rejected optional {command}: {error}")


def main() -> None:
    args = parser().parse_args()
    if (args.provider is None) != (args.model is None):
        raise SystemExit("--provider and --model must be supplied together")

    pi = resolve_pi(args.pi)
    version = bounded_run([pi, "--version"], input_bytes=None, timeout=VERSION_TIMEOUT_SECONDS)
    version_text = version.stdout.decode("utf-8", errors="replace").strip()
    if not version_text:
        raise SystemExit("Pi --version returned no version text")

    argv = [
        pi,
        "--mode",
        "rpc",
        "--no-session",
        "--no-context-files",
        "--no-extensions",
        "--no-approve",
        "--offline",
    ]
    if args.provider is not None:
        argv.extend(["--provider", args.provider, "--model", args.model])
    if args.thinking is not None:
        argv.extend(["--thinking", args.thinking])

    requests = [
        {"id": "state", "type": "get_state"},
        {"id": "models", "type": "get_available_models"},
        {"id": "thinking", "type": "get_available_thinking_levels"},
        {"id": "auto-retry-off", "type": "set_auto_retry", "enabled": False},
        {"id": "clear-queue", "type": "clear_queue"},
    ]
    payload = b"".join(
        json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"
        for request in requests
    )
    completed = bounded_run(argv, input_bytes=payload, timeout=RPC_TIMEOUT_SECONDS)
    responses = parse_responses(completed.stdout)
    state = require_response(responses, "state", "get_state")
    models_data = require_response(responses, "models", "get_available_models")
    require_response(responses, "thinking", "get_available_thinking_levels")
    require_response(responses, "auto-retry-off", "set_auto_retry")
    clear_queue_supported = optional_command_supported(responses, "clear-queue", "clear_queue")

    if state.get("sessionFile") is not None:
        raise SystemExit("live Pi smoke unexpectedly created or attached a session file")
    models = models_data.get("models")
    if not isinstance(models, list) or not models:
        raise SystemExit("Pi get_available_models returned no selectable models")
    if args.provider is not None:
        model = state.get("model")
        if not isinstance(model, dict):
            raise SystemExit("Pi get_state did not return model identity")
        if model.get("provider") != args.provider or model.get("id") != args.model:
            raise SystemExit(
                "Pi get_state model does not match the explicitly requested provider/model"
            )
    if args.thinking is not None and state.get("thinkingLevel") != args.thinking:
        raise SystemExit(
            f"Pi get_state thinking level {state.get('thinkingLevel')!r} does not match {args.thinking!r}"
        )

    print(f"live Pi RPC smoke passed: {version_text}")
    print(f"available Pi models: {len(models)}")
    print(
        "verified: ephemeral session, offline startup, context/extensions disabled, "
        "state/models/thinking RPC, native auto-retry control"
    )
    print(
        "clear_queue RPC: supported"
        if clear_queue_supported
        else "clear_queue RPC: unavailable; Pi Wizard uses exact-process Stop fallback"
    )
    print("no prompt or provider model request was sent")


if __name__ == "__main__":
    main()
