# Anchor Hooks — Setup Guide for Your LangGraph Agent

This package adds activity logging to a LangGraph agent: session lifecycle,
instructions received, tool actions taken, agent-to-agent handoffs, and a
periodic liveness heartbeat — written locally and optionally forwarded to
Tally. It is designed to attach to your existing agent without needing to
restructure it.

## 1. What you were given

- `anchor_hooks-0.1.0-py3-none-any.whl` — the installable package.
- This guide.

## 2. Prerequisites

- Python 3.10+
- Your agent already depends on `langgraph` and `langchain-core` (both
  `>=1.0,<2`). This has been built and tested against `langgraph==1.2.9` /
  `langchain-core==1.5.1` — see **Section 7** if you're on a different major
  version.
- Outbound HTTPS access to your Tally ingest endpoint, if you intend to
  enable log forwarding (default `https://api.dev2.openorigins.com/v1/tally/logs`).

## 3. Install

```bash
pip install anchor_hooks-0.1.0-py3-none-any.whl
anchor-hooks init
```

`anchor-hooks init` (run from your project root) creates:

```
your-project/
├── anchor_config.yaml            # configuration (edit this)
├── logs/
│   ├── log.jsonl                 # created on first run
│   ├── log.sqlite                # created on first run
│   ├── .state/                   # PID files, forwarder offset (internal)
│   └── .anchor_state/
│       └── api_key.txt           # put your real Tally API key here
```

Re-running `anchor-hooks init` is safe — it won't overwrite an existing
`anchor_config.yaml` or a non-empty `api_key.txt`.

## 4. Configure

Edit `anchor_config.yaml`:

```yaml
agent_id: local-agent:your-agent-name
agent_version: unknown        # e.g. your model name/build version — see 4.1
principal_id: user:you@your-domain.com
log_dir: logs
heartbeat_interval_seconds: 60
heartbeat_max_hours: 6
forwarder:
  api_url: https://api.dev2.openorigins.com/v1/tally/logs
  api_key_file: logs/.anchor_state/api_key.txt
  max_hours: 24
```

Every field can also be set via environment variable instead of editing the
file, which is usually cleaner for deployment (env var wins if both are set):

| Env var | Overrides |
|---|---|
| `ANCHOR_CONFIG_PATH` | Path to the YAML file itself (default `anchor_config.yaml`) |
| `ANCHOR_AGENT_ID` | `agent_id` |
| `ANCHOR_AGENT_VERSION` | `agent_version` |
| `TALLY_USER_EMAIL` | `principal_id` (becomes `user:<email>`) |
| `TALLY_API_URL` | `forwarder.api_url` |
| `TALLY_API_KEY_FILE` | `forwarder.api_key_file` |

### 4.1 `agent_version` is not auto-detected

By design, `agent_version` is **not** discovered dynamically from your LLM
calls. The `SESSION_START` record is written the instant your invoke call
starts — before any model call happens — so there's nothing to discover yet
at that point. Set it explicitly: either in the YAML, or per-call:

```python
AnchorSession(agent_version="your-model-name-or-build-id")
```

### 4.2 Put your real API key in place

```bash
echo "sk-your-real-tally-key" > logs/.anchor_state/api_key.txt
chmod 600 logs/.anchor_state/api_key.txt
```

The forwarder re-reads this file on every drain cycle (every 5s), so
rotating the key is just overwriting the file — no restart needed. If the
file is empty or missing, the forwarder logs a warning and skips forwarding
(nothing is lost — records still accumulate in `log.jsonl`/`log.sqlite` and
will be forwarded once a key is present).

**Add `logs/.anchor_state/` to your `.gitignore`.** Do not commit the API key.

## 5. Integrate into your agent

This is the one manual step — it cannot be safely automated (see Section 8.1
for why). Wrap wherever you currently call `.invoke()` / `.stream()` on your
compiled graph:

```python
from anchor_hooks import AnchorSession

session = AnchorSession()          # reads anchor_config.yaml
app = your_graph.compile()

with session.track(source="startup") as s:
    session.emit_instruction_received(the_user_facing_input_text)
    result = app.invoke(
        your_inputs,
        config={"callbacks": [s.callback_handler]},
    )
```

If you already pass `config={"callbacks": [...]}` elsewhere, **append**
`s.callback_handler` to that existing list — don't replace it.

If you call `.ainvoke()`/`.astream()` (async), the same snippet applies
(`with session.track(...)`, `await app.ainvoke(...)`) — LangChain will run
the handler's sync methods through its own thread-pool bridge. This path is
not separately tested by us; see Section 8.4.

## 6. What gets captured, and how

