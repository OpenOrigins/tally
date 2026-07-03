from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import socket
import subprocess
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


DEFAULT_LOG_ROOT = "/var/log/tally-codex"
DEFAULT_WORKSPACE = "/workspace"
MAX_HASH_BYTES = int(os.environ.get("TALLY_MAX_HASH_BYTES", str(25 * 1024 * 1024)))


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def safe_slug(value: str, *, default: str = "value") -> str:
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip("_")
    return (slug[:96] or default)


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sha256_hex(value: Any) -> str:
    if isinstance(value, bytes):
        data = value
    elif isinstance(value, str):
        data = value.encode("utf-8", errors="replace")
    else:
        data = canonical_bytes(value)
    return "sha256:" + hashlib.sha256(data).hexdigest()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    size = path.stat().st_size
    if size > MAX_HASH_BYTES:
        return sha256_hex({"path": str(path), "size": size, "hash_skipped": "file-too-large"})
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def write_json_atomic(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + f".tmp-{os.getpid()}-{uuid.uuid4().hex[:8]}")
    tmp.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False), encoding="utf-8")
    tmp.replace(path)


def append_jsonl_locked(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.with_suffix(path.suffix + ".lock")
    with lock_path.open("a", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(value, sort_keys=True, ensure_ascii=False) + "\n")


def run_command(argv: list[str], *, cwd: Path | None = None, timeout: float = 3.0) -> dict[str, Any]:
    started = time.time()
    try:
        proc = subprocess.run(
            argv,
            cwd=str(cwd) if cwd else None,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
        return {
            "argv": argv,
            "exit_code": proc.returncode,
            "duration_ms": int((time.time() - started) * 1000),
            "stdout": proc.stdout[-20000:],
            "stderr": proc.stderr[-20000:],
        }
    except Exception as exc:
        return {
            "argv": argv,
            "exit_code": None,
            "duration_ms": int((time.time() - started) * 1000),
            "error": repr(exc),
        }


def scrub_environment(env: dict[str, str] | None = None) -> dict[str, str]:
    env = env or os.environ
    allowed_exact = {
        "CODEX_HOME",
        "HOME",
        "HOSTNAME",
        "LANG",
        "LC_ALL",
        "LOGNAME",
        "PATH",
        "PWD",
        "SHELL",
        "TERM",
        "USER",
    }
    denied_fragments = ("KEY", "TOKEN", "SECRET", "PASSWORD", "COOKIE", "AUTH", "CREDENTIAL")
    result: dict[str, str] = {}
    for key, value in sorted(env.items()):
        upper = key.upper()
        if any(fragment in upper for fragment in denied_fragments):
            continue
        if key in allowed_exact or key.startswith("TALLY_") or key.startswith("CODEX_"):
            result[key] = value[:500]
    return result


def workspace_path() -> Path:
    return Path(os.environ.get("TALLY_WORKSPACE", DEFAULT_WORKSPACE)).resolve()


def log_root() -> Path:
    return Path(os.environ.get("TALLY_LOG_ROOT", DEFAULT_LOG_ROOT)).resolve()


def run_id() -> str:
    value = os.environ.get("TALLY_RUN_ID")
    if value:
        return safe_slug(value, default="run")
    return safe_slug(f"run_{socket.gethostname()}_{os.getuid()}", default="run")


def light_git_state(cwd: Path | None = None) -> dict[str, Any]:
    cwd = cwd or workspace_path()
    if not (cwd / ".git").exists():
        return {"is_git_repo": False}
    head = run_command(["git", "rev-parse", "--verify", "HEAD"], cwd=cwd, timeout=2.0)
    branch = run_command(["git", "branch", "--show-current"], cwd=cwd, timeout=2.0)
    status = run_command(["git", "status", "--short", "--branch"], cwd=cwd, timeout=3.0)
    return {
        "is_git_repo": True,
        "head": (head.get("stdout") or "").strip(),
        "branch": (branch.get("stdout") or "").strip(),
        "status_hash": sha256_hex(status.get("stdout", "")),
        "status": status.get("stdout", "")[-20000:],
    }


class AuditSink:
    def __init__(self, source: str) -> None:
        self.source = safe_slug(source)
        self.root = log_root()
        self.run_id = run_id()
        self.workspace = workspace_path()
        self.jsonl_dir = self.root / "jsonl"
        self.tally_dir = self.root / "tally" / self.source
        self.private_dir = self.root / "private" / self.run_id / self.source
        self.state_dir = self.root / "state"
        for path in (self.jsonl_dir, self.tally_dir, self.private_dir, self.state_dir):
            path.mkdir(parents=True, exist_ok=True)

    def next_sequence(self) -> int:
        counter = self.state_dir / f"{self.source}.counter"
        lock_path = self.state_dir / f"{self.source}.counter.lock"
        with lock_path.open("a", encoding="utf-8") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            if counter.exists():
                value = int(counter.read_text(encoding="utf-8").strip() or "0")
            else:
                value = 0
            value += 1
            counter.write_text(str(value), encoding="utf-8")
            return value

    def private_payload(self, label: str, payload: Any) -> dict[str, str]:
        event_label = safe_slug(label)
        path = self.private_dir / f"{event_label}.json"
        write_json_atomic(path, payload)
        return {
            "hash": sha256_hex(payload),
            "uri": f"private://{self.run_id}/{self.source}/{path.name}",
            "path": str(path),
        }

    def append_jsonl(self, stream_name: str, event: dict[str, Any]) -> None:
        append_jsonl_locked(self.jsonl_dir / f"{safe_slug(stream_name)}.jsonl", event)

    def write_tally_record(self, record: dict[str, Any]) -> Path:
        if "record_id" not in record:
            record["record_id"] = "rec_" + uuid.uuid4().hex[:16]
        if "schema_version" not in record:
            record["schema_version"] = "0.2-mvp"
        record.setdefault("run_id", self.run_id)
        seq = self.next_sequence()
        name = f"{seq:06d}_{safe_slug(record.get('record_type', 'RECORD'))}_{safe_slug(record['record_id'])}.json"
        path = self.tally_dir / name
        write_json_atomic(path, record)
        return path

    def emit_heartbeat(
        self,
        *,
        active_sessions: list[str] | None = None,
        agent_id: str | None = None,
        jsonl_stream: str | None = None,
        extra: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        timestamp = utc_now()
        event = {
            "schema_version": "tally-codex-container.v1",
            "run_id": self.run_id,
            "source": self.source,
            "event_type": "heartbeat",
            "observed_at": timestamp,
            "workspace": str(self.workspace),
        }
        if extra:
            event.update(extra)
        self.append_jsonl(jsonl_stream or self.source, event)
        self.write_tally_record(
            {
                "record_type": "HEARTBEAT",
                "schema_version": "0.2-mvp",
                "session_id": self.run_id,
                "agent_id": agent_id or os.environ.get("TALLY_AGENT_ID", "codex-container"),
                "active_sessions": active_sessions or [self.run_id],
                "timestamp": timestamp,
                "source": self.source,
                "metadata": extra or {},
            }
        )
        return event

    def emit_observation(
        self,
        *,
        event_type: str,
        payload: Any,
        record_type: str,
        extra: dict[str, Any] | None = None,
        jsonl_stream: str | None = None,
    ) -> dict[str, Any]:
        event_id = "evt_" + uuid.uuid4().hex[:16]
        ref = self.private_payload(event_id, payload)
        event = {
            "schema_version": "tally-codex-container.v1",
            "event_id": event_id,
            "run_id": self.run_id,
            "source": self.source,
            "event_type": event_type,
            "observed_at": utc_now(),
            "cwd": os.getcwd(),
            "workspace": str(self.workspace),
            "payload_hash": ref["hash"],
            "payload_uri": ref["uri"],
        }
        if extra:
            event.update(extra)
        self.append_jsonl(jsonl_stream or self.source, event)
        tally_record = {
            "record_type": record_type,
            "record_id": "rec_" + event_id.removeprefix("evt_"),
            "schema_version": "0.2-mvp",
            "session_id": self.run_id,
            "event_id": event_id,
            "source": self.source,
            "event_type": event_type,
            "observed_at": event["observed_at"],
            "payload_hash": ref["hash"],
            "payload_uri": ref["uri"],
            "workspace": str(self.workspace),
        }
        if extra:
            tally_record["metadata"] = extra
        self.write_tally_record(tally_record)
        return event
