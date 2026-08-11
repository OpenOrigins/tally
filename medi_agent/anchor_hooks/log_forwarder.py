"""Singleton log forwarder daemon (one process, not per-session).

Spawned opportunistically by AnchorSession._start_forwarder_daemon on
SESSION_START if one isn't already running (tracked via forwarder.pid), and
keeps running independently of any single session, up to a 24h safety cap.

Tails logs/log.jsonl from a persisted byte offset (forwarder_offset.txt) and
POSTs each JSON record, one at a time and in file order, to the Tally ingest
API. The API key is re-read from disk on every drain cycle (not cached at
startup) so rotating the key file takes effect without restarting the daemon.
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import requests

try:
    import fcntl
except ImportError:  # pragma: no cover -- non-POSIX platform
    fcntl = None

from .config import load_config

DRAIN_INTERVAL_SECONDS = 5
REQUEST_TIMEOUT_SECONDS = 10


def _read_api_key(path: Path):
    if not path.exists():
        return None
    key = path.read_text().strip()
    return key or None


def _read_offset(path: Path) -> int:
    if not path.exists():
        return 0
    try:
        return int(path.read_text().strip() or 0)
    except ValueError:
        return 0


def _write_offset(path: Path, offset: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(str(offset))


def _read_complete_lines(log_path: Path, offset: int):
    """Read whole lines starting at byte `offset`, under a shared lock so we
    don't read a line the sink is mid-write on. Returns (lines, new_offset),
    where `lines` excludes a trailing partial line (no b"\\n" yet) so the
    caller never has to guess whether the last line was fully written.
    """
    with log_path.open("rb") as f:
        if fcntl is not None:
            fcntl.flock(f.fileno(), fcntl.LOCK_SH)
        try:
            f.seek(offset)
            chunk = f.read()
        finally:
            if fcntl is not None:
                fcntl.flock(f.fileno(), fcntl.LOCK_UN)

    if not chunk:
        return [], offset

    complete_len = chunk.rfind(b"\n") + 1  # 0 if no newline found yet
    # split on the literal separator the sink wrote ("\n"), not str.splitlines()
    # (which also breaks on \r, \x1c-\x1e, U+2028, ... and would desync byte offsets)
    lines = chunk[:complete_len].decode("utf-8").split("\n")[:-1]
    return lines, offset + complete_len


def _drain_once(config) -> None:
    api_key = _read_api_key(Path(config.forwarder.api_key_file))
    if not api_key:
        print("[log_forwarder] no API key file found, skipping drain", flush=True)
        return

    log_path = config.log_jsonl_path
    if not log_path.exists():
        return

    offset = _read_offset(config.forwarder_offset_path)
    lines, _ = _read_complete_lines(log_path, offset)

    consumed_through = offset
    for line in lines:
        line_end = consumed_through + len(line.encode("utf-8")) + 1  # +1 for the newline
        stripped = line.strip()
        if not stripped:
            consumed_through = line_end
            continue
        try:
            record = json.loads(stripped)
        except json.JSONDecodeError:
            print(f"[log_forwarder] dropping malformed line at offset {consumed_through}", flush=True)
            consumed_through = line_end
            continue
        try:
            requests.post(
                config.forwarder.api_url,
                json=record,
                headers={
                    "Content-Type": "application/json",
                    "x-api-key": api_key,
                    "x-oo-tally-source": "sdk",
                    "x-oo-tally-ingest-path": "anchor-log-forwarder",
                },
                timeout=REQUEST_TIMEOUT_SECONDS,
            )
        except requests.RequestException as exc:
            print(f"[log_forwarder] POST failed, will retry next cycle: {exc}", flush=True)
            break  # stop this cycle; consumed_through is left before the failed record
        consumed_through = line_end
        _write_offset(config.forwarder_offset_path, consumed_through)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", default=None)
    args = parser.parse_args()

    config = load_config(args.config)
    deadline = time.monotonic() + config.forwarder.max_hours * 3600
    while time.monotonic() < deadline:
        _drain_once(config)
        time.sleep(DRAIN_INTERVAL_SECONDS)


if __name__ == "__main__":
    main()
