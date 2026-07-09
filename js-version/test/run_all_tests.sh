#!/usr/bin/env bash
# Runs the full logging test: a real dummy-API session (organic hooks) plus
# synthetic injection (hard-to-trigger hooks), then verifies every one of the
# 10 requested events produced a log line.
set -uo pipefail

LOG_DIR="${CLAUDE_HOOK_LOG_DIR:-/var/log/claude-hooks}"
mkdir -p "$LOG_DIR"

echo "=== Step 1: dummy API session (organic hook firing) ==="
bash /workspace/test/run_dummy_session.sh

echo
echo "=== Step 2: synthetic injection (hard-to-trigger events) ==="
bash /workspace/test/inject_synthetic_events.sh

EXPECTED_EVENTS=(
  SessionStart
  UserPromptSubmit
  PreToolUse
  PermissionRequest
  PostToolUse
  PreCompact
  PostCompact
  SubagentStart
  SubagentStop
  Stop
)

echo
echo "=== Step 3: verifying logs ==="
FAILED=0
for ev in "${EXPECTED_EVENTS[@]}"; do
  f="$LOG_DIR/${ev}.jsonl"
  if [ -s "$f" ]; then
    n="$(wc -l < "$f")"
    echo "  [OK]   $ev -> $f ($n line(s))"
  else
    echo "  [MISS] $ev -> $f not found or empty"
    FAILED=1
  fi
done

echo
if [ -f "$LOG_DIR/all-events.jsonl" ]; then
  total="$(wc -l < "$LOG_DIR/all-events.jsonl")"
  echo "Combined log: $LOG_DIR/all-events.jsonl ($total line(s) total)"
fi

if [ "$FAILED" -eq 0 ]; then
  echo
  echo "ALL 10 REQUESTED HOOK EVENTS LOGGED SUCCESSFULLY."
else
  echo
  echo "SOME EVENTS DID NOT PRODUCE A LOG ENTRY. See [MISS] lines above."
  exit 1
fi
