import json

from langchain_core.tools import tool

from . import config, db, github_client, secrets_scanner


def _owner_repo(owner: str = "", repo: str = ""):
    return owner or config.GITHUB_REPO_OWNER, repo or config.GITHUB_REPO_NAME


@tool
def fetch_pr_diff(pr_number: int, owner: str = "", repo: str = "") -> str:
    """Fetch the unified diff for a pull request from GitHub. Use this before doing a
    deep risk analysis of a PR's code changes (only needed for medium/high risk PRs).
    Returns the diff text, truncated if very large."""
    o, r = _owner_repo(owner, repo)
    try:
        return github_client.get_pr_diff(o, r, pr_number)
    except Exception as e:
        return f"ERROR fetching diff: {e}"


@tool
def fetch_push_diff(before: str, after: str, owner: str = "", repo: str = "") -> str:
    """Fetch the actual textual patch content (added/removed lines) for a push event,
    given the 'before' and 'after' commit SHAs from the webhook payload. Use this to get
    real code content to scan for secrets, since the push payload itself only lists
    filenames, not diff content."""
    o, r = _owner_repo(owner, repo)
    try:
        return github_client.get_compare_diff(o, r, before, after)
    except Exception as e:
        return f"ERROR fetching push diff: {e}"


@tool
def classify_pr_risk(pr_number: int, owner: str = "", repo: str = "") -> str:
    """Classify a pull request's risk level as 'low', 'medium', or 'high' based on which
    files it touches (database/schema/migrations/auth/API routes = high risk; docs/CSS/
    README = low risk). Call this first when a PR is opened or updated, before deciding
    whether a deep LLM diff analysis is warranted or a short summary is enough."""
    o, r = _owner_repo(owner, repo)
    try:
        files = github_client.get_pr_files(o, r, pr_number)
    except Exception as e:
        return f"ERROR fetching PR files: {e}"

    high_hits, low_hits, other = [], [], []
    for f in files:
        path = f.get("filename", "").lower()
        if any(p in path for p in config.HIGH_RISK_PATH_PATTERNS):
            high_hits.append(f["filename"])
        elif any(p in path for p in config.LOW_RISK_PATH_PATTERNS):
            low_hits.append(f["filename"])
        else:
            other.append(f["filename"])

    risk = "high" if high_hits else ("low" if not other else "medium")
    return json.dumps({
        "risk_level": risk,
        "high_risk_files": high_hits,
        "low_risk_files": low_hits,
        "other_files": other,
        "total_files_changed": len(files),
    })


@tool
def post_pr_review_comment(pr_number: int, body: str, owner: str = "", repo: str = "") -> str:
    """Post a structured review comment on a GitHub pull request with your summary, risk
    level, performance/breaking-change concerns, and missing-test callouts. In dry-run
    mode this only logs what would be posted and does not touch GitHub."""
    o, r = _owner_repo(owner, repo)
    result = github_client.post_pr_comment(o, r, pr_number, body)
    return json.dumps(result)


@tool
def scan_diff_for_secrets(text: str) -> str:
    """Scan diff/patch text (or any code text) for leaked secrets -- API keys, AWS keys,
    GitHub tokens, private keys, generic password/secret assignments -- using local regex
    only (no LLM call). Use this on push events before deciding whether to block the
    branch or send a notification."""
    findings = secrets_scanner.scan_text_for_secrets(text)
    if not findings:
        return "No secrets detected."
    return json.dumps(findings)


@tool
def block_branch(branch: str, reason: str, owner: str = "", repo: str = "") -> str:
    """Block/protect a branch on GitHub after a leaked secret or dangerous commit is
    found. In dry-run mode this only logs the intended action instead of calling GitHub."""
    o, r = _owner_repo(owner, repo)
    result = github_client.set_branch_protection_block(o, r, branch, reason)
    return json.dumps(result)


@tool
def notify_slack(message: str) -> str:
    """Send an alert message to Slack (e.g. a leaked secret was found, a branch was
    blocked). In dry-run mode, or when no SLACK_WEBHOOK_URL is configured, this only
    logs the message instead of sending it."""
    if config.DRY_RUN or not config.SLACK_WEBHOOK_URL:
        return f"[DRY RUN] Would notify Slack: {message}"
    import requests
    resp = requests.post(config.SLACK_WEBHOOK_URL, json={"text": message}, timeout=10)
    return f"Slack notified, status={resp.status_code}"


@tool
def notify_discord(message: str) -> str:
    """Send an alert message to Discord (e.g. a leaked secret was found, a branch was
    blocked). In dry-run mode, or when no DISCORD_WEBHOOK_URL is configured, this only
    logs the message instead of sending it."""
    if config.DRY_RUN or not config.DISCORD_WEBHOOK_URL:
        return f"[DRY RUN] Would notify Discord: {message}"
    import requests
    resp = requests.post(config.DISCORD_WEBHOOK_URL, json={"content": message}, timeout=10)
    return f"Discord notified, status={resp.status_code}"


@tool
def query_repo_events(hours: int = 24, event_type: str = "") -> str:
    """Query the local event log for what has happened in the repo (pushes, PRs, commits,
    merges, secret alerts) in the last N hours. Use this to answer questions like 'what
    happened today', 'give me a report', or 'summarize recent activity'. Pass hours=0 (or
    omit it) to ask for 'what is the latest/most recent activity' regardless of how long
    ago it was -- it returns the most recent rows with no time cutoff, it does NOT mean
    'nothing in the last 0 hours'. event_type can be one of: push, pull_request, commit,
    dependabot_alert, daily_report, or left empty for all types. For pull_request rows,
    check the action field: 'merged' means the PR was merged, 'closed' means it was
    closed without merging, 'opened' means newly opened."""
    valid_types = {"push", "pull_request", "commit", "dependabot_alert", "daily_report"}
    if event_type not in valid_types:
        event_type = None  # small models sometimes pass an unrelated word here -- ignore
    rows = db.query_events(since_hours=hours, event_type=event_type)
    if not rows:
        return f"No events found in the last {hours} hours."
    for r in rows:
        r.pop("raw_payload", None)
    return json.dumps(rows, default=str)


@tool
def list_recent_commits(branch: str = "", limit: int = 10, owner: str = "", repo: str = "") -> str:
    """List the most recent commits on GitHub for a branch (defaults to the repo's
    default branch). Use this to answer questions about recent commit activity."""
    o, r = _owner_repo(owner, repo)
    try:
        commits = github_client.list_recent_commits(o, r, branch or None, limit)
    except Exception as e:
        return f"ERROR listing commits: {e}"
    simplified = [
        {
            "sha": c["sha"][:7],
            "message": c["commit"]["message"].splitlines()[0],
            "author": (c.get("author") or {}).get("login") or c["commit"]["author"]["name"],
            "date": c["commit"]["author"]["date"],
        }
        for c in commits
    ]
    return json.dumps(simplified)
