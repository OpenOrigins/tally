"""Minimal LangGraph example with automatic tool and lifecycle capture."""

from typing import TypedDict

from langchain_core.runnables import RunnableConfig
from langchain_core.tools import tool
from langgraph.graph import END, START, StateGraph

from tally_langgraph import TallyClient


class State(TypedDict):
    text: str
    word_count: int


@tool
def count_words(text: str) -> int:
    """Count whitespace-separated words."""

    return len(text.split())


def analyze(state: State, config: RunnableConfig) -> dict[str, int]:
    count = count_words.invoke({"text": state["text"]}, config=config)
    return {"word_count": count}


builder = StateGraph(State)
builder.add_node("analyze", analyze)
builder.add_edge(START, "analyze")
builder.add_edge("analyze", END)
graph = builder.compile()

tally = TallyClient.from_env()
result = graph.invoke(
    {"text": "Tally records this tool call", "word_count": 0},
    config={"callbacks": [tally.callback(source="example")]},
)
print(result)
print([record["record_type"] for record in tally.journal.records()])
tally.close(flush=False)
