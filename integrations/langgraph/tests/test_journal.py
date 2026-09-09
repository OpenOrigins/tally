import hashlib
import multiprocessing
import os
import stat
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from tally_langgraph.journal import Journal


def _record(record_id: str) -> dict[str, str]:
    return {"record_id": record_id, "record_type": "SESSION_START", "schema_version": "0.2"}


def _heartbeat_factory(
    record_id: str,
) -> Callable[[list[str]], tuple[dict[str, Any], list[tuple[str, str]]]]:
    def build(sessions: list[str]) -> tuple[dict[str, Any], list[tuple[str, str]]]:
        return (
            {
                "record_id": record_id,
                "record_type": "HEARTBEAT",
                "schema_version": "0.2",
                "active_sessions": sessions,
            },
            [],
        )

    return build


def _enqueue_batch(state_dir: str, process_index: int, count: int) -> None:
    journal = Journal(Path(state_dir), max_record_bytes=10_000)
    for item_index in range(count):
        record_id = f"process-{process_index}-record-{item_index}"
        journal.enqueue(_record(record_id), [], agent_id="agent:test")


def test_claim_lease_recovers_and_preserves_order(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    journal.enqueue(_record("one"), [], agent_id="agent:test", now=1)
    journal.enqueue(_record("two"), [], agent_id="agent:test", now=2)

    first = journal.claim(lease_seconds=10, now=3)
    assert first is not None
    assert first.record_id == "one"
    assert journal.claim(lease_seconds=10, now=4) is None

    reclaimed = journal.claim(lease_seconds=10, now=14)
    assert reclaimed is not None
    assert reclaimed.record_id == "one"
    assert reclaimed.attempts == 2
    assert journal.mark_delivered(reclaimed, receipt={"id": "receipt-1"}, now=15)

    second = journal.claim(lease_seconds=10, now=16)
    assert second is not None
    assert second.record_id == "two"


def test_retry_dead_letter_and_pruning(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    journal.enqueue(_record("retry"), [], agent_id="agent:test", now=1)
    item = journal.claim(lease_seconds=10, now=2)
    assert item is not None
    assert journal.mark_retry(item, detail="later", next_attempt_at=20)
    assert journal.claim(lease_seconds=10, now=19) is None

    item = journal.claim(lease_seconds=10, now=20)
    assert item is not None
    assert journal.mark_dead_letter(item, detail="invalid", now=21)
    assert journal.statuses() == ["dead_letter"]
    assert journal.prune_delivered(before=100) == 0


def test_evidence_is_deduplicated_and_pruned_with_delivered_record(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    payload = '{"secret":1}'
    digest = f"sha256:{hashlib.sha256(payload.encode()).hexdigest()}"
    journal.enqueue(
        _record("one"),
        [(digest, payload), (digest, payload)],
        agent_id="agent:test",
        now=1,
    )
    assert journal.evidence_payload(digest) == payload
    item = journal.claim(lease_seconds=10, now=2)
    assert item is not None
    journal.mark_delivered(item, now=3)

    assert journal.prune_delivered(before=4) == 1
    assert journal.evidence_payload(digest) is None


def test_heartbeat_claim_is_shared_across_journal_instances(tmp_path: Path) -> None:
    first = Journal(tmp_path, max_record_bytes=10_000)
    second = Journal(tmp_path, max_record_bytes=10_000)
    first.register_session("session-a", "agent:test", now=1)
    second.register_session("session-b", "agent:test", now=1)

    assert first.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("heartbeat-1"),
        now=601,
    )
    assert not second.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("heartbeat-2"),
        now=601,
    )
    assert first.records()[-1]["active_sessions"] == ["session-a", "session-b"]

    first.unregister_session("session-a")
    second.unregister_session("session-b")
    assert not first.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("heartbeat-3"),
        now=1_202,
    )


def test_last_record_time_never_moves_backwards(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    journal.register_session("session-a", "agent:test", now=100)
    journal.enqueue(_record("newer"), [], agent_id="agent:test", now=100)
    journal.enqueue(_record("older-clock"), [], agent_id="agent:test", now=50)

    assert not journal.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("heartbeat"),
        now=650,
    )


def test_heartbeat_transaction_rolls_back_when_record_building_fails(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    journal.register_session("session-a", "agent:test", now=1)

    def fail(sessions: list[str]) -> tuple[dict[str, Any], list[tuple[str, str]]]:
        raise RuntimeError("record construction failed")

    with pytest.raises(RuntimeError, match="construction"):
        journal.enqueue_heartbeat_if_due(
            "agent:test",
            interval_seconds=600,
            stale_after_seconds=1_800,
            record_factory=fail,
            now=601,
        )

    assert journal.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("heartbeat"),
        now=601,
    )


def test_session_state_changes_atomically_with_lifecycle_records(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    journal.enqueue(
        _record("start"),
        [],
        agent_id="agent:test",
        activate_session_id="session-a",
        now=1,
    )
    assert journal.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("heartbeat"),
        now=601,
    )

    journal.enqueue(
        _record("end"),
        [],
        agent_id="agent:test",
        deactivate_session_id="session-a",
        now=602,
    )
    assert not journal.enqueue_heartbeat_if_due(
        "agent:test",
        interval_seconds=600,
        stale_after_seconds=1_800,
        record_factory=_heartbeat_factory("later-heartbeat"),
        now=1_202,
    )


def test_rejects_private_evidence_with_the_wrong_digest(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    with pytest.raises(ValueError, match="does not match digest"):
        journal.enqueue(
            _record("record"),
            [("sha256:" + "0" * 64, "different payload")],
            agent_id="agent:test",
        )


def test_multiple_processes_can_enqueue_without_losing_records(tmp_path: Path) -> None:
    context = multiprocessing.get_context("spawn")
    processes = [
        context.Process(target=_enqueue_batch, args=(str(tmp_path), process_index, 20))
        for process_index in range(4)
    ]
    for process in processes:
        process.start()
    for process in processes:
        process.join(timeout=20)
        assert process.exitcode == 0

    records = Journal(tmp_path, max_record_bytes=10_000).records()
    assert len(records) == 80
    assert len({record["record_id"] for record in records}) == 80


@pytest.mark.skipif(os.name != "posix", reason="POSIX permissions are not available")
def test_journal_uses_private_filesystem_permissions(tmp_path: Path) -> None:
    state_dir = tmp_path / "state"
    journal = Journal(state_dir, max_record_bytes=10_000)
    journal.enqueue(_record("private"), [], agent_id="agent:test")

    assert stat.S_IMODE(state_dir.stat().st_mode) == 0o700
    assert stat.S_IMODE(journal.path.stat().st_mode) == 0o600
