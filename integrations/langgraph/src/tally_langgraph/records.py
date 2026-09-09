"""Builders for Tally 0.2 records."""

from __future__ import annotations

import uuid
from datetime import datetime, timezone
from typing import Any

from .config import TallyConfig
from .evidence import evidence_summary, private_evidence, server_evidence

Evidence = list[tuple[str, str]]


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _base(record_type: str) -> dict[str, Any]:
    return {
        "record_id": f"rec_{uuid.uuid4().hex}",
        "record_type": record_type,
        "schema_version": "0.2",
    }


def _reference(value: Any) -> tuple[str, str, Evidence]:
    digest, uri, payload = private_evidence(value)
    return digest, uri, [(digest, payload)]


def session_start(
    *,
    session_id: str,
    agent_id: str,
    config: TallyConfig,
    source: str,
) -> tuple[dict[str, Any], Evidence]:
    principal_available = bool(config.principal_id and config.principal_type)
    record = {
        **_base("SESSION_START"),
        "session_id": session_id,
        "agent_id": agent_id,
        "agent_version": config.agent_version,
        "principal": {
            "type": config.principal_type,
            "id": config.principal_id,
            "capture_status": "captured" if principal_available else "unavailable",
        },
        "authority_scope_hash": None,
        "authority_scope_uri": None,
        "authority_capture_status": "unavailable",
        "authority_granted_at": None,
        "authority_expires_at": None,
        "delegation_chain": {
            "depth": None,
            "chain_hash": None,
            "chain": None,
            "chain_uri": None,
            "capture_status": "unavailable",
        },
        "source": source,
        "session_started_at": now_iso(),
    }
    return record, []


def instruction_received(
    *,
    session_id: str,
    instruction_id: str,
    value: Any,
    context: Any,
    config: TallyConfig,
) -> tuple[dict[str, Any], Evidence]:
    instruction_hash, instruction_uri, evidence = _reference(value)
    context_hash, context_uri, context_evidence = _reference(context)
    projection = server_evidence(
        value,
        enabled=config.server_evidence_enabled,
        max_chars=config.server_evidence_max_chars,
    )
    record = {
        **_base("INSTRUCTION_RECEIVED"),
        "session_id": session_id,
        "instruction_id": instruction_id,
        "sender": {
            "id": config.principal_id,
            "signature": None,
            "signature_status": "unavailable",
        },
        "instruction_hash": instruction_hash,
        "instruction_uri": instruction_uri,
        "instruction_received_at": now_iso(),
        "context_snapshot_hash": context_hash,
        "context_snapshot_uri": context_uri,
        "declared_intent": {
            "summary": None,
            "detail_hash": None,
            "detail_uri": None,
            "capture_status": "unavailable",
        },
        "instruction_summary": f"[ARB] {evidence_summary(projection, 'Instruction received')}",
        "server_evidence": projection,
    }
    return record, evidence + context_evidence


def action_taken(
    *,
    session_id: str,
    instruction_id: str,
    action_id: str,
    tool_server: str,
    tool_name: str,
    params: Any,
    config: TallyConfig,
) -> tuple[dict[str, Any], Evidence]:
    params_hash, params_uri, evidence = _reference(params)
    projection = server_evidence(
        params,
        enabled=config.server_evidence_enabled,
        max_chars=config.server_evidence_max_chars,
    )
    record = {
        **_base("ACTION_TAKEN"),
        "session_id": session_id,
        "instruction_id": instruction_id,
        "action_id": action_id,
        "action_type": "tool_call",
        "tool": {
            "server": tool_server,
            "name": tool_name,
            "params_hash": params_hash,
            "params_uri": params_uri,
        },
        "pre_state_hash": None,
        "pre_state_uri": None,
        "post_state_hash": None,
        "post_state_uri": None,
        "state_capture_status": "unavailable",
        "action_timestamp": now_iso(),
        "deviance_flag": {
            "deviated": None,
            "evaluation_status": "unavailable",
            "delta_category": None,
            "delta_hash": None,
            "delta_uri": None,
        },
        "server_evidence": projection,
    }
    return record, evidence


