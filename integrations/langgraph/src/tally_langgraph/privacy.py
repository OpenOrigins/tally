"""Local repository exclusions shared with the Rust Tally clients."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from urllib.parse import urlsplit

POLICY_PATH = Path.home() / ".config/tally/privacy.json"


def _policy() -> tuple[set[str], list[Path]]:
    try:
        value = json.loads(POLICY_PATH.read_text())
    except FileNotFoundError:
        return set(), []
    if not isinstance(value, dict):
        raise ValueError("privacy policy must be an object")
    owners = value.get("excluded_git_owners", [])
    paths = value.get("excluded_paths", [])
    if not isinstance(owners, list) or not all(isinstance(v, str) and v for v in owners):
        raise ValueError("invalid excluded_git_owners")
    if not isinstance(paths, list) or not all(isinstance(v, str) and v for v in paths):
        raise ValueError("invalid excluded_paths")
    if any(not Path(path).is_absolute() for path in paths):
        raise ValueError("excluded_paths must be absolute")
    return {owner.lower() for owner in owners}, [Path(path).resolve(strict=True) for path in paths]


def _git(path: Path, *args: str) -> str | None:
    result = subprocess.run(
        ["git", "-C", str(path), *args], capture_output=True, text=True, check=False
    )
    return result.stdout.strip() if result.returncode == 0 else None


def _remote_owner(remote: str) -> str | None:
    if "://" in remote:
        parsed = urlsplit(remote)
        if parsed.hostname is None or parsed.hostname.lower() != "github.com":
            return None
        path = parsed.path.lstrip("/")
    else:
        host, separator, path = remote.partition(":")
        if not separator or host.rsplit("@", 1)[-1].lower() != "github.com":
            return None
    parts = path.split("/")
    return parts[0].lower() if len(parts) >= 2 and parts[0] else None


def _excluded(path: Path, owners: set[str], paths: list[Path]) -> bool:
    path = path.resolve(strict=True)
    if any(path == blocked or blocked in path.parents for blocked in paths):
        return True
    if not owners:
        return False
    root = _git(path.parent if path.is_file() else path, "rev-parse", "--show-toplevel")
    if root is None:
        raise ValueError("workspace is not a classifiable Git repository")
    remotes = _git(Path(root), "remote")
    if not remotes:
        raise ValueError("Git remotes unavailable")
    for remote in remotes.splitlines():
        urls = _git(Path(root), "remote", "get-url", "--all", remote)
        if not urls:
            raise ValueError("Git remote URL unavailable")
        for url in urls.splitlines():
            owner = _remote_owner(url)
            if owner is None:
                raise ValueError("Git remote owner cannot be classified")
            if owner in owners:
                return True
    return False


def capture_blocked(path: Path | None = None) -> bool:
    try:
        owners, paths = _policy()
        if not owners and not paths:
            return False
        return _excluded(path or Path.cwd(), owners, paths)
    except (OSError, ValueError, subprocess.SubprocessError):
        return True


def delivery_blocked(record: dict[str, object]) -> bool:
    try:
        owners, paths = _policy()
        if not owners and not paths:
            return False
        workspace = record.get("workspace")
        if not isinstance(workspace, str) or not workspace:
            return True
        return _excluded(Path(workspace), owners, paths)
    except (OSError, ValueError, subprocess.SubprocessError):
        return True
