# GitHub Monitor Agent

A LangGraph + LangChain agent that monitors a GitHub repository, powered entirely by a
local Ollama model (no cloud LLM calls). It:

- Reviews pull requests and posts risk-based review comments (low-risk = short summary,
  high-risk = deep diff analysis covering performance, breaking changes, missing tests).
- Scans pushes for leaked secrets (regex, local, no LLM) and simulates branch-blocking +
  Slack/Discord alerts in dry-run mode.
- Logs every GitHub event (push, PR, dependabot alert, etc.) to a local SQLite database,
  independent of what the agent decides to do with it.
- Answers ad-hoc questions like "what happened today?" via an interactive chat CLI.
- Produces an automated daily report on a schedule.

## Architecture

There is a single, always-on LangGraph pipeline (`github_monitor/graph.py`) that every
input goes through -- a GitHub webhook event, an interactive chat question, or a
scheduled daily-report trigger. Nothing bypasses it:

1. **`log_input`** (entry node) writes the input to the SQLite `.db` first, then asks the
   LLM for a one-sentence summary of it and stores that too, in the new `ai_summary`
   column.
2. A **router** looks at `input_type` (and, for `pull_request`, the webhook `action`) and
   sends the run down one fixed, pre-wired branch:
   - **`pull_request`** -> `classify_risk` -> (low risk) `low_risk_summary` or
     (medium/high) `deep_review` -> `post_review_comment`
   - **`push`** -> `fetch_push_diff` -> `scan_secrets` -> (found) `remediate` or (clean)
     `clean_note`
   - **`chat`** / **`daily_report`** -> `gather_facts` -> (empty + narrow window)
     `widen_window` retry -> `summarize_report`

The orchestrator is **not** a dynamic ReAct loop that lets an LLM pick tools/order at
runtime -- the sequence of tool calls for each situation is fixed in the graph's edges.
The LLM is only ever called to *write text* (the one-line event summary, PR review
prose, the report/chat answer), never to decide what happens next.

```
github_monitor/
  config.py          env-driven settings
  db.py               SQLite event log (init, insert, query) incl. ai_summary column
  github_client.py     thin GitHub REST API wrapper (diffs, files, comments, commits)
  secrets_scanner.py    regex-based secret detection (no LLM)
  tools.py             LangChain @tool wrappers, invoked directly (not LLM-chosen)
  graph.py             the single LangGraph StateGraph every input runs through
  event_handler.py      deterministic webhook payload -> DB log helpers, used by graph.py
  webhook_server.py     FastAPI app exposing POST /webhook, hands off to graph.run()
  scheduler.py          APScheduler daily report job, calls graph.run("daily_report")
  chat.py               interactive CLI, calls graph.run("chat", text=...) per question
```

## Setup

1. Install and start Ollama, then pull a lightweight tool-calling model (good on Apple
   Silicon, e.g. M1):

   ```bash
   ollama pull llama3.2:3b
   ollama serve   # if not already running
   ```

2. Install Python dependencies (Python 3.10+):

   ```bash
   pip install -r requirements.txt
   ```

3. Configure environment:

   ```bash
   cp .env.example .env
   ```

   Fill in `GITHUB_REPO_OWNER` / `GITHUB_REPO_NAME`, and a `GITHUB_TOKEN` (fine-grained
   PAT with `contents:read`, `pull_requests:read` scopes is enough while `DRY_RUN=true`;
   you'll need `pull_requests:write`/`issues:write` and `administration:write` once you
   flip to live mode). Leave `DRY_RUN=true` until you've watched it behave correctly.

4. Run the webhook server (this also starts the daily-report scheduler):

   ```bash
   uvicorn github_monitor.webhook_server:app --reload --port 8000
   ```

5. Expose it to GitHub (locally, via a tunnel) and register the webhook:

   ```bash
   ngrok http 8000
   ```

   In your repo's Settings -> Webhooks -> Add webhook:
   - Payload URL: `https://<your-ngrok-domain>/webhook`
   - Content type: `application/json`
   - Secret: same value as `GITHUB_WEBHOOK_SECRET` in `.env`
   - Events: select "Pull requests", "Pushes", and "Dependabot alerts" (or "Send me
     everything" if you want the agent to log other event types too, even though only
     these three currently trigger agent reasoning).

## Asking the agent what happened

```bash
python -m github_monitor.chat
```

Example prompts: `what happened in the last hour?`, `summarize today's PR activity`,
`give me the daily report`, `were there any secret alerts this week?`.

The chat CLI and the webhook server share the same SQLite DB (`DB_PATH` in `.env`), so
questions asked via chat reflect everything the webhook server has logged.

## Going live

Once you've verified behavior in dry-run mode (check the `agent_response` column in
`github_monitor.db`, or just ask the chat CLI), set `DRY_RUN=false` and fill in
`SLACK_WEBHOOK_URL` / `DISCORD_WEBHOOK_URL` to have the agent actually post PR comments,
protect branches, and send real alerts.

## Anchor hooks (Tally audit logging)

`anchor_hooks/` is a standalone, pure-Python package -- distributed separately from
this demo agent as its own `.whl` -- that gives any LangGraph agent Anchor-style audit
logging: session/heartbeat/action records plus a background daemon that ships them to
the Tally ingest API using an API key, after an initial handshake confirms the client
is connected. In this repo, it's already wired into `github_monitor/graph.py`'s
`run()` -- one import plus `AnchorCallbackHandler()` added to the existing
`callbacks` list. See [anchor_hooks/README.md](anchor_hooks/README.md) for a quick
start and [anchor_hooks/INTEGRATION.md](anchor_hooks/INTEGRATION.md) for the full
procedure to integrate the built `.whl` into any other agent, including config
reference and restart/troubleshooting notes.
