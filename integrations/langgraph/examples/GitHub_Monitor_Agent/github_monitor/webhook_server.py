import hashlib
import hmac
import json
import logging

from fastapi import BackgroundTasks, FastAPI, Header, HTTPException, Request

from . import config, db, graph
from .scheduler import start_scheduler

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("github_monitor")

app = FastAPI(title="GitHub Monitor Agent")


@app.on_event("startup")
def _startup():
    db.init_db()
    start_scheduler()
    logger.info(
        "GitHub Monitor Agent ready (dry_run=%s, model=%s, base_url=%s)",
        config.DRY_RUN, config.OLLAMA_MODEL, config.OLLAMA_BASE_URL,
    )


def _verify_signature(raw_body: bytes, signature_header: str) -> bool:
    if not config.GITHUB_WEBHOOK_SECRET:
        return True  # no secret configured -> skip verification (local/dev mode only)
    if not signature_header:
        return False
    expected = "sha256=" + hmac.new(
        config.GITHUB_WEBHOOK_SECRET.encode(), raw_body, hashlib.sha256
    ).hexdigest()
    return hmac.compare_digest(expected, signature_header)


@app.post("/webhook")
async def webhook(
    request: Request,
    background_tasks: BackgroundTasks,
    x_hub_signature_256: str = Header(None),
    x_github_event: str = Header(None),
):
    raw_body = await request.body()
    if not _verify_signature(raw_body, x_hub_signature_256):
        raise HTTPException(status_code=401, detail="Invalid webhook signature")

    payload = json.loads(raw_body or b"{}")
    event_type = x_github_event or "unknown"
    logger.info("Received GitHub event: %s (action=%s)", event_type, payload.get("action"))

    if event_type == "ping":
        return {"status": "ok", "event_type": "ping"}

    # Every event activates the same LangGraph pipeline in a background task, so GitHub's
    # webhook delivery doesn't time out waiting on a local LLM call. The pipeline's own
    # entry node is what writes the row to the .db -- nothing is logged before this.
    background_tasks.add_task(graph.run, event_type, payload)
    return {"status": "accepted", "event_type": event_type}


@app.get("/health")
def health():
    return {"status": "ok", "dry_run": config.DRY_RUN, "model": config.OLLAMA_MODEL}
