#!/usr/bin/env bash
# Generic Codex CLI hook logger.
#
# Codex invokes this script once per hook firing, feeding a single JSON
# object on stdin (the shape depends on which event fired, but every event
# includes "hook_event_name" or is tagged via the CLI argument this script's
# hooks.json entry passes as $1). This script never blocks on argv — the
# event type is read from stdin first and falls back to $1 — and it must
# exit 0 with no stdout so it stays a pure observer and never influences
# permission decisions, prompt injection, or blocking behavior for the real
# hooks wired to this same script in hooks.json (see install/install_hooks.js).
set -uo pipefail

# Default lands inside this project's own logs/ dir (resolved from this
# script's own location, not the caller's cwd) so it's always writable and
# easy to find, instead of a Unix-only path (/var/log/...) that doesn't
# exist and usually isn't writable on Windows/Git Bash.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${CODEX_HOOK_LOG_DIR:-$SCRIPT_DIR/../logs/raw_hooks}"
mkdir -p "$LOG_DIR"

ARG_EVENT="${1:-}"
INPUT="$(cat)"
# Strip a leading UTF-8 BOM some Windows process-spawning paths prepend to
# stdin — otherwise JSON.parse below fails and gets silently swallowed,
# looking identical to "hook never fired".
INPUT="${INPUT#$'\xef\xbb\xbf'}"

# %3N (milliseconds) isn't supported by BusyBox date (Alpine); plain
# second-resolution UTC timestamps are portable everywhere this runs.
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Built with Node instead of jq: jq is an extra system dependency this
# project doesn't otherwise need (and isn't reliably present, e.g. on a
# fresh Windows/Git Bash install), whereas Node is already required by
# anchor_hook.js/package.json.
EVENT="$(ARG_EVENT="$ARG_EVENT" node -e '
  let raw = "";
  process.stdin.on("data", (d) => { raw += d; });
  process.stdin.on("end", () => {
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed.hook_event_name === "string" && parsed.hook_event_name) {
        process.stdout.write(parsed.hook_event_name);
        return;
      }
    } catch (_) {}
    process.stdout.write(process.env.ARG_EVENT || "");
  });
' <<< "$INPUT")"
[ -z "$EVENT" ] && EVENT="Unknown"

ENTRY="$(TS="$TS" EVENT="$EVENT" node -e '
  let raw = "";
  process.stdin.on("data", (d) => { raw += d; });
  process.stdin.on("end", () => {
    const ts = process.env.TS;
    const event = process.env.EVENT;
    try {
      const payload = JSON.parse(raw);
      process.stdout.write(JSON.stringify({ logged_at: ts, hook_event_name: event, payload }));
    } catch (_) {
      process.stdout.write(JSON.stringify({ logged_at: ts, hook_event_name: event, parse_error: true, raw }));
    }
  });
' <<< "$INPUT")"

printf '%s\n' "$ENTRY" >> "$LOG_DIR/${EVENT}.jsonl"
printf '%s\n' "$ENTRY" >> "$LOG_DIR/all-events.jsonl"

exit 0
