"""HTTP delivery with explicit status handling and stable idempotency keys."""

from __future__ import annotations

import http.client
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from typing import Any, Literal, Protocol
from urllib.parse import urlsplit

from ._tls import ssl_context
from ._version import __version__
from .config import TallyConfig

_MAX_RESPONSE_BYTES = 64 * 1024


@dataclass(frozen=True, slots=True)
class DeliveryResult:
    disposition: Literal["delivered", "retry", "dead_letter"]
    detail: str | None = None
    receipt: Any = None
    retry_after_seconds: float | None = None


class Transport(Protocol):
    def deliver(self, record_id: str, record: dict[str, Any]) -> DeliveryResult: ...


class HttpTransport:
    def __init__(self, config: TallyConfig) -> None:
        self.config = config

    def deliver(self, record_id: str, record: dict[str, Any]) -> DeliveryResult:
        if not self.config.forwarding_enabled:
            return DeliveryResult("retry", "forwarding is disabled", retry_after_seconds=60)
        if not self.config.api_key:
            return DeliveryResult(
                "retry", "TALLY_API_KEY is not configured", retry_after_seconds=60
            )

        body = json.dumps(
            record,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
        parts = urlsplit(self.config.api_url)
        assert parts.hostname  # guaranteed by TallyConfig's _validate_api_url
        headers = {
            "Content-Type": "application/json",
            "Content-Length": str(len(body)),
            "Idempotency-Key": record_id,
            "User-Agent": f"tally-langgraph/{__version__}",
            # urllib.request always re-title-cases header names before sending
            # ("x-api-key" -> "X-Api-Key"), and the API Gateway authorizer only
            # accepts the exact lowercase "x-api-key" as its identity source, so
            # this uses http.client directly to send it byte-for-byte as given.
            # http.client also never follows redirects on its own, so the Agent
            # API key is never at risk of being forwarded to another origin.
            "x-api-key": self.config.api_key,
            "X-Oo-Tally-Ingest-Path": "tally-langgraph",
            "X-Oo-Tally-Source": "sdk",
            "X-Tally-Record-Id": record_id,
        }
        path = parts.path or "/"
        if parts.query:
            path = f"{path}?{parts.query}"
        connection: http.client.HTTPConnection
        if parts.scheme == "https":
            connection = http.client.HTTPSConnection(
                parts.hostname,
                parts.port,
                timeout=self.config.request_timeout_seconds,
                context=ssl_context(),
            )
        else:
            connection = http.client.HTTPConnection(
                parts.hostname, parts.port, timeout=self.config.request_timeout_seconds
            )
        try:
            connection.request("POST", path, body=body, headers=headers)
            response = connection.getresponse()
            response_body = response.read(_MAX_RESPONSE_BYTES + 1)
            if len(response_body) > _MAX_RESPONSE_BYTES:
                return DeliveryResult("retry", "server response exceeded 64 KiB")
            if not 200 <= response.status < 300:
                detail = self._response_detail(response.status, response_body)
                return self._status_result(
                    response.status, detail, response.getheader("Retry-After")
                )
            return self._success_result(response_body)
        except (TimeoutError, OSError, http.client.HTTPException) as error:
            return DeliveryResult("retry", f"delivery failed: {error}")
        finally:
            connection.close()

    @staticmethod
    def _success_result(body: bytes) -> DeliveryResult:
        if not body.strip():
            return DeliveryResult("delivered")
        try:
            value = json.loads(body)
        except (UnicodeDecodeError, json.JSONDecodeError):
            return DeliveryResult("delivered")
        if isinstance(value, dict):
            embedded_status = value.get("status_code", value.get("statusCode", 0))
            if isinstance(embedded_status, int) and embedded_status >= 400:
                message = value.get("message", value.get("error", "API error"))
                return DeliveryResult("retry", f"server returned {embedded_status}: {message}")
            receipt = value.get("receipt", value.get("anchor_receipt", value))
        else:
            receipt = value
        return DeliveryResult("delivered", receipt=receipt)

    @staticmethod
    def _response_detail(status: int, body: bytes) -> str:
        if len(body) > _MAX_RESPONSE_BYTES:
            return f"server returned HTTP {status} with an oversized response"
        try:
            value = json.loads(body)
        except (UnicodeDecodeError, json.JSONDecodeError):
            return f"server returned HTTP {status}"
        if isinstance(value, dict):
            message = value.get("message", value.get("error"))
            if isinstance(message, str) and message:
                return f"server returned HTTP {status}: {message[:1_024]}"
        return f"server returned HTTP {status}"

    @staticmethod
    def _status_result(status: int, detail: str, retry_after: str | None) -> DeliveryResult:
        if status in {400, 413, 415, 422}:
            return DeliveryResult("dead_letter", detail)
        delay = _retry_after_seconds(retry_after)
        if delay is None and status in {401, 403, 404}:
            delay = 60
        return DeliveryResult("retry", detail, retry_after_seconds=delay)


def _retry_after_seconds(value: str | None) -> float | None:
    if not value:
        return None
    try:
        return float(min(max(int(value.strip()), 0), 300))
    except ValueError:
        try:
            parsed = parsedate_to_datetime(value)
        except (TypeError, ValueError):
            return None
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=timezone.utc)
        return min(max((parsed - datetime.now(timezone.utc)).total_seconds(), 0), 300)
