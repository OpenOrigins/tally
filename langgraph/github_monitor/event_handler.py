from . import db


def _repo_full_name(payload: dict) -> str:
    return payload.get("repository", {}).get("full_name", "unknown/unknown")


# ---- Deterministic logging (always runs, independent of the agent) ----------------

def log_pull_request_event(payload: dict) -> int:
    action = payload.get("action")
    pr = payload.get("pull_request", {})
    # GitHub has no separate "merge" webhook event -- a merge arrives as a pull_request
    # event with action=closed and merged=true. Relabel it so reports/queries can tell
    # a merge apart from a PR that was just closed without merging.
    if action == "closed" and pr.get("merged"):
        action = "merged"
    return db.insert_event(
        event_type="pull_request",
        action=action,
        repo=_repo_full_name(payload),
        actor=payload.get("sender", {}).get("login"),
        pr_number=pr.get("number"),
        summary=pr.get("title"),
        raw_payload=payload,
    )


def log_push_event(payload: dict) -> int:
    commits = payload.get("commits", [])
    repo_full = _repo_full_name(payload)
    ref = payload.get("ref")
    push_event_id = db.insert_event(
        event_type="push",
        action="push",
        repo=repo_full,
        actor=payload.get("pusher", {}).get("name") or payload.get("sender", {}).get("login"),
        ref=ref,
        commit_sha=payload.get("after"),
        summary=f"{len(commits)} commit(s) pushed to {ref}",
        raw_payload=payload,
    )
    # Also log each individual commit as its own row so "all commits" is queryable
    # on its own, not just bundled into the parent push's count.
    for c in commits:
        db.insert_event(
            event_type="commit",
            action="commit",
            repo=repo_full,
            actor=(c.get("author") or {}).get("name"),
            ref=ref,
            commit_sha=c.get("id"),
            summary=(c.get("message") or "").splitlines()[0] if c.get("message") else None,
        )
    return push_event_id


def log_dependabot_alert_event(payload: dict) -> int:
    alert = payload.get("alert", {})
    return db.insert_event(
        event_type="dependabot_alert",
        action=payload.get("action"),
        repo=_repo_full_name(payload),
        actor=payload.get("sender", {}).get("login"),
        summary=alert.get("security_advisory", {}).get("summary"),
        raw_payload=payload,
    )


def log_generic_event(event_type: str, payload: dict) -> int:
    return db.insert_event(
        event_type=event_type,
        action=payload.get("action"),
        repo=_repo_full_name(payload),
        actor=payload.get("sender", {}).get("login"),
        raw_payload=payload,
    )


# Only these pull_request actions get a review reaction from the pipeline; others
# (closed, labeled, assigned, etc.) are still logged above but the graph's router
# sends them straight to END with no classify_risk/review branch.
PR_ACTIONS_WORTH_REVIEWING = {"opened", "synchronize", "reopened", "ready_for_review"}
