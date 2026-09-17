import json
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import pytest

from tally_langgraph.onboarding import OnboardingError, handshake_url, notify_client_connected


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
        if status == 302:
            host, port = self.server.server_address  # type: ignore[attr-defined]
            self.send_header("Location", f"http://{host}:{port}/redirected")
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


def test_handshake_url_targets_onboarding_path_on_same_host() -> None:
    assert (
        handshake_url("https://api.prod.openorigins.com/v1/tally/logs")
        == "https://api.prod.openorigins.com/v1/tally/onboarding/client-connected"
    )


def test_handshake_url_rejects_unsafe_endpoint() -> None:
    with pytest.raises(ValueError, match="HTTPS"):
        handshake_url("http://example.com/v1/tally/logs")
    assert (
        handshake_url("https://api.dev2.openorigins.com/v1/tally/logs?x=1")
        == "https://api.dev2.openorigins.com/v1/tally/onboarding/client-connected"
    )


def test_notify_client_connected_sends_expected_request() -> None:
    with _server((200, b"")) as server:
        host, port = server.server_address
        notify_client_connected(
            api_key="test-key", api_url=f"http://{host}:{port}/v1/tally/logs", source="langgraph"
        )

    request = server.requests[0]
    assert request["path"] == "/v1/tally/onboarding/client-connected"
    assert request["body"] == {"source": "langgraph"}
    assert request["headers"]["X-Api-Key"] == "test-key"
    assert request["headers"]["Content-Type"] == "application/json"


def test_notify_client_connected_raises_on_http_error() -> None:
    with _server((401, b'{"message":"invalid key"}')) as server:
        host, port = server.server_address
        with pytest.raises(OnboardingError, match="invalid key"):
            notify_client_connected(
                api_key="bad-key", api_url=f"http://{host}:{port}/v1/tally/logs", source="langgraph"
            )


def test_notify_client_connected_raises_when_unreachable() -> None:
    with pytest.raises(OnboardingError, match="could not reach"):
        notify_client_connected(
            api_key="test-key", api_url="http://127.0.0.1:1/v1/tally/logs", source="langgraph"
        )


def test_notify_client_connected_does_not_follow_redirects() -> None:
    with _server((302, b"redirect")) as server:
        host, port = server.server_address
        with pytest.raises(OnboardingError, match="HTTP 302"):
            notify_client_connected(
                api_key="test-key",
                api_url=f"http://{host}:{port}/v1/tally/logs",
                source="langgraph",
            )

    assert len(server.requests) == 1


@pytest.mark.parametrize(
    ("api_key", "source"),
    [
        ("bad\nkey", "langgraph"),
        ("test-key", ""),
        ("test-key", "bad\nsource"),
    ],
)
def test_notify_client_connected_validates_headers(api_key: str, source: str) -> None:
    with pytest.raises(ValueError):
        notify_client_connected(
            api_key=api_key,
            api_url="https://api.prod.openorigins.com/v1/tally/logs",
            source=source,
        )
