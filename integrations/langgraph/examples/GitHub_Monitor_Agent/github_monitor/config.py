"""Environment-backed configuration for the GitHub monitor example."""

from __future__ import annotations

import os
from pathlib import Path

from dotenv import load_dotenv

EXAMPLE_ROOT = Path(__file__).resolve().parent.parent
load_dotenv(EXAMPLE_ROOT / ".env")


def _env_bool(name: str, default: bool) -> bool:
    value = os.getenv(name)
    if value is None:
        return default
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise ValueError(f"{name} must be a boolean value")


GITHUB_TOKEN = os.getenv("GITHUB_TOKEN", "")
GITHUB_WEBHOOK_SECRET = os.getenv("GITHUB_WEBHOOK_SECRET", "")
GITHUB_REPO_OWNER = os.getenv("GITHUB_REPO_OWNER", "")
GITHUB_REPO_NAME = os.getenv("GITHUB_REPO_NAME", "")

OLLAMA_MODEL = os.getenv("OLLAMA_MODEL", "llama3.2:3b")
OLLAMA_BASE_URL = os.getenv("OLLAMA_BASE_URL", "http://localhost:11434")

DRY_RUN = _env_bool("DRY_RUN", True)

SLACK_WEBHOOK_URL = os.getenv("SLACK_WEBHOOK_URL", "")
DISCORD_WEBHOOK_URL = os.getenv("DISCORD_WEBHOOK_URL", "")

DB_PATH = os.getenv("DB_PATH", str(EXAMPLE_ROOT / "github_monitor.db"))
DAILY_REPORT_TIME = os.getenv("DAILY_REPORT_TIME", "18:00")

GITHUB_API_BASE = "https://api.github.com"

HIGH_RISK_PATH_PATTERNS = [
    "migrations/",
    "schema",
    "models/",
    "auth",
    "security",
    "routes/",
    "api/",
    "controllers/",
    "middleware",
    ".sql",
    "permissions",
    "payment",
    "billing",
]
LOW_RISK_PATH_PATTERNS = [
    ".md",
    "docs/",
    ".css",
    ".scss",
    "readme",
    "changelog",
    ".txt",
    "license",
]


def repository() -> tuple[str, str]:
    """Return the configured repository or fail before making a malformed API call."""

    if not GITHUB_REPO_OWNER or not GITHUB_REPO_NAME:
        raise RuntimeError("GITHUB_REPO_OWNER and GITHUB_REPO_NAME must be configured")
    return GITHUB_REPO_OWNER, GITHUB_REPO_NAME
