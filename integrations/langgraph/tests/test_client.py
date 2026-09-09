import json
import time
from pathlib import Path
from typing import Any

import pytest

from tally_langgraph import TallyClient, TallyConfig
from tally_langgraph.client import _validate_record
from tally_langgraph.transport import DeliveryResult


class RecordingTransport:
    def __init__(self, *results: DeliveryResult) -> None:
        self.results = list(results)
        self.deliveries: list[tuple[str, dict[str, Any]]] = []

    def deliver(self, record_id: str, record: dict[str, Any]) -> DeliveryResult:
        self.deliveries.append((record_id, record))
        if self.results:
            return self.results.pop(0)
        return DeliveryResult("delivered", receipt={"accepted": record_id})


def _config(tmp_path: Path, **kwargs: Any) -> TallyConfig:
    return TallyConfig(
        state_dir=tmp_path,
        api_key="test-key",
        agent_id="agent:test",
        agent_version="test/1",
        retry_base_seconds=0.01,
        retry_max_seconds=0.02,
        **kwargs,
    )


def test_full_manual_lifecycle_is_ordered_and_delivered(tmp_path: Path) -> None:
    transport = RecordingTransport()
    client = TallyClient(_config(tmp_path), transport=transport, background=False)

    client.start_session("session-1", source="test")
    client.record_instruction("session-1", "instruction-1", {"prompt": "hello"}, context={})
    client.record_action(
        "session-1",
        "instruction-1",
        "action-1",
        tool_server="langchain",
        tool_name="search",
        params={"query": "hello"},
    )
    client.record_result("session-1", "action-1", {"items": [1]})
    client.end_turn("session-1", "turn-1", outcome="completed", value={"answer": "done"})
    client.end_session("session-1", outcome="success", value={"answer": "done"})

    assert client.flush(timeout=1)
    assert [record[1]["record_type"] for record in transport.deliveries] == [
        "SESSION_START",
        "INSTRUCTION_RECEIVED",
        "ACTION_TAKEN",
        "RESULT_RECEIVED",
        "TURN_END",
        "SESSION_END",
    ]
    assert client.journal.statuses() == ["delivered"] * 6


def test_transient_failure_remains_pending_then_retries(tmp_path: Path) -> None:
    transport = RecordingTransport(DeliveryResult("retry", "temporary", retry_after_seconds=0))
    client = TallyClient(_config(tmp_path), transport=transport, background=False)
    client.start_session("session-1", source="test")

    assert client.drain_once()
    assert client.journal.statuses() == ["pending"]
    assert client.drain_once()
    assert client.journal.statuses() == ["delivered"]
    assert transport.deliveries[0][0] == transport.deliveries[1][0]


def test_dead_letter_does_not_block_later_records(tmp_path: Path) -> None:
    transport = RecordingTransport(DeliveryResult("dead_letter", "invalid"))
    client = TallyClient(_config(tmp_path), transport=transport, background=False)
    client.start_session("session-1", source="test")
    client.record_instruction("session-1", "instruction-1", "hello", context={})

    assert client.flush(timeout=1)
    assert client.journal.statuses() == ["dead_letter", "delivered"]


def test_secret_is_local_and_server_projection_is_redacted(tmp_path: Path) -> None:
    client = TallyClient(_config(tmp_path), transport=RecordingTransport(), background=False)
    client.start_session("session-1", source="test")
    client.record_instruction(
        "session-1",
        "instruction-1",
        {"api_key": "do-not-send", "prompt": "hello"},
        context={},
    )

    record = client.journal.records()[1]
    assert "do-not-send" not in json.dumps(record)
    assert record["server_evidence"]["text"] == '{"api_key":"[REDACTED]","prompt":"hello"}'
    assert "do-not-send" in (client.journal.evidence_payload(record["instruction_hash"]) or "")


def test_agent_identity_persists(tmp_path: Path) -> None:
    config = TallyConfig(state_dir=tmp_path, api_key=None)
    first = TallyClient(config, transport=RecordingTransport(), background=False)
    second = TallyClient(config, transport=RecordingTransport(), background=False)
    assert first.agent_id == second.agent_id
    assert first.anchor_instance_id == second.anchor_instance_id


def test_agent_scoped_heartbeat_contains_all_active_sessions(tmp_path: Path) -> None:
    client = TallyClient(_config(tmp_path), transport=RecordingTransport(), background=False)
    client.start_session("session-b", source="test")
    client.start_session("session-a", source="test")

    assert client._maybe_emit_heartbeat(now=time.time() + 601)
    heartbeat = client.journal.records()[-1]
    assert heartbeat["record_type"] == "HEARTBEAT"
    assert heartbeat["active_sessions"] == ["session-a", "session-b"]


