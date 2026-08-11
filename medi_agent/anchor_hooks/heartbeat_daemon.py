"""Standalone heartbeat daemon, one per session_id.

Spawned detached by AnchorSession._start_heartbeat_daemon on SESSION_START.
Appends a HEARTBEAT record every `heartbeat_interval_seconds` so gaps
between real actions still show periodic liveness instead of silence.
Stops when:
  - its own PID file is removed (AnchorSession._stop_heartbeat_daemon on
    SESSION_END unlinks it, and we notice within one interval), or
  - it receives SIGTERM (also sent by _stop_heartbeat_daemon), or
  - the 6-hour safety cap elapses, in case SESSION_END never fires (window
    closed, process killed) -- this avoids leaking an orphaned process.
"""
from __future__ import annotations

import argparse
import signal
import time
from datetime import datetime, timezone

from .config import load_config
from .sink import LogSink

_stop = False


def _handle_sigterm(signum, frame):
    global _stop
    _stop = True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--config", default=None)
    args = parser.parse_args()

    signal.signal(signal.SIGTERM, _handle_sigterm)

    config = load_config(args.config)
    sink = LogSink(config)
    pid_path = config.heartbeat_pid_path(args.session_id)

    deadline = time.monotonic() + config.heartbeat_max_hours * 3600
    while not _stop and time.monotonic() < deadline:
        if not pid_path.exists():
            break
        sink.write({
            "schema_version": config.schema_version,
            "record_type": "HEARTBEAT",
            "session_id": args.session_id,
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        })
        time.sleep(config.heartbeat_interval_seconds)


if __name__ == "__main__":
    main()
