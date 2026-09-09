"""HTTP delivery with explicit status handling and stable idempotency keys."""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from typing import Any, Literal, Protocol
from urllib.error import HTTPError, URLError
from urllib.request import HTTPRedirectHandler, Request, build_opener

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


class _NoRedirectHandler(HTTPRedirectHandler):
    """Do not risk forwarding the Agent API key to a redirected origin."""

    def redirect_request(
        self, req: Any, fp: Any, code: int, msg: str, headers: Any, newurl: str
    ) -> None:
        return None


class HttpTransport:
    def __init__(self, config: TallyConfig) -> None:
        self.config = config
        self._opener = build_opener(_NoRedirectHandler())

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
        request = Request(
            self.config.api_url,
            data=body,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Idempotency-Key": record_id,
                "User-Agent": f"tally-langgraph/{__version__}",
                "X-Api-Key": self.config.api_key,
                "X-Oo-Tally-Ingest-Path": "tally-langgraph",
                "X-Oo-Tally-Source": "sdk",
                "X-Tally-Record-Id": record_id,
            },
        )
        try:
            with self._opener.open(
                request, timeout=self.config.request_timeout_seconds
            ) as response:
                response_body = response.read(_MAX_RESPONSE_BYTES + 1)
                if len(response_body) > _MAX_RESPONSE_BYTES:
                    return DeliveryResult("retry", "server response exceeded 64 KiB")
                return self._success_result(response.status, response_body)
        except HTTPError as error:
            body_bytes = error.read(_MAX_RESPONSE_BYTES + 1)
            detail = self._response_detail(error.code, body_bytes)
            return self._status_result(error.code, detail, error.headers.get("Retry-After"))
        except (TimeoutError, URLError, OSError) as error:
            return DeliveryResult("retry", f"delivery failed: {error}")

    @staticmethod
    def _success_result(status: int, body: bytes) -> DeliveryResult:
        if not 200 <= status < 300:
            return HttpTransport._status_result(status, f"server returned HTTP {status}", None)
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
