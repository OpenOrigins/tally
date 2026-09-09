"""LangChain callback handler for LangGraph invocation and tool events."""

from __future__ import annotations

import logging
import threading
from typing import Any
from uuid import UUID

from langchain_core.callbacks import BaseCallbackHandler

from .events import HANDOFF_EVENT_NAME

logger = logging.getLogger("tally_langgraph")


class TallyCallbackHandler(BaseCallbackHandler):
    """Capture one LangGraph invocation as a Tally session and turn.

    Obtain handlers from :meth:`tally_langgraph.TallyClient.callback`. A handler
    may be reused sequentially, but should not be shared by concurrent invocations.
    """

    raise_error = False
    run_inline = True

    def __init__(self, client: Any, *, source: str = "langgraph") -> None:
        self.client = client
        self.source = source
        self._lock = threading.RLock()
        self._root_run_id: UUID | None = None
        self._session_id: str | None = None
        self._instruction_id: str | None = None
        self._turn_id: str | None = None
        self._action_ids: dict[UUID, str] = {}

    def on_chain_start(
        self,
        serialized: dict[str, Any] | None,
        inputs: dict[str, Any],
        *,
        run_id: UUID,
        parent_run_id: UUID | None = None,
        tags: list[str] | None = None,
        metadata: dict[str, Any] | None = None,
        **kwargs: Any,
    ) -> None:
        if parent_run_id is not None:
            return
        with self._lock:
            if self._root_run_id is not None:
                logger.warning("Tally callback is already tracking another root invocation")
                return
            self._root_run_id = run_id
            self._session_id = f"sess_{run_id}"
            self._instruction_id = f"instr_{run_id}"
            self._turn_id = f"turn_{run_id}"
            session_started = False
            try:
                self.client.start_session(self._session_id, source=self.source)
                session_started = True
                self.client.record_instruction(
                    self._session_id,
                    self._instruction_id,
                    inputs,
                    context={"metadata": metadata or {}, "tags": tags or []},
                )
            except Exception:
                logger.exception("Failed to capture the start of a Tally session")
                if session_started:
                    try:
                        self.client.end_session(
                            self._session_id,
                            outcome="failure",
                            value={"reason": "instruction_capture_failed"},
                        )
                    except Exception:
                        logger.exception("Failed to close an incomplete Tally session")
                self._reset()

    def on_chain_end(
        self,
        outputs: Any,
        *,
        run_id: UUID,
        parent_run_id: UUID | None = None,
        **kwargs: Any,
    ) -> None:
        self._finish(run_id, outputs, turn_outcome="completed", session_outcome="success")

    def on_chain_error(
        self,
        error: BaseException,
        *,
        run_id: UUID,
        parent_run_id: UUID | None = None,
        **kwargs: Any,
    ) -> None:
        value = {"error_type": type(error).__name__, "message": str(error)}
        self._finish(run_id, value, turn_outcome="failed", session_outcome="failure")

    def _finish(
        self,
        run_id: UUID,
        value: Any,
        *,
        turn_outcome: str,
        session_outcome: str,
    ) -> None:
        with self._lock:
            if run_id != self._root_run_id or self._session_id is None or self._turn_id is None:
                return
            session_id = self._session_id
            turn_id = self._turn_id
            try:
                self.client.end_turn(
                    session_id,
                    turn_id,
                    outcome=turn_outcome,
                    value=value,
                )
            except Exception:
                logger.exception("Failed to capture the end of a Tally turn")
            try:
                self.client.end_session(session_id, outcome=session_outcome, value=value)
            except Exception:
                logger.exception("Failed to capture the end of a Tally session")
            finally:
                self._reset()

    def on_tool_start(
        self,
        serialized: dict[str, Any] | None,
        input_str: str,
        *,
        run_id: UUID,
        parent_run_id: UUID | None = None,
        tags: list[str] | None = None,
        metadata: dict[str, Any] | None = None,
        inputs: dict[str, Any] | None = None,
        **kwargs: Any,
    ) -> None:
        with self._lock:
            if self._session_id is None or self._instruction_id is None:
                return
            action_id = f"act_{run_id}"
            self._action_ids[run_id] = action_id
            tool_name = str((serialized or {}).get("name") or "unknown_tool")
            tool_server = str((metadata or {}).get("tally_tool_server") or "langchain")
            try:
                self.client.record_action(
                    self._session_id,
                    self._instruction_id,
                    action_id,
                    tool_server=tool_server,
                    tool_name=tool_name,
                    params=inputs if inputs is not None else input_str,
                )
            except Exception:
                self._action_ids.pop(run_id, None)
                logger.exception("Failed to capture a Tally tool action")

    def on_tool_end(
        self,
        output: Any,
        *,
        run_id: UUID,
        parent_run_id: UUID | None = None,
        **kwargs: Any,
    ) -> None:
        self._record_tool_result(run_id, output, error=None)

    def on_tool_error(
        self,
        error: BaseException,
        *,
        run_id: UUID,
        parent_run_id: UUID | None = None,
        **kwargs: Any,
    ) -> None:
        value = {"error_type": type(error).__name__, "message": str(error)}
        self._record_tool_result(run_id, value, error=error)

    def _record_tool_result(
        self,
        run_id: UUID,
        value: Any,
        *,
        error: BaseException | None,
    ) -> None:
        with self._lock:
            action_id = self._action_ids.pop(run_id, None)
            if action_id is None or self._session_id is None:
                return
            try:
                self.client.record_result(
                    self._session_id,
                    action_id,
                    value,
                    error=error,
                )
            except Exception:
                logger.exception("Failed to capture a Tally tool result")

    def on_custom_event(
        self,
        name: str,
        data: Any,
        *,
        run_id: UUID,
        tags: list[str] | None = None,
        metadata: dict[str, Any] | None = None,
        **kwargs: Any,
    ) -> None:
        if name != HANDOFF_EVENT_NAME or not isinstance(data, dict):
            return
        with self._lock:
            if self._session_id is None:
                return
            receiving_agent = data.get("receiving_agent")
            if not isinstance(receiving_agent, str) or not receiving_agent:
                logger.warning("Ignored Tally handoff without a receiving_agent")
                return
            try:
                self.client.record_handoff(
                    self._session_id,
                    receiving_agent=receiving_agent,
                    payload=data.get("payload"),
                    handoff_id=data.get("handoff_id"),
                    acknowledgement_status=data.get("acknowledgement_status", "pending"),
                )
            except Exception:
                logger.exception("Failed to capture a Tally handoff")

    def _reset(self) -> None:
        self._root_run_id = None
        self._session_id = None
        self._instruction_id = None
        self._turn_id = None
        self._action_ids.clear()
