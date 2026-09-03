import json
import logging
from typing import Optional, TypedDict

from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.messages import HumanMessage, SystemMessage
from langchain_ollama import ChatOllama
from langgraph.graph import END, START, StateGraph

from . import config, db, event_handler
from .tools import (
    block_branch,
    classify_pr_risk,
    fetch_pr_diff,
    fetch_push_diff,
    list_recent_commits,
    notify_discord,
    notify_slack,
    post_pr_review_comment,
    query_repo_events,
    scan_diff_for_secrets,
)

logger = logging.getLogger("github_monitor")

MODE_NOTE = (
    "The system is running in DRY-RUN mode: GitHub write actions and external "
    "notifications are simulated and only logged, never actually sent."
    if config.DRY_RUN
    else "The system is running in LIVE mode: GitHub write actions and notifications are real."
)

# Every input that reaches the pipeline is one of these; anything else is logged
# (in log_input) and then the pipeline ends without a dedicated reaction branch.
ROUTES = {"pull_request", "push", "chat", "daily_report"}


class StdoutCallbackHandler(BaseCallbackHandler):
    """Prints every LLM/chain/tool callback event to stdout for local visibility."""

    def on_llm_start(self, serialized, prompts, **kwargs):
        print(f"[llm start] prompts={prompts}")

    def on_llm_end(self, response, **kwargs):
        print(f"[llm end] response={response}")

    def on_llm_error(self, error, **kwargs):
        print(f"[llm error] {error}")

    def on_chain_start(self, serialized, inputs, **kwargs):
        name = (serialized or {}).get("name") or kwargs.get("name")
        print(f"[chain start] {name} inputs={inputs}")

    def on_chain_end(self, outputs, **kwargs):
        print(f"[chain end] outputs={outputs}")

    def on_chain_error(self, error, **kwargs):
        print(f"[chain error] {error}")

    def on_tool_start(self, serialized, input_str, **kwargs):
        name = (serialized or {}).get("name") or kwargs.get("name")
        print(f"[tool start] {name} input={input_str}")

    def on_tool_end(self, output, **kwargs):
        print(f"[tool end] output={output}")

    def on_tool_error(self, error, **kwargs):
        print(f"[tool error] {error}")

    def on_text(self, text, **kwargs):
        print(f"[text] {text}")


class PipelineState(TypedDict, total=False):
    input_type: str
    payload: dict
    text: str
    owner: str
    repo: str
    event_id: int
    ai_summary: str
    pr_number: int
    risk_level: str
    diff: str
    secrets_findings: list
    hours: int
    widened: bool
    events_empty: bool
    final_response: str


def _hours_from_text(text: str) -> int:
    t = (text or "").lower()
    if "week" in t:
        return 24 * 7
    if "hour" in t:
        return 1
    if "latest" in t or "most recent" in t:
        return 0
    return 24


