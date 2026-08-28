# Local storage and detection

Tally treats the local filesystem as a delivery spool and bounded evidence cache,
not as the permanent system of record.

## Storage lifecycle

1. A hook validates each structured record and appends it to a user-private,
   segmented JSONL journal. The append and directory entry are synced before the
   hook returns; a torn final line is repaired on the next append.
2. Journal sequence numbers preserve capture order across clients and processes.
   A single coalesced worker materializes private evidence and drains records in
   that order.
3. Transient network and server failures use bounded exponential backoff with
   jitter and `Retry-After` support. Every attempt carries the stable record ID
   as its idempotency key. A permanently invalid record is appended to the local
   dead-letter journal so it cannot poison delivery of later records.
4. Successful response receipts are preserved in an append-only outcome journal
   beside the record ID and sequence. Closed segments are removed only when all
   of their records have terminal outcomes; the active segment remains append-only.
5. Raw evidence is stored once by SHA-256 content hash. Identical payloads share
   one object, similar to Git's object store. Evidence objects are written by the
   background worker so the synchronous hook path performs one durable append.
6. Garbage collection removes unreferenced private objects and optional local
   debug output after 30 days or when their combined size exceeds 256 MiB.
   Objects referenced by pending or unsent records are never removed to enforce
   those limits. Collection runs in the background at most hourly by default and
   refuses an unexpectedly large traversal rather than consuming unbounded memory.

The journal lives under each integration's Tally state directory:

```text
journal/
  segments/segment-00000000000000000001.jsonl
  sequence.checkpoint
  terminal.checkpoint
  delivery-outcomes.jsonl
  dead-letters.jsonl
```

Journal segments contain local-only metadata and staged private payloads. The
wire request contains only the structured `record` object; local paths and raw
private values are never included in the request body.

The defaults can be changed for managed deployments:

| Environment variable | Default | Purpose |
|---|---:|---|
| `TALLY_PRIVATE_RETENTION_DAYS` | `30` | Maximum age of unreferenced raw evidence |
| `TALLY_PRIVATE_STORAGE_LIMIT_MIB` | `256` | Soft size limit for unreferenced raw evidence |
| `TALLY_JOURNAL_SEGMENT_BYTES` | `4194304` | Target size before rotating to a new journal segment |
| `TALLY_JOURNAL_COALESCE_MILLIS` | `100` | Short worker window used to combine hook bursts |
| `TALLY_FORWARD_MAX_RETRIES` | `5` | In-process retries for a transient delivery failure |
| `TALLY_FORWARD_RETRY_BASE_MILLIS` | `500` | Initial exponential retry delay |
| `TALLY_FORWARD_RETRY_MAX_MILLIS` | `30000` | Maximum retry delay |
| `TALLY_MAX_HOOK_INPUT_BYTES` | `16777216` | Maximum stdin accepted from a hook runner |
| `TALLY_GIT_TIMEOUT_MILLIS` | `2000` | Deadline for the single lightweight Git status capture |
| `TALLY_GIT_INCLUDE_UNTRACKED` | `0` | Include untracked filenames in private Git evidence |
| `TALLY_STORAGE_GC_MAX_ENTRIES` | `250000` | Safety bound for one managed-storage traversal |
| `TALLY_SERVER_EVIDENCE_MAX_CHARS` | `8192` | Maximum plaintext evidence sent per record |
| `TALLY_SERVER_EVIDENCE_ENABLED` | `1` | Set to `0` to send hashes and URIs without plaintext evidence |
| `TALLY_DEBUG_JSONL` | `0` | Set to `1` only when duplicate local debug streams are needed |

The size limit is intentionally soft: delivery safety wins over disk limits. A
machine that remains offline can exceed it because Tally will not delete pending
evidence.

On Unix, state directories are mode `0700` and files are mode `0600`. On Windows,
new private directories have inherited ACLs removed and grant access only to the
current user and `SYSTEM`. Atomic replacement syncs the containing directory on
Unix and uses write-through replacement on Windows.

## Capture semantics and current limits

Tally reports only facts exposed by Codex or Claude Code hooks. The installation
gets a persistent random agent ID; record IDs use 128 bits of randomness, while
correlation IDs are deterministic 128-bit prefixes of SHA-256.

Hook APIs do not currently provide a verified principal identity or type, instruction
signature, signed authority scope, pre-action declared intent, or a trustworthy
intent-versus-action evaluation. Those fields are emitted as `null` with an
explicit `capture_status`, `signature_status`, or `evaluation_status` of
`unavailable`; Tally does not substitute a prompt summary for declared intent or
claim that an unevaluated action did not deviate. Prompt/result summaries are
labelled separately as arbitrator evidence. Subagent lifecycle events produce
`HANDOFF` records, but signatures remain unavailable until the clients expose
cryptographic handoff attestations.

## Server-side detection fields

Instruction, action, result, and turn-end records include `server_evidence`:

```json
{
  "schema_version": "tally-server-evidence.v1",
  "visibility": "arbitrator",
  "text": "bounded, redacted plaintext",
  "content_hash": "sha256:...",
  "truncated": false,
  "redaction_count": 1,
  "risk_signals": ["credential_access", "destructive_change"]
}
```

Tally redacts values under credential-like keys and common token formats before
forwarding. It also emits conservative candidate signals for destructive
changes, credential access, privilege escalation, external transfer,
persistence changes, and dynamic execution. These signals are hints for server
ranking and review, not proof that an action is malicious.

The ingest and dashboard services live outside this repository. They should
store `server_evidence` under ARBITRATOR controls, independently scan its `text`,
index the resulting categories, and highlight matching records for review. They
must not treat client-provided `risk_signals` as trusted findings or place the
plaintext in general application logs, search telemetry, or error reports.

Redaction is defense in depth, not a complete data-loss-prevention system. The
plaintext field is arbitrator-tier data and must receive the access controls,
encryption, audit trail, and deletion behavior defined by the Tally
specification. Deployments that cannot provide those controls should disable
plaintext evidence and use hashes plus private URIs only.
