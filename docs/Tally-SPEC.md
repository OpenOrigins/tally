# OpenOrigins Agent Log Specification
### Version 0.2 — Draft for Public Comment

> **Status**: Draft. We welcome issues and pull requests.  
> **Maintainer**: [OpenOrigins](https://openorigins.com)  
> **License**: CC BY 4.0  
> **Changelog**: v0.2 introduces three-tier field visibility (PUBLIC / ARBITRATOR / PRIVATE) and the Anchor local scanning model.

---

## Overview

This specification defines the minimum set of events and fields that an agentic system must record to be **OpenOrigins compatible**.

Compatibility means that, in the event of a dispute between agents or between an agent and a human principal, OpenOrigins can use independently anchored logs from each party to determine:

1. Who authorised the action
2. What the agent knew at the time it acted
3. What the agent actually did
4. Whether both sides of a handoff agreed on its contents

### How Anchor works

**Anchor runs locally on each organisation's own infrastructure.** It is a lightweight daemon that scans a designated log directory for new records, hashes their contents, and transmits fields to OpenOrigins according to their visibility tier (see below).

Each organisation runs its own Anchor instance independently. This means:

- Organisations do not send their full logs to OpenOrigins. They send hashes and, for certain fields, plaintext.
- OpenOrigins receives independent transmissions from each party. Neither party can alter what the other's Anchor has already sent.
- In a dispute, OpenOrigins holds two independent anchored records. Contradictions between them — different hashes for the same claimed event — are themselves findings.
- Gaps in one party's log are detectable because Anchor emits a periodic heartbeat. Silence is distinguishable from absence.

---

## Terminology

| Term | Definition |
|---|---|
| **Agent** | Any automated system acting on behalf of a principal |
| **Principal** | The human, organisation, or upstream agent that authorised this agent to act |
| **Turn** | One user instruction and the agent response produced for it within a session |
| **Action** | A single atomic operation performed by an agent |
| **Handoff** | The transfer of a task or result from one agent to another |
| **Anchor** | The OpenOrigins local daemon that scans, hashes, and transmits log records |
| **Receipt** | The artefact returned by OpenOrigins after a successful anchoring |
| **Dispute** | A formally raised disagreement between two parties, triggering the unsealing process |

---

## Core Principles

**1. Anchor runs independently on each party's infra.**  
Neither party's log is authoritative on its own. OpenOrigins holds independently anchored records from both sides. The comparison is the evidence.

**2. Hash content, don't transmit it by default.**  
Raw prompt and response content is not transmitted to OpenOrigins unless the field's visibility tier requires it. Plaintext stays on company infrastructure and is retrieved only under dispute.

**3. Record intent before action.**  
A log that only records what happened is insufficient. The spec requires recording what the agent declared it would do before acting. The delta between declared intent and actual action is a primary signal in dispute resolution.

**4. Both sides of a handoff must sign.**  
Each party's Anchor independently records the handoff event using the same `handoff_id`. OpenOrigins matches them. If the payload hashes differ, this is a material finding.

**5. Timestamps must be externally attested.**  
Anchor attaches an OpenOrigins receipt to each transmitted record, carrying an external time attestation.

**6. The log is append-only.**  
No record may be modified or deleted after being written. Corrections are made by appending a new record referencing the corrected one.

**7. Heartbeat ensures gap detection.**  
Anchor emits one agent-scoped `HEARTBEAT` record every 10 minutes (600 seconds) whenever no other records are being written for that agent. Concurrent sessions share the same heartbeat window.

---

## Field Visibility Tiers

Every field in this spec carries one of three visibility tiers. Anchor reads the tier from the schema and handles transmission accordingly.

### PUBLIC `[PUB]`
Transmitted as plaintext to OpenOrigins. May appear in published dispute findings. Contains no commercially sensitive information. Sufficient for OpenOrigins to establish a timeline and identify structural anomalies without reading any content.

### ARBITRATOR `[ARB]`
Transmitted as plaintext to OpenOrigins but subject to strict non-disclosure obligations. OpenOrigins may read arbitrator-tier fields to reach a dispute finding but may not disclose them to any third party — including the opposing party — except by order of a competent court.

Arbitrator-tier fields are sealed by default. On routine ingestion they are stored encrypted and inaccessible to OpenOrigins staff. They are unsealed only when a formal dispute has been raised and the unsealing process completes (see below).

### PRIVATE `[PRV]`
Only the SHA-256 hash and a company-held URI are transmitted to OpenOrigins. The plaintext never leaves company infrastructure. OpenOrigins can confirm the content existed and has not been altered, but cannot read it. If a dispute requires inspection of a private field, the company must voluntarily produce the plaintext, which is verified against the anchored hash.

---

## Unsealing Process

Arbitrator-tier fields are sealed by default. The process for unsealing them is:

1. **Formal notice**: Either party submits a dispute notice to OpenOrigins identifying the session IDs in question and the nature of the disagreement.
2. **Notification**: OpenOrigins notifies the other party within 24 hours.
3. **Waiting period**: A 5-business-day period during which the notified party may object on specified grounds (e.g. the fields contain legally privileged material; the notice is frivolous).
4. **Determination**: OpenOrigins determines whether a prima facie dispute exists. If yes, arbitrator-tier fields for the relevant sessions are unsealed to OpenOrigins staff handling the dispute.
5. **Finding**: OpenOrigins issues a finding. Arbitrator-tier content is summarised but not reproduced verbatim.
6. **Retention**: Following resolution, arbitrator-tier plaintext is deleted from OpenOrigins systems within 30 days. Hashes are retained indefinitely as part of the immutable record.

**Known limitation**: OpenOrigins's non-disclosure obligation does not protect arbitrator-tier data from court orders or regulatory compulsion. Companies with particularly sensitive data should treat arbitrator-tier fields as equivalent to PRIVATE for their highest-sensitivity content.

---

## Record Types

The spec defines eight record types. A compliant implementation must be capable of emitting all eight. Field definitions include their visibility tier in brackets.

---

### 1. `SESSION_START`

Emitted once when an agent begins a task session.

```json
{
  "record_type": "SESSION_START",
  "schema_version": "0.2",

  "session_id":     "<uuid>",                                   // [PUB]
  "agent_id":       "<DID or public key fingerprint>",          // [PUB]
  "agent_version":  "<model name and version string>",          // [PUB]

  "principal": {
    "type":  "human | organisation | agent",                    // [PUB]
    "id":    "<DID, email, or org identifier>"                  // [ARB]
  },

  "authority_scope_hash": "<SHA-256>",                          // [PUB]
  "authority_scope_uri":  "<URI>",                              // [PRV]
  "authority_granted_at": "<ISO 8601>",                         // [PUB]
  "authority_expires_at": "<ISO 8601>",                         // [ARB]

  "delegation_chain": {
    "depth":      2,                                            // [PUB]
    "chain_hash": "<SHA-256>",                                  // [PUB]
    "chain":      ["<principal_id>", "<agent_id>"],             // [ARB]
    "chain_uri":  "<URI>"                                       // [PRV]
  },

  "session_started_at": "<ISO 8601>",                           // [PUB]
  "anchor_receipt":     "<receipt ID>"                          // [PUB]
}
```

---

### 2. `INSTRUCTION_RECEIVED`

Emitted when the agent receives an instruction it intends to act on.

```json
{
  "record_type": "INSTRUCTION_RECEIVED",
  "schema_version": "0.2",

  "session_id":     "<uuid>",                                   // [PUB]
  "instruction_id": "<uuid>",                                   // [PUB]

  "sender": {
    "id":        "<identifier of instructing party>",           // [ARB]
    "signature": "<signature over instruction_hash>"            // [PUB]
  },

  "instruction_hash":        "<SHA-256>",                       // [PUB]
  "instruction_uri":         "<URI>",                           // [PRV]
  "instruction_received_at": "<ISO 8601>",                      // [PUB]

  "context_snapshot_hash": "<SHA-256>",                         // [PUB]
  "context_snapshot_uri":  "<URI>",                             // [PRV]

  "declared_intent": {
    "summary":     "<one-sentence description of planned action>", // [ARB]
    "detail_hash": "<SHA-256>",                                    // [PUB]
    "detail_uri":  "<URI>"                                         // [PRV]
  },

  "anchor_receipt": "<receipt ID>"                              // [PUB]
}
```

---

### 3. `ACTION_TAKEN`

Emitted for each atomic action the agent takes.

```json
{
  "record_type": "ACTION_TAKEN",
  "schema_version": "0.2",

  "session_id":     "<uuid>",                                   // [PUB]
  "action_id":      "<uuid>",                                   // [PUB]
  "instruction_id": "<uuid>",                                   // [PUB]

  "action_type": "read | write | tool_call | decision | handoff", // [PUB]

  "tool": {
    "server":      "<MCP server name or endpoint>",             // [PUB]
    "name":        "<tool name>",                               // [PUB]
    "params_hash": "<SHA-256>",                                 // [PUB]
    "params_uri":  "<URI>"                                      // [PRV]
  },

  "pre_state_hash":  "<SHA-256>",                               // [PUB]
  "pre_state_uri":   "<URI>",                                   // [PRV]
  "post_state_hash": "<SHA-256>",                               // [PUB]
  "post_state_uri":  "<URI>",                                   // [PRV]

  "action_timestamp": "<ISO 8601>",                             // [PUB]
  "anchor_receipt":   "<receipt ID>",                           // [PUB]

  "deviance_flag": {
    "deviated":       false,                                    // [PUB]
    "delta_category": null,                                     // [ARB]
    "delta_hash":     null,                                     // [PUB]
    "delta_uri":      null                                      // [PRV]
  }
}
```

**Note on `deviance_flag`**: `deviated` is a boolean visible to all — it is a core signal. `delta_category` is a coarse classification (e.g. `"scope_exceeded"`, `"tool_substituted"`, `"instruction_reinterpreted"`) visible to OpenOrigins under arbitration. The full delta description remains private.

---

### 4. `RESULT_RECEIVED`

Emitted when the agent receives the result of an action.

```json
{
  "record_type": "RESULT_RECEIVED",
  "schema_version": "0.2",

  "session_id": "<uuid>",                                       // [PUB]
  "action_id":  "<uuid>",                                       // [PUB]

  "result_hash":        "<SHA-256>",                            // [PUB]
  "result_uri":         "<URI>",                                // [PRV]
  "result_received_at": "<ISO 8601>",                           // [PUB]

  "result_interpretation": {
    "summary":     "<one-sentence description of what result meant>", // [ARB]
    "detail_hash": "<SHA-256>",                                        // [PUB]
    "detail_uri":  "<URI>"                                             // [PRV]
  },

  "exception": {
    "occurred":         false,                                  // [PUB]
    "type":             null,                                   // [PUB]
    "description_hash": null,                                   // [PUB]
    "description_uri":  null                                    // [PRV]
  }
}
```

---

### 5. `HANDOFF`

Emitted when an agent transfers a task or result to another agent.

**Each party's Anchor independently records this event using the same `handoff_id`.** The sending party records it with `emitting_party: "sender"` and no receiver signature. The receiving party records it with `emitting_party: "receiver"` and both signatures. OpenOrigins matches the two records by `handoff_id` and compares `payload_hash`. A mismatch is a material finding.

```json
{
  "record_type": "HANDOFF",
  "schema_version": "0.2",

  "session_id":      "<uuid>",                                  // [PUB]
  "handoff_id":      "<uuid>",                                  // [PUB]
  "emitting_party":  "sender | receiver",                       // [PUB]

  "sender": {
    "agent_id":  "<DID>",                                       // [PUB]
    "org_id":    "<identifier>",                                // [ARB]
    "signature": "<sig over payload_hash>"                      // [PUB]
  },

  "receiver": {
    "agent_id":        "<DID>",                                 // [PUB]
    "org_id":          "<identifier>",                          // [ARB]
    "signature":       "<sig or null>",                         // [PUB]
    "acknowledged_at": "<ISO 8601 or null>"                     // [PUB]
  },

  "payload_hash":            "<SHA-256>",                       // [PUB]
  "payload_uri":             "<URI>",                           // [PRV]
  "handoff_timestamp":       "<ISO 8601>",                      // [PUB]
  "acknowledgement_status":  "pending | acknowledged | rejected | timeout", // [PUB]

  "anchor_receipt": "<receipt ID>"                              // [PUB]
}
```

---

### 6. `TURN_END`

Emitted when the agent finishes one response. A session can contain many turns;
this record does not conclude the session.

```json
{
  "record_type": "TURN_END",
  "schema_version": "0.2",

  "session_id": "<uuid>",                                       // [PUB]
  "turn_id":    "<uuid>",                                       // [PUB]

  "outcome":      "completed | failed | interrupted",           // [PUB]
  "outcome_hash": "<SHA-256>",                                  // [PUB]
  "outcome_uri":  "<URI>",                                     // [PRV]

  "turn_ended_at":  "<ISO 8601>",                              // [PUB]
  "anchor_receipt": "<receipt ID>"                              // [PUB]
}
```

---

### 7. `SESSION_END`

Emitted once when the agent's task session concludes.

```json
{
  "record_type": "SESSION_END",
  "schema_version": "0.2",

  "session_id": "<uuid>",                                       // [PUB]

  "outcome":      "success | failure | partial | interrupted",  // [PUB]
  "outcome_hash": "<SHA-256>",                                  // [PUB]
  "outcome_uri":  "<URI>",                                      // [PRV]

  "human_review": {
    "required":      false,                                     // [PUB]
    "reviewer_id":   null,                                      // [ARB]
    "approved_at":   null,                                      // [PUB]
    "approval_hash": null                                       // [PUB]
  },

  "session_ended_at": "<ISO 8601>",                             // [PUB]
  "anchor_receipt":   "<receipt ID>"                            // [PUB]
}
```

---

### 8. `HEARTBEAT`

Emitted by Anchor every 10 minutes (600 seconds) whenever no other records are being written for the same agent. Concurrent sessions share one heartbeat window. This allows OpenOrigins to distinguish genuine inactivity from a stopped or tampered Anchor instance without multiplying records by session count.

```json
{
  "record_type":        "HEARTBEAT",                            // [PUB]
  "schema_version":     "0.2",                                  // [PUB]

  "agent_id":           "<DID>",                                // [PUB]
  "anchor_instance_id": "<uuid>",                               // [PUB]
  "active_sessions":    ["<session_id>"],                       // [PUB]
  "timestamp":          "<ISO 8601>",                           // [PUB]
  "anchor_receipt":     "<receipt ID>"                          // [PUB]
}
```

---

## Anchoring

Anchor transmits records to OpenOrigins in two passes:

**Pass 1 — Public fields**: Transmitted immediately as plaintext on record creation.

**Pass 2 — Arbitrator fields**: Transmitted as plaintext but stored encrypted at rest, inaccessible to OpenOrigins staff until a dispute is formally raised.

**Private fields**: Only hash and URI are transmitted. Plaintext never leaves company infrastructure.

### Minimum anchoring requirement

These record types must each receive an individual Anchor receipt (not batched):

- `SESSION_START`
- Every `HANDOFF`
- `SESSION_END`
- Every `HEARTBEAT`

`INSTRUCTION_RECEIVED`, `ACTION_TAKEN`, `RESULT_RECEIVED`, and `TURN_END`
records may be batched into a Merkle root and anchored together.

---

## Dispute Resolution Process

1. **Public-field comparison** is performed first. Timelines, action types, tool calls, and handoff hashes are compared. Contradictions at this level are resolved without unsealing.
2. If public fields are insufficient, **arbitrator-tier fields are unsealed** following the waiting period.
3. If arbitrator fields are still insufficient, **voluntary production** of private-field plaintext is requested. Produced content is verified against the anchored hash.
4. OpenOrigins issues a **finding** identifying which party's log is consistent with the anchored record and where the contradiction arose.

---

## Compliance Levels

| Level | Requirement |
|---|---|
| **Level 1 — Basic** | Emits all eight record types. Runs Anchor locally. Anchors SESSION_START, HANDOFF, SESSION_END, and HEARTBEAT individually. |
| **Level 2 — Standard** | Level 1 plus: all three visibility tiers correctly implemented; private-field URIs resolvable on request during dispute; Data Processing Agreement with OpenOrigins signed. |
| **Level 3 — Full** | Level 2 plus: Cambium proof required before each cross-agent handoff; real-time streaming to Anchor rather than batch submission. |

---

## What This Spec Does Not Cover

- How you store logs internally
- What your agents do
- Identity issuance (how agents acquire DIDs or key pairs)
- The capability token format referenced by `authority_scope` — we will update this repo with recommended formats as and when they are developed

---

## Versioning

The `schema_version` field is present in every record. Breaking changes increment the minor version. Implementations should reject records with schema versions they do not recognise.

---

## Contributing

- Open an issue describing the problem
- Submit a PR against `main`
- Breaking changes require a discussion issue with a two-week comment period before merge

---

## Contact

[openorigins.com](https://openorigins.com) · m@openorigins.com