def result_received(
    *,
    session_id: str,
    action_id: str,
    value: Any,
    error: BaseException | None,
    config: TallyConfig,
) -> tuple[dict[str, Any], Evidence]:
    result_hash, result_uri, evidence = _reference(value)
    projection = server_evidence(
        value,
        enabled=config.server_evidence_enabled,
        max_chars=config.server_evidence_max_chars,
    )
    record = {
        **_base("RESULT_RECEIVED"),
        "session_id": session_id,
        "action_id": action_id,
        "result_hash": result_hash,
        "result_uri": result_uri,
        "result_received_at": now_iso(),
        "post_state_hash": None,
        "post_state_uri": None,
        "state_capture_status": "unavailable",
        "result_interpretation": {
            "summary": f"[ARB] {evidence_summary(projection, 'Tool result received')}",
            "detail_hash": result_hash,
            "detail_uri": result_uri,
        },
        "exception": {
            "occurred": error is not None,
            "type": type(error).__name__ if error is not None else None,
            "description_hash": result_hash if error is not None else None,
            "description_uri": result_uri if error is not None else None,
        },
        "server_evidence": projection,
    }
    return record, evidence


def handoff(
    *,
    session_id: str,
    handoff_id: str,
    agent_id: str,
    receiving_agent: str,
    payload: Any,
    acknowledgement_status: str,
) -> tuple[dict[str, Any], Evidence]:
    payload_hash, payload_uri, evidence = _reference(payload)
    record = {
        **_base("HANDOFF"),
        "session_id": session_id,
        "handoff_id": handoff_id,
        "emitting_party": "sender",
        "sender": {
            "agent_id": agent_id,
            "org_id": None,
            "signature": None,
            "signature_status": "unavailable",
        },
        "receiver": {
            "agent_id": receiving_agent,
            "org_id": None,
            "signature": None,
            "acknowledged_at": None,
        },
        "payload_hash": payload_hash,
        "payload_uri": payload_uri,
        "handoff_timestamp": now_iso(),
        "acknowledgement_status": acknowledgement_status,
    }
    return record, evidence


def turn_end(
    *,
    session_id: str,
    turn_id: str,
    outcome: str,
    value: Any,
    config: TallyConfig,
) -> tuple[dict[str, Any], Evidence]:
    outcome_hash, outcome_uri, evidence = _reference(value)
    projection = server_evidence(
        value,
        enabled=config.server_evidence_enabled,
        max_chars=config.server_evidence_max_chars,
    )
    record = {
        **_base("TURN_END"),
        "session_id": session_id,
        "turn_id": turn_id,
        "outcome": outcome,
        "outcome_hash": outcome_hash,
        "outcome_uri": outcome_uri,
        "turn_ended_at": now_iso(),
        "server_evidence": projection,
    }
    return record, evidence


def session_end(
    *,
    session_id: str,
    outcome: str,
    value: Any,
) -> tuple[dict[str, Any], Evidence]:
    outcome_hash, outcome_uri, evidence = _reference(value)
    record = {
        **_base("SESSION_END"),
        "session_id": session_id,
        "outcome": outcome,
        "outcome_hash": outcome_hash,
        "outcome_uri": outcome_uri,
        "human_review": {
            "required": False,
            "reviewer_id": None,
            "approved_at": None,
            "approval_hash": None,
        },
        "session_ended_at": now_iso(),
    }
    return record, evidence


def heartbeat(
    *,
    agent_id: str,
    anchor_instance_id: str,
    active_sessions: list[str],
) -> tuple[dict[str, Any], Evidence]:
    record = {
        **_base("HEARTBEAT"),
        "agent_id": agent_id,
        "anchor_instance_id": anchor_instance_id,
        "active_sessions": sorted(active_sessions),
        "timestamp": now_iso(),
    }
    return record, []