| Record | Captured when | Requires anything from you? |
|---|---|---|
| `SESSION_START` / `SESSION_END` | Entering/exiting the `with session.track(...)` block | No |
| `INSTRUCTION_RECEIVED` | You call `session.emit_instruction_received(text)` | Yes — one call, shown above |
| `ACTION_TAKEN` / `RESULT_RECEIVED` | Any LangChain `Tool` invocation (`@tool`, `bind_tools`, `ToolNode`) inside the graph | No, if your agent already calls tools this way |
| `HANDOFF` | Entering a compiled subgraph node (supervisor → subagent pattern) | No, but **verify it** — see Section 6.1 |
| `HEARTBEAT` | Every `heartbeat_interval_seconds`, from a background daemon | No |

### 6.1 HANDOFF — verify before relying on it

`HANDOFF` detection uses an **internal LangGraph metadata field**
(`langgraph_checkpoint_ns`), not a documented public API. It has been
verified against `langgraph==1.2.9` with a supervisor node routing into two
subagent subgraphs, producing correctly attributed
`sending_agent → receiving_subagent_type` pairs. It has **not** been tested
against your specific multi-agent topology or your installed LangGraph
version.

Run this after installing, and again after any LangGraph upgrade:

```bash
python -m anchor_hooks.tests.test_handoff
```

It prints `PASS` or `FAIL` with the actual handoff sequence it detected. If
it fails, HANDOFF records may be missing, mis-attributed, or absent entirely
for your topology — treat that as a signal to stop relying on HANDOFF until
this is revisited, not as something to silently ignore.

## 7. Your LangGraph/LangChain version

If `pip install` reports a dependency conflict, or you're on `langgraph`/
`langchain-core` outside the `1.x` range this was built against: the
callback events used here (`on_tool_start`, `on_tool_end`, `on_chain_start`,
and the `langgraph_checkpoint_ns`/`langgraph_node` metadata keys) may not
exist or may behave differently on older/newer major versions. Run
`python -m anchor_hooks.tests.test_handoff` after install to sanity-check;
if it fails, that's the version mismatch surfacing, not a data problem.

## 8. Known limitations — read before you rely on this in production

**8.1 — Wiring into your code is manual, on purpose.** We can't safely
auto-detect where your `.invoke()`/`.stream()` calls live, what your
"instruction text" field is called, or whether you already pass other
callbacks that a blind auto-patch would silently clobber. Automatically
rewriting your source file is a worse failure mode than one clearly
documented manual step.

**8.2 — Only real LangChain Tool calls are captured as actions.** If any part
of your agent calls an external API, database, or subprocess directly from
plain node code (not through `@tool`/`bind_tools`/`ToolNode`), that action
will not appear in `ACTION_TAKEN`/`RESULT_RECEIVED` — silently, with no
error. If you add such a code path later, either convert it to a LangChain
Tool, or call `session.emit_action_taken(...)` / `session.emit_result_received(...)`
directly (same methods the callback handler uses internally).

**8.3 — Concurrent/multi-process deployment.** Log writes take a
cross-process file lock (`fcntl.flock`) and SQLite runs in WAL mode with a
busy-timeout, so concurrent processes writing to the same `log.jsonl`/
`log.sqlite` won't corrupt data or hard-fail. What does **not** scale
automatically: the heartbeat daemon spawns **one OS process per session**.
That's fine for low/moderate session volume. If your agent is a
high-throughput request/response server (many concurrent invocations per
second, each opening its own `AnchorSession`), you will spawn a
correspondingly large number of heartbeat processes. Talk to us before
deploying this pattern at that kind of volume — the heartbeat daemon model
needs to change (one shared daemon tracking many session IDs) rather than
being used as-is.

**8.4 — Async (`ainvoke`/`astream`) is untested.** Should work via
LangChain's handler-bridging, but wasn't exercised in our test environment.

**8.5 — No log rotation.** `log.jsonl` and `log.sqlite` grow indefinitely.
This package does not rotate or prune them — plan for that on your end (e.g.
standard `logrotate`, or a periodic archive/delete job once the forwarder has
confirmed delivery), especially since the forwarder currently only *reads*
the file and never truncates it.

**8.6 — Plaintext API key file.** `logs/.anchor_state/api_key.txt` is read as
plain text from disk. Restrict its file permissions and exclude it from
version control (Section 4.2). If your infrastructure has a secrets manager,
consider pointing `TALLY_API_KEY_FILE` at a file populated from it rather
than hand-editing.

## 9. Smoke test

After installing and wiring in the snippet, run your agent once and check:

```bash
tail -f logs/log.jsonl
```

You should see, in order: `SESSION_START`, `INSTRUCTION_RECEIVED`, one or
more `HEARTBEAT` records if the run takes over a minute, `ACTION_TAKEN`/
`RESULT_RECEIVED` pairs for each tool call, `HANDOFF` if your graph routes
into a subagent, and finally `SESSION_END`.

If you've placed a real API key, also confirm records are reaching Tally
(check your ingest dashboard, or watch for `[log_forwarder] POST failed`
messages in stderr, which mean it's retrying rather than dropping data).
