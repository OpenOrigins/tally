import asyncio
from pathlib import Path
from typing import TypedDict
from unittest.mock import Mock
from uuid import uuid4

import pytest
from langchain_core.runnables import RunnableConfig
from langchain_core.tools import tool
from langgraph.graph import END, START, StateGraph

from tally_langgraph import TallyClient, TallyConfig, adispatch_handoff, dispatch_handoff
from tally_langgraph.callback import TallyCallbackHandler
from tally_langgraph.transport import DeliveryResult


class State(TypedDict):
    text: str
    count: int


class DeliveredTransport:
    def deliver(self, record_id: str, record: dict) -> DeliveryResult:
        return DeliveryResult("delivered")


def _client(tmp_path: Path) -> TallyClient:
    return TallyClient(
        TallyConfig(state_dir=tmp_path, api_key="test", agent_id="agent:test"),
        transport=DeliveredTransport(),
        background=False,
    )


@tool
def count_words(text: str) -> int:
    """Count words in text."""

    return len(text.split())


def _tool_graph():
    def node(state: State, config: RunnableConfig) -> dict[str, int]:
        return {"count": count_words.invoke({"text": state["text"]}, config=config)}

    builder = StateGraph(State)
    builder.add_node("node", node)
    builder.add_edge(START, "node")
    builder.add_edge("node", END)
    return builder.compile()


def test_graph_lifecycle_and_tool_events(tmp_path: Path) -> None:
    client = _client(tmp_path)
    graph = _tool_graph()

    result = graph.invoke(
        {"text": "one two three", "count": 0},
        config={"callbacks": [client.callback(source="test")]},
    )

    assert result["count"] == 3
    assert [record["record_type"] for record in client.journal.records()] == [
        "SESSION_START",
        "INSTRUCTION_RECEIVED",
        "ACTION_TAKEN",
        "RESULT_RECEIVED",
        "TURN_END",
        "SESSION_END",
    ]


def test_handler_can_be_reused_sequentially(tmp_path: Path) -> None:
    client = _client(tmp_path)
    graph = _tool_graph()
    handler = client.callback(source="test")

    graph.invoke({"text": "first", "count": 0}, config={"callbacks": [handler]})
    graph.invoke({"text": "second", "count": 0}, config={"callbacks": [handler]})

    assert [record["record_type"] for record in client.journal.records()].count(
        "SESSION_START"
    ) == 2
    assert [record["record_type"] for record in client.journal.records()].count("SESSION_END") == 2


def test_graph_error_closes_failed_session(tmp_path: Path) -> None:
    client = _client(tmp_path)

    def fail(state: State) -> dict:
        raise RuntimeError("boom")

    builder = StateGraph(State)
    builder.add_node("fail", fail)
    builder.add_edge(START, "fail")
    graph = builder.compile()

    with pytest.raises(RuntimeError, match="boom"):
        graph.invoke(
            {"text": "hello", "count": 0},
            config={"callbacks": [client.callback(source="test")]},
        )

    records = client.journal.records()
    assert records[-2]["record_type"] == "TURN_END"
    assert records[-2]["outcome"] == "failed"
    assert records[-1]["record_type"] == "SESSION_END"
    assert records[-1]["outcome"] == "failure"


def test_explicit_handoff_uses_public_custom_event(tmp_path: Path) -> None:
    client = _client(tmp_path)

    def handoff(state: State, config: RunnableConfig) -> dict:
        dispatch_handoff(
            "agent:researcher",
            payload={"text": state["text"]},
            handoff_id="handoff-test",
            config=config,
        )
        return {}

    builder = StateGraph(State)
    builder.add_node("handoff", handoff)
    builder.add_edge(START, "handoff")
    builder.add_edge("handoff", END)
    graph = builder.compile()

    graph.invoke(
        {"text": "research this", "count": 0},
        config={"callbacks": [client.callback(source="test")]},
    )

    record = next(
        record for record in client.journal.records() if record["record_type"] == "HANDOFF"
    )
    assert record["handoff_id"] == "handoff-test"
    assert record["receiver"]["agent_id"] == "agent:researcher"


def test_async_handoff(tmp_path: Path) -> None:
    client = _client(tmp_path)

    async def handoff(state: State, config: RunnableConfig) -> dict:
        await adispatch_handoff("agent:async-worker", payload=state, config=config)
        return {}

    builder = StateGraph(State)
    builder.add_node("handoff", handoff)
    builder.add_edge(START, "handoff")
    builder.add_edge("handoff", END)
    graph = builder.compile()

    asyncio.run(
        graph.ainvoke(
            {"text": "async", "count": 0},
            config={"callbacks": [client.callback(source="test")]},
        )
    )

    handoffs = [record for record in client.journal.records() if record["record_type"] == "HANDOFF"]
    assert len(handoffs) == 1
    assert handoffs[0]["receiver"]["agent_id"] == "agent:async-worker"


