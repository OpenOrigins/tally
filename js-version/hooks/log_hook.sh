#!/usr/bin/env bash
# Generic Claude Code hook logger.
#
# Claude Code invokes this script once per hook firing, feeding a single JSON
# object on stdin (the shape depends on which event fired, but every event
# includes "hook_event_name"). This script never inspects argv — the event
# type only ever arrives via that stdin field — and it must exit 0 with no
# stdout so it stays a pure observer and never influences permission
# decisions, prompt injection, or blocking behavior for the real hooks
# wired to this same script in .claude/settings.json.
set -uo pipefail

LOG_DIR="${CLAUDE_HOOK_LOG_DIR:-/var/log/claude-hooks}"
mkdir -p "$LOG_DIR"

INPUT="$(cat)"

EVENT="$(printf '%s' "$INPUT" | jq -r '.hook_event_name // "Unknown"' 2>/dev/null)"
[ -z "$EVENT" ] && EVENT="Unknown"

# %3N (milliseconds) isn't supported by BusyBox date (Alpine); plain
# second-resolution UTC timestamps are portable everywhere this runs.
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

ENTRY="$(printf '%s' "$INPUT" | jq -c --arg ts "$TS" --arg event "$EVENT" \
  '{logged_at: $ts, hook_event_name: $event, payload: .}' 2>/dev/null)"

# If jq failed to parse stdin (malformed payload), still record that the hook
# fired so nothing goes silently missing.
if [ -z "$ENTRY" ]; then
  ENTRY="$(jq -n -c --arg ts "$TS" --arg event "$EVENT" --arg raw "$INPUT" \
    '{logged_at: $ts, hook_event_name: $event, parse_error: true, raw: $raw}')"
fi

printf '%s\n' "$ENTRY" >> "$LOG_DIR/${EVENT}.jsonl"
printf '%s\n' "$ENTRY" >> "$LOG_DIR/all-events.jsonl"

exit 0
