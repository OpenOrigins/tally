import base64
import hashlib
import json
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from email.utils import format_datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

from tally_langgraph import __version__
from tally_langgraph.config import TallyConfig
from tally_langgraph.transport import HttpTransport, _retry_after_seconds


class _Server(ThreadingHTTPServer):
    responses: list[tuple[int, dict[str, str], bytes]]
    requests: list[dict[str, Any]]


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        self.server.requests.append(  # type: ignore[attr-defined]
            {"headers": dict(self.headers), "body": json.loads(body)}
        )
        status, headers, response_body = self.server.responses.pop(0)  # type: ignore[attr-defined]
        self.send_response(status)
        for key, value in headers.items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(response_body)

    def log_message(self, format: str, *args: Any) -> None:
        return


@contextmanager
def _server(*responses: tuple[int, dict[str, str], bytes]) -> Iterator[_Server]:
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


def _transport(server: _Server, tmp_path: Path) -> HttpTransport:
    host, port = server.server_address
    return HttpTransport(
        TallyConfig(
            api_url=f"http://{host}:{port}/v1/tally/logs",
            api_key="test-key",
            state_dir=tmp_path,
        )
    )


def test_success_sends_stable_idempotency_headers(tmp_path: Path) -> None:
    with _server((200, {}, b'{"receipt":{"id":"receipt-1"}}')) as server:
        result = _transport(server, tmp_path).deliver("record-1", {"hello": "world"})

    assert result.disposition == "delivered"
    assert result.receipt == {"id": "receipt-1"}
    assert server.requests[0]["headers"]["Idempotency-Key"] == "record-1"
    assert server.requests[0]["headers"]["X-Tally-Record-Id"] == "record-1"
    assert server.requests[0]["headers"]["X-Api-Key"] == "test-key"
    assert server.requests[0]["headers"]["User-Agent"] == f"tally-langgraph/{__version__}"


def test_large_content_uploads_before_its_log_record(tmp_path: Path) -> None:
    text = "x" * (256 * 1024 + 1)
    object_id = hashlib.sha256(b"record-1\0" + text.encode()).hexdigest()
    replies = [
        (200, {}, json.dumps({"status": "stored", "object_id": object_id}).encode())
        for _ in range(3)
    ] + [
        (200, {}, json.dumps({"status": "ready", "object_id": object_id}).encode()),
        (200, {}, b'{"status":"accepted"}'),
    ]
    record = {
        "record_type": "TURN_END",
        "captured_content": {"capture_status": "complete", "text": text, "kind": "agent.output"},
    }
    with _server(*replies) as server:
        result = _transport(server, tmp_path).deliver("record-1", record)

    assert result.disposition == "delivered"
    chunks = [request["body"]["tally_content_upload"] for request in server.requests[:3]]
    assert b"".join(base64.b64decode(chunk["data_base64"]) for chunk in chunks) == text.encode()
    assert [chunk["chunk_index"] for chunk in chunks] == [0, 1, 2]
    assert server.requests[3]["body"]["tally_content_upload"]["operation"] == "complete"
    assert server.requests[4]["body"]["captured_content"]["content_id"] == object_id
    assert server.requests[4]["body"]["captured_content"]["text"] is None
    assert record["captured_content"]["text"] == text  # the durable journal copy is unchanged
    assert all(request["headers"]["X-Tally-Record-Id"] == "record-1" for request in server.requests)


def test_failed_chunk_keeps_the_log_pending(tmp_path: Path) -> None:
    record = {"captured_content": {"capture_status": "complete", "text": "x" * (256 * 1024 + 1)}}
    with _server((503, {}, b'{"message":"temporary"}')) as server:
        result = _transport(server, tmp_path).deliver("record-1", record)
    assert result.disposition == "retry"
    assert len(server.requests) == 1
    assert server.requests[0]["body"]["tally_content_upload"]["operation"] == "chunk"
    assert record["captured_content"]["text"] is not None


def test_server_error_retries_and_honors_retry_after(tmp_path: Path) -> None:
    with _server((503, {"Retry-After": "12"}, b'{"message":"maintenance"}')) as server:
        result = _transport(server, tmp_path).deliver("record-1", {})

    assert result.disposition == "retry"
    assert result.retry_after_seconds == 12
    assert result.detail == "server returned HTTP 503: maintenance"


def test_invalid_record_is_dead_lettered(tmp_path: Path) -> None:
    with _server((422, {}, b'{"error":"invalid record"}')) as server:
        result = _transport(server, tmp_path).deliver("record-1", {})

    assert result.disposition == "dead_letter"
    assert "invalid record" in (result.detail or "")


def test_redirect_is_not_followed(tmp_path: Path) -> None:
    with _server((302, {"Location": "https://example.com/steal"}, b"")) as server:
        result = _transport(server, tmp_path).deliver("record-1", {})

    assert result.disposition == "retry"
    assert len(server.requests) == 1


def test_missing_key_keeps_record_pending(tmp_path: Path) -> None:
    config = TallyConfig(state_dir=tmp_path, api_key=None)
    result = HttpTransport(config).deliver("record-1", {})
    assert result.disposition == "retry"
    assert result.retry_after_seconds == 60


def test_disabled_forwarding_keeps_record_pending(tmp_path: Path) -> None:
    config = TallyConfig(state_dir=tmp_path, api_key="key", forwarding_enabled=False)
    result = HttpTransport(config).deliver("record-1", {})
    assert result.disposition == "retry"
    assert result.retry_after_seconds == 60


@pytest.mark.parametrize(
    ("body", "receipt", "disposition"),
    [
        (b"", None, "delivered"),
        (b"not json", None, "delivered"),
        (b'["receipt"]', ["receipt"], "delivered"),
        (b'{"anchor_receipt":{"id":"legacy"}}', {"id": "legacy"}, "delivered"),
        (b'{"statusCode":500,"error":"embedded"}', None, "retry"),
    ],
)
def test_success_response_shapes(body: bytes, receipt: Any, disposition: str) -> None:
    result = HttpTransport._success_result(200, body)
    assert result.disposition == disposition
    assert result.receipt == receipt


def test_response_size_and_auth_statuses(tmp_path: Path) -> None:
    oversized = b"x" * (64 * 1_024 + 1)
    with _server((200, {}, oversized)) as server:
        assert _transport(server, tmp_path).deliver("record-1", {}).disposition == "retry"
    assert "oversized" in HttpTransport._response_detail(500, oversized)
    assert HttpTransport._response_detail(500, b"not-json") == "server returned HTTP 500"
    auth = HttpTransport._status_result(401, "unauthorized", None)
    assert auth.retry_after_seconds == 60


def test_retry_after_parsing() -> None:
    future = format_datetime(datetime.now(timezone.utc) + timedelta(seconds=30))
    assert _retry_after_seconds("999") == 300
    assert _retry_after_seconds("-1") == 0
    assert _retry_after_seconds(future) is not None
    assert _retry_after_seconds("not-a-date") is None
    assert _retry_after_seconds(None) is None
