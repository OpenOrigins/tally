"""Private evidence references and bounded server-visible projections."""

from __future__ import annotations

import base64
import dataclasses
import hashlib
import json
import math
import re
from collections.abc import Mapping, Sequence, Set
from datetime import date, datetime
from enum import Enum
from pathlib import Path
from typing import Any
from uuid import UUID

_MAX_DEPTH = 32
_MAX_COLLECTION_ITEMS = 10_000
_SENSITIVE_SUFFIXES = ("_password", "_secret", "_token", "_api_key", "_credential")
_SENSITIVE_KEYS = {
    "access_key",
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "credential",
    "credentials",
    "password",
    "passwd",
    "private_key",
    "secret",
    "token",
}
_SECRET_PATTERNS = (
    re.compile(
        r"(?i)\b([A-Z0-9_]*(?:API[_-]?KEY|ACCESS[_-]?KEY|SECRET|TOKEN|PASSWORD|PASSWD|"
        r"CREDENTIAL)[A-Z0-9_]*)\b\s*=\s*(?:\"[^\"\r\n]*\"|'[^'\r\n]*'|[^\s;&|]+)"
    ),
    re.compile(
        r"(?i)\b(api[ _-]?key|access[ _-]?key|secret|token|password|authorization|cookie|"
        r"credential)\b\s*(?::|=|\bis\b)?\s*[\"']?([A-Za-z0-9_./+=-]{8,})"
    ),
    re.compile(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}"),
    re.compile(
        r"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{16,}|"
        r"github_pat_[A-Za-z0-9_]{16,}|(?:AKIA|ASIA)[0-9A-Z]{16}|"
        r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})\b"
    ),
    re.compile(
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        re.DOTALL,
    ),
)
_RISK_RULES = {
    "credential_access": (
        ".aws/credentials",
        ".env",
        ".ssh/",
        "api_key",
        "keychain",
        "password",
        "private_key",
    ),
    "destructive_change": (
        "del /f",
        "drop table",
        "format c:",
        "git clean -fd",
        "git push --force",
        "git reset --hard",
        "rm -rf",
        "truncate table",
    ),
    "dynamic_execution": ("bash -c", "eval(", "invoke-expression", "powershell -enc", "sh -c"),
    "external_transfer": ("curl ", "invoke-webrequest", "nc ", "rsync ", "scp ", "wget "),
    "persistence_change": (
        "crontab",
        "currentversion\\run",
        "launchctl",
        "schtasks",
        "systemctl enable",
    ),
    "privilege_escalation": ("chmod 777", "runas ", "setfacl ", "sudo ", "takeown "),
}


def to_jsonable(value: Any, *, _depth: int = 0, _seen: set[int] | None = None) -> Any:
    """Convert common LangChain/Python values into deterministic JSON data."""

    if isinstance(value, Enum):
        return to_jsonable(value.value, _depth=_depth, _seen=_seen)
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        return value if math.isfinite(value) else repr(value)
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, UUID):
        return str(value)
    if isinstance(value, bytes):
        return {"_type": "bytes", "base64": base64.b64encode(value).decode("ascii")}
    if _depth >= _MAX_DEPTH:
        return "[MAX_DEPTH]"

    seen = _seen if _seen is not None else set()
    identity = id(value)
    if identity in seen:
        return "[RECURSIVE]"
    seen.add(identity)
    try:
        if dataclasses.is_dataclass(value) and not isinstance(value, type):
            return {
                field.name: to_jsonable(getattr(value, field.name), _depth=_depth + 1, _seen=seen)
                for field in dataclasses.fields(value)
            }
        model_dump = getattr(value, "model_dump", None)
        if callable(model_dump):
            return to_jsonable(model_dump(mode="json"), _depth=_depth + 1, _seen=seen)
        if isinstance(value, Mapping):
            result = {}
            for index, (key, item) in enumerate(value.items()):
                if index >= _MAX_COLLECTION_ITEMS:
                    result["_tally_truncated"] = True
                    break
                result[str(key)] = to_jsonable(item, _depth=_depth + 1, _seen=seen)
            return result
        if isinstance(value, Sequence) and not isinstance(value, (str, bytes, bytearray)):
            items = [
                to_jsonable(item, _depth=_depth + 1, _seen=seen)
                for item in value[:_MAX_COLLECTION_ITEMS]
            ]
            if len(value) > _MAX_COLLECTION_ITEMS:
                items.append("[TRUNCATED]")
            return items
        if isinstance(value, Set):
            items = [
                to_jsonable(item, _depth=_depth + 1, _seen=seen)
                for item in list(value)[:_MAX_COLLECTION_ITEMS]
            ]
            items.sort(
                key=lambda item: json.dumps(
                    item,
                    ensure_ascii=False,
                    allow_nan=False,
                    separators=(",", ":"),
                    sort_keys=True,
                )
            )
            if len(value) > _MAX_COLLECTION_ITEMS:
                items.append("[TRUNCATED]")
            return items
        return repr(value)
    finally:
        seen.discard(identity)


