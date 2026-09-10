import os

from dotenv import load_dotenv

load_dotenv()

GITHUB_TOKEN = os.getenv("GITHUB_TOKEN", "")
GITHUB_WEBHOOK_SECRET = os.getenv("GITHUB_WEBHOOK_SECRET", "")
GITHUB_REPO_OWNER = os.getenv("GITHUB_REPO_OWNER", "")
GITHUB_REPO_NAME = os.getenv("GITHUB_REPO_NAME", "")

OLLAMA_MODEL = os.getenv("OLLAMA_MODEL", "llama3.2:3b")
OLLAMA_BASE_URL = os.getenv("OLLAMA_BASE_URL", "http://localhost:11434")

DRY_RUN = os.getenv("DRY_RUN", "true").lower() == "true"

SLACK_WEBHOOK_URL = os.getenv("SLACK_WEBHOOK_URL", "")
DISCORD_WEBHOOK_URL = os.getenv("DISCORD_WEBHOOK_URL", "")

DB_PATH = os.getenv("DB_PATH", "./github_monitor.db")
DAILY_REPORT_TIME = os.getenv("DAILY_REPORT_TIME", "18:00")

GITHUB_API_BASE = "https://api.github.com"

# Heuristic path signals used by the classify_pr_risk tool. This is a fast, deterministic
# pre-filter the LLM orchestrator can call before deciding whether a deep review is needed.
HIGH_RISK_PATH_PATTERNS = [
    "migrations/", "schema", "models/", "auth", "security",
    "routes/", "api/", "controllers/", "middleware", ".sql",
    "permissions", "payment", "billing",
]
LOW_RISK_PATH_PATTERNS = [
    ".md", "docs/", ".css", ".scss", "readme", "changelog",
    ".txt", "license",
]
