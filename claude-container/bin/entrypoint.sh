#!/usr/bin/env bash
set -Eeuo pipefail

export TALLY_LOG_ROOT="${TALLY_LOG_ROOT:-/var/log/tally-claude}"
export TALLY_WORKSPACE="${TALLY_WORKSPACE:-/workspace}"
export TALLY_RUN_ID="${TALLY_RUN_ID:-run_$(date -u +%Y%m%dT%H%M%SZ)_$HOSTNAME}"
export TALLY_AGENT_ID="${TALLY_AGENT_ID:-claude-container}"
export TALLY_AGENT_VERSION="${TALLY_AGENT_VERSION:-claude-code-cli}"
export PATH="/opt/tally-claude/bin:$PATH"

CLAUDE_HOME="${HOME:-/home/claude}/.claude"
mkdir -p "$CLAUDE_HOME" "$TALLY_LOG_ROOT" "$TALLY_LOG_ROOT/jsonl" "$TALLY_LOG_ROOT/tally" "$TALLY_LOG_ROOT/private" "$TALLY_LOG_ROOT/state"

if [ ! -f "$CLAUDE_HOME/settings.json" ]; then
  cp /opt/tally-claude/claude/settings.json "$CLAUDE_HOME/settings.json"
fi

if [ "${TALLY_OVERWRITE_CLAUDE_HOOKS:-1}" = "1" ]; then
  /opt/tally-claude/bin/tally-claude install-desktop-hooks >/dev/null
fi

if [ "$#" -eq 0 ]; then
  set -- claude
fi

case "$1" in
  bash|sh|/bin/bash|/bin/sh)
    exec "$@"
    ;;
  claude)
    shift
    exec /opt/tally-claude/bin/tally-claude wrap "$@"
    ;;
  *)
    exec /opt/tally-claude/bin/tally-claude wrap "$@"
    ;;
esac