def test_background_worker_delivers_and_stops(tmp_path: Path) -> None:
    client = TallyClient(
        _config(tmp_path, worker_poll_seconds=0.01),
        transport=RecordingTransport(),
        background=True,
    )
    client.start_session("session-1", source="test")

    deadline = time.monotonic() + 2
    while client.journal.statuses() != ["delivered"] and time.monotonic() < deadline:
        time.sleep(0.01)

    assert client.journal.statuses() == ["delivered"]
    assert client.close(timeout=1)


def test_closed_client_rejects_new_records(tmp_path: Path) -> None:
    client = TallyClient(_config(tmp_path), transport=RecordingTransport(), background=False)
    assert client.close(flush=False) is False
    try:
        client.start_session("session-1", source="test")
    except RuntimeError as error:
        assert str(error) == "TallyClient is closed"
    else:
        raise AssertionError("closed client accepted a new record")


def test_public_helpers_and_context_manager(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("TALLY_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("TALLY_AGENT_ID", "agent:environment")
    client = TallyClient.from_env(transport=RecordingTransport(), background=False)

    assert client.callback(source="custom").source == "custom"
    assert client.drain_once() is False
    assert client._maybe_emit_heartbeat() is False
    assert client.__enter__() is client
    client.__exit__(None, None, None)
    assert client.close() is True


@pytest.mark.parametrize(
    ("method", "kwargs", "message"),
    [
        (
            "record_handoff",
            {"receiving_agent": "agent:x", "payload": None, "acknowledgement_status": "lost"},
            "acknowledgement_status",
        ),
        ("end_turn", {"turn_id": "turn", "outcome": "unknown", "value": None}, "turn outcome"),
        ("end_session", {"outcome": "unknown", "value": None}, "session outcome"),
    ],
)
def test_public_methods_validate_outcomes(
    tmp_path: Path, method: str, kwargs: dict[str, Any], message: str
) -> None:
    client = TallyClient(_config(tmp_path), transport=RecordingTransport(), background=False)
    with pytest.raises(ValueError, match=message):
        getattr(client, method)("session", **kwargs)


def test_transport_exception_is_retried_with_backoff(tmp_path: Path) -> None:
    class ExplodingTransport:
        def deliver(self, record_id: str, record: dict[str, Any]) -> DeliveryResult:
            raise RuntimeError("network adapter failed")

    client = TallyClient(_config(tmp_path), transport=ExplodingTransport(), background=False)
    client.start_session("session-1", source="test")

    assert client.drain_once()
    assert client.journal.statuses() == ["pending"]
    assert client._retry_delay(client.journal.claim(lease_seconds=1, now=time.time() + 1)) <= 0.02  # type: ignore[arg-type]


def test_start_session_rolls_back_registration_on_capture_failure(tmp_path: Path) -> None:
    config = _config(tmp_path, max_record_bytes=1_024)
    client = TallyClient(config, transport=RecordingTransport(), background=False)

    with pytest.raises(ValueError, match="maximum"):
        client.start_session("session-1", source="x" * 2_000)

    assert (
        client.journal.claim_heartbeat(
            client.agent_id,
            interval_seconds=600,
            stale_after_seconds=1_800,
            now=time.time() + 601,
        )
        == []
    )


@pytest.mark.parametrize(
    ("record", "message"),
    [
        ({}, "unsupported record_type"),
        (
            {"record_type": "SESSION_START", "schema_version": "0.1", "record_id": "x"},
            "schema_version",
        ),
        ({"record_type": "SESSION_START", "schema_version": "0.2"}, "record_id"),
        (
            {
                "record_type": "SESSION_START",
                "schema_version": "0.2",
                "record_id": "x",
                "input_hash": "sha256:abc",
                "input_uri": "private://sha256/def",
            },
            "invalid evidence pair",
        ),
        (
            {
                "record_type": "SESSION_START",
                "schema_version": "0.2",
                "record_id": "x",
                "input_hash": 1,
                "input_uri": "private://sha256/abc",
            },
            "must both be strings",
        ),
        (
            {
                "record_type": "SESSION_START",
                "schema_version": "0.2",
                "record_id": "x",
                "input_uri": f"private://sha256/{'a' * 64}",
            },
            "missing its matching input_hash",
        ),
    ],
)
def test_record_validation_rejects_invalid_records(record: dict[str, Any], message: str) -> None:
    with pytest.raises(ValueError, match=message):
        _validate_record(record)


def test_record_validation_accepts_null_and_nested_evidence() -> None:
    digest = "a" * 64
    _validate_record(
        {
            "record_type": "SESSION_START",
            "schema_version": "0.2",
            "record_id": "x",
            "ignored_hash": "anything",
            "values": [
                {"input_hash": None, "input_uri": None},
                {
                    "input_hash": f"sha256:{digest}",
                    "input_uri": f"private://sha256/{digest}",
                },
            ],
        }
    )
