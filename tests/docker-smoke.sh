#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TALLY_VERSION="${TALLY_VERSION:-0.1.0}"
if [ -z "${SOURCE_REVISION:-}" ]; then
  SOURCE_REVISION="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || printf unknown)"
  if [ -n "$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null)" ]; then
    SOURCE_REVISION="${SOURCE_REVISION}-dirty"
  fi
fi
CODEX_IMAGE="${CODEX_IMAGE:-tally-codex-audit:release-test}"
CLAUDE_IMAGE="${CLAUDE_IMAGE:-tally-claude-audit:release-test}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tally-docker-smoke.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

build_image() {
  local dockerfile="$1"
  local image="$2"
  docker build \
    --file "$REPO_ROOT/$dockerfile" \
    --label "org.opencontainers.image.version=$TALLY_VERSION" \
    --label "org.opencontainers.image.revision=$SOURCE_REVISION" \
    --tag "$image" \
    "$REPO_ROOT"
}

assert_record_set() {
  local log_root="$1"
  local source="$2"
  local expected actual
  expected=$'ACTION_TAKEN\nINSTRUCTION_RECEIVED\nRESULT_RECEIVED\nSESSION_END\nSESSION_START'
  actual="$(find "$log_root/tally/$source" -type f -name '*.json' -exec jq -r '.record_type' {} + | sort)"
  if [ "$actual" != "$expected" ]; then
    printf 'Unexpected records for %s:\n%s\n' "$source" "$actual" >&2
    exit 1
  fi

  local action_id result_id
  action_id="$(find "$log_root/tally/$source" -type f -name '*.json' -exec jq -r 'select(.record_type == "ACTION_TAKEN") | .action_id' {} +)"
  result_id="$(find "$log_root/tally/$source" -type f -name '*.json' -exec jq -r 'select(.record_type == "RESULT_RECEIVED") | .action_id' {} +)"
  test -n "$action_id"
  test "$action_id" = "$result_id"
}

exercise_hooks() {
  local agent="$1"
  local image="$2"
  local binary="/opt/tally-$agent/bin/tally-$agent"
  local source="$agent-hooks"
  local log_root="$TEST_ROOT/$agent-hooks"
  mkdir -p "$log_root"
  chmod 0777 "$log_root"

  docker run --rm \
    -e TALLY_HOOK_HEARTBEAT_ENABLED=0 \
    -e TALLY_LOG_ROOT=/logs \
    -e TALLY_RUN_ID="release-$agent-hooks" \
    -v "$log_root:/logs" \
    "$image" bash -lc "
      printf '%s' '{\"session_id\":\"release-session\"}' | $binary hook SessionStart
      printf '%s' '{\"session_id\":\"release-session\",\"prompt\":\"test prompt\"}' | $binary hook UserPromptSubmit
      printf '%s' '{\"session_id\":\"release-session\",\"tool_call_id\":\"tool-1\",\"tool_use_id\":\"tool-1\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"true\"}}' | $binary hook PreToolUse
      printf '%s' '{\"session_id\":\"release-session\",\"tool_call_id\":\"tool-1\",\"tool_use_id\":\"tool-1\",\"tool_response\":{\"stdout\":\"\"}}' | $binary hook PostToolUse
      printf '%s' '{\"session_id\":\"release-session\"}' | $binary hook Stop
    "

  assert_record_set "$log_root" "$source"
}

build_image codex-container/Dockerfile "$CODEX_IMAGE"
build_image claude-container/Dockerfile "$CLAUDE_IMAGE"

docker run --rm "$CODEX_IMAGE" --version | grep -F 'codex-cli 0.146.1'
docker run --rm "$CLAUDE_IMAGE" --version | grep -F '2.1.223'

exercise_hooks codex "$CODEX_IMAGE"
exercise_hooks claude "$CLAUDE_IMAGE"

CLAUDE_LOG_ROOT="$TEST_ROOT/claude-session"
mkdir -p "$CLAUDE_LOG_ROOT"
chmod 0777 "$CLAUDE_LOG_ROOT"
docker run --rm \
  -e TALLY_HOOK_HEARTBEAT_ENABLED=0 \
  -e TALLY_LOG_ROOT=/logs \
  -e TALLY_RUN_ID=release-claude-session \
  -v "$REPO_ROOT:/workspace:ro" \
  -v "$CLAUDE_LOG_ROOT:/logs" \
  "$CLAUDE_IMAGE" bash /workspace/tests/run-claude-session.sh \
  | grep -F 'Dummy task complete. Stopping now.'

assert_record_set "$CLAUDE_LOG_ROOT" claude-hooks

printf 'Docker release smoke tests passed.\n'
