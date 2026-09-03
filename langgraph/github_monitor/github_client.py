import requests

from . import config


def _headers(diff: bool = False) -> dict:
    headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
    if diff:
        headers["Accept"] = "application/vnd.github.v3.diff"
    if config.GITHUB_TOKEN:
        headers["Authorization"] = f"Bearer {config.GITHUB_TOKEN}"
    return headers


def get_pr_diff(owner: str, repo: str, pr_number: int, max_chars: int = 8000) -> str:
    url = f"{config.GITHUB_API_BASE}/repos/{owner}/{repo}/pulls/{pr_number}"
    resp = requests.get(url, headers=_headers(diff=True), timeout=15)
    resp.raise_for_status()
    diff = resp.text
    if len(diff) > max_chars:
        diff = diff[:max_chars] + f"\n... [truncated, {len(diff) - max_chars} more chars]"
    return diff


def get_pr_files(owner: str, repo: str, pr_number: int) -> list:
    url = f"{config.GITHUB_API_BASE}/repos/{owner}/{repo}/pulls/{pr_number}/files"
    resp = requests.get(url, headers=_headers(), timeout=15)
    resp.raise_for_status()
    return resp.json()


def post_pr_comment(owner: str, repo: str, pr_number: int, body: str) -> dict:
    if config.DRY_RUN:
        return {"dry_run": True, "would_post_comment_on": f"{owner}/{repo}#{pr_number}", "body": body}
    url = f"{config.GITHUB_API_BASE}/repos/{owner}/{repo}/issues/{pr_number}/comments"
    resp = requests.post(url, headers=_headers(), json={"body": body}, timeout=15)
    resp.raise_for_status()
    return resp.json()


def list_recent_commits(owner: str, repo: str, branch: str = None, limit: int = 10) -> list:
    url = f"{config.GITHUB_API_BASE}/repos/{owner}/{repo}/commits"
    params = {"per_page": limit}
    if branch:
        params["sha"] = branch
    resp = requests.get(url, headers=_headers(), params=params, timeout=15)
    resp.raise_for_status()
    return resp.json()


def get_compare_diff(owner: str, repo: str, base: str, head: str, max_chars: int = 8000) -> str:
    """Fetch the textual patch content between two commits (used to scan a push's actual
    added/removed lines for secrets, since the push webhook payload itself only carries
    commit metadata, not diff content)."""
    url = f"{config.GITHUB_API_BASE}/repos/{owner}/{repo}/compare/{base}...{head}"
    resp = requests.get(url, headers=_headers(), timeout=15)
    resp.raise_for_status()
    data = resp.json()
    patches = []
    for f in data.get("files", []):
        patch = f.get("patch")
        if patch:
            patches.append(f"--- {f['filename']} ---\n{patch}")
    text = "\n\n".join(patches)
    if len(text) > max_chars:
        text = text[:max_chars] + f"\n... [truncated, {len(text) - max_chars} more chars]"
    return text or "(no textual patch content available - binary files, or an empty diff)"


def set_branch_protection_block(owner: str, repo: str, branch: str, reason: str) -> dict:
    if config.DRY_RUN:
        return {"dry_run": True, "would_block_branch": branch, "reason": reason}
    url = f"{config.GITHUB_API_BASE}/repos/{owner}/{repo}/branches/{branch}/protection"
    payload = {
        "required_status_checks": None,
        "enforce_admins": True,
        "required_pull_request_reviews": None,
        "restrictions": None,
    }
    resp = requests.put(url, headers=_headers(), json=payload, timeout=15)
    resp.raise_for_status()
    return resp.json()