def build_graph():
    llm = ChatOllama(
        model=config.OLLAMA_MODEL,
        base_url=config.OLLAMA_BASE_URL,
        temperature=0.2,
        callbacks=[StdoutCallbackHandler()],
    )

    def _ask(prompt: str) -> str:
        response = llm.invoke([SystemMessage(content=MODE_NOTE), HumanMessage(content=prompt)])
        return response.content

    def _owner_repo(payload: dict):
        full = payload.get("repository", {}).get("full_name", "")
        if "/" in full:
            owner, repo = full.split("/", 1)
            return owner, repo
        return config.GITHUB_REPO_OWNER, config.GITHUB_REPO_NAME

    # ---- Entry node: every input hits the .db here, before any reaction happens -------

    def log_input(state: PipelineState) -> dict:
        input_type = state["input_type"]
        payload = state.get("payload") or {}

        if input_type == "pull_request":
            event_id = event_handler.log_pull_request_event(payload)
        elif input_type == "push":
            event_id = event_handler.log_push_event(payload)
        elif input_type == "dependabot_alert":
            event_id = event_handler.log_dependabot_alert_event(payload)
        elif input_type == "chat":
            event_id = db.insert_event(event_type="chat_query", action="chat", summary=state.get("text"))
        elif input_type == "daily_report":
            event_id = db.insert_event(
                event_type="daily_report", action="scheduled", summary="Automated daily report trigger"
            )
        else:
            event_id = event_handler.log_generic_event(input_type, payload)

        summary_source = state.get("text") or json.dumps(payload)[:2000]
        try:
            ai_summary = _ask(
                f"In one short sentence, summarize this {input_type} event for someone "
                f"skimming an activity log. Be concrete and factual, do not invent details "
                f"beyond what's given.\n\n{summary_source}"
            )
        except Exception:
            logger.exception("Failed to generate AI summary for event %s", event_id)
            ai_summary = ""
        db.update_event_ai_summary(event_id, ai_summary)

        owner, repo = _owner_repo(payload)
        return {"event_id": event_id, "ai_summary": ai_summary, "owner": owner, "repo": repo}

    def route_after_log(state: PipelineState) -> str:
        input_type = state["input_type"]
        if input_type == "pull_request":
            action = state.get("payload", {}).get("action")
            if action not in event_handler.PR_ACTIONS_WORTH_REVIEWING:
                return "end"
        return input_type if input_type in ROUTES else "end"

    # ---- Fixed branch 1: PR review ------------------------------------------------------

    def classify_risk(state: PipelineState) -> dict:
        pr_number = state["payload"].get("pull_request", {}).get("number")
        result = json.loads(
            classify_pr_risk.invoke({"pr_number": pr_number, "owner": state["owner"], "repo": state["repo"]})
        )
        return {"pr_number": pr_number, "risk_level": result.get("risk_level", "medium")}

    def route_risk(state: PipelineState) -> str:
        return "low" if state.get("risk_level") == "low" else "deep"

    def low_risk_summary(state: PipelineState) -> dict:
        pr = state["payload"].get("pull_request", {})
        body = _ask(
            "Write a short, friendly PR review summary (2-4 sentences) for this low-risk "
            f"change (docs/CSS/UI-only). Title: \"{pr.get('title')}\". "
            f"Description: {pr.get('body') or '(none)'}"
        )
        return {"final_response": body}

    def deep_review(state: PipelineState) -> dict:
        diff = fetch_pr_diff.invoke({"pr_number": state["pr_number"], "owner": state["owner"], "repo": state["repo"]})
        body = _ask(
            "Review this pull request diff. It touches schema/auth/API/security-sensitive "
            f"paths (risk level: {state.get('risk_level', 'medium')}). Write a structured "
            "review comment with sections: Summary, Risk Level, Concerns, Missing Tests, "
            f"Recommendation.\n\nDiff:\n{diff}"
        )
        return {"diff": diff, "final_response": body}

    def post_review_comment(state: PipelineState) -> dict:
        post_pr_review_comment.invoke(
            {
                "pr_number": state["pr_number"],
                "body": state["final_response"],
                "owner": state["owner"],
                "repo": state["repo"],
            }
        )
        db.update_event_agent_response(state["event_id"], state["final_response"], state.get("risk_level"))
        return {}

    # ---- Fixed branch 2: push -> secret scan -------------------------------------------

    def do_fetch_push_diff(state: PipelineState) -> dict:
        payload = state["payload"]
        diff = fetch_push_diff.invoke(
            {
                "before": payload.get("before"),
                "after": payload.get("after"),
                "owner": state["owner"],
                "repo": state["repo"],
            }
        )
        return {"diff": diff}

    def do_scan_secrets(state: PipelineState) -> dict:
        result = scan_diff_for_secrets.invoke({"text": state.get("diff", "")})
        findings = [] if result == "No secrets detected." else json.loads(result)
        return {"secrets_findings": findings}

    def route_secrets(state: PipelineState) -> str:
        return "found" if state.get("secrets_findings") else "clean"

    def remediate(state: PipelineState) -> dict:
        branch = (state["payload"].get("ref") or "").split("/")[-1]
        findings = state["secrets_findings"]
        findings_desc = "; ".join(f"{f['type']} ({f['match']})" for f in findings)
        reason = f"Leaked secret(s) detected in push: {findings_desc}"

        block_branch.invoke({"branch": branch, "reason": reason, "owner": state["owner"], "repo": state["repo"]})
        message = f"Push to {state['owner']}/{state['repo']}@{branch} blocked: {findings_desc}"
        notify_slack.invoke({"message": message})
        notify_discord.invoke({"message": message})

        db.update_event_agent_response(state["event_id"], message)
        return {"final_response": message}

    def clean_note(state: PipelineState) -> dict:
        message = "Push scanned clean, no secrets detected."
        db.update_event_agent_response(state["event_id"], message)
        return {"final_response": message}

    # ---- Fixed branch 3: chat question / scheduled daily report -----------------------

    def gather_facts(state: PipelineState) -> dict:
        hours = state.get("hours", _hours_from_text(state.get("text")))
        raw_events = query_repo_events.invoke({"hours": hours, "event_type": ""})
        commits = list_recent_commits.invoke({})
        events_empty = raw_events.startswith("No events found")
        return {
            "hours": hours,
            "diff": json.dumps({"events": raw_events, "commits": commits}),
            "events_empty": events_empty,
        }

    def route_after_gather(state: PipelineState) -> str:
        # One bounded, deterministic retry with a wider window if the narrow one came up
        # empty -- mirrors the old "retry once before saying nothing happened" behavior
        # without letting an LLM decide whether to do it.
        if state.get("events_empty") and not state.get("widened") and state.get("hours", 24) < 72:
            return "widen"
        return "summarize"

    def widen_window(state: PipelineState) -> dict:
        return {"hours": 72, "widened": True}

    def summarize_report(state: PipelineState) -> dict:
        question = state.get("text") or "Generate today's daily activity report."
        body = _ask(
            f"Question/request: {question}\n\nRepo event log and recent commits (JSON):\n"
            f"{state.get('diff')}\n\nWrite a clear, concise, human-readable answer grounded "
            "only in this data, grouped by category if it's a report. If the data is "
            f"empty even after checking the last {state.get('hours', 24)} hours, say so plainly."
        )
        db.update_event_agent_response(state["event_id"], body)
        return {"final_response": body}

    graph = StateGraph(PipelineState)
    graph.add_node("log_input", log_input)
    graph.add_node("classify_risk", classify_risk)
    graph.add_node("low_risk_summary", low_risk_summary)
    graph.add_node("deep_review", deep_review)
    graph.add_node("post_review_comment", post_review_comment)
    graph.add_node("fetch_push_diff", do_fetch_push_diff)
    graph.add_node("scan_secrets", do_scan_secrets)
    graph.add_node("remediate", remediate)
    graph.add_node("clean_note", clean_note)
    graph.add_node("gather_facts", gather_facts)
    graph.add_node("widen_window", widen_window)
    graph.add_node("summarize_report", summarize_report)

    graph.add_edge(START, "log_input")
    graph.add_conditional_edges(
        "log_input",
        route_after_log,
        {
            "pull_request": "classify_risk",
            "push": "fetch_push_diff",
            "chat": "gather_facts",
            "daily_report": "gather_facts",
            "end": END,
        },
    )

    graph.add_conditional_edges("classify_risk", route_risk, {"low": "low_risk_summary", "deep": "deep_review"})
    graph.add_edge("low_risk_summary", "post_review_comment")
    graph.add_edge("deep_review", "post_review_comment")
    graph.add_edge("post_review_comment", END)

    graph.add_edge("fetch_push_diff", "scan_secrets")
    graph.add_conditional_edges("scan_secrets", route_secrets, {"found": "remediate", "clean": "clean_note"})
    graph.add_edge("remediate", END)
    graph.add_edge("clean_note", END)

    graph.add_conditional_edges("gather_facts", route_after_gather, {"widen": "widen_window", "summarize": "summarize_report"})
    graph.add_edge("widen_window", "gather_facts")
    graph.add_edge("summarize_report", END)

    return graph.compile()


_pipeline = None


def get_agent():
    global _pipeline
    if _pipeline is None:
        _pipeline = build_graph()
    return _pipeline


def run(input_type: str, payload: Optional[dict] = None, text: Optional[str] = None) -> dict:
    """The one entry point into the system. Every input -- a GitHub webhook event, an
    interactive chat question, or a scheduled daily-report trigger -- activates this
    graph, whose first node (log_input) is what writes the row to the .db. The
    orchestrator does not choose tools/order dynamically: each input_type follows a
    fixed, pre-wired sequence of nodes."""
    return get_agent().invoke(
        {"input_type": input_type, "payload": payload or {}, "text": text or ""},
        config={"callbacks": [StdoutCallbackHandler()]},
    )
