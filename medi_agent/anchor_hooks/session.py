"""AnchorSession: the single integration point a client wires into their
LangGraph app. Emits SESSION_START/SESSION_END around a graph run, exposes a
callback_handler for ACTION_TAKEN/RESULT_RECEIVED/HANDOFF, and manages the
per-session heartbeat daemon plus the singleton log forwarder daemon.
"""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import uuid
from contextlib import contextmanager
from datetime import datetime, timezone
from typing import Optional

from .callback_handler import AnchorCallbackHandler
from .config import AnchorConfig, load_config
from .schema import truncate
from .sink import LogSink

INSTRUCTION_TRUNCATE_LEN = 500


def _now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class AnchorSession:
    def __init__(
        self,
        config_path: Optional[str] = None,
        agent_id: Optional[str] = None,
        agent_version: Optional[str] = None,
        principal_id: Optional[str] = None,
        config: Optional[AnchorConfig] = None,
    ):
        self.config = config or load_config(config_path)
        if agent_id:
            self.config.agent_id = agent_id
        if agent_version:
            self.config.agent_version = agent_version
        if principal_id:
            self.config.principal_id = principal_id

        self.sink = LogSink(self.config)
        self.session_id = f"sess_{uuid.uuid4()}"
        self.callback_handler = AnchorCallbackHandler(self)
        self._instruction_id: Optional[str] = None

    # -- record emitters ----------------------------------------------------
    def emit_session_start(self, source: str) -> None:
        self.sink.write({
            "schema_version": self.config.schema_version,
            "record_type": "SESSION_START",
            "session_id": self.session_id,
            "agent_id": self.config.agent_id,
            "agent_version": self.config.agent_version,
            "principal": {"type": "user", "id": self.config.principal_id},
            "source": source,
            "session_started_at": _now_iso(),
        })

    def emit_instruction_received(self, text: str) -> str:
        self._instruction_id = f"instr_{uuid.uuid4().hex[:8]}"
        self.sink.write({
            "schema_version": self.config.schema_version,
            "record_type": "INSTRUCTION_RECEIVED",
            "session_id": self.session_id,
            "instruction_id": self._instruction_id,
            "sender": {"id": self.config.principal_id},
            "declared_intent": {"summary": truncate(text, INSTRUCTION_TRUNCATE_LEN)},
            "instruction_received_at": _now_iso(),
        })
        return self._instruction_id

    def emit_action_taken(self, action_id: str, tool_name: str, params: str) -> None:
        self.sink.write({
            "schema_version": self.config.schema_version,
            "record_type": "ACTION_TAKEN",
            "session_id": self.session_id,
            "action_id": action_id,
            "instruction_id": self._instruction_id,
            "action_type": "tool_call",
            "tool": {"name": tool_name, "params": params},
            "pre_state_hash": "0" * 64,
            "post_state_hash": None,
            "action_timestamp": _now_iso(),
            "deviance_flag": {"deviated": False, "delta_category": None},
        })

    def emit_result_received(self, action_id: str, summary: str, exception: bool) -> None:
        self.sink.write({
            "schema_version": self.config.schema_version,
            "record_type": "RESULT_RECEIVED",
            "session_id": self.session_id,
            "action_id": action_id,
            "result_interpretation": {"summary": summary},
            "result_received_at": _now_iso(),
            "exception": {"occurred": exception},
        })

    def emit_handoff(self, sending_agent: str, receiving_subagent: str) -> None:
        self.sink.write({
            "schema_version": self.config.schema_version,
            "record_type": "HANDOFF",
            "session_id": self.session_id,
            "handoff_id": f"handoff_{uuid.uuid4().hex[:8]}",
            "sending_agent": sending_agent,
            "receiving_subagent_type": receiving_subagent,
            "acknowledgement_status": "pending",
            "handoff_at": _now_iso(),
        })

    def emit_session_end(self, outcome: str) -> None:
        self.sink.write({
            "schema_version": self.config.schema_version,
            "record_type": "SESSION_END",
            "session_id": self.session_id,
            "outcome": outcome,
            "session_ended_at": _now_iso(),
        })

    # -- daemons --------------------------------------------------------------
    def _daemon_cmd(self, module: str, *extra_args: str):
        cmd = [sys.executable, "-m", module, *extra_args]
        if self.config.source_path:
            cmd += ["--config", self.config.source_path]
        return cmd

    def _start_heartbeat_daemon(self) -> None:
        pid_path = self.config.heartbeat_pid_path(self.session_id)
        pid_path.parent.mkdir(parents=True, exist_ok=True)
        if pid_path.exists():
            return  # already running for this session
        proc = subprocess.Popen(
            self._daemon_cmd("anchor_hooks.heartbeat_daemon", "--session-id", self.session_id),
            start_new_session=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        pid_path.write_text(str(proc.pid))

    def _stop_heartbeat_daemon(self) -> None:
        pid_path = self.config.heartbeat_pid_path(self.session_id)
        if not pid_path.exists():
            return
        try:
            pid = int(pid_path.read_text().strip())
            os.kill(pid, signal.SIGTERM)
        except (ValueError, ProcessLookupError, FileNotFoundError):
            pass
        finally:
            pid_path.unlink(missing_ok=True)

    def _start_forwarder_daemon(self) -> None:
        pid_path = self.config.forwarder_pid_path
        pid_path.parent.mkdir(parents=True, exist_ok=True)
        if pid_path.exists():
            try:
                pid = int(pid_path.read_text().strip())
                os.kill(pid, 0)  # raises if not alive
                return  # already running
            except (ValueError, ProcessLookupError, FileNotFoundError):
                pass  # stale pid file -- fall through and restart
        proc = subprocess.Popen(
            self._daemon_cmd("anchor_hooks.log_forwarder"),
            start_new_session=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        pid_path.write_text(str(proc.pid))

    # -- public entrypoint ------------------------------------------------
    @contextmanager
    def track(self, source: str = "startup", outcome_on_success: str = "completed"):
        self.emit_session_start(source)
        self._start_heartbeat_daemon()
        self._start_forwarder_daemon()
        try:
            yield self
        except BaseException:
            self.emit_session_end("error")
            self._stop_heartbeat_daemon()
            raise
        else:
            self.emit_session_end(outcome_on_success)
            self._stop_heartbeat_daemon()
