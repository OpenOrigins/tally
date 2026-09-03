"""
Multi-agent symptom triage pipeline built with LangGraph + Ollama (llama3.2).

Flow:
    START -> summarizer_agent -> advisor_agent -> END

- summarizer_agent: condenses a long, free-text symptom description into a
  short clinical-style summary.
- advisor_agent: reads that summary and recommends whether the person should
  see a doctor, with urgency level and reasoning.
"""

from typing import TypedDict

from langchain_ollama import ChatOllama
from langgraph.graph import StateGraph, START, END

from anchor_hooks import AnchorSession

MODEL_NAME = "llama3.2"

llm = ChatOllama(model=MODEL_NAME, temperature=0)


class TriageState(TypedDict):
    symptoms_text: str
    summary: str
    recommendation: str


def summarizer_agent(state: TriageState) -> dict:
    prompt = (
        "You are a medical summarization assistant. Read the patient's "
        "symptom description below and produce a short, clear summary "
        "(3-5 bullet points) covering: key symptoms, duration, severity, "
        "and any relevant details (age, existing conditions, medications) "
        "if mentioned. Do not diagnose. Do not add information that isn't "
        "in the text.\n\n"
        f"Patient description:\n{state['symptoms_text']}\n\nSummary:"
    )
    response = llm.invoke(prompt)
    return {"summary": response.content.strip()}


def advisor_agent(state: TriageState) -> dict:
    prompt = (
        "You are a cautious medical triage assistant (not a doctor). Based "
        "only on the symptom summary below, decide whether the person "
        "should see a doctor, and how urgently. Respond in this format:\n\n"
        "Urgency: <Emergency / See a doctor soon / Monitor at home / Self-care likely sufficient>\n"
        "Recommendation: <2-4 sentences explaining the reasoning and next steps>\n"
        "Disclaimer: <one line reminding this is not a medical diagnosis>\n\n"
        f"Symptom summary:\n{state['summary']}"
    )
    response = llm.invoke(prompt)
    return {"recommendation": response.content.strip()}


def build_graph():
    graph = StateGraph(TriageState)
    graph.add_node("summarizer_agent", summarizer_agent)
    graph.add_node("advisor_agent", advisor_agent)

    graph.add_edge(START, "summarizer_agent")
    graph.add_edge("summarizer_agent", "advisor_agent")
    graph.add_edge("advisor_agent", END)

    return graph.compile()


def run(symptoms_text: str) -> TriageState:
    app = build_graph()
    session = AnchorSession(agent_version=MODEL_NAME)
    with session.track(source="startup") as s:
        session.emit_instruction_received(symptoms_text)
        return app.invoke(
            {"symptoms_text": symptoms_text, "summary": "", "recommendation": ""},
            config={"callbacks": [s.callback_handler]},
        )


SAMPLE_SYMPTOMS = """
For the past four days I've had a persistent dry cough that gets worse at night,
along with a low-grade fever that hovers around 100.4F. Yesterday I started
feeling really fatigued, my whole body aches, and I've had a mild headache
that comes and goes. I also noticed some tightness in my chest when I take a
deep breath, though it's not exactly painful. My appetite has dropped and I've
been having trouble sleeping because of the coughing fits. I don't have any
known chronic conditions, I'm 34 years old, and I haven't traveled recently.
I've been taking over-the-counter cough syrup and acetaminophen, which helps
a little but the fever keeps coming back after a few hours. This morning I
also felt a bit short of breath after climbing a flight of stairs, which is
unusual for me.
"""


if __name__ == "__main__":
    result = run(SAMPLE_SYMPTOMS)

    print("=" * 60)
    print("SUMMARY (summarizer_agent)")
    print("=" * 60)
    print(result["summary"])

    print("\n" + "=" * 60)
    print("RECOMMENDATION (advisor_agent)")
    print("=" * 60)
    print(result["recommendation"])
