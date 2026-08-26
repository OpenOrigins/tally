# Local storage and detection

Tally treats the local filesystem as a delivery spool and bounded evidence cache,
not as the permanent system of record.

## Storage lifecycle

1. A hook writes a structured record and raw evidence locally with permissions
   restricted to the current user.
2. When forwarding is configured, the record is atomically copied into the
   delivery queue. The queue is the one durable structured copy while offline.
3. Tally removes a queued record only after the ingest API returns a successful
   response. Failed and interrupted deliveries remain queued for retry.
4. Raw evidence is stored once by SHA-256 content hash. Identical payloads share
   one object, similar to Git's object store.
5. Garbage collection removes unreferenced private objects and optional local
   debug output after 30 days or when their combined size exceeds 256 MiB.
   Objects referenced by pending or unsent records are never removed to enforce
   those limits.

The defaults can be changed for managed deployments:

| Environment variable | Default | Purpose |
|---|---:|---|
| `TALLY_PRIVATE_RETENTION_DAYS` | `30` | Maximum age of unreferenced raw evidence |
| `TALLY_PRIVATE_STORAGE_LIMIT_MIB` | `256` | Soft size limit for unreferenced raw evidence |
| `TALLY_SERVER_EVIDENCE_MAX_CHARS` | `8192` | Maximum plaintext evidence sent per record |
| `TALLY_SERVER_EVIDENCE_ENABLED` | `1` | Set to `0` to send hashes and URIs without plaintext evidence |
| `TALLY_DEBUG_JSONL` | `0` | Set to `1` only when duplicate local debug streams are needed |

The size limit is intentionally soft: delivery safety wins over disk limits. A
machine that remains offline can exceed it because Tally will not delete pending
evidence.

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
