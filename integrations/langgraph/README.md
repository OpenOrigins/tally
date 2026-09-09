# Tally for LangGraph

`tally-langgraph` adds durable Tally audit records to an existing LangGraph or
LangChain application. It is a pure-Python library: install one platform-independent
wheel, create one `TallyClient`, and add a callback to each graph invocation.

The package follows the repository's Tally `0.2` record contract and mirrors the
important delivery properties of the Rust clients:

- records are written to a local SQLite outbox before network delivery;
- capture order is preserved across threads and same-host processes;
- each retry uses a stable record ID as its idempotency key;
- non-2xx responses never advance a record as delivered;
- malformed records are dead-lettered while transient failures remain queued;
- complete inputs, parameters, and results stay in local private evidence;
- only a bounded, best-effort-redacted projection is sent to the server; and
- one agent-scoped heartbeat covers all active sessions after ten minutes of inactivity.

## Install

After the package is published to your configured Python package index:

```bash
python -m pip install 'tally-langgraph[langgraph]'
```

From a source checkout:

```bash
python -m pip install './integrations/langgraph[langgraph]'
```

From a built wheel:

```bash
python -m pip install 'tally_langgraph-0.1.0-py3-none-any.whl[langgraph]'
```

The `langgraph` extra is convenient for a new project. Existing LangGraph projects can
install `tally-langgraph` without the extra; the only runtime dependency added directly
by this package is `langchain-core`.

## Configure

Set the Agent API key issued by the OpenOrigins dashboard:

```bash
export TALLY_API_KEY='your-agent-api-key'
```

Do not put the key in source code or pass it on a command line. The library reads the
process environment and never creates or modifies a `.env` file.

Useful optional settings:

```bash
export TALLY_AGENT_ID='agent:my-langgraph-service'
export TALLY_AGENT_VERSION='orders-agent/2026-09-09'
export TALLY_STATE_DIR='/var/lib/my-agent/tally'
```

If `TALLY_AGENT_ID` is omitted, a random ID is created once and retained in the local
journal. The default state directory is `.tally/langgraph`; add `.tally/` to the host
project's `.gitignore`. Production deployments should put the state directory on a
persistent local volume; SQLite WAL mode is not intended for a network filesystem.

