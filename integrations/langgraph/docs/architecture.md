# Architecture

## Package boundary

The LangGraph integration is a separate pure-Python distribution under
`integrations/langgraph/`. It is deliberately not a Cargo workspace member and does not
bind the Rust crates through PyO3.

That boundary keeps the existing Rust clients unchanged and gives Python users one
universal wheel instead of a CPython/operating-system/architecture wheel matrix. The
Python implementation shares the Tally `0.2` wire contract and the same delivery
invariants, while its tests protect parity with the repository specification.

## Capture flow

```text
LangGraph invocation
        |
        v
TallyCallbackHandler (inline, no network)
        |
        v
SQLite transaction: private evidence + ordered record
        |
        v
single client worker / cross-process lease
        |
        v
OpenOrigins ingest API (stable idempotency key)
```

Callback handling is inline so `SESSION_START`, tools, results, `TURN_END`, and
`SESSION_END` enter the journal in observed order. The network is never called from a
callback. A background thread drains the journal; a synchronous `flush()` is available
for short-lived processes.

SQLite was chosen instead of detached Python daemons or a plain JSONL byte offset:

- transactions atomically persist records and their private evidence;
- WAL mode and `BEGIN IMMEDIATE` coordinate multiple application processes;
- expiring leases recover records claimed by a crashed worker;
- terminal outcomes are distinct from pending delivery state; and
- a permanent invalid record cannot block later delivery forever.

## Failure semantics

Delivery is at least once. Every HTTP attempt carries the same `record_id` in both
`Idempotency-Key` and `X-Tally-Record-Id`, so the server can deduplicate the only
ambiguous case: a response lost after the server accepted the record.

| Failure | Behavior |
|---|---|
| HTTP 2xx | Mark delivered and retain the receipt |
| HTTP 400/413/415/422 | Mark dead-letter; continue with later records |
| HTTP 408/425/429/5xx | Retry with bounded exponential backoff and jitter |
| HTTP 401/403/404 | Keep pending and retry slowly so configuration can be repaired |
| Timeout/DNS/connect failure | Keep pending and retry |
| Worker/process crash | Lease expires; another worker reclaims the same record ID |
| Redirect | Refused, preventing API-key forwarding to another origin |

The oldest non-terminal sequence blocks later records until it is delivered or
dead-lettered, preserving capture order.

## Evidence boundary

Full inputs, tool parameters, results, and outcomes are canonicalized and stored only in
the local evidence table. Their SHA-256 hash and `private://sha256/...` URI travel in the
Tally record. The server-visible projection is separately bounded and redacted, and can
be disabled entirely.

The Python serializer handles LangChain/Pydantic models, dataclasses, mappings,
sequences, bytes, dates, paths, recursion, and non-finite floats. Collection and depth
limits prevent hostile objects from creating unbounded traversal.

## Session and heartbeat model

A callback maps one top-level `.invoke()`/`.ainvoke()`/`.stream()`/`.astream()` call to
one session and one turn. A long-lived `TallyClient` owns any number of concurrent
sessions.

Active sessions and the last-record timestamp are stored in SQLite. Heartbeat claiming
is transactional, so multiple processes sharing a state directory emit one agent-level
heartbeat rather than one heartbeat per request. Any normal record resets the ten-minute
inactivity window. Stale sessions are removed after three heartbeat intervals.

## Handoffs

The package does not infer subagents from `langgraph_checkpoint_ns`; that is an internal
field and has changed across LangGraph versions. `dispatch_handoff()` uses LangChain's
public custom-event mechanism, making the handoff explicit and testable. Signature
fields are reported as unavailable rather than fabricated.

## Known limits

- Plain Python work performed inside a graph node is invisible unless it is exposed as
  a LangChain tool or recorded explicitly through `TallyClient`.
- The callback cannot infer a verified human principal, authority grant, cryptographic
  handoff signature, state snapshot, or intent/deviance decision. Those fields are null
  with explicit unavailable status.
- Local SQLite encryption is not provided. Use an encrypted volume where local evidence
  requires encryption at rest.
- SQLite WAL coordination is for processes on one host. Keep the state directory on a
  persistent local filesystem, not NFS or another network filesystem.
- Server acknowledgement behavior depends on support for the stable idempotency key.
