# GitHub Monitor Agent

This example shows a non-trivial LangGraph service instrumented with
[`tally-langgraph`](../../README.md). It receives GitHub webhooks, records them in a local
SQLite activity log, uses a local Ollama model to summarize activity and review pull
requests, and attaches a fresh Tally callback to every graph invocation.

The graph follows fixed routes rather than allowing the model to choose arbitrary tools:

- pull request events are classified by changed paths and summarized or reviewed;
- push diffs are scanned locally for secret-like values on added lines;
- chat questions and scheduled reports combine the SQLite log with recent commits; and
- other webhook events are logged without triggering an LLM action.

`DRY_RUN=true` is the default. In dry-run mode the example does not post GitHub comments
or send Slack/Discord messages.

## Safety scope

This is an integration example, not a replacement for GitHub secret scanning or a
production review gate. The regex scanner can produce false positives and false
negatives. It redacts matching values before logging or alerting, refuses to report an
oversized diff as clean, and never changes branch-protection settings.

Webhook requests are rejected unless `GITHUB_WEBHOOK_SECRET` is configured and the
`X-Hub-Signature-256` signature matches. Do not expose the server without TLS and a
strong webhook secret.

## Setup

Use Python 3.10 or newer. From this directory:

```bash
python -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -r requirements.txt
cp .env.example .env
```

Install and start Ollama, then pull the configured model:

```bash
ollama pull llama3.2:3b
ollama serve
```

Set at least these values in `.env`:

```dotenv
GITHUB_TOKEN=github-token
GITHUB_WEBHOOK_SECRET=long-random-webhook-secret
GITHUB_REPO_OWNER=your-org
GITHUB_REPO_NAME=your-repo
DRY_RUN=true
```

For Tally delivery, also set `TALLY_API_KEY`. Without it, Tally records remain in the
durable local journal. `TALLY_FORWARDING_ENABLED=0` explicitly disables forwarding.

The GitHub token needs `contents:read` and `pull_requests:read` while dry-run is enabled.
Live PR comments additionally need `issues:write`. Slack and Discord webhook URLs are
optional and only used when `DRY_RUN=false`.

## Run

Start the webhook server:

```bash
uvicorn github_monitor.webhook_server:app --port 8000
```

The health endpoint reports whether webhook signing is configured:

```bash
curl http://localhost:8000/health
```

Expose the server through a TLS tunnel of your choice, then create a GitHub webhook:

- payload URL: `https://your-host.example/webhook`
- content type: `application/json`
- secret: the value of `GITHUB_WEBHOOK_SECRET`
- events: pull requests, pushes, and Dependabot alerts

Run the interactive activity query CLI in another terminal:

```bash
python -m github_monitor.chat
```

The server and CLI share `github_monitor.db` by default. Override `DB_PATH` when a
different location is required. Generated databases, `.env`, bytecode, and Tally state
are ignored by Git.

## Tally instrumentation

`github_monitor.graph` creates one long-lived `TallyClient` and requests a new callback
for each invocation:

```python
from tally_langgraph import TallyClient

tally = TallyClient.from_env()
result = graph.invoke(
    inputs,
    config={"callbacks": [tally.callback(source="github-monitor:push")]},
)
```

The callback captures graph lifecycle, model usage, and LangChain tool calls. The client
is flushed and closed during FastAPI shutdown or when the interactive CLI exits.

## Architecture

```text
github_monitor/
  config.py           environment-backed settings
  db.py               SQLite event log
  event_handler.py    deterministic webhook-to-row mapping
  github_client.py    GitHub REST calls
  secrets_scanner.py  local added-line regex scan with redaction
  tools.py            LangChain tools
  graph.py            fixed LangGraph routes and Tally callback
  scheduler.py        daily report scheduler
  webhook_server.py   signed FastAPI webhook endpoint
  chat.py             interactive report client
```

## Tests

The repository test extra includes the example's dependencies. From
`integrations/langgraph`:

```bash
python -m pip install -e '.[test]'
python -m ruff format --check .
python -m ruff check .
python -m pytest
```

Tests use temporary databases, mocked GitHub/Ollama calls, and dry-run notifications;
they do not write to GitHub or external chat systems.
