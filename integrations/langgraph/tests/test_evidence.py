import json
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from tally_langgraph.evidence import canonical_json, private_evidence, server_evidence, to_jsonable


@dataclass
class Payload:
    path: Path
    created_at: datetime


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
    }

    converted = to_jsonable(value)
    encoded = canonical_json(value)

    assert converted["recursive"] == ["[RECURSIVE]"]
    assert converted["bytes"]["base64"] == "aGVsbG8="
    assert json.loads(encoded)["payload"]["path"] == "example"
