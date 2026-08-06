#!/usr/bin/env bash
set -Eeuo pipefail

MOCK_PORT="${MOCK_PORT:-4141}"
node /workspace/tests/fixtures/claude-api-mock.js &
MOCK_PID=$!
trap 'kill "$MOCK_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 20); do
  if curl -fs -o /dev/null -X POST "http://127.0.0.1:${MOCK_PORT}/v1/messages" -d '{}'; then
    break
  fi
  sleep 0.25
done

export ANTHROPIC_BASE_URL="http://127.0.0.1:${MOCK_PORT}"
export ANTHROPIC_API_KEY="dummy-test-key-not-real"

tally-claude wrap --print \
  "Run the Bash tool to echo a greeting, then stop." \
  --dangerously-skip-permissions
