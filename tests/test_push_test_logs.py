#!/usr/bin/env python3
"""Regression contract for behavior introduced by commit 31b44c5."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import socket
import threading
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).parents[1] / "scripts" / "push_test_logs.py"
SPEC = importlib.util.spec_from_file_location("push_test_logs", SCRIPT)
assert SPEC and SPEC.loader
push_test_logs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(push_test_logs)


class RawHttpServer:
    def __init__(self, statuses: list[int]) -> None:
        self.statuses = statuses
        self.requests: list[bytes] = []
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen()
        self.port = self.socket.getsockname()[1]
        self.thread = threading.Thread(target=self._serve, daemon=True)

    def __enter__(self) -> "RawHttpServer":
        self.thread.start()
        return self

    def __exit__(self, *args: object) -> None:
        self.thread.join(timeout=5)
        self.socket.close()
        if self.thread.is_alive():
            raise AssertionError("HTTP test server did not stop")

    def _serve(self) -> None:
        for status in self.statuses:
            connection, _ = self.socket.accept()
            with connection:
                request = b""
                while b"\r\n\r\n" not in request:
                    request += connection.recv(4096)
                head, body = request.split(b"\r\n\r\n", 1)
                content_length = 0
                for line in head.split(b"\r\n")[1:]:
                    name, value = line.split(b":", 1)
                    if name.lower() == b"content-length":
                        content_length = int(value.strip())
                while len(body) < content_length:
                    body += connection.recv(4096)
                self.requests.append(head + b"\r\n\r\n" + body[:content_length])
                reason = b"OK" if 200 <= status < 300 else b"Error"
                response = json.dumps({"status": status}).encode("ascii")
                connection.sendall(
                    b"HTTP/1.1 "
                    + str(status).encode("ascii")
                    + b" "
                    + reason
                    + b"\r\nContent-Type: application/json\r\nContent-Length: "
                    + str(len(response)).encode("ascii")
                    + b"\r\nConnection: close\r\n\r\n"
                    + response
                )


class PushTestLogsRegressionTests(unittest.TestCase):
    def test_lowercase_auth_header_path_and_body_are_preserved(self) -> None:
        body = {"records": [{"record_type": "HEARTBEAT"}]}
        with RawHttpServer([200]) as server:
            status, response = push_test_logs.post_logs(
                f"http://127.0.0.1:{server.port}/v1/tally/logs?source=test",
                "secret-key",
                body,
                source="regression-test",
                ingest_path="tests/test_push_test_logs.py",
                timeout=3,
            )

        self.assertEqual(status, 200)
        self.assertEqual(json.loads(response), {"status": 200})
        request = server.requests[0]
        head, raw_body = request.split(b"\r\n\r\n", 1)
        self.assertTrue(head.startswith(b"POST /v1/tally/logs?source=test HTTP/1.1\r\n"))
        self.assertIn(b"\r\nx-api-key: secret-key\r\n", b"\r\n" + head + b"\r\n")
        self.assertNotIn(b"X-Api-Key:", head)
        self.assertIn(b"x-oo-tally-source: regression-test", head)
        self.assertIn(b"x-oo-tally-ingest-path: tests/test_push_test_logs.py", head)
        self.assertEqual(json.loads(raw_body), body)

    def test_all_v02_record_types_remain_generatable(self) -> None:
        for record_type in push_test_logs.RECORD_TYPES:
            record = push_test_logs.build_record(
                record_type,
                agent_id="test-agent",
                agent_version="1.0",
                session_id="session-1",
                run_id="run-1",
                ts=datetime.now(timezone.utc),
                instruction_id="instr-1",
                action_id="act-1",
                invalid=False,
            )
            self.assertEqual(record["record_type"], record_type)
            self.assertEqual(record["schema_version"], "0.2")
            self.assertEqual(record["session_id"], "session-1")
            self.assertIn("record_id", record)
            self.assertIn("audit_event_id", record)

    def test_cli_defaults_and_count_bounds_are_preserved(self) -> None:
        args = push_test_logs.parse_args(
            ["--env", "dev2", "--api-key", "secret", "--dry-run"]
        )
        self.assertEqual(args.count, 5)
        self.assertEqual(args.source, "push-test-logs")
        self.assertEqual(args.ingest_path, "scripts/push_test_logs.py")
        self.assertEqual(args.timeout, 30.0)

        for count in (0, 51):
            with contextlib.redirect_stderr(io.StringIO()):
                code = push_test_logs.main(
                    [
                        "--url",
                        "http://127.0.0.1/",
                        "--api-key",
                        "secret",
                        "--count",
                        str(count),
                    ]
                )
            self.assertEqual(code, 2)

    def test_dry_run_never_posts_and_seed_controls_random_choices(self) -> None:
        argv = [
            "--url",
            "http://127.0.0.1/v1/tally/logs",
            "--api-key",
            "secret",
            "--count",
            "3",
            "--seed",
            "42",
            "--dry-run",
        ]
        with mock.patch.object(push_test_logs, "post_logs") as post:
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(push_test_logs.main(argv), 0)
            post.assert_not_called()

        push_test_logs.random.seed(42)
        first = [push_test_logs._rand_hex(8) for _ in range(3)]
        push_test_logs.random.seed(42)
        second = [push_test_logs._rand_hex(8) for _ in range(3)]
        self.assertEqual(first, second)

    def test_one_per_request_posts_each_record_and_propagates_failure(self) -> None:
        with mock.patch.object(
            push_test_logs,
            "post_logs",
            side_effect=[(200, "ok"), (503, "no"), (201, "ok")],
        ) as post:
            with contextlib.redirect_stdout(io.StringIO()):
                code = push_test_logs.main(
                    [
                        "--url",
                        "http://127.0.0.1/v1/tally/logs",
                        "--api-key",
                        "secret",
                        "--count",
                        "3",
                        "--seed",
                        "7",
                        "--one-per-request",
                    ]
                )
        self.assertEqual(code, 1)
        self.assertEqual(post.call_count, 3)
        self.assertEqual(
            [call.kwargs["ingest_path"] for call in post.call_args_list],
            [
                "scripts/push_test_logs.py#1",
                "scripts/push_test_logs.py#2",
                "scripts/push_test_logs.py#3",
            ],
        )

    def test_batch_mode_resolves_environment_and_forwards_options(self) -> None:
        records = [{"record_type": "SESSION_START"}, {"record_type": "SESSION_END"}]
        with (
            mock.patch.object(push_test_logs, "generate_records", return_value=records) as generate,
            mock.patch.object(push_test_logs, "post_logs", return_value=(503, "unavailable")) as post,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            code = push_test_logs.main(
                [
                    "--env",
                    "dev3",
                    "--api-key",
                    "secret",
                    "--count",
                    "2",
                    "--include-invalid",
                    "--source",
                    "custom-source",
                    "--ingest-path",
                    "custom/path",
                    "--timeout",
                    "4.5",
                ]
            )

        self.assertEqual(code, 1)
        generate.assert_called_once_with(2, True)
        post.assert_called_once_with(
            push_test_logs.ENV_URLS["dev3"],
            "secret",
            {"records": records},
            source="custom-source",
            ingest_path="custom/path",
            timeout=4.5,
        )

    def test_default_batch_source_is_selected_from_known_sources(self) -> None:
        with (
            mock.patch.object(push_test_logs, "generate_records", return_value=[]) as generate,
            mock.patch.object(push_test_logs.random, "choice", return_value="manual-qa"),
            mock.patch.object(push_test_logs, "post_logs", return_value=(204, "")) as post,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            code = push_test_logs.main(
                [
                    "--url",
                    "http://127.0.0.1/v1/tally/logs",
                    "--api-key",
                    "secret",
                    "--count",
                    "1",
                ]
            )

        self.assertEqual(code, 0)
        generate.assert_called_once_with(1, False)
        self.assertEqual(post.call_args.kwargs["source"], "manual-qa")


if __name__ == "__main__":
    unittest.main()
