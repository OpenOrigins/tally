# Integrating anchor-hooks into any agent

This is the detailed procedure for wiring the `anchor-hooks` build into a LangGraph/
LangChain-based agent that isn't this repo -- e.g. handing the `.whl` to another team.
For a quick reference, see [README.md](README.md); this document goes deeper on config,
what gets logged, and troubleshooting.

## What you're integrating

`anchor-hooks` is a self-contained Python package (built as `anchor_hooks-0.1.0-py3-none-any.whl`)
that gives any LangGraph agent Anchor-style audit logging: every tool call, subagent
handoff, and session boundary is recorded locally and shipped to a Tally ingest server
over HTTPS using an API key. It has two independent pieces:

1. **`AnchorCallbackHandler`** -- a LangChain callback handler. Add one instance to a
   compiled graph's existing `callbacks` list and it captures the whole run.
2. **Background daemons** (heartbeat + log forwarder) -- spawned automatically the first
   time the handler sees a run start; no separate process to manage by hand.

## Prerequisites

- Python 3.10+
- An agent built on LangGraph and/or LangChain, with at least one place that calls
  `.invoke(...)` or `.stream(...)` on a compiled graph / runnable, and a `config=` dict
  you control at that call site (or can add one to).
- A Tally API key (ask whoever is operating the ingest server for one).

