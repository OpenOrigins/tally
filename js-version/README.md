# Anchor-style Audit Log for Claude Code

A Claude Code hook that turns real session activity into an append-only,
Anchor-schema audit log — no external service, no real cryptography, just a
faithful local reshaping of Claude Code's own hook events into
`SESSION_START` / `HEARTBEAT` / `INSTRUCTION_RECEIVED` / `ACTION_TAKEN` /
`RESULT_RECEIVED` / `HANDOFF` / `SESSION_END` records.

## Layout

```
.claude/settings.json          wires hook events to hooks/anchor_hook.js (and hooks/log_hook.sh)
hooks/anchor_hook.js            reshapes each event into one Anchor-schema JSON line
hooks/heartbeat_daemon.js       background process emitting periodic HEARTBEAT records
hooks/log_hook.sh               generic raw-event logger, runs alongside anchor_hook.js
logs/anchor_log.jsonl           the anchor log itself — one JSON object per line, append-only
logs/.anchor_state/             per-session scratch state (last instruction id, heartbeat pid)
```

## How it works

Claude Code dispatches hook events as JSON on stdin to whatever commands
`.claude/settings.json` wires up. `hooks/anchor_hook.js` is wired to the
events that map cleanly onto the Anchor schema, and turns each into one
record appended to `logs/anchor_log.jsonl`:

| Claude Code event | Anchor record      | Notes |
|---|---|---|
| `SessionStart`     | `SESSION_START`     | also spawns the heartbeat daemon for this session |
| `UserPromptSubmit` | `INSTRUCTION_RECEIVED` | assigns a new `instruction_id`, remembered for the actions that follow |
| `PreToolUse`       | `ACTION_TAKEN`      | tagged with the most recent `instruction_id` |
| `PostToolUse`      | `RESULT_RECEIVED`   | keyed by the same `action_id` as its `ACTION_TAKEN` |
| `SubagentStart`    | `HANDOFF`           | sender/receiver framed as local agent → subagent |
| `SessionEnd`       | `SESSION_END`       | stops the heartbeat daemon, clears session state |
| (background timer) | `HEARTBEAT`         | emitted every 60s by `heartbeat_daemon.js` while a session is open |

Events that don't map cleanly onto the Anchor schema (`PermissionRequest`,
`PreCompact`, `PostCompact`, `SubagentStop`) are left to `hooks/log_hook.sh`,
which logs every event's raw payload separately — see that script for
details.

The hook is a pure observer: it always exits `0` with no stdout, so it can
never block a tool call or influence a permission decision, no matter what
it's logging.

## Anchor record shape

Every record shares:

```json
{
  "record_type": "ACTION_TAKEN",
  "session_id": "sess_<claude session id>",
  "anchor_receipt": "rcpt_local_000042_a1b2c3d4",
  "...": "record-specific fields (see hooks/anchor_hook.js)"
}
```

- **`anchor_receipt`** is a locally-generated, monotonically increasing
  sequence id (`logs/.anchor_state/seq.txt`) plus a random suffix — not a
  receipt from any real anchoring/timestamping service.
- **`pre_state_hash` / `post_state_hash`** on `ACTION_TAKEN` are fixed
  placeholder strings, not computed from real state. This is deliberate: the
  schema shape matches the Anchor spec without pretending to verify
  something it doesn't.
- Free-text fields (prompts, tool params, tool responses) are truncated to
  500 characters to keep the log file bounded.

## Heartbeats

`hooks/anchor_hook.js` spawns `hooks/heartbeat_daemon.js` as a detached
background process on `SessionStart`, one per `session_id`. It appends a
`HEARTBEAT` record to the shared `anchor_log.jsonl` every 60 seconds so gaps
between real actions still show periodic liveness, and self-terminates after
6 hours as a safety net in case `SessionEnd` never fires (e.g. the window is
closed instead of exited cleanly). `SessionEnd` stops it directly via its
recorded pid when that path does fire.

## Try it

Wire it into any project's `.claude/settings.json` (see this repo's copy for
the exact hook declarations), then run a normal session and tail the log:

```bash
tail -f logs/anchor_log.jsonl
```

Ask Claude to run a tool call and watch `INSTRUCTION_RECEIVED` →
`ACTION_TAKEN` → `RESULT_RECEIVED` land in order, with `HEARTBEAT` records
filling the quiet periods in between.

## What this is not

This is a local, unsigned log for demonstrating the Anchor schema shape
against real Claude Code session data — not a substitute for a real
cryptographic anchoring/timestamping service. Nothing here is signed, hashed
against real state, or externally verifiable.
