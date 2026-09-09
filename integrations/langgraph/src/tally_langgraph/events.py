"""Public custom events used for explicit, stable LangGraph handoff capture."""

from __future__ import annotations

import uuid
from typing import Any

from langchain_core.callbacks.manager import (
    adispatch_custom_event as _adispatch_custom_event,
)
from langchain_core.callbacks.manager import (
    dispatch_custom_event as _dispatch_custom_event,
)
from langchain_core.runnables import RunnableConfig

HANDOFF_EVENT_NAME = "tally.handoff"
_ACKNOWLEDGEMENT_STATUSES = {"pending", "acknowledged", "rejected", "timeout"}


def _validate_handoff(
    receiving_agent: str,
    handoff_id: str | None,
    acknowledgement_status: str,
) -> None:
    if not isinstance(receiving_agent, str) or not receiving_agent:
        raise ValueError("receiving_agent must be a non-empty string")
    if handoff_id is not None and (not isinstance(handoff_id, str) or not handoff_id):
        raise ValueError("handoff_id must be a non-empty string or None")
    if (
        not isinstance(acknowledgement_status, str)
        or acknowledgement_status not in _ACKNOWLEDGEMENT_STATUSES
    ):
        raise ValueError("invalid acknowledgement_status")


def dispatch_handoff(
    receiving_agent: str,
    *,
    payload: Any = None,
    handoff_id: str | None = None,
    acknowledgement_status: str = "pending",
    config: RunnableConfig | None = None,
) -> str:
    """Emit a handoff using LangChain's public custom-event API."""

    _validate_handoff(receiving_agent, handoff_id, acknowledgement_status)
    resolved_id = handoff_id if handoff_id is not None else f"handoff_{uuid.uuid4().hex}"
    _dispatch_custom_event(
        HANDOFF_EVENT_NAME,
        {
            "receiving_agent": receiving_agent,
            "payload": payload,
            "handoff_id": resolved_id,
            "acknowledgement_status": acknowledgement_status,
        },
        config=config,
    )
    return resolved_id


async def adispatch_handoff(
    receiving_agent: str,
    *,
    payload: Any = None,
    handoff_id: str | None = None,
    acknowledgement_status: str = "pending",
    config: RunnableConfig | None = None,
) -> str:
    """Async counterpart to :func:`dispatch_handoff`."""

    _validate_handoff(receiving_agent, handoff_id, acknowledgement_status)
    resolved_id = handoff_id if handoff_id is not None else f"handoff_{uuid.uuid4().hex}"
    await _adispatch_custom_event(
        HANDOFF_EVENT_NAME,
        {
            "receiving_agent": receiving_agent,
            "payload": payload,
            "handoff_id": resolved_id,
            "acknowledgement_status": acknowledgement_status,
        },
        config=config,
    )
    return resolved_id