See [Configuration](#configuration) for the complete list.

## Add Tally to a graph

Create one long-lived client for the application and a fresh callback for each
concurrent invocation:

```python
from tally_langgraph import TallyClient

tally = TallyClient.from_env()

result = graph.invoke(
    {"messages": [{"role": "user", "content": "Summarize this order"}]},
    config={"callbacks": [tally.callback(source="orders-api")]},
)
```

If the application already has callbacks, append the Tally callback instead of
replacing them:

```python
config = {"callbacks": [existing_handler, tally.callback(source="worker")]}
result = graph.invoke(inputs, config=config)
```

`ainvoke()` and `astream()` use the same callback configuration. The handler uses
LangChain's synchronous callback interface with `run_inline=True`, which preserves
capture order; network delivery remains asynchronous.

At application shutdown, ask the worker to flush its durable queue:

```python
tally.close(timeout=10)
```

Queued records remain on disk if the network or server is unavailable and will be
retried when the client starts again. In short-lived/serverless jobs, call
`tally.flush(timeout=10)` before exit and inspect its Boolean return value.

## What is captured automatically

One top-level graph invocation becomes one Tally session and one turn:

| LangChain callback | Tally record |
|---|---|
| Root `on_chain_start` | `SESSION_START`, `INSTRUCTION_RECEIVED` |
| `on_tool_start` | `ACTION_TAKEN` |
| `on_tool_end` / `on_tool_error` | `RESULT_RECEIVED` |
| Root `on_chain_end` / `on_chain_error` | `TURN_END`, `SESSION_END` |
| Ten minutes with active sessions and no records | `HEARTBEAT` |

Only operations exposed as LangChain tools generate action/result records. Direct API,
database, or subprocess calls inside ordinary node functions are not observable through
the callback interface.

## Record a handoff

Automatic handoff inference based on `langgraph_checkpoint_ns` is intentionally not
used because that field is an internal LangGraph implementation detail. Emit an explicit
custom event at the point control transfers to a subagent:

```python
from langchain_core.runnables import RunnableConfig
from tally_langgraph import dispatch_handoff


def route_to_researcher(state: dict, config: RunnableConfig) -> dict:
    dispatch_handoff(
        "agent:researcher",
        payload={"request_id": state["request_id"]},
        config=config,
    )
    return state
```

For asynchronous nodes use `await adispatch_handoff(...)`. Both helpers use LangChain's
public custom-event API and return the generated `handoff_id`; callers may provide their
own ID when the receiver needs to emit the matching acknowledgement.

## Privacy and local storage

Complete evidence is serialized deterministically, hashed with SHA-256, and stored in
`.tally/langgraph/journal.sqlite3`. Records sent to OpenOrigins contain the hash,
`private://sha256/...` URI, and a bounded server-evidence projection. Values under
credential-like keys and common inline token forms are redacted from that projection.

Redaction is defense in depth, not a complete data-loss-prevention boundary. To send no
prompt, parameter, or result text, set:

```bash
export TALLY_SERVER_EVIDENCE_ENABLED=0
```

On POSIX systems, the state directory is set to mode `0700` and SQLite files to `0600`.
Delivered payloads and unreferenced evidence are pruned after 30 days by default;
lightweight delivery outcomes are retained. Permanently malformed records remain in the
dead-letter state for inspection.

## Configuration

Constructor arguments override environment-derived configuration. `TallyConfig` is an
immutable dataclass and can also be constructed directly for dependency injection.

| Environment variable | Default | Purpose |
|---|---:|---|
| `TALLY_API_KEY` | unset | Agent API key; capture still works locally when unset |
| `TALLY_API_URL` | production ingest URL | HTTPS ingest endpoint; HTTP is accepted only for localhost |
| `TALLY_STATE_DIR` | `.tally/langgraph` | Durable journal/private-evidence directory |
| `TALLY_AGENT_ID` | generated and persisted | Stable public agent identifier |
| `TALLY_AGENT_VERSION` | `unknown` | Model/build/application version |
| `TALLY_PRINCIPAL_ID` | unset | Optional principal identifier |
| `TALLY_PRINCIPAL_TYPE` | unset | `human`, `organisation`, or `agent` |
| `TALLY_FORWARDING_ENABLED` | `true` | Keep records local when false |
| `TALLY_SERVER_EVIDENCE_ENABLED` | `true` | Include bounded redacted arbitrator text |
| `TALLY_SERVER_EVIDENCE_MAX_CHARS` | `8192` | Projection limit, clamped to 256–32768 |
| `TALLY_MAX_RECORD_BYTES` | `16777216` | Maximum serialized record size |
| `TALLY_HEARTBEAT_SECONDS` | `600` | Inactivity interval; values below 600 are rejected |
| `TALLY_WORKER_POLL_SECONDS` | `1` | Idle background-worker polling interval |
| `TALLY_REQUEST_TIMEOUT_SECONDS` | `5` | HTTP timeout |
| `TALLY_RETRY_BASE_SECONDS` | `0.5` | Initial retry backoff |
| `TALLY_RETRY_MAX_SECONDS` | `30` | Maximum retry backoff |
| `TALLY_CLAIM_LEASE_SECONDS` | `30` | Cross-process delivery lease duration |
| `TALLY_DELIVERED_RETENTION_DAYS` | `30` | Retention for delivered record payloads |

## Examples

- [`examples/basic_agent.py`](https://github.com/OpenOrigins/tally/blob/main/integrations/langgraph/examples/basic_agent.py)
  demonstrates lifecycle and tool capture.
- [`examples/handoff_agent.py`](https://github.com/OpenOrigins/tally/blob/main/integrations/langgraph/examples/handoff_agent.py)
  demonstrates an explicit subagent handoff.

Both examples use deterministic local functions; no model provider or API key is needed
to see records appear in the journal.

## Development and release

```bash
cd integrations/langgraph
python -m venv .venv
source .venv/bin/activate
python -m pip install -e '.[test]'
ruff check .
mypy --strict src/tally_langgraph
pytest
python -m build
python -m twine check dist/*
```

Build artifacts belong in `dist/`. Publishing remains an explicit release step after
the distributions pass `twine check`; wheels, SQLite databases, logs, virtual
environments, and bytecode are not committed.

Design rationale and failure semantics are documented in
[`docs/architecture.md`](https://github.com/OpenOrigins/tally/blob/main/integrations/langgraph/docs/architecture.md).
