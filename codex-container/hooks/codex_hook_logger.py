#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import subprocess
import sys
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from lib.tally_audit import AuditSink, light_git_state, scrub_environment, sha256_hex, utc_now, write_json_atomic


def read_hook_payload() -> tuple[Any, str]:
    raw = sys.stdin.read()
    if not raw:
        return {}, ""
    try:
        return json.loads(raw), raw
    except json.JSONDecodeError:
        return {"raw_stdin": raw}, raw


def first_string_by_key(value: Any, names: set[str]) -> str | None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key in names and isinstance(item, str) and item:
                return item
        for item in value.values():
            found = first_string_by_key(item, names)
            if found:
                return found
    elif isinstance(value, list):
        for item in value:
            found = first_string_by_key(item, names)
            if found:
                return found
    return None


def first_mapping_by_key(value: Any, names: set[str]) -> dict[str, Any] | None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key in names and isinstance(item, dict):
                return item
        for item in value.values():
            found = first_mapping_by_key(item, names)
            if found:
                return found
    elif isinstance(value, list):
        for item in value:
            found = first_mapping_by_key(item, names)
            if found:
                return found
    return None


def extract_session_id(payload: Any, sink: AuditSink) -> str:
    return (
        first_string_by_key(payload, {"session_id", "thread_id", "conversation_id", "conversationId"})
        or sink.run_id
    )


def extract_tool_name(payload: Any, fallback: str) -> str:
    return (
        first_string_by_key(
            payload,
            {
                "tool_name",
                "toolName",
                "name",
                "command",
                "mcp_tool_name",
                "recipient_name",
            },
        )
        or fallback
        or "unknown"
    )


def extract_tool_server(payload: Any) -> str:
    return (
        first_string_by_key(payload, {"server", "server_name", "mcp_server", "recipient_namespace"})
        or "codex"
    )


def stable_id(prefix: str, value: Any) -> str:
    digest = sha256_hex(value).split(":", 1)[1][:16]
    return f"{prefix}_{digest}"


def build_tally_record(
    *,
    sink: AuditSink,
    event_type: str,
    payload: Any,
    raw_hash: str,
    raw_uri: str,
    metadata: dict[str, Any],
) -> dict[str, Any]:
    session_id = extract_session_id(payload, sink)
    prompt = first_string_by_key(payload, {"prompt", "user_prompt", "input", "text", "content"})
    tool_params = first_mapping_by_key(payload, {"arguments", "args", "params", "input"}) or payload

    if event_type == "SessionStart":
        return {
            "record_type": "SESSION_START",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "agent_id": os.environ.get("TALLY_AGENT_ID", "codex-container"),
            "agent_version": os.environ.get("TALLY_AGENT_VERSION", "codex-cli"),
            "principal": {
                "type": "human",
                "id": "[ARB] container-user",
            },
            "authority_scope_hash": sha256_hex(metadata),
            "authority_scope_uri": raw_uri,
            "authority_granted_at": metadata["observed_at"],
            "session_started_at": metadata["observed_at"],
            "codex_hook_event": event_type,
            "raw_hook_hash": raw_hash,
        }

    if event_type == "UserPromptSubmit":
        instruction_id = stable_id("instr", payload)
        return {
            "record_type": "INSTRUCTION_RECEIVED",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "instruction_id": instruction_id,
            "sender": {
                "id": "[ARB] user",
                "signature": sha256_hex(payload),
            },
            "instruction_hash": raw_hash,
            "instruction_uri": raw_uri,
            "instruction_received_at": metadata["observed_at"],
            "context_snapshot_hash": sha256_hex(metadata.get("git_state", {})),
            "context_snapshot_uri": raw_uri,
            "declared_intent": {
                "summary": f"[ARB] {prompt[:240] if prompt else 'User prompt submitted to Codex'}",
                "detail_hash": raw_hash,
                "detail_uri": raw_uri,
            },
            "codex_hook_event": event_type,
        }

    if event_type in {"PreToolUse", "PermissionRequest"}:
        action_id = stable_id("act", payload)
        return {
            "record_type": "ACTION_TAKEN",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "action_id": action_id,
            "instruction_id": first_string_by_key(payload, {"instruction_id", "turn_id", "turnId"})
            or stable_id("instr", session_id),
            "action_type": "decision" if event_type == "PermissionRequest" else "tool_call",
            "tool": {
                "server": extract_tool_server(payload),
                "name": extract_tool_name(payload, event_type),
                "params_hash": sha256_hex(tool_params),
                "params_uri": raw_uri,
            },
            "pre_state_hash": sha256_hex(metadata.get("git_state", {})),
            "pre_state_uri": raw_uri,
            "post_state_hash": None,
            "post_state_uri": None,
            "action_timestamp": metadata["observed_at"],
            "deviance_flag": {
                "deviated": False,
                "delta_category": None,
                "delta_hash": None,
                "delta_uri": None,
            },
            "codex_hook_event": event_type,
            "raw_hook_hash": raw_hash,
        }

    if event_type == "PostToolUse":
        return {
            "record_type": "RESULT_RECEIVED",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "action_id": first_string_by_key(payload, {"action_id", "tool_call_id", "call_id", "id"})
            or stable_id("act", payload),
            "result_hash": raw_hash,
            "result_uri": raw_uri,
            "result_received_at": metadata["observed_at"],
            "result_interpretation": {
                "summary": "[ARB] Codex reported a tool result",
                "detail_hash": raw_hash,
                "detail_uri": raw_uri,
            },
            "exception": {
                "occurred": bool(first_string_by_key(payload, {"error", "exception"})),
                "type": first_string_by_key(payload, {"error_type", "type"}) if isinstance(payload, dict) else None,
                "description_hash": raw_hash if first_string_by_key(payload, {"error", "exception"}) else None,
                "description_uri": raw_uri if first_string_by_key(payload, {"error", "exception"}) else None,
            },
            "codex_hook_event": event_type,
        }

    if event_type == "Stop":
        return {
            "record_type": "SESSION_END",
            "schema_version": "0.2-mvp",
            "session_id": session_id,
            "outcome": "codex_turn_stopped",
            "outcome_hash": raw_hash,
            "outcome_uri": raw_uri,
            "session_ended_at": metadata["observed_at"],
            "codex_hook_event": event_type,
        }

    return {
        "record_type": "CODEX_LIFECYCLE",
        "schema_version": "0.2-mvp",
        "session_id": session_id,
        "codex_hook_event": event_type,
        "event_hash": raw_hash,
        "event_uri": raw_uri,
        "observed_at": metadata["observed_at"],
        "metadata": metadata,
    }


