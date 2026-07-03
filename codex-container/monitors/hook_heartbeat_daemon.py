#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import signal
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from lib.tally_audit import AuditSink, utc_now, write_json_atomic


def parse_time(value: str) -> float:
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    except Exception:
        return time.time()


class HookHeartbeatDaemon:
    def __init__(self) -> None:
        self.sink = AuditSink("hook-heartbeat")
        self.state_path = self.sink.state_dir / f"hook-heartbeat.{self.sink.run_id}.json"
        self.pid_path = self.sink.state_dir / f"hook-heartbeat.{self.sink.run_id}.pid"
        self.interval = float(
            os.environ.get("TALLY_HOOK_HEARTBEAT_SECONDS")
            or os.environ.get("TALLY_HEARTBEAT_SECONDS")
            or "60"
        )
        self.idle_timeout = float(os.environ.get("TALLY_HOOK_HEARTBEAT_IDLE_SECONDS", "300"))
        self.stop_requested = False

    def load_state(self) -> dict[str, Any]:
        try:
            return json.loads(self.state_path.read_text(encoding="utf-8"))
        except Exception:
            return {}

    def write_daemon_status(self, status: str) -> None:
        write_json_atomic(
            self.sink.state_dir / f"hook-heartbeat.{self.sink.run_id}.daemon.json",
            {
                "pid": os.getpid(),
                "status": status,
                "updated_at": utc_now(),
                "state_path": str(self.state_path),
            },
        )

    def run(self) -> int:
        self.pid_path.write_text(str(os.getpid()), encoding="utf-8")
        self.write_daemon_status("started")
        while not self.stop_requested:
            state = self.load_state()
            if state.get("stop_requested"):
                break
            last_update = parse_time(state.get("updated_at", utc_now()))
            if time.time() - last_update > self.idle_timeout:
                self.sink.emit_heartbeat(
                    active_sessions=[state.get("session_id") or self.sink.run_id],
                    jsonl_stream="hook-heartbeat",
                    extra={
                        "heartbeat_kind": "hook-daemon-timeout",
                        "last_hook_event": state.get("last_hook_event"),
                        "last_hook_observed_at": state.get("updated_at"),
                    },
                )
                break
            self.sink.emit_heartbeat(
                active_sessions=[state.get("session_id") or self.sink.run_id],
                jsonl_stream="hook-heartbeat",
                extra={
                    "heartbeat_kind": "hook-daemon",
                    "last_hook_event": state.get("last_hook_event"),
                    "last_hook_observed_at": state.get("updated_at"),
                },
            )
            time.sleep(self.interval)
        self.write_daemon_status("stopped")
        try:
            self.pid_path.unlink()
        except FileNotFoundError:
            pass
        return 0


def main() -> int:
    daemon = HookHeartbeatDaemon()

    def stop(_signum: int, _frame: Any) -> None:
        daemon.stop_requested = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    return daemon.run()


if __name__ == "__main__":
    raise SystemExit(main())
