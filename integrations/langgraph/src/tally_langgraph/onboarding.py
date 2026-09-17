"""One-time dashboard handshake performed during installation.

Mirrors the ``notify_client_connected`` contract shared by the other Tally
clients (``tally/common/src/lib.rs``): a POST to ``/v1/tally/onboarding/client-connected``
on the same host as the configured ingest URL, with an ``x-api-key`` header and a
``{"source": ...}`` body. It tells the dashboard this installation is wired up before
any log record arrives; log delivery itself does not depend on it succeeding.
"""

from __future__ import annotations

import json
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit, urlunsplit
from urllib.request import Request, build_opener

from ._tls import NoRedirectHandler, https_handler
from ._version import __version__
from .config import normalize_api_key, validate_api_url

_HANDSHAKE_PATH = "/v1/tally/onboarding/client-connected"
_MAX_RESPONSE_BYTES = 64 * 1024


class OnboardingError(RuntimeError):
    """Raised when the dashboard handshake fails. Callers should treat this as best-effort."""


def normalize_source(source: str) -> str:
    normalized = source.strip()
    if (
        not normalized
        or len(normalized) > 128
        or any(ord(character) < 32 or ord(character) == 127 for character in normalized)
    ):
        raise ValueError("source must be a non-empty printable value of at most 128 characters")
    return normalized


def handshake_url(api_url: str) -> str:
    """Return the onboarding endpoint on the same host as ``api_url``."""

    parts = urlsplit(validate_api_url(api_url))
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

    normalized_key = normalize_api_key(api_key)
    if normalized_key is None:
        raise ValueError("Tally API key must not be empty")
    normalized_source = normalize_source(source)

    url = handshake_url(api_url)
    body = json.dumps({"source": normalized_source}, separators=(",", ":")).encode("utf-8")
    request = Request(
        url,
        data=body,
        method="POST",
        headers={
            "Content-Type": "application/json",
            "User-Agent": f"tally-langgraph/{__version__}",
            "X-Api-Key": normalized_key,
        },
    )
    opener = build_opener(NoRedirectHandler(), https_handler())
    try:
        with opener.open(request, timeout=timeout) as response:
            response.read(_MAX_RESPONSE_BYTES)
            if not 200 <= response.status < 300:
                raise OnboardingError(f"server returned HTTP {response.status}")
    except HTTPError as error:
        detail = error.read(_MAX_RESPONSE_BYTES).decode("utf-8", "replace").strip()
        message = f"server returned HTTP {error.code}"
        if detail:
            message = f"{message}: {detail[:500]}"
        raise OnboardingError(message) from error
    except (TimeoutError, URLError, OSError) as error:
        raise OnboardingError(f"could not reach {url}: {error}") from error