def update_heartbeat_state(
    *,
    sink: AuditSink,
    event_type: str,
    payload: Any,
    observed_at: str,
) -> None:
    if os.environ.get("TALLY_HOOK_HEARTBEAT_ENABLED", "1") in {"0", "false", "False", "no"}:
        return

    state_path = sink.state_dir / f"hook-heartbeat.{sink.run_id}.json"
    pid_path = sink.state_dir / f"hook-heartbeat.{sink.run_id}.pid"
    session_id = extract_session_id(payload, sink)
    write_json_atomic(
        state_path,
        {
            "run_id": sink.run_id,
            "session_id": session_id,
            "updated_at": observed_at,
            "last_hook_event": event_type,
            "stop_requested": event_type == "Stop",
        },
    )
    AuditSink("hook-heartbeat").emit_heartbeat(
        active_sessions=[session_id],
        jsonl_stream="hook-heartbeat",
        extra={
            "heartbeat_kind": "hook-event",
            "hook_event": event_type,
        },
    )

    if event_type != "SessionStart":
        return
    if pid_path.exists():
        try:
            os.kill(int(pid_path.read_text(encoding="utf-8").strip()), 0)
            return
        except Exception:
            try:
                pid_path.unlink()
            except FileNotFoundError:
                pass

    daemon_path = Path(__file__).resolve().parents[1] / "monitors" / "hook_heartbeat_daemon.py"
    subprocess.Popen(
        [sys.executable, str(daemon_path)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def main() -> int:
    event_type = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("CODEX_HOOK_EVENT", "unknown")
    sink = AuditSink("codex-hooks")
    payload, raw = read_hook_payload()
    raw_ref = sink.private_payload(f"hook_{event_type}_{uuid.uuid4().hex[:8]}", payload)
    observed_at = utc_now()
    metadata = {
        "observed_at": observed_at,
        "hook_event": event_type,
        "cwd": os.getcwd(),
        "argv": sys.argv,
        "raw_stdin_hash": sha256_hex(raw),
        "environment": scrub_environment(),
        "git_state": light_git_state(),
    }
    event = {
        "schema_version": "tally-codex-container.v1",
        "event_id": "evt_" + uuid.uuid4().hex[:16],
        "run_id": sink.run_id,
        "source": "codex-hooks",
        "event_type": event_type,
        "observed_at": observed_at,
        "payload_hash": raw_ref["hash"],
        "payload_uri": raw_ref["uri"],
        "metadata": metadata,
    }
    sink.append_jsonl("codex-hooks", event)
    update_heartbeat_state(
        sink=sink,
        event_type=event_type,
        payload=payload,
        observed_at=observed_at,
    )
    record = build_tally_record(
        sink=sink,
        event_type=event_type,
        payload=payload,
        raw_hash=raw_ref["hash"],
        raw_uri=raw_ref["uri"],
        metadata=metadata,
    )
    record.setdefault("record_id", "rec_" + event["event_id"].removeprefix("evt_"))
    record["container_event_id"] = event["event_id"]
    sink.write_tally_record(record)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
