#!/usr/bin/env bash
# Drives a real, non-interactive `claude --print` run against the dummy mock
# API server (test/mock-server.js). This organically fires SessionStart,
# UserPromptSubmit, PreToolUse, PostToolUse, and Stop through Claude Code's
# real hook dispatch — not a synthetic injection.
set -uo pipefail

MOCK_PORT="${MOCK_PORT:-4141}"

node /workspace/test/mock-server.js &
MOCK_PID=$!
trap 'kill "$MOCK_PID" 2>/dev/null' EXIT

# Give the mock server a moment to bind before the CLI starts hammering it.
for _ in $(seq 1 20); do
  if curl -s -o /dev/null "http://127.0.0.1:${MOCK_PORT}/v1/messages" -X POST -d '{}'; then
    break
  fi
  sleep 0.25
done

export ANTHROPIC_BASE_URL="http://127.0.0.1:${MOCK_PORT}"
export ANTHROPIC_API_KEY="dummy-test-key-not-real"

cd /workspace
echo "[run_dummy_session] invoking claude --print against the dummy API..."
claude --dangerously-skip-permissions --print \
  "Run the Bash tool to echo a greeting, then stop." || true

echo "[run_dummy_session] done."