def canonical_json(value: Any) -> str:
    return json.dumps(
        to_jsonable(value),
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    )


def private_evidence(value: Any) -> tuple[str, str, str]:
    payload = canonical_json(value)
    digest = hashlib.sha256(payload.encode("utf-8")).hexdigest()
    return f"sha256:{digest}", f"private://sha256/{digest}", payload


def _sensitive_key(key: str) -> bool:
    normalized = key.lower().replace("-", "_").replace(" ", "_")
    return normalized in _SENSITIVE_KEYS or normalized.endswith(_SENSITIVE_SUFFIXES)


def _redact_string(value: str) -> tuple[str, int]:
    redactions = 0

    def replacement(match: re.Match[str]) -> str:
        nonlocal redactions
        redactions += 1
        label = match.group(1) if match.lastindex else None
        if "PRIVATE KEY" in match.group(0):
            return "[REDACTED PRIVATE KEY]"
        return f"{label}=[REDACTED]" if label else "[REDACTED]"

    result = value
    for pattern in _SECRET_PATTERNS:
        result = pattern.sub(replacement, result)
    return result, redactions


def _redact(value: Any, key: str | None = None) -> tuple[Any, int]:
    if key is not None and _sensitive_key(key):
        return "[REDACTED]", 1
    if isinstance(value, dict):
        redacted_mapping: dict[Any, Any] = {}
        count = 0
        for child_key, child in value.items():
            redacted, child_count = _redact(child, str(child_key))
            redacted_mapping[child_key] = redacted
            count += child_count
        return redacted_mapping, count
    if isinstance(value, list):
        redacted_items: list[Any] = []
        count = 0
        for child in value:
            redacted, child_count = _redact(child)
            redacted_items.append(redacted)
            count += child_count
        return redacted_items, count
    if isinstance(value, str):
        return _redact_string(value)
    return value, 0


def server_evidence(value: Any, *, enabled: bool, max_chars: int) -> dict[str, Any]:
    jsonable = to_jsonable(value)
    content_hash, _, _ = private_evidence(jsonable)
    if not enabled:
        return {
            "schema_version": "tally-server-evidence.v1",
            "visibility": "private",
            "text": None,
            "content_hash": content_hash,
            "truncated": False,
            "redaction_count": 0,
            "risk_signals": [],
            "disabled": True,
        }

    redacted, redaction_count = _redact(jsonable)
    text = redacted if isinstance(redacted, str) else canonical_json(redacted)
    truncated = len(text) > max_chars
    text = text[:max_chars]
    lowered = text.lower()
    signals = sorted(
        name for name, patterns in _RISK_RULES.items() if any(item in lowered for item in patterns)
    )
    return {
        "schema_version": "tally-server-evidence.v1",
        "visibility": "arbitrator",
        "text": text,
        "content_hash": content_hash,
        "truncated": truncated,
        "redaction_count": redaction_count,
        "risk_signals": signals,
    }


def evidence_summary(evidence: Mapping[str, Any], fallback: str) -> str:
    text = evidence.get("text")
    if isinstance(text, str) and text:
        return text[:240]
    return fallback
