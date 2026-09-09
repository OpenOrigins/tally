"""A durable, cross-process SQLite outbox for Tally records."""

from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import time
import uuid
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_SCHEMA = """
CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS evidence (
    digest TEXT PRIMARY KEY,
    payload TEXT NOT NULL,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS outbox (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    record_id TEXT NOT NULL UNIQUE,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'sending', 'delivered', 'dead_letter')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at REAL NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at REAL,
    last_error TEXT,
    created_at REAL NOT NULL,
    completed_at REAL
);

CREATE INDEX IF NOT EXISTS idx_outbox_status_sequence
    ON outbox(status, sequence);

CREATE TABLE IF NOT EXISTS record_evidence (
    record_id TEXT NOT NULL REFERENCES outbox(record_id) ON DELETE CASCADE,
    digest TEXT NOT NULL REFERENCES evidence(digest),
    PRIMARY KEY (record_id, digest)
);

CREATE TABLE IF NOT EXISTS delivery_outcomes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sequence INTEGER NOT NULL,
    record_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('delivered', 'dead_letter')),
    receipt TEXT,
    detail TEXT,
    recorded_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS active_sessions (
    session_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    last_seen_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_active_sessions_agent
    ON active_sessions(agent_id, last_seen_at);
"""


@dataclass(frozen=True, slots=True)
class OutboxItem:
    sequence: int
    record_id: str
    record: dict[str, Any]
    attempts: int
    lease_owner: str