If your agent has no LangChain Runnable at all (no `.invoke()`/`.stream()` call
anywhere), see [No LangChain Runnable? (AnchorSession)](#no-langchain-runnable-anchorsession)
near the end.

## Step 1 -- install the wheel

```bash
pip install anchor_hooks-0.1.0-py3-none-any.whl
```

This installs the `anchor_hooks` Python package plus an `anchor-hooks` console script,
and pulls in its own dependencies (`requests`, `pyyaml`, `python-dotenv`, `langchain-core`)
-- nothing else from the source repo is needed.

## Step 2 -- scaffold config

From your agent's project root:

```bash
anchor-hooks init
```

This writes `anchor_config.yaml` (if one doesn't already exist) and prints the
integration snippet from Step 4. Open `anchor_config.yaml` and set at least:

```yaml
agent_id: local-agent:YOUR-AGENT-NAME
principal_id: user:YOUR-TEAM-OR-EMAIL
```

Everything else in the file has a reasonable default (see
[Config reference](#config-reference) below).

## Step 3 -- connect: save the API key and handshake with the server

```bash
anchor-hooks connect --api-key <YOUR-API-KEY>
```

This does two things:
- Writes `TALLY_API_KEY=<key>` into your project's `.env` (creating it if needed,
  preserving every other line already in it).
- POSTs a one-time "client connected" notification to the server using that key, so the
  server can confirm this client is wired up before any log records arrive.

The handshake is best-effort: if the server is unreachable right now, the key is still
saved and log shipping still works once it is reachable -- this step is not a
precondition for logging to function, just a courtesy notification.

If you need a non-default ingest endpoint:

```bash
anchor-hooks connect --api-key <YOUR-API-KEY> --api-url https://your-tally-host/v1/tally/logs
```

This is saved as `TALLY_API_URL` in the same `.env`. Unless told otherwise, leave it
unset -- `https://api.dev2.openorigins.com/v1/tally/logs` is the default and the only
ingest URL referenced anywhere in the reference installer this package was ported from.

## Step 4 -- add the handler to your agent (the only code change)

Add **one entry** to whatever `callbacks` list you already pass into your compiled
graph's invoke/stream call. If you don't already build a `callbacks` list, add one.
Nothing else about your agent's code changes -- no wrapping, no explicit
start/end calls, no change to how or where you call `.invoke()`:

```python
from anchor_hooks import AnchorCallbackHandler

result = app.invoke(inputs, config={"callbacks": [AnchorCallbackHandler()]})
```

If you already pass other callbacks, just append to that same list:

```python
result = app.invoke(
    inputs,
    config={"callbacks": [MyExistingHandler(), AnchorCallbackHandler(source="my-event-type")]},
)
```

**Construct a fresh `AnchorCallbackHandler()` per invocation**, not a shared singleton
reused across many calls. It derives SESSION_START/SESSION_END and
INSTRUCTION_RECEIVED from the `on_chain_start`/`on_chain_end` callback events LangChain
already fires for the *root* run around a single `.invoke()`/`.stream()` call, so one
instance tracks exactly one run. This is also why no `AnchorSession`/`session.track()`
wrapper is needed for LangChain-based agents -- the handler is self-sufficient.

The optional `source=` argument is just a label recorded on SESSION_START (e.g. the
kind of event that triggered this run -- `"webhook"`, `"chat"`, `"scheduled"`, whatever
makes sense for your agent). Other optional constructor args: `agent_id=`,
`agent_version=`, `principal_id=`, `config_path=` (all override the corresponding
`anchor_config.yaml`/env value for just this instance).

That's the entire integration. Steps 1-3 are one-time setup; step 4 is the only line
that touches your agent's code, and it's copy-pasteable regardless of what your graph
actually does internally.

## What gets logged

| Event | Record type | Trigger |
|---|---|---|
| Run starts | `SESSION_START` | Root `on_chain_start` (your `.invoke()`/`.stream()` call) |
| Run's input | `INSTRUCTION_RECEIVED` | Same root call, `inputs` truncated to 500 chars |
| A tool is called | `ACTION_TAKEN` | `on_tool_start` |
| A tool returns/errors | `RESULT_RECEIVED` | `on_tool_end` / `on_tool_error` |
| Control passes into a compiled subgraph node | `HANDOFF` | Best-effort, via LangGraph's internal `langgraph_checkpoint_ns` metadata -- see the docstring in `callback_handler.py` for caveats; re-verify after any LangGraph upgrade using `anchor_hooks/tests/test_handoff.py` |
| Run ends | `SESSION_END` | Root `on_chain_end` / `on_chain_error` (`outcome: completed` or `error`) |
| Liveness ping | `HEARTBEAT` | Every `heartbeat_interval_seconds` while a session is open |

Every record is appended as one JSON line to `logs/log.jsonl` (and mirrored into
`logs/log.sqlite`) under whatever `log_dir` is configured, then shipped to the ingest
API by the forwarder daemon, one record at a time, in file order, with the API key
attached as the `x-api-key` header.

## How delivery works

- **Heartbeat daemon**: one per session, spawned on SESSION_START, stopped on
  SESSION_END (or after `heartbeat_max_hours` as a safety net if SESSION_END never
  fires -- e.g. the process was killed).
- **Log forwarder daemon**: a single long-lived process shared across every session
  (not one per session). Spawned opportunistically on the first SESSION_START if one
  isn't already running (tracked via a pid file), and keeps running independently of
  any one session so it can catch up on anything queued while the API was unreachable.
  It tails `logs/log.jsonl` from a persisted byte offset, so a crash or API outage just
  pauses shipping -- it never skips a record (at-least-once delivery, not
  exactly-once: at most the one record in flight when it died gets re-sent).
- The API key is re-read from `.env`/the environment on every drain cycle (every 5
  seconds), so rotating it via `anchor-hooks connect --api-key <NEW-KEY>` takes effect
  on the already-running forwarder daemon -- no restart required for a key or URL
  change specifically.

## Config reference

`anchor_config.yaml` (all keys optional, shown with defaults):

```yaml
agent_id: local-agent:langgraph-agent
agent_version: unknown
principal_id: user:unknown
log_dir: logs
heartbeat_interval_seconds: 60
heartbeat_max_hours: 6
forwarder:
  api_url: https://api.dev2.openorigins.com/v1/tally/logs
  max_hours: 24
```

Environment variable overrides (highest precedence, read from `.env` or the real
environment): `ANCHOR_AGENT_ID`, `ANCHOR_AGENT_VERSION`, `TALLY_USER_EMAIL` (sets
`principal_id` to `user:<email>`), `TALLY_API_URL`, `TALLY_API_KEY`, `ANCHOR_CONFIG_PATH`
(path to a non-default `anchor_config.yaml`).

## Restarting after config/code changes

| You changed... | What to do |
|---|---|
| `TALLY_API_KEY` / `TALLY_API_URL` in `.env` | Nothing -- the forwarder daemon re-reads `.env` every drain cycle. |
| `anchor_config.yaml` (agent_id, principal_id, heartbeat interval, etc.) | Nothing -- read fresh from disk on every `AnchorCallbackHandler()` construction, i.e. every new run. |
| Your agent's own code | Whatever your agent's own reload story already is (e.g. `uvicorn --reload`). |
| The `anchor_hooks` package itself (upgraded the wheel, edited its source) | Restart your agent process, **and** kill the long-lived forwarder daemon so a fresh one respawns with the new code: `pkill -f anchor_hooks.log_forwarder`. Heartbeat daemons don't need this -- each is spawned fresh (`python -m anchor_hooks.heartbeat_daemon`) per session anyway. |

If a forwarder daemon seems stuck (not shipping despite a valid key), check
`logs/.state/forwarder_errors.log` for the last error, and confirm it's actually
running: `pgrep -fl anchor_hooks.log_forwarder`.

## No LangChain Runnable? (`AnchorSession`)

If your agent has no LangChain `.invoke()`/`.stream()` call anywhere (so there's no
callback flow to attach a handler to at all), use `AnchorSession` instead for explicit,
manual control:

```python
from anchor_hooks import AnchorSession

session = AnchorSession()
with session.track(source="startup") as s:
    instruction_id = s.emit_instruction_received(user_input_text)
    # ... do the work ...
    s.emit_action_taken(action_id="act_1", tool_name="my_tool", params=str(args))
    s.emit_result_received(action_id="act_1", summary=str(result), exception=False)
```

This is the exception, not the default -- prefer `AnchorCallbackHandler` (Step 4
above) for any LangGraph/LangChain agent.

## Building the wheel (for whoever is distributing this)

From this repo's root (`GitHub_Monitor/`):

```bash
pip install build
python -m build --wheel
```

The wheel lands in `dist/anchor_hooks-0.1.0-py3-none-any.whl`, self-contained --
hand it to anyone with Python 3.10+ and Steps 1-4 above are all they need, regardless
of what their agent does internally.
