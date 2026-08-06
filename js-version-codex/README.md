# Anchor-style Audit Log for Codex CLI

A Codex CLI hook that turns real session activity into an append-only,
Anchor-schema audit log — no external service, no real cryptography, just a
faithful local reshaping of Codex's own hook events into `SESSION_START` /
`HEARTBEAT` / `INSTRUCTION_RECEIVED` / `ACTION_TAKEN` / `RESULT_RECEIVED` /
`HANDOFF` / `SESSION_END` records.

This is the Codex counterpart to [`../js-version`](../js-version), which does
the same thing for Claude Code. The two are deliberately kept structurally
identical — same log shape, same record types, same heartbeat mechanism — so
a session run through either CLI produces the same kind of audit trail.

## Layout

```
.codex/hooks.json.template     hook wiring template (merged into the real, global hooks.json)
install/install_hooks.js       merges the template into $CODEX_HOME/hooks.json
install/uninstall_hooks.js     removes this project's handlers from that file
hooks/anchor_hook.js           reshapes each Codex hook event into one Anchor-schema record
hooks/heartbeat_daemon.js      background process emitting periodic HEARTBEAT records (unchanged from js-version)
hooks/log_hook.sh              generic raw-event logger, runs alongside anchor_hook.js
logs/anchor_log.jsonl          the anchor log — one JSON object per line, append-only
logs/anchor_log.sqlite         the same records as rows in an `anchor_log` table (queryable)
logs/.anchor_state/            per-session scratch state (last instruction id, heartbeat pid)
```

## Why there's an install step (unlike js-version)

Claude Code auto-discovers a project-local `.claude/settings.json`, which is
why `js-version` can just commit that file directly. Codex has no such
per-project hook scoping — it only reads hooks from one global file at
`$CODEX_HOME/hooks.json` (default `~/.codex/hooks.json`). So instead of a
committed settings file, this folder ships a **template**
(`.codex/hooks.json.template`) plus a small installer that merges this
project's absolute hook paths into that global file.

Install:

```bash
cd js-version-codex
npm install          # only needed if you didn't get node_modules via the repo copy
node install/install_hooks.js
```

This will:

- Read `.codex/hooks.json.template` and substitute this checkout's absolute
  path in for every hook command.
- Back up any existing `$CODEX_HOME/hooks.json` before touching it.
- Remove any handlers this same project previously installed (safe to
  re-run after moving/renaming the checkout).
- Merge in the current set of handlers, leaving any hooks from other tools
  or other Tally checkouts untouched.

It will also print a reminder that Codex needs hooks enabled in its
`config.toml`:

```toml
[features]
hooks = true
```

Add that to `$CODEX_HOME/config.toml` if it isn't already there (the
installer does not edit `config.toml` itself, to avoid mangling an existing
file's formatting).

Uninstall:

```bash
node install/uninstall_hooks.js
```

## How it works

Codex dispatches hook events as JSON on stdin to whatever commands
`hooks.json` wires up. `hooks/anchor_hook.js` is wired to the events that map
cleanly onto the Anchor schema, and turns each into one record appended to
`logs/anchor_log.jsonl` **and** inserted as a row into `logs/anchor_log.sqlite`
(table `anchor_log`, one row per record):

| Codex event        | Anchor record          | Notes |
|---|---|---|
| `SessionStart`      | `SESSION_START`         | also spawns the heartbeat daemon for this session |
| `UserPromptSubmit`  | `INSTRUCTION_RECEIVED`  | assigns a new `instruction_id`, remembered for the actions that follow |
| `PreToolUse`        | `ACTION_TAKEN`          | tagged with the most recent `instruction_id` |
| `PostToolUse`       | `RESULT_RECEIVED`       | keyed by the same `action_id` as its `ACTION_TAKEN` |
| `SubagentStart`     | `HANDOFF`               | sender/receiver framed as local agent → subagent |
| `Stop`              | `SESSION_END`           | stops the heartbeat daemon, clears session state |
| (background timer)  | `HEARTBEAT`             | emitted every 60s by `heartbeat_daemon.js` while a session is open |

Codex has no `SessionEnd` event the way Claude Code does — the closest
equivalent is `Stop`, so that's what closes out the session record here.
Session ids are read defensively from whichever field Codex actually sends
(`session_id`, `thread_id`, or `conversation_id`), since Codex's hook payload
shapes aren't documented as strictly as Claude Code's.

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
  "session_id": "sess_<codex session/thread id>",
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
6 hours as a safety net in case `Stop` never fires (e.g. the window is closed
instead of exited cleanly). `Stop` stops it directly via its recorded pid when
that path does fire. This daemon's logic is unchanged from
`js-version/hooks/heartbeat_daemon.js` — it doesn't know or care which CLI is
driving it.

## Try it

After running `install/install_hooks.js` and enabling `features.hooks = true`
in Codex's `config.toml`, open Codex normally in any project and tail the log:

```bash
tail -f logs/anchor_log.jsonl
```

or query the SQLite log directly:

```bash
sqlite3 logs/anchor_log.sqlite "SELECT record_type, session_id, recorded_at FROM anchor_log ORDER BY id DESC LIMIT 10;"
```

Ask Codex to run a tool call and watch `INSTRUCTION_RECEIVED` →
`ACTION_TAKEN` → `RESULT_RECEIVED` land in order, with `HEARTBEAT` records
filling the quiet periods in between.

## What this is not

This is a local, unsigned log for demonstrating the Anchor schema shape
against real Codex session data — not a substitute for a real cryptographic
anchoring/timestamping service. Nothing here is signed, hashed against real
state, or externally verifiable.
