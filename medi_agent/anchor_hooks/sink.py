"""Writes anchor records to logs/log.jsonl and logs/log.sqlite.

Both files are append-only and are the shared surface that the heartbeat
daemon and the log forwarder daemon also read/write. Writers take an
in-process thread lock AND a cross-process advisory file lock (fcntl.flock),
because the deployment model of the client's agent (single worker vs.
multi-worker/multi-process server) isn't known -- without the file lock,
concurrent processes appending to the same log.jsonl can interleave partial
lines, and concurrent SQLite writers can hit "database is locked" errors.
SQLite is additionally put in WAL mode with a busy_timeout so a writer that
loses the race retries instead of failing outright.
"""
from __future__ import annotations

import json
import sqlite3
import threading
from pathlib import Path
from typing import Any, Dict

try:
    import fcntl
except ImportError:  # pragma: no cover -- non-POSIX platform
    fcntl = None

_LOCK = threading.Lock()

_CREATE_TABLE_SQL = """
CREATE TABLE IF NOT EXISTS anchor_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    record_type TEXT NOT NULL,
    session_id TEXT,
    tool_name TEXT,
    recorded_at TEXT NOT NULL,
    payload TEXT NOT NULL
)
"""

_TIMESTAMP_KEYS = (
    "session_started_at",
    "instruction_received_at",
    "action_timestamp",
    "result_received_at",
    "handoff_at",
    "session_ended_at",
    "timestamp",
)


class LogSink:
    def __init__(self, config):
        self.config = config
        Path(self.config.log_dir).mkdir(parents=True, exist_ok=True)
        with self._sqlite_connect() as conn:
            conn.execute(_CREATE_TABLE_SQL)

    def _sqlite_connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.config.log_sqlite_path, timeout=10)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA busy_timeout=5000")
        return conn

    def write(self, record: Dict[str, Any]) -> None:
        line = json.dumps(record, separators=(",", ":"))
        tool_name = None
        if record.get("record_type") == "ACTION_TAKEN":
            tool_name = (record.get("tool") or {}).get("name")
        recorded_at = next((record[k] for k in _TIMESTAMP_KEYS if k in record), "")

        with _LOCK:
            with open(self.config.log_jsonl_path, "a") as f:
                if fcntl is not None:
                    fcntl.flock(f.fileno(), fcntl.LOCK_EX)
                try:
                    f.write(line + "\n")
                finally:
                    if fcntl is not None:
                        fcntl.flock(f.fileno(), fcntl.LOCK_UN)
            with self._sqlite_connect() as conn:
                conn.execute(
                    "INSERT INTO anchor_log (record_type, session_id, tool_name, recorded_at, payload) "
                    "VALUES (?, ?, ?, ?, ?)",
                    (record.get("record_type"), record.get("session_id"), tool_name, recorded_at, line),
                )
