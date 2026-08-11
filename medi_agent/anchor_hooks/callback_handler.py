"""LangChain callback handler that turns LangGraph run events into anchor
records. Attach one instance to any compiled graph's invoke/stream config
(`config={"callbacks": [handler]}`) and it captures every node's tool calls
and subgraph handoffs, regardless of that graph's internal structure.

Mapping (see AnchorSession for SESSION_START/END and INSTRUCTION_RECEIVED,
which have no LangChain run event to hang off of and are emitted explicitly):
  - on_tool_start/on_tool_end/on_tool_error -> ACTION_TAKEN / RESULT_RECEIVED
  - on_chain_start (nested subgraph frame)  -> HANDOFF (best-effort, see below)

agent_version is deliberately NOT captured here from on_chat_model_start: by
the time any model call fires, SESSION_START has already been written (it's
emitted at session.track() entry, before the graph even runs), so a
dynamically-discovered model name would always arrive too late. Configure
agent_version explicitly instead (AnchorConfig.agent_version / ANCHOR_AGENT_VERSION).

HANDOFF detection relies on LangGraph's `langgraph_checkpoint_ns` metadata,
which grows one `|`-joined segment per level of subgraph nesting -- verified
against langgraph==1.2.9 with a supervisor node routing into two compiled
subgraphs (see anchor_hooks/tests or the project chat log for the harness).
`langgraph_checkpoint_ns` is an internal implementation detail, not documented
public API, so re-verify this against the client's actual LangGraph version
before relying on it in production.
"""
from __future__ import annotations

from typing import Any, Dict, Optional
from uuid import UUID

from langchain_core.callbacks import BaseCallbackHandler

from .schema import truncate

TOOL_TRUNCATE_LEN = 2000


class AnchorCallbackHandler(BaseCallbackHandler):
    def __init__(self, session):
        self.session = session
        self._action_ids: Dict[UUID, str] = {}
        self._seen_subgraph_entries = set()
        self._current_toplevel_node: Optional[str] = None
        self._current_toplevel_ns: Optional[str] = None
        self._previous_toplevel_node: Optional[str] = None

    # -- ACTION_TAKEN / RESULT_RECEIVED -----------------------------------
    def on_tool_start(self, serialized, input_str, *, run_id, parent_run_id=None, tags=None,
                       metadata=None, inputs=None, **kwargs):
        action_id = f"act_{run_id}"
        self._action_ids[run_id] = action_id
        tool_name = (serialized or {}).get("name", "unknown_tool")
        params = truncate(str(inputs) if inputs is not None else input_str, TOOL_TRUNCATE_LEN)
        self.session.emit_action_taken(action_id=action_id, tool_name=tool_name, params=params)

    def on_tool_end(self, output, *, run_id, parent_run_id=None, **kwargs):
        action_id = self._action_ids.pop(run_id, f"act_{run_id}")
        self.session.emit_result_received(
            action_id=action_id, summary=truncate(str(output), TOOL_TRUNCATE_LEN), exception=False
        )

    def on_tool_error(self, error, *, run_id, parent_run_id=None, **kwargs):
        action_id = self._action_ids.pop(run_id, f"act_{run_id}")
        self.session.emit_result_received(
            action_id=action_id, summary=truncate(str(error), TOOL_TRUNCATE_LEN), exception=True
        )

    # -- HANDOFF (best-effort) ---------------------------------------------
    def on_chain_start(self, serialized, inputs, *, run_id, parent_run_id=None, tags=None,
                        metadata=None, **kwargs):
        self._maybe_emit_handoff(metadata)

    def _maybe_emit_handoff(self, metadata: Optional[Dict[str, Any]]):
        if not metadata:
            return
        ns = metadata.get("langgraph_checkpoint_ns", "")
        segments = [s for s in ns.split("|") if s]
        if not segments:
            return

        if len(segments) == 1:
            # A depth-1 frame is some top-level node of the (sub)graph we're
            # currently in. LangGraph fires this for a plain node AND for a
            # node that turns out to be a nested compiled subgraph -- at this
            # point we can't yet tell which, so just track "most recent
            # distinct top-level node" and wait for a depth-2 frame to prove
            # it was a subgraph.
            if segments[0] != self._current_toplevel_ns:
                self._previous_toplevel_node = self._current_toplevel_node
                self._current_toplevel_node = segments[0].split(":")[0]
                self._current_toplevel_ns = segments[0]
            return

        # depth >= 2: we're inside a nested subgraph -- confirms the current
        # top-level node is actually a subagent. Fire HANDOFF once per entry.
        subgraph_key = segments[0]
        if subgraph_key in self._seen_subgraph_entries:
            return
        self._seen_subgraph_entries.add(subgraph_key)
        receiving_subagent = subgraph_key.split(":")[0]
        sending_agent = self._previous_toplevel_node or "unknown"
        self.session.emit_handoff(sending_agent=sending_agent, receiving_subagent=receiving_subagent)
