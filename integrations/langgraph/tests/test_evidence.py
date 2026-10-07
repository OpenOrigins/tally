import hashlib
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from uuid import UUID

from tally_langgraph.evidence import (
    canonical_json,
    captured_content,
    private_evidence,
    server_evidence,
    to_jsonable,
)


def test_captured_content_is_redacted_bounded_and_labeled() -> None:
    value = {"prompt": "hello 🙂", "api_key": "THIS_SECRET_MUST_NOT_LEAVE"}
    captured = captured_content(
        value, kind="user.input", source_field="value", enabled=True, max_bytes=32
    )
    assert captured["capture_status"] == "partial"
    assert len(captured["text"].encode("utf-8")) <= 32
    assert "THIS_SECRET_MUST_NOT_LEAVE" not in captured["text"]
    assert captured["redaction_count"] == 1
    expected_hash = "sha256:" + hashlib.sha256(canonical_json(value).encode("utf-8")).hexdigest()
    assert captured["content_hash"] == expected_hash
    missing = captured_content(
        None, kind="agent.output", source_field="value", enabled=True, max_bytes=32
    )
    excluded = captured_content(
        value, kind="user.input", source_field="value", enabled=False, max_bytes=32
    )
    assert missing["capture_status"] == "unavailable"
    assert excluded["capture_status"] == "excluded"
    assert excluded["content_hash"] == expected_hash


def test_personal_information_is_removed_from_readable_content() -> None:
    captured = captured_content(
        {
            "prompt": "Email ada@example.test or call phone: +1 415 555 0123 about the sprint",
            "full_name": "Ada Example",
            "email_address": "other@example.test",
            "command": "git status",
        },
        kind="user.input",
        source_field="value",
        enabled=True,
        max_bytes=4096,
    )
    text = captured["text"]
    for personal in ("ada@example.test", "+1 415 555 0123", "Ada Example", "other@example.test"):
        assert personal not in text
    assert "git status" in text
    assert captured["capture_status"] == "complete"
    assert captured["redaction_count"] == 4


@dataclass
class Payload:
    path: Path
    created_at: datetime


class Priority(Enum):
    HIGH = "high"


def test_private_evidence_is_deterministic() -> None:
    first = private_evidence({"b": 2, "a": 1})
    second = private_evidence({"a": 1, "b": 2})

    assert first == second
    assert first[0].startswith("sha256:")
    assert first[1] == first[0].replace("sha256:", "private://sha256/")


def test_projection_redacts_secrets_and_reports_risk() -> None:
    evidence = server_evidence(
        {
            "command": "sudo rm -rf /tmp/demo && curl https://example.test",
            "api_key": "must-not-leave",
            "message": "Authorization: Bearer abcdefghijklmnop",
            "token": "ghp_abcdefghijklmnopqrstuvwxyz",
        },
        enabled=True,
        max_chars=8_192,
    )

    text = evidence["text"]
    assert "must-not-leave" not in text
    assert "abcdefghijklmnop" not in text
    assert "ghp_abcdefghijklmnopqrstuvwxyz" not in text
    assert evidence["redaction_count"] >= 3
    assert "destructive_change" in evidence["risk_signals"]
    assert "external_transfer" in evidence["risk_signals"]
    assert "privilege_escalation" in evidence["risk_signals"]


def test_projection_can_be_disabled() -> None:
    evidence = server_evidence("private text", enabled=False, max_chars=256)
    assert evidence["text"] is None
    assert evidence["visibility"] == "private"
    assert evidence["disabled"] is True


def test_projection_is_bounded() -> None:
    evidence = server_evidence("x" * 1_000, enabled=True, max_chars=256)
    assert len(evidence["text"]) == 256
    assert evidence["truncated"] is True


def test_json_conversion_handles_common_and_recursive_values() -> None:
    recursive: list[object] = []
    recursive.append(recursive)
    value = {
        "payload": Payload(Path("example"), datetime(2026, 1, 1, tzinfo=timezone.utc)),
        "bytes": b"hello",
        "recursive": recursive,
        "infinity": float("inf"),
        "priority": Priority.HIGH,
        "set": {"beta", "alpha"},
        "uuid": UUID("12345678-1234-5678-1234-567812345678"),
    }

    converted = to_jsonable(value)
    encoded = canonical_json(value)

    assert converted["recursive"] == ["[RECURSIVE]"]
    assert converted["bytes"]["base64"] == "aGVsbG8="
    assert converted["priority"] == "high"
    assert converted["set"] == ["alpha", "beta"]
    assert converted["uuid"] == "12345678-1234-5678-1234-567812345678"
    assert json.loads(encoded)["payload"]["path"] == "example"
