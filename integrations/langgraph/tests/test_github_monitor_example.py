import hashlib
import hmac
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
from fastapi.testclient import TestClient
from langchain_core.messages import AIMessage

from examples.GitHub_Monitor_Agent.github_monitor import (
    config,
    db,
    event_handler,
    github_client,
    graph,
    scheduler,
    secrets_scanner,
    tools,
    webhook_server,
)


@pytest.fixture
def monitor_db(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    path = tmp_path / "monitor.sqlite3"
    monkeypatch.setattr(config, "DB_PATH", str(path))
    db.init_db()
    return path


def test_config_requires_a_complete_repository(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(config, "GITHUB_REPO_OWNER", "")
    monkeypatch.setattr(config, "GITHUB_REPO_NAME", "repo")
    with pytest.raises(RuntimeError, match="must be configured"):
        config.repository()

    monkeypatch.setattr(config, "GITHUB_REPO_OWNER", "openorigins")
    assert config.repository() == ("openorigins", "repo")


def test_database_records_and_filters_events(monitor_db: Path) -> None:
    event_id = db.insert_event(
        event_type="push",
        action="push",
        repo="OpenOrigins/tally",
        raw_payload={"after": "abc"},
    )
    db.update_event_ai_summary(event_id, "A push happened")
    db.update_event_agent_response(event_id, "Scanned clean", "low")

    rows = db.query_events(event_type="push")

    assert monitor_db.exists()
    assert len(rows) == 1
    assert rows[0]["ai_summary"] == "A push happened"
    assert rows[0]["agent_response"] == "Scanned clean"
    assert rows[0]["risk_level"] == "low"
    assert json.loads(rows[0]["raw_payload"]) == {"after": "abc"}


def test_push_handler_records_parent_and_commits(monitor_db: Path) -> None:
    parent_id = event_handler.log_push_event(
        {
            "ref": "refs/heads/feature/example",
            "after": "abc123",
            "repository": {"full_name": "OpenOrigins/tally"},
            "pusher": {"name": "octocat"},
            "commits": [
                {"id": "abc123", "message": "Add example\n\nDetails", "author": {"name": "A"}},
                {"id": "def456", "message": "Add tests", "author": {"name": "B"}},
            ],
        }
    )

    rows = db.query_events(since_hours=0)

    assert parent_id > 0
    assert [row["event_type"] for row in rows].count("commit") == 2
    assert next(row for row in rows if row["id"] == parent_id)["summary"] == (
        "2 commit(s) pushed to refs/heads/feature/example"
    )


def test_secret_scanner_only_reports_added_lines_and_redacts_values() -> None:
    removed = "ghp_" + "A" * 36
    added = "sk-proj-" + "B" * 24
    diff = f"--- a/.env\n+++ b/.env\n-{removed}\n+OPENAI_API_KEY='{added}'\n context"

    findings = secrets_scanner.scan_diff_for_secrets(diff)
    serialized = json.dumps(findings)

    assert {finding["type"] for finding in findings} == {
        "OpenAI API Key",
        "Generic Secret Assignment",
    }
    assert removed not in serialized
    assert added not in serialized
    assert "<redacted>" in serialized or "..." in serialized


def test_compare_diff_refuses_truncation(monkeypatch: pytest.MonkeyPatch) -> None:
    response = Mock()
    response.raise_for_status.return_value = None
    response.json.return_value = {"files": [{"filename": "large.txt", "patch": "+" + "x" * 20}]}
    monkeypatch.setattr(github_client.requests, "get", Mock(return_value=response))

    with pytest.raises(ValueError, match="too large to scan safely"):
        github_client.get_compare_diff("OpenOrigins", "tally", "a", "b", max_chars=10)


def test_pr_file_listing_is_paginated(monkeypatch: pytest.MonkeyPatch) -> None:
    first = Mock()
    first.raise_for_status.return_value = None
    first.json.return_value = [{"filename": f"file-{index}"} for index in range(100)]
    second = Mock()
    second.raise_for_status.return_value = None
    second.json.return_value = [{"filename": "last"}]
    request = Mock(side_effect=[first, second])
    monkeypatch.setattr(github_client.requests, "get", request)

    files = github_client.get_pr_files("OpenOrigins", "tally", 1)

    assert len(files) == 101
    assert request.call_args_list[0].kwargs["params"]["page"] == 1
    assert request.call_args_list[1].kwargs["params"]["page"] == 2


def test_tools_propagate_github_failures(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        github_client,
        "get_compare_diff",
        Mock(side_effect=RuntimeError("GitHub unavailable")),
    )

    with pytest.raises(RuntimeError, match="GitHub unavailable"):
        tools.fetch_push_diff.invoke(
            {"before": "a", "after": "b", "owner": "OpenOrigins", "repo": "tally"}
        )


def _signature(secret: str, body: bytes) -> str:
    return "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()


def test_signature_verification_is_fail_closed(monkeypatch: pytest.MonkeyPatch) -> None:
    body = b'{"zen":"safe"}'
    monkeypatch.setattr(config, "GITHUB_WEBHOOK_SECRET", "")
    assert not webhook_server._verify_signature(body, None)

    monkeypatch.setattr(config, "GITHUB_WEBHOOK_SECRET", "secret")
    assert not webhook_server._verify_signature(body, "sha256=bad")
    assert webhook_server._verify_signature(body, _signature("secret", body))


def test_webhook_validates_and_dispatches_signed_payload(monkeypatch: pytest.MonkeyPatch) -> None:
    secret = "webhook-secret"
    body = json.dumps(
        {
            "action": "opened",
            "repository": {"full_name": "OpenOrigins/tally"},
            "pull_request": {"number": 45, "title": "Example"},
        }
    ).encode()
    run = Mock(return_value={})
    monkeypatch.setattr(config, "GITHUB_WEBHOOK_SECRET", secret)
    monkeypatch.setattr(webhook_server.db, "init_db", Mock())
    monkeypatch.setattr(webhook_server, "start_scheduler", Mock())
    monkeypatch.setattr(webhook_server, "stop_scheduler", Mock())
    monkeypatch.setattr(webhook_server.graph, "run", run)
    monkeypatch.setattr(webhook_server.graph, "close", Mock())

    with TestClient(webhook_server.app) as client:
        response = client.post(
            "/webhook",
            content=body,
            headers={
                "content-type": "application/json",
                "x-github-event": "pull_request",
                "x-hub-signature-256": _signature(secret, body),
            },
        )
        unsigned = client.post("/webhook", content=body)
        invalid_json_body = b"not-json"
        invalid_json = client.post(
            "/webhook",
            content=invalid_json_body,
            headers={
                "x-github-event": "push",
                "x-hub-signature-256": _signature(secret, invalid_json_body),
            },
        )

    assert response.status_code == 200
    assert response.json() == {"status": "accepted", "event_type": "pull_request"}
    assert unsigned.status_code == 401
    assert invalid_json.status_code == 400
    run.assert_called_once()
    assert run.call_args.args[0] == "pull_request"


class FakeLlm:
    def __init__(self) -> None:
        self.prompts: list[list] = []

    def invoke(self, messages: list) -> AIMessage:
        self.prompts.append(messages)
        return AIMessage(content="local model response")


def test_push_graph_redacts_secret_and_records_alert(
    monitor_db: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    secret = "ghp_" + "Z" * 36
    monkeypatch.setattr(
        graph,
        "fetch_push_diff",
        SimpleNamespace(invoke=lambda _inputs: f"--- a/.env\n+++ b/.env\n+TOKEN='{secret}'"),
    )
    alerts: list[str] = []
    notifier = SimpleNamespace(invoke=lambda inputs: alerts.append(inputs["message"]))
    monkeypatch.setattr(graph, "notify_slack", notifier)
    monkeypatch.setattr(graph, "notify_discord", notifier)

    pipeline = graph.build_graph(FakeLlm())
    result = pipeline.invoke(
        {
            "input_type": "push",
            "payload": {
                "before": "a",
                "after": "b",
                "ref": "refs/heads/feature/example",
                "repository": {"full_name": "OpenOrigins/tally"},
                "commits": [],
            },
            "text": "",
        }
    )

    assert "feature/example" in result["final_response"]
    assert secret not in result["final_response"]
    assert len(alerts) == 2
    assert all(secret not in alert for alert in alerts)
    row = next(row for row in db.query_events(since_hours=0) if row["event_type"] == "push")
    assert secret not in row["agent_response"]


def test_pull_request_graph_posts_dry_run_review(
    monitor_db: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        graph,
        "classify_pr_risk",
        SimpleNamespace(invoke=lambda _inputs: json.dumps({"risk_level": "low"})),
    )
    posted: list[dict] = []
    monkeypatch.setattr(
        graph,
        "post_pr_review_comment",
        SimpleNamespace(invoke=lambda inputs: posted.append(inputs)),
    )

    pipeline = graph.build_graph(FakeLlm())
    result = pipeline.invoke(
        {
            "input_type": "pull_request",
            "payload": {
                "action": "opened",
                "repository": {"full_name": "OpenOrigins/tally"},
                "pull_request": {"number": 46, "title": "Docs", "body": "Update docs"},
            },
            "text": "",
        }
    )

    assert result["risk_level"] == "low"
    assert posted[0]["pr_number"] == 46
    assert posted[0]["body"] == "local model response"


def test_run_attaches_fresh_tally_callback(monkeypatch: pytest.MonkeyPatch) -> None:
    callback = object()
    client = Mock()
    client.callback.return_value = callback
    agent = Mock()
    agent.invoke.return_value = {"final_response": "ok"}
    monkeypatch.setattr(graph, "get_tally_client", Mock(return_value=client))
    monkeypatch.setattr(graph, "get_agent", Mock(return_value=agent))

    result = graph.run("chat", text="what happened?")

    assert result == {"final_response": "ok"}
    client.callback.assert_called_once_with(source="github-monitor:chat")
    assert agent.invoke.call_args.kwargs["config"] == {"callbacks": [callback]}


def test_scheduler_rejects_invalid_time(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(config, "DAILY_REPORT_TIME", "25:90")
    monkeypatch.setattr(scheduler, "_scheduler", None)

    with pytest.raises(ValueError, match="valid 24-hour time"):
        scheduler.start_scheduler()
