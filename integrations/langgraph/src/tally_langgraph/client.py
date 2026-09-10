"""High-level client that records locally first and delivers in the background."""

from __future__ import annotations

import hashlib
import logging
import threading
import time
import uuid
from datetime import timedelta
from typing import Any

from . import records
from .config import TallyConfig
from .journal import Journal, OutboxItem
from .transport import DeliveryResult, HttpTransport, Transport

logger = logging.getLogger("tally_langgraph")

_RECORD_TYPES = {
    "ACTION_TAKEN",
    "HANDOFF",
    "HEARTBEAT",
    "INSTRUCTION_RECEIVED",
    "RESULT_RECEIVED",
    "SESSION_END",
    "SESSION_START",
    "TURN_END",
}


class TallyClient:
    """Capture Tally records and deliver them from a durable local outbox.

    A client is safe to share across requests in one process. Create a fresh
    callback handler with :meth:`callback` for each concurrent graph invocation.
    """

    def __init__(
        self,
        config: TallyConfig | None = None,
        *,
        transport: Transport | None = None,
        background: bool = True,
    ) -> None:
        self.config = config or TallyConfig.from_env()
        self.journal = Journal(
            self.config.state_dir,
            max_record_bytes=self.config.max_record_bytes,
        )
        self.agent_id = self.config.agent_id or self.journal.get_or_create_metadata(
            "agent_id", lambda: f"agent:{uuid.uuid4()}"
        )
        self.anchor_instance_id = self.journal.get_or_create_metadata(
            "anchor_instance_id", lambda: f"anchor:{uuid.uuid4()}"
        )
        self.transport = transport or HttpTransport(self.config)
        self._owned_sessions: set[str] = set()
        self._sessions_lock = threading.RLock()
        self._state_lock = threading.RLock()
        self._close_lock = threading.Lock()
        self._stop = threading.Event()
        self._wake = threading.Event()
        self._worker: threading.Thread | None = None
        self._closed = False
        self.journal.prune_delivered(
            before=time.time()
            - timedelta(days=self.config.delivered_retention_days).total_seconds()
        )
        if background:
            self._worker = threading.Thread(
                target=self._worker_loop,
                name="tally-langgraph-forwarder",
                daemon=True,
            )
            self._worker.start()

    @classmethod
    def from_env(cls, **kwargs: Any) -> TallyClient:
        return cls(TallyConfig.from_env(), **kwargs)

    def callback(self, *, source: str = "langgraph") -> Any:
        """Return a callback handler for one graph invocation."""

        from .callback import TallyCallbackHandler

        return TallyCallbackHandler(self, source=source)

    def _enqueue(
        self,
        record: dict[str, Any],
        evidence: records.Evidence,
        *,
        activate_session_id: str | None = None,
        deactivate_session_id: str | None = None,
    ) -> int:
        with self._state_lock:
            if self._closed:
                raise RuntimeError("TallyClient is closed")
            _validate_record(record)
            sequence = self.journal.enqueue(
                record,
                evidence,
                agent_id=self.agent_id,
                activate_session_id=activate_session_id,
                deactivate_session_id=deactivate_session_id,
            )
        self._wake.set()
        return sequence

    def start_session(self, session_id: str, *, source: str) -> None:
        record, evidence = records.session_start(
            session_id=session_id,
            agent_id=self.agent_id,
            config=self.config,
            source=source,
        )
        with self._state_lock:
            self._enqueue(
                record,
                evidence,
                activate_session_id=session_id,
            )
            with self._sessions_lock:
                self._owned_sessions.add(session_id)

    def record_instruction(
        self,
        session_id: str,
        instruction_id: str,
        value: Any,
        *,
        context: Any,
    ) -> None:
        self._enqueue(
            *records.instruction_received(
                session_id=session_id,
                instruction_id=instruction_id,
                value=value,
                context=context,
                config=self.config,
            )
        )

    def record_action(
        self,
        session_id: str,
        instruction_id: str,
        action_id: str,
        *,
        tool_server: str,
        tool_name: str,
        params: Any,
    ) -> None:
        self._enqueue(
            *records.action_taken(
                session_id=session_id,
                instruction_id=instruction_id,
                action_id=action_id,
                tool_server=tool_server,
                tool_name=tool_name,
                params=params,
                config=self.config,
            )
        )

    def record_result(
        self,
        session_id: str,
        action_id: str,
        value: Any,
        *,
        error: BaseException | None = None,
    ) -> None:
        self._enqueue(
            *records.result_received(
                session_id=session_id,
                action_id=action_id,
                value=value,
                error=error,
                config=self.config,
            )
        )

    def record_handoff(
        self,
        session_id: str,
        *,
        receiving_agent: str,
        payload: Any,
        handoff_id: str | None = None,
        acknowledgement_status: str = "pending",
    ) -> str:
        if not isinstance(receiving_agent, str) or not receiving_agent:
            raise ValueError("receiving_agent must be a non-empty string")
        if not isinstance(acknowledgement_status, str) or acknowledgement_status not in {
            "pending",
            "acknowledged",
            "rejected",
            "timeout",
        }:
            raise ValueError("invalid acknowledgement_status")
        if handoff_id is not None and (not isinstance(handoff_id, str) or not handoff_id):
            raise ValueError("handoff_id must be a non-empty string or None")
        resolved_id = handoff_id if handoff_id is not None else f"handoff_{uuid.uuid4().hex}"
        self._enqueue(
            *records.handoff(
                session_id=session_id,
                handoff_id=resolved_id,
                agent_id=self.agent_id,
                receiving_agent=receiving_agent,
                payload=payload,
                acknowledgement_status=acknowledgement_status,
            )
        )
        return resolved_id

    def end_turn(
        self,
        session_id: str,
        turn_id: str,
        *,
        outcome: str,
        value: Any,
    ) -> None:
        if outcome not in {"completed", "failed", "interrupted"}:
            raise ValueError("invalid turn outcome")
        self._enqueue(
            *records.turn_end(
                session_id=session_id,
                turn_id=turn_id,
                outcome=outcome,
                value=value,
                config=self.config,
            )
        )

    def end_session(
        self,
        session_id: str,
        *,
        outcome: str,
        value: Any,
        token_usage: dict[str, int] | None = None,
    ) -> None:
        if outcome not in {"success", "failure", "partial", "interrupted"}:
            raise ValueError("invalid session outcome")
        record, evidence = records.session_end(
            session_id=session_id,
            outcome=outcome,
            value=value,
            token_usage=token_usage,
        )
        with self._state_lock:
            self._enqueue(
                record,
                evidence,
                deactivate_session_id=session_id,
            )
            with self._sessions_lock:
                self._owned_sessions.discard(session_id)

    def drain_once(self) -> bool:
        """Attempt one ordered delivery. Return whether a record was claimed."""

        item = self.journal.claim(lease_seconds=self.config.claim_lease_seconds)
        if item is None:
            return False
        try:
            result = self.transport.deliver(item.record_id, item.record)
        except Exception as error:
            logger.exception("Unexpected Tally transport failure")
            result = DeliveryResult("retry", f"transport raised {type(error).__name__}: {error}")
        self._apply_delivery_result(item, result)
        return True

    def _apply_delivery_result(self, item: OutboxItem, result: DeliveryResult) -> None:
        if result.disposition == "delivered":
            self.journal.mark_delivered(item, receipt=result.receipt)
            return
        detail = (result.detail or "delivery failed")[:2_048]
        if result.disposition == "dead_letter":
            self.journal.mark_dead_letter(item, detail=detail)
            return
        delay = result.retry_after_seconds
        if delay is None:
            delay = self._retry_delay(item)
        self.journal.mark_retry(item, detail=detail, next_attempt_at=time.time() + delay)

    def _retry_delay(self, item: OutboxItem) -> float:
        exponent = min(max(item.attempts - 1, 0), 16)
        base = min(self.config.retry_max_seconds, self.config.retry_base_seconds * (2**exponent))
        digest = hashlib.sha256(item.record_id.encode("utf-8")).digest()
        jitter = 0.75 + (int.from_bytes(digest[:2], "big") / 65_535) * 0.5
        return float(min(self.config.retry_max_seconds, base * jitter))

    def _worker_loop(self) -> None:
        last_session_touch = 0.0
        while not self._stop.is_set():
            try:
                now = time.time()
                if now - last_session_touch >= 30:
                    with self._sessions_lock:
                        sessions = set(self._owned_sessions)
                    self.journal.touch_sessions(sessions, now=now)
                    last_session_touch = now
                self._maybe_emit_heartbeat(now=now)
                if self.drain_once():
                    continue
            except Exception:
                logger.exception("Tally background worker iteration failed")
            self._wake.wait(self.config.worker_poll_seconds)
            self._wake.clear()

    def _maybe_emit_heartbeat(self, *, now: float | None = None) -> bool:
        with self._state_lock:
            if self._closed:
                return False
            enqueued = self.journal.enqueue_heartbeat_if_due(
                self.agent_id,
                interval_seconds=self.config.heartbeat_interval_seconds,
                stale_after_seconds=max(self.config.heartbeat_interval_seconds * 3, 1_800),
                record_factory=lambda sessions: records.heartbeat(
                    agent_id=self.agent_id,
                    anchor_instance_id=self.anchor_instance_id,
                    active_sessions=sessions,
                ),
                now=now,
            )
        if enqueued:
            self._wake.set()
        return enqueued

    def flush(self, timeout: float = 5.0) -> bool:
        """Try to deliver pending records until empty or until ``timeout`` expires."""

        deadline = time.monotonic() + max(timeout, 0)
        self._wake.set()
        while self.journal.pending_count() and time.monotonic() < deadline:
            if not self.drain_once():
                time.sleep(min(0.05, max(deadline - time.monotonic(), 0)))
        return self.journal.pending_count() == 0

    def close(self, *, flush: bool = True, timeout: float = 5.0) -> bool:
        with self._close_lock:
            with self._state_lock:
                if self._closed:
                    return self.journal.pending_count() == 0
                self._closed = True
            self._stop.set()
            self._wake.set()
            started = time.monotonic()
            if self._worker is not None:
                self._worker.join(timeout=min(timeout, 2.0))
            remaining = max(timeout - (time.monotonic() - started), 0)
            if flush:
                return self.flush(timeout=remaining)
            return self.journal.pending_count() == 0

    def __enter__(self) -> TallyClient:
        return self

    def __exit__(self, exc_type: Any, exc: Any, traceback: Any) -> None:
        self.close()


