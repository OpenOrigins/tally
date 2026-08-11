"""Regression check for HANDOFF detection against a synthetic supervisor +
two subagent subgraphs. HANDOFF relies on LangGraph's internal
`langgraph_checkpoint_ns` metadata (not public API) -- run this after any
LangGraph version upgrade to confirm handoffs still fire and are attributed
to the correct sending/receiving agent before trusting HANDOFF in production.

Run directly: python -m anchor_hooks.tests.test_handoff
"""
from __future__ import annotations

import shutil
import tempfile
from pathlib import Path
from typing import TypedDict

from langgraph.graph import END, START, StateGraph

from anchor_hooks.config import AnchorConfig
from anchor_hooks.session import AnchorSession


class _State(TypedDict):
    x: str


def _build_supervisor_graph():
    def sub_a_step(state):
        return {"x": state["x"] + "-suba"}

    def sub_b_step(state):
        return {"x": state["x"] + "-subb"}

    def supervisor(state):
        return {"x": state["x"] + "-sup"}

    sub_a = StateGraph(_State)
    sub_a.add_node("sub_a_step", sub_a_step)
    sub_a.add_edge(START, "sub_a_step")
    sub_a.add_edge("sub_a_step", END)

    sub_b = StateGraph(_State)
    sub_b.add_node("sub_b_step", sub_b_step)
    sub_b.add_edge(START, "sub_b_step")
    sub_b.add_edge("sub_b_step", END)

    outer = StateGraph(_State)
    outer.add_node("supervisor", supervisor)
    outer.add_node("subagent_a", sub_a.compile())
    outer.add_node("subagent_b", sub_b.compile())
    outer.add_edge(START, "supervisor")
    outer.add_edge("supervisor", "subagent_a")
    outer.add_edge("subagent_a", "subagent_b")
    outer.add_edge("subagent_b", END)
    return outer.compile()


def run_check() -> bool:
    tmp_dir = tempfile.mkdtemp(prefix="anchor_handoff_test_")
    try:
        config = AnchorConfig(log_dir=tmp_dir)
        session = AnchorSession(config=config)
        app = _build_supervisor_graph()

        with session.track(source="test") as s:
            app.invoke({"x": "start"}, config={"callbacks": [s.callback_handler]})

        records = []
        with open(config.log_jsonl_path) as f:
            for line in f:
                import json

                records.append(json.loads(line))

        handoffs = [r for r in records if r["record_type"] == "HANDOFF"]
        expected = [
            ("supervisor", "subagent_a"),
            ("subagent_a", "subagent_b"),
        ]
        actual = [(h["sending_agent"], h["receiving_subagent_type"]) for h in handoffs]

        ok = actual == expected
        print(f"HANDOFF sequence: {actual}")
        print("PASS" if ok else f"FAIL -- expected {expected}")
        return ok
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


if __name__ == "__main__":
    import sys

    sys.exit(0 if run_check() else 1)
