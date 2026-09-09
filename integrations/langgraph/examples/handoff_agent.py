"""Explicit handoff capture using LangChain's public custom-event API."""

from typing import TypedDict

from langchain_core.runnables import RunnableConfig
from langgraph.graph import END, START, StateGraph

from tally_langgraph import TallyClient, dispatch_handoff


class State(TypedDict):
    request_id: str
    result: str


def supervisor(state: State, config: RunnableConfig) -> dict[str, str]:
    dispatch_handoff(
        "agent:researcher",
        payload={"request_id": state["request_id"]},
        config=config,
    )
    return {}


def researcher(state: State) -> dict[str, str]:
    return {"result": f"Research completed for {state['request_id']}"}


builder = StateGraph(State)
builder.add_node("supervisor", supervisor)
builder.add_node("researcher", researcher)
builder.add_edge(START, "supervisor")
builder.add_edge("supervisor", "researcher")
builder.add_edge("researcher", END)
graph = builder.compile()

tally = TallyClient.from_env()
result = graph.invoke(
    {"request_id": "req-123", "result": ""},
    config={"callbacks": [tally.callback(source="example")]},
)
print(result)
print([record["record_type"] for record in tally.journal.records()])
tally.close(flush=False)