def _validate_record(record: dict[str, Any]) -> None:
    if record.get("record_type") not in _RECORD_TYPES:
        raise ValueError(f"unsupported record_type {record.get('record_type')!r}")
    if record.get("schema_version") != "0.2":
        raise ValueError("records must use schema_version 0.2")
    record_id = record.get("record_id")
    if not isinstance(record_id, str) or not record_id:
        raise ValueError("records require a non-empty record_id")

    stack: list[Any] = [record]
    visited = 0
    while stack:
        value = stack.pop()
        visited += 1
        if visited > 100_000:
            raise ValueError("record contains too many values")
        if isinstance(value, dict):
            for key, digest in value.items():
                if key.endswith("_uri"):
                    stem = key[: -len("_uri")]
                    if (
                        isinstance(digest, str)
                        and digest.startswith("private://sha256/")
                        and f"{stem}_hash" not in value
                    ):
                        raise ValueError(f"{key} is missing its matching {stem}_hash field")
                if not key.endswith("_hash"):
                    continue
                stem = key[: -len("_hash")]
                if f"{stem}_uri" not in value:
                    continue
                uri = value[f"{stem}_uri"]
                if digest is None and uri is None:
                    continue
                if not isinstance(digest, str) or not isinstance(uri, str):
                    raise ValueError(f"{key} and {stem}_uri must both be strings or null")
                hash_value = digest.removeprefix("sha256:")
                uri_value = uri.removeprefix("private://sha256/")
                if (
                    hash_value == digest
                    or uri_value == uri
                    or hash_value != uri_value
                    or len(hash_value) != 64
                    or any(character not in "0123456789abcdefABCDEF" for character in hash_value)
                ):
                    raise ValueError(f"invalid evidence pair {key}/{stem}_uri")
            stack.extend(value.values())
        elif isinstance(value, list):
            stack.extend(value)