class Journal:
    """Store records before delivery and preserve capture order across processes."""

    def __init__(self, state_dir: Path, *, max_record_bytes: int) -> None:
        self.state_dir = Path(state_dir)
        self.path = self.state_dir / "journal.sqlite3"
        self.max_record_bytes = max_record_bytes
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self._secure_path(self.state_dir, 0o700)
        self._initialize_schema()
        self._secure_sqlite_files()

    def _initialize_schema(self) -> None:
        deadline = time.monotonic() + 10
        while True:
            try:
                with self._connect() as connection:
                    connection.execute("PRAGMA journal_mode=WAL")
                    connection.executescript(_SCHEMA)
                return
            except sqlite3.OperationalError as error:
                if "locked" not in str(error).lower() or time.monotonic() >= deadline:
                    raise
                time.sleep(0.01)

    @staticmethod
    def _secure_path(path: Path, mode: int) -> None:
        if os.name == "posix":
            try:
                path.chmod(mode)
            except FileNotFoundError:
                # SQLite may remove its transient -wal/-shm file between exists() and chmod().
                pass

    def _secure_sqlite_files(self) -> None:
        for suffix in ("", "-shm", "-wal"):
            path = Path(f"{self.path}{suffix}")
            if path.exists():
                self._secure_path(path, 0o600)

    @contextmanager
    def _connect(self) -> Iterator[sqlite3.Connection]:
        connection = sqlite3.connect(self.path, timeout=10)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA busy_timeout=10000")
        connection.execute("PRAGMA foreign_keys=ON")
        connection.execute("PRAGMA synchronous=FULL")
        try:
            yield connection
            connection.commit()
        except BaseException:
            connection.rollback()
            raise
        finally:
            connection.close()
            self._secure_sqlite_files()

    def get_or_create_metadata(self, key: str, factory: Callable[[], str]) -> str:
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute("SELECT value FROM metadata WHERE key = ?", (key,)).fetchone()
            if row is not None:
                return str(row["value"])
            value = str(factory())
            connection.execute("INSERT INTO metadata (key, value) VALUES (?, ?)", (key, value))
            return value

    def enqueue(
        self,
        record: dict[str, Any],
        evidence: list[tuple[str, str]],
        *,
        agent_id: str,
        activate_session_id: str | None = None,
        deactivate_session_id: str | None = None,
        now: float | None = None,
    ) -> int:
        payload, record_id = self._prepare_record(record, evidence)
        created_at = time.time() if now is None else now
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            sequence = self._insert_record(
                connection,
                payload=payload,
                record_id=record_id,
                evidence=evidence,
                agent_id=agent_id,
                created_at=created_at,
            )
            if activate_session_id is not None:
                connection.execute(
                    """INSERT INTO active_sessions (session_id, agent_id, last_seen_at)
                       VALUES (?, ?, ?)
                       ON CONFLICT(session_id) DO UPDATE SET
                           agent_id = excluded.agent_id,
                           last_seen_at = excluded.last_seen_at""",
                    (activate_session_id, agent_id, created_at),
                )
            if deactivate_session_id is not None:
                connection.execute(
                    "DELETE FROM active_sessions WHERE session_id = ? AND agent_id = ?",
                    (deactivate_session_id, agent_id),
                )
            return sequence

    def _prepare_record(
        self,
        record: dict[str, Any],
        evidence: list[tuple[str, str]],
    ) -> tuple[str, str]:
        payload = json.dumps(
            record,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        payload_bytes = len(payload.encode("utf-8"))
        if payload_bytes > self.max_record_bytes:
            raise ValueError(
                f"record is {payload_bytes} bytes; maximum is {self.max_record_bytes} bytes"
            )
        record_id = record.get("record_id")
        if not isinstance(record_id, str) or not record_id:
            raise ValueError("record requires a non-empty record_id")
        for digest, private_payload in evidence:
            expected = f"sha256:{hashlib.sha256(private_payload.encode('utf-8')).hexdigest()}"
            if digest != expected:
                raise ValueError(f"private evidence payload does not match digest {digest}")
        return payload, record_id

    @staticmethod
    def _insert_record(
        connection: sqlite3.Connection,
        *,
        payload: str,
        record_id: str,
        evidence: list[tuple[str, str]],
        agent_id: str,
        created_at: float,
    ) -> int:
        for digest, private_payload in evidence:
            connection.execute(
                "INSERT OR IGNORE INTO evidence (digest, payload, created_at) VALUES (?, ?, ?)",
                (digest, private_payload, created_at),
            )
        cursor = connection.execute(
            "INSERT INTO outbox (record_id, payload, created_at) VALUES (?, ?, ?)",
            (record_id, payload, created_at),
        )
        for digest, _ in evidence:
            connection.execute(
                "INSERT OR IGNORE INTO record_evidence (record_id, digest) VALUES (?, ?)",
                (record_id, digest),
            )
        connection.execute(
            """INSERT INTO metadata (key, value) VALUES (?, ?)
               ON CONFLICT(key) DO UPDATE SET value =
                   CASE
                       WHEN CAST(metadata.value AS REAL) > CAST(excluded.value AS REAL)
                       THEN metadata.value
                       ELSE excluded.value
                   END""",
            (f"last_record_at:{agent_id}", str(created_at)),
        )
        if cursor.lastrowid is None:
            raise RuntimeError("SQLite did not return an outbox sequence")
        return cursor.lastrowid

    def register_session(self, session_id: str, agent_id: str, *, now: float | None = None) -> None:
        current = time.time() if now is None else now
        with self._connect() as connection:
            connection.execute(
                """INSERT INTO active_sessions (session_id, agent_id, last_seen_at)
                   VALUES (?, ?, ?)
                   ON CONFLICT(session_id) DO UPDATE SET
                       agent_id = excluded.agent_id,
                       last_seen_at = excluded.last_seen_at""",
                (session_id, agent_id, current),
            )

    def unregister_session(self, session_id: str) -> None:
        with self._connect() as connection:
            connection.execute("DELETE FROM active_sessions WHERE session_id = ?", (session_id,))

    def touch_sessions(self, session_ids: set[str], *, now: float | None = None) -> None:
        if not session_ids:
            return
        current = time.time() if now is None else now
        with self._connect() as connection:
            connection.executemany(
                "UPDATE active_sessions SET last_seen_at = ? WHERE session_id = ?",
                [(current, session_id) for session_id in session_ids],
            )

    def enqueue_heartbeat_if_due(
        self,
        agent_id: str,
        *,
        interval_seconds: float,
        stale_after_seconds: float,
        record_factory: Callable[[list[str]], tuple[dict[str, Any], list[tuple[str, str]]]],
        now: float | None = None,
    ) -> bool:
        """Atomically decide, build, and persist the next agent heartbeat."""

        current = time.time() if now is None else now
        key = f"last_record_at:{agent_id}"
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            connection.execute(
                "DELETE FROM active_sessions WHERE agent_id = ? AND last_seen_at < ?",
                (agent_id, current - stale_after_seconds),
            )
            sessions = [
                str(row["session_id"])
                for row in connection.execute(
                    "SELECT session_id FROM active_sessions WHERE agent_id = ? ORDER BY session_id",
                    (agent_id,),
                ).fetchall()
            ]
            if not sessions:
                return False
            row = connection.execute("SELECT value FROM metadata WHERE key = ?", (key,)).fetchone()
            last_record_at = float(row["value"]) if row is not None else 0.0
            if current - last_record_at < interval_seconds:
                return False
            record, evidence = record_factory(sessions)
            payload, record_id = self._prepare_record(record, evidence)
            self._insert_record(
                connection,
                payload=payload,
                record_id=record_id,
                evidence=evidence,
                agent_id=agent_id,
                created_at=current,
            )
            return True

    def claim(self, *, lease_seconds: float, now: float | None = None) -> OutboxItem | None:
        current = time.time() if now is None else now
        owner = uuid.uuid4().hex
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                """SELECT sequence, record_id, payload, status, attempts,
                          next_attempt_at, lease_expires_at
                   FROM outbox
                   WHERE status IN ('pending', 'sending')
                   ORDER BY sequence
                   LIMIT 1"""
            ).fetchone()
            if row is None:
                return None
            if row["status"] == "pending" and float(row["next_attempt_at"]) > current:
                return None
            if (
                row["status"] == "sending"
                and row["lease_expires_at"] is not None
                and float(row["lease_expires_at"]) > current
            ):
                return None

            attempts = int(row["attempts"]) + 1
            connection.execute(
                """UPDATE outbox
                   SET status = 'sending', attempts = ?, lease_owner = ?, lease_expires_at = ?
                   WHERE sequence = ?""",
                (attempts, owner, current + lease_seconds, int(row["sequence"])),
            )
            return OutboxItem(
                sequence=int(row["sequence"]),
                record_id=str(row["record_id"]),
                record=json.loads(str(row["payload"])),
                attempts=attempts,
                lease_owner=owner,
            )

    def mark_delivered(
        self,
        item: OutboxItem,
        *,
        receipt: Any = None,
        now: float | None = None,
    ) -> bool:
        return self._mark_terminal(item, "delivered", receipt=receipt, detail=None, now=now)

    def mark_dead_letter(
        self,
        item: OutboxItem,
        *,
        detail: str,
        now: float | None = None,
    ) -> bool:
        return self._mark_terminal(item, "dead_letter", receipt=None, detail=detail, now=now)

    def _mark_terminal(
        self,
        item: OutboxItem,
        status: str,
        *,
        receipt: Any,
        detail: str | None,
        now: float | None,
    ) -> bool:
        completed_at = time.time() if now is None else now
        receipt_json = None if receipt is None else json.dumps(receipt, separators=(",", ":"))
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            cursor = connection.execute(
                """UPDATE outbox
                   SET status = ?, completed_at = ?, lease_owner = NULL,
                       lease_expires_at = NULL, last_error = ?
                   WHERE sequence = ? AND status = 'sending' AND lease_owner = ?""",
                (status, completed_at, detail, item.sequence, item.lease_owner),
            )
            if cursor.rowcount != 1:
                return False
            connection.execute(
                """INSERT INTO delivery_outcomes
                   (sequence, record_id, status, receipt, detail, recorded_at)
                   VALUES (?, ?, ?, ?, ?, ?)""",
                (item.sequence, item.record_id, status, receipt_json, detail, completed_at),
            )
            return True

    def mark_retry(
        self,
        item: OutboxItem,
        *,
        detail: str,
        next_attempt_at: float,
    ) -> bool:
        with self._connect() as connection:
            cursor = connection.execute(
                """UPDATE outbox
                   SET status = 'pending', next_attempt_at = ?, lease_owner = NULL,
                       lease_expires_at = NULL, last_error = ?
                   WHERE sequence = ? AND status = 'sending' AND lease_owner = ?""",
                (next_attempt_at, detail, item.sequence, item.lease_owner),
            )
            return cursor.rowcount == 1

    def pending_count(self) -> int:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT COUNT(*) AS count FROM outbox WHERE status IN ('pending', 'sending')"
            ).fetchone()
            return int(row["count"])

    def records(self) -> list[dict[str, Any]]:
        """Return locally stored records in capture order (primarily for diagnostics/tests)."""

        with self._connect() as connection:
            rows = connection.execute("SELECT payload FROM outbox ORDER BY sequence").fetchall()
            return [json.loads(str(row["payload"])) for row in rows]

    def statuses(self) -> list[str]:
        with self._connect() as connection:
            rows = connection.execute("SELECT status FROM outbox ORDER BY sequence").fetchall()
            return [str(row["status"]) for row in rows]

    def evidence_payload(self, digest: str) -> str | None:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT payload FROM evidence WHERE digest = ?", (digest,)
            ).fetchone()
            return None if row is None else str(row["payload"])

    def prune_delivered(self, *, before: float) -> int:
        """Remove old delivered payloads while retaining lightweight delivery outcomes."""

        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            cursor = connection.execute(
                "DELETE FROM outbox WHERE status = 'delivered' AND completed_at < ?", (before,)
            )
            connection.execute(
                "DELETE FROM evidence WHERE digest NOT IN (SELECT digest FROM record_evidence)"
            )
            return cursor.rowcount
