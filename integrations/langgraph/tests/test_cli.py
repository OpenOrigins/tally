import json
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

from tally_langgraph.cli import main


class _Server(ThreadingHTTPServer):
    responses: list[tuple[int, bytes]]
    requests: list[dict[str, Any]]


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        self.server.requests.append(  # type: ignore[attr-defined]
            {"path": self.path, "headers": dict(self.headers), "body": json.loads(body)}
        )
        status, response_body = self.server.responses.pop(0)  # type: ignore[attr-defined]
        self.send_response(status)
        self.end_headers()
        self.wfile.write(response_body)

    def log_message(self, format: str, *args: Any) -> None:
        return


@contextmanager
def _server(*responses: tuple[int, bytes]) -> Iterator[_Server]:
    server = _Server(("127.0.0.1", 0), _Handler)
    server.responses = list(responses)
    server.requests = []
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_connect_writes_env_file_and_handshakes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    with _server((200, b"")) as server:
        host, port = server.server_address
        exit_code = main(
            [
                "connect",
                "--api-key",
                "my-key",
                "--api-url",
                f"http://{host}:{port}/v1/tally/logs",
                "--state-dir",
                str(tmp_path / "state"),
            ]
        )

    assert exit_code == 0
    env_contents = (tmp_path / ".env").read_text(encoding="utf-8")
    assert "TALLY_API_KEY=my-key" in env_contents
    assert f"TALLY_API_URL=http://{host}:{port}/v1/tally/logs" in env_contents
    assert server.requests[0]["body"] == {"source": "langgraph"}
    assert server.requests[0]["headers"]["X-Api-Key"] == "my-key"


def test_connect_preserves_existing_env_lines(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    (tmp_path / ".env").write_text("OTHER_VAR=keep-me\n", encoding="utf-8")

    with _server((200, b"")) as server:
        host, port = server.server_address
        exit_code = main(
            [
                "connect",
                "--api-key",
                "my-key",
                "--api-url",
                f"http://{host}:{port}/v1/tally/logs",
                "--state-dir",
                str(tmp_path / "state"),
            ]
        )

    assert exit_code == 0
    env_contents = (tmp_path / ".env").read_text(encoding="utf-8")
    assert "OTHER_VAR=keep-me" in env_contents
    assert "TALLY_API_KEY=my-key" in env_contents


def test_connect_reuses_existing_key_when_not_passed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    with _server((200, b""), (200, b"")) as server:
        host, port = server.server_address
        api_url = f"http://{host}:{port}/v1/tally/logs"
        state_dir = str(tmp_path / "state")
        main(["connect", "--api-key", "my-key", "--api-url", api_url, "--state-dir", state_dir])
        exit_code = main(["connect", "--state-dir", state_dir])

    assert exit_code == 0
    assert len(server.requests) == 2
    assert server.requests[1]["headers"]["X-Api-Key"] == "my-key"


def test_connect_without_key_fails(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("TALLY_API_KEY", raising=False)
    assert main(["connect", "--state-dir", str(tmp_path / "state")]) == 1


def test_connect_handshake_failure_is_non_fatal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    with _server((503, b"maintenance")) as server:
        host, port = server.server_address
        exit_code = main(
            [
                "connect",
                "--api-key",
                "my-key",
                "--api-url",
                f"http://{host}:{port}/v1/tally/logs",
                "--state-dir",
                str(tmp_path / "state"),
            ]
        )

    assert exit_code == 0
    assert (tmp_path / ".env").read_text(encoding="utf-8").find("TALLY_API_KEY=my-key") != -1


def test_connect_persists_agent_id_across_runs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    state_dir = tmp_path / "state"
    with _server((200, b""), (200, b"")) as server:
        host, port = server.server_address
        api_url = f"http://{host}:{port}/v1/tally/logs"
        args = [
            "connect",
            "--api-key",
            "my-key",
            "--api-url",
            api_url,
            "--state-dir",
            str(state_dir),
        ]
        main(args)

        from tally_langgraph.journal import Journal

        journal = Journal(state_dir, max_record_bytes=16 * 1024 * 1024)
        first_agent_id = journal.get_or_create_metadata("agent_id", lambda: "should-not-be-used")

        main(["connect", "--api-url", api_url, "--state-dir", str(state_dir)])
        second_agent_id = journal.get_or_create_metadata("agent_id", lambda: "should-not-be-used")

    assert first_agent_id == second_agent_id
