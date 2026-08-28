from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch


TOOLS = Path(__file__).resolve().parent
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import smoke_live_pi  # noqa: E402


class SmokeLivePiTests(unittest.TestCase):
    def test_response_parser_ignores_events_and_keeps_exact_response_ids(self) -> None:
        stdout = b"\n".join(
            [
                json.dumps({"type": "agent_start"}).encode(),
                json.dumps(
                    {
                        "id": "state",
                        "type": "response",
                        "command": "get_state",
                        "success": True,
                        "data": {"sessionFile": None},
                    }
                ).encode(),
            ]
        )
        responses = smoke_live_pi.parse_responses(stdout)
        self.assertEqual(set(responses), {"state"})
        data = smoke_live_pi.require_response(responses, "state", "get_state")
        self.assertIsNone(data["sessionFile"])

    def test_bounded_run_captures_small_stdout_and_stderr(self) -> None:
        result = smoke_live_pi.bounded_run(
            [
                sys.executable,
                "-c",
                "import sys; sys.stdout.write('ok'); sys.stderr.write('note')",
            ],
            input_bytes=None,
            timeout=2,
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"ok")
        self.assertEqual(result.stderr, b"note")

    def test_bounded_run_terminates_output_flood_without_buffering_it(self) -> None:
        with patch.object(smoke_live_pi, "MAX_CAPTURE_BYTES", 128):
            with self.assertRaisesRegex(SystemExit, "output exceeded|exceeded 128 bytes"):
                smoke_live_pi.bounded_run(
                    [
                        sys.executable,
                        "-c",
                        "import sys,time; sys.stdout.buffer.write(b'x'*65536); sys.stdout.flush(); time.sleep(1)",
                    ],
                    input_bytes=None,
                    timeout=2,
                )

    def test_bounded_run_terminates_deadline(self) -> None:
        with self.assertRaisesRegex(SystemExit, "timed out"):
            smoke_live_pi.bounded_run(
                [sys.executable, "-c", "import time; time.sleep(2)"],
                input_bytes=None,
                timeout=0,
            )

    def test_rejected_response_is_not_accepted_as_compatibility(self) -> None:
        responses = {
            "state": {
                "id": "state",
                "type": "response",
                "command": "get_state",
                "success": False,
                "error": "fixture rejection",
            }
        }
        with self.assertRaisesRegex(SystemExit, "fixture rejection"):
            smoke_live_pi.require_response(responses, "state", "get_state")

    def test_optional_command_support_distinguishes_unknown_from_other_rejection(self) -> None:
        supported = {
            "clear": {
                "id": "clear",
                "type": "response",
                "command": "clear_queue",
                "success": True,
                "data": {"steering": [], "followUp": []},
            }
        }
        self.assertTrue(
            smoke_live_pi.optional_command_supported(supported, "clear", "clear_queue")
        )

        unknown = {
            "clear": {
                "id": "clear",
                "type": "response",
                "command": "clear_queue",
                "success": False,
                "error": "Unknown command: clear_queue",
            }
        }
        self.assertFalse(
            smoke_live_pi.optional_command_supported(unknown, "clear", "clear_queue")
        )

        rejected = {
            "clear": {
                "id": "clear",
                "type": "response",
                "command": "clear_queue",
                "success": False,
                "error": "queue failure",
            }
        }
        with self.assertRaisesRegex(SystemExit, "queue failure"):
            smoke_live_pi.optional_command_supported(rejected, "clear", "clear_queue")


if __name__ == "__main__":
    unittest.main()
