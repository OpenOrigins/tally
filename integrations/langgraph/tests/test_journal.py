from pathlib import Path

from tally_langgraph.journal import Journal


def _record(record_id: str) -> dict[str, str]:
    return {"record_id": record_id, "record_type": "SESSION_START", "schema_version": "0.2"}


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
    digest = "sha256:" + "a" * 64
    journal.enqueue(_record("one"), [(digest, '{"secret":1}')], agent_id="agent:test", now=1)
    assert journal.evidence_payload(digest) == '{"secret":1}'
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

    assert first.claim_heartbeat(
        "agent:test", interval_seconds=600, stale_after_seconds=1_800, now=601
    ) == ["session-a", "session-b"]
    assert (
        second.claim_heartbeat(
            "agent:test", interval_seconds=600, stale_after_seconds=1_800, now=601
        )
        == []
    )

    first.unregister_session("session-a")
    second.unregister_session("session-b")
    assert (
        first.claim_heartbeat(
            "agent:test", interval_seconds=600, stale_after_seconds=1_800, now=1_202
        )
        == []
    )


def test_last_record_time_never_moves_backwards(tmp_path: Path) -> None:
    journal = Journal(tmp_path, max_record_bytes=10_000)
    journal.register_session("session-a", "agent:test", now=100)
    journal.enqueue(_record("newer"), [], agent_id="agent:test", now=100)
    journal.enqueue(_record("older-clock"), [], agent_id="agent:test", now=50)

    assert (
        journal.claim_heartbeat(
            "agent:test",
            interval_seconds=600,
            stale_after_seconds=1_800,
            now=650,
        )
        == []
    )
