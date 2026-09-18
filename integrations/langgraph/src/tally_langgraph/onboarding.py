"""One-time dashboard handshake performed during installation.

Mirrors the ``notify_client_connected`` contract shared by the other Tally
clients (``tally/common/src/lib.rs``): a POST to ``/v1/tally/onboarding/client-connected``
on the same host as the configured ingest URL, with an ``x-api-key`` header and a
``{"source": ...}`` body. It tells the dashboard this installation is wired up before
any log record arrives; log delivery itself does not depend on it succeeding.
"""

from __future__ import annotations

import http.client
import json
from urllib.parse import urlsplit, urlunsplit

from ._tls import ssl_context
from ._version import __version__

_HANDSHAKE_PATH = "/v1/tally/onboarding/client-connected"
_MAX_RESPONSE_BYTES = 64 * 1024


class OnboardingError(RuntimeError):
    """Raised when the dashboard handshake fails. Callers should treat this as best-effort."""


def handshake_url(api_url: str) -> str:
    """Return the onboarding endpoint on the same host as ``api_url``."""

    parts = urlsplit(api_url)
    return urlunsplit((parts.scheme, parts.netloc, _HANDSHAKE_PATH, "", ""))


def notify_client_connected(
    *,
    api_key: str,
    api_url: str,
    source: str,
    timeout: float = 5.0,
) -> None:
    """POST the one-time "client connected" notification.

    Raises :class:`OnboardingError` on any failure; the caller decides whether
    that should block setup or just be reported as a warning.
    """

    url = handshake_url(api_url)
    parts = urlsplit(url)
    if not parts.hostname:
        raise OnboardingError(f"invalid onboarding URL: {url}")
    body = json.dumps({"source": source}, separators=(",", ":")).encode("utf-8")
    headers = {
        "Content-Type": "application/json",
        "Content-Length": str(len(body)),
        "User-Agent": f"tally-langgraph/{__version__}",
        # urllib.request always re-title-cases header names before sending
        # ("x-api-key" -> "X-Api-Key"), and the API Gateway authorizer only
        # accepts the exact lowercase "x-api-key" as its identity source, so
        # this uses http.client directly to send it byte-for-byte as given.
        "x-api-key": api_key,
    }
    path = parts.path or "/"
    if parts.query:
        path = f"{path}?{parts.query}"
    connection: http.client.HTTPConnection
    if parts.scheme == "https":
        connection = http.client.HTTPSConnection(
            parts.hostname, parts.port, timeout=timeout, context=ssl_context()
        )
    else:
        connection = http.client.HTTPConnection(parts.hostname, parts.port, timeout=timeout)
    try:
        connection.request("POST", path, body=body, headers=headers)
        response = connection.getresponse()
        detail = response.read(_MAX_RESPONSE_BYTES).decode("utf-8", "replace").strip()
        if not 200 <= response.status < 300:
            message = f"server returned HTTP {response.status}"
            if detail:
                message = f"{message}: {detail[:500]}"
            raise OnboardingError(message)
    except (TimeoutError, OSError, http.client.HTTPException) as error:
        raise OnboardingError(f"could not reach {url}: {error}") from error
    finally:
        connection.close()
