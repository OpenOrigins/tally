#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tally-host-smoke.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

assert_records() {
  local log_root="$1"
  local source="$2"
  local expected actual
  expected=$'ACTION_TAKEN\nINSTRUCTION_RECEIVED\nRESULT_RECEIVED\nSESSION_END\nSESSION_START'
  actual="$(find "$log_root/tally/$source" -type f -name '*.json' -exec jq -r '.record_type' {} + | sort)"
  test "$actual" = "$expected"

  local action_id result_id
  action_id="$(find "$log_root/tally/$source" -type f -name '*.json' -exec jq -r 'select(.record_type == "ACTION_TAKEN") | .action_id' {} +)"
  result_id="$(find "$log_root/tally/$source" -type f -name '*.json' -exec jq -r 'select(.record_type == "RESULT_RECEIVED") | .action_id' {} +)"
  test -n "$action_id"
  test "$action_id" = "$result_id"
}

record_session() {
  local binary="$1"
  local log_root="$2"
  local -a command=(env TALLY_HOOK_HEARTBEAT_ENABLED=0 TALLY_LOG_ROOT="$log_root" "$binary" hook)
  printf '%s' '{"session_id":"host-release-session"}' | "${command[@]}" SessionStart
  printf '%s' '{"session_id":"host-release-session","prompt":"test prompt"}' | "${command[@]}" UserPromptSubmit
  printf '%s' '{"session_id":"host-release-session","tool_call_id":"tool-1","tool_use_id":"tool-1","tool_name":"Bash","tool_input":{"command":"true"}}' | "${command[@]}" PreToolUse
  printf '%s' '{"session_id":"host-release-session","tool_call_id":"tool-1","tool_use_id":"tool-1","tool_response":{"stdout":""}}' | "${command[@]}" PostToolUse
  printf '%s' '{"session_id":"host-release-session"}' | "${command[@]}" Stop
}

CODEX_ROOT="$TEST_ROOT/codex"
CODEX_BIN="$CODEX_ROOT/bin/tally-codex"
CODEX_HOOKS="$CODEX_ROOT/config/hooks.json"
mkdir -p "$(dirname "$CODEX_HOOKS")"
jq -n '{theme:"dark",hooks:{SessionStart:[{hooks:[{type:"command",command:"/bin/echo keep"}]}]}}' > "$CODEX_HOOKS"

CODEX_HOOKS_PATH="$CODEX_HOOKS" \
TALLY_CODEX_BIN="$CODEX_BIN" \
TALLY_HOOK_HEARTBEAT_ENABLED=0 \
TALLY_LOG_ROOT="$CODEX_ROOT/logs" \
  "$REPO_ROOT/codex-host/install-host-hooks.sh"

test -x "$CODEX_BIN"
test "$(jq -r '.theme' "$CODEX_HOOKS")" = dark
jq -e '.. | objects | .command? | select(. == "/bin/echo keep")' "$CODEX_HOOKS" >/dev/null
record_session "$CODEX_BIN" "$CODEX_ROOT/logs"
assert_records "$CODEX_ROOT/logs" codex-hooks

CODEX_HOOKS_PATH="$CODEX_HOOKS" \
TALLY_CODEX_BIN="$CODEX_BIN" \
  "$REPO_ROOT/codex-host/uninstall-host-hooks.sh"

jq -e '.. | objects | .command? | select(. == "/bin/echo keep")' "$CODEX_HOOKS" >/dev/null
if jq -e '.. | objects | .command? | strings | select(contains("tally-codex"))' "$CODEX_HOOKS" >/dev/null; then
  echo "Codex uninstall left a Tally hook behind." >&2
  exit 1
fi

CLAUDE_ROOT="$TEST_ROOT/claude"
CLAUDE_BIN="$CLAUDE_ROOT/bin/tally-claude"
CLAUDE_SETTINGS="$CLAUDE_ROOT/config/settings.json"
mkdir -p "$(dirname "$CLAUDE_SETTINGS")"
jq -n '{theme:"dark",hooks:{SessionStart:[{hooks:[{type:"command",command:"/bin/echo keep"}]}]}}' > "$CLAUDE_SETTINGS"

TALLY_CLAUDE_SETTINGS_PATH="$CLAUDE_SETTINGS" \
TALLY_CLAUDE_BIN="$CLAUDE_BIN" \
TALLY_HOOK_HEARTBEAT_ENABLED=0 \
TALLY_LOG_ROOT="$CLAUDE_ROOT/logs" \
  "$REPO_ROOT/claude-host/install-host-hooks.sh"

test -x "$CLAUDE_BIN"
test "$(jq -r '.theme' "$CLAUDE_SETTINGS")" = dark
jq -e '.. | objects | .command? | select(. == "/bin/echo keep")' "$CLAUDE_SETTINGS" >/dev/null
record_session "$CLAUDE_BIN" "$CLAUDE_ROOT/logs"
assert_records "$CLAUDE_ROOT/logs" claude-hooks

TALLY_CLAUDE_SETTINGS_PATH="$CLAUDE_SETTINGS" \
TALLY_CLAUDE_BIN="$CLAUDE_BIN" \
  "$REPO_ROOT/claude-host/uninstall-host-hooks.sh"

jq -e '.. | objects | .command? | select(. == "/bin/echo keep")' "$CLAUDE_SETTINGS" >/dev/null
if jq -e '.. | objects | .command? | strings | select(contains("tally-claude"))' "$CLAUDE_SETTINGS" >/dev/null; then
  echo "Claude uninstall left a Tally hook behind." >&2
  exit 1
fi

printf 'Host release smoke tests passed.\n'