def test_stream_and_astream_complete_their_sessions(tmp_path: Path) -> None:
    graph = _tool_graph()
    sync_client = _client(tmp_path / "sync")
    assert list(
        graph.stream(
            {"text": "sync stream", "count": 0},
            config={"callbacks": [sync_client.callback(source="test")]},
        )
    )
    assert sync_client.journal.records()[-1]["record_type"] == "SESSION_END"

    async_client = _client(tmp_path / "async")

    async def consume() -> list[dict]:
        return [
            chunk
            async for chunk in graph.astream(
                {"text": "async stream", "count": 0},
                config={"callbacks": [async_client.callback(source="test")]},
            )
        ]

    assert asyncio.run(consume())
    assert async_client.journal.records()[-1]["record_type"] == "SESSION_END"


def test_handler_ignores_unrelated_and_malformed_events() -> None:
    client = Mock()
    handler = TallyCallbackHandler(client)
    root = uuid4()

    handler.on_chain_start(None, {}, run_id=uuid4(), parent_run_id=root)
    handler.on_chain_end({}, run_id=root)
    handler.on_tool_start(None, "input", run_id=uuid4())
    handler.on_tool_end("output", run_id=uuid4())
    handler.on_custom_event("another.event", {}, run_id=uuid4())
    handler.on_custom_event("tally.handoff", "invalid", run_id=uuid4())
    handler.on_custom_event("tally.handoff", {}, run_id=uuid4())

    client.assert_not_called()
    assert not client.method_calls


def test_llm_token_usage_recorded_on_session_end() -> None:
    from types import SimpleNamespace

    root = uuid4()
    client = Mock()
    handler = TallyCallbackHandler(client)

    handler.on_chain_start(None, {}, run_id=root)

    usage_metadata_response = SimpleNamespace(
        generations=[
            [
                SimpleNamespace(
                    message=SimpleNamespace(
                        usage_metadata={
                            "input_tokens": 10,
                            "output_tokens": 5,
                            "total_tokens": 15,
                        }
                    )
                )
            ]
        ],
        llm_output=None,
    )
    llm_output_response = SimpleNamespace(
        generations=[],
        llm_output={
            "token_usage": {
                "prompt_tokens": 20,
                "completion_tokens": 8,
                "total_tokens": 28,
            }
        },
    )
    handler.on_llm_end(usage_metadata_response, run_id=uuid4())
    handler.on_llm_end(llm_output_response, run_id=uuid4())

    handler.on_chain_end({"answer": "done"}, run_id=root)

    _, kwargs = client.end_session.call_args
    assert kwargs["token_usage"] == {
        "prompt_tokens": 30,
        "completion_tokens": 13,
        "total_tokens": 43,
        "llm_call_count": 2,
    }


def test_handler_contains_capture_failures() -> None:
    root = uuid4()
    action = uuid4()
    client = Mock()
    client.record_instruction.side_effect = RuntimeError("instruction")
    client.end_session.side_effect = RuntimeError("close")
    handler = TallyCallbackHandler(client)

    handler.on_chain_start(None, {}, run_id=root)
    client.start_session.assert_called_once()
    client.end_session.assert_called_once()

    client.reset_mock()
    client.record_instruction.side_effect = None
    client.end_session.side_effect = RuntimeError("end")
    client.end_turn.side_effect = RuntimeError("turn")
    client.record_action.side_effect = RuntimeError("action")
    handler.on_chain_start(None, {}, run_id=root)
    handler.on_chain_start(None, {}, run_id=uuid4())
    handler.on_tool_start({"name": "search"}, "query", run_id=action)
    handler.on_tool_end("ignored", run_id=action)
    handler.on_chain_end({"answer": "done"}, run_id=root)

    client.record_action.assert_called_once()
    client.record_result.assert_not_called()
    client.end_turn.assert_called_once()
    client.end_session.assert_called_once()


def test_handler_records_tool_errors_and_validates_handoffs() -> None:
    root = uuid4()
    action = uuid4()
    client = Mock()
    handler = TallyCallbackHandler(client)
    handler.on_chain_start(None, {}, run_id=root)

    handler.on_tool_start(
        None,
        "raw input",
        run_id=action,
        metadata={"tally_tool_server": "custom"},
    )
    client.record_result.side_effect = RuntimeError("result")
    handler.on_tool_error(ValueError("bad result"), run_id=action)
    handler.on_custom_event("tally.handoff", {}, run_id=uuid4())
    client.record_handoff.side_effect = RuntimeError("handoff")
    handler.on_custom_event(
        "tally.handoff",
        {"receiving_agent": "agent:worker", "payload": {"task": "review"}},
        run_id=uuid4(),
    )

    client.record_action.assert_called_once()
    assert client.record_action.call_args.kwargs["tool_name"] == "unknown_tool"
    assert client.record_action.call_args.kwargs["tool_server"] == "custom"
    client.record_result.assert_called_once()
    client.record_handoff.assert_called_once()


@pytest.mark.parametrize(
    "kwargs",
    [
        {"receiving_agent": ""},
        {"receiving_agent": "agent:worker", "handoff_id": ""},
        {"receiving_agent": "agent:worker", "acknowledgement_status": "lost"},
    ],
)
def test_handoff_helper_rejects_invalid_events(kwargs: dict) -> None:
    with pytest.raises(ValueError):
        dispatch_handoff(**kwargs)
