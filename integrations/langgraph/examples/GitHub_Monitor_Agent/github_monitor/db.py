import json
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timedelta

from . import config

SCHEMA = """
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    event_type TEXT NOT NULL,
    action TEXT,
    repo TEXT,
    actor TEXT,
    ref TEXT,
    pr_number INTEGER,
    commit_sha TEXT,
    summary TEXT,
    risk_level TEXT,
    agent_response TEXT,
    ai_summary TEXT,
    raw_payload TEXT
);
CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
"""


@contextmanager
def get_conn():
    conn = sqlite3.connect(config.DB_PATH)
    conn.row_factory = sqlite3.Row
    try:
        yield conn
        conn.commit()
    finally:
        conn.close()


def init_db():
    with get_conn() as conn:
        conn.executescript(SCHEMA)
        # ai_summary was added after the initial schema -- backfill it on DBs created
        # before this column existed, since CREATE TABLE IF NOT EXISTS won't alter them.
        existing_cols = {row["name"] for row in conn.execute("PRAGMA table_info(events)")}
        if "ai_summary" not in existing_cols:
            conn.execute("ALTER TABLE events ADD COLUMN ai_summary TEXT")


def insert_event(
    event_type: str,
    action: str = None,
    repo: str = None,
    actor: str = None,
    ref: str = None,
    pr_number: int = None,
    commit_sha: str = None,
    summary: str = None,
    risk_level: str = None,
    agent_response: str = None,
    raw_payload: dict = None,
) -> int:
    """Deterministically append one row to the audit log. This always runs at the
    webhook layer, independent of whatever the LLM orchestrator later decides to do,
    so the event history is complete even if the agent's reasoning goes off track."""
    with get_conn() as conn:
        cur = conn.execute(
            """INSERT INTO events
               (ts, event_type, action, repo, actor, ref, pr_number, commit_sha,
                summary, risk_level, agent_response, raw_payload)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                datetime.utcnow().isoformat(),
                event_type, action, repo, actor, ref, pr_number, commit_sha,
                summary, risk_level, agent_response,
                json.dumps(raw_payload) if raw_payload is not None else None,
            ),
        )
        return cur.lastrowid


def update_event_agent_response(event_id: int, agent_response: str, risk_level: str = None):
    with get_conn() as conn:
        if risk_level is not None:
            conn.execute(
                "UPDATE events SET agent_response = ?, risk_level = ? WHERE id = ?",
                (agent_response, risk_level, event_id),
            )
        else:
            conn.execute(
                "UPDATE events SET agent_response = ? WHERE id = ?",
                (agent_response, event_id),
            )


def update_event_ai_summary(event_id: int, ai_summary: str):
    """Store the LLM-generated one-line summary of an event. This runs from the graph's
    entry node for every input, independent of whichever fixed pipeline branch runs next."""
    with get_conn() as conn:
        conn.execute("UPDATE events SET ai_summary = ? WHERE id = ?", (ai_summary, event_id))


def query_events(since_hours: int = 24, event_type: str = None, limit: int = 200):
    # since_hours <= 0 has no meaningful "since now" reading -- treat it as "no time
    # floor, just give me the most recent rows" rather than an always-empty window.
    # Small local models sometimes pass 0 to mean "right now / the latest", so this
    # keeps that intent from silently returning nothing.
    with get_conn() as conn:
        if since_hours and since_hours > 0:
            since = (datetime.utcnow() - timedelta(hours=since_hours)).isoformat()
            if event_type:
                rows = conn.execute(
                    "SELECT * FROM events WHERE ts >= ? AND event_type = ? ORDER BY ts DESC LIMIT ?",
                    (since, event_type, limit),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM events WHERE ts >= ? ORDER BY ts DESC LIMIT ?",
                    (since, limit),
                ).fetchall()
        else:
            if event_type:
                rows = conn.execute(
                    "SELECT * FROM events WHERE event_type = ? ORDER BY ts DESC LIMIT ?",
                    (event_type, limit),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM events ORDER BY ts DESC LIMIT ?",
                    (limit,),
                ).fetchall()
        return [dict(r) for r in rows]
