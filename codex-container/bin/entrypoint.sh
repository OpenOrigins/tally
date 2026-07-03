#!/usr/bin/env bash
set -Eeuo pipefail

export CODEX_HOME="${CODEX_HOME:-/codex-home}"
export TALLY_LOG_ROOT="${TALLY_LOG_ROOT:-/var/log/tally-codex}"
export TALLY_WORKSPACE="${TALLY_WORKSPACE:-/workspace}"
export TALLY_RUN_ID="${TALLY_RUN_ID:-run_$(date -u +%Y%m%dT%H%M%SZ)_$HOSTNAME}"
export TALLY_AGENT_ID="${TALLY_AGENT_ID:-codex-container}"
export TALLY_AGENT_VERSION="${TALLY_AGENT_VERSION:-codex-cli}"

mkdir -p "$CODEX_HOME" "$TALLY_LOG_ROOT" "$TALLY_LOG_ROOT/jsonl" "$TALLY_LOG_ROOT/tally" "$TALLY_LOG_ROOT/private" "$TALLY_LOG_ROOT/state"

if [ "${TALLY_OVERWRITE_CODEX_HOOKS:-1}" = "1" ] || [ ! -f "$CODEX_HOME/hooks.json" ]; then
  cp /opt/tally-codex/codex/hooks.json "$CODEX_HOME/hooks.json"
fi

if [ "${TALLY_OVERWRITE_CODEX_CONFIG:-0}" = "1" ] || [ ! -f "$CODEX_HOME/config.toml" ]; then
  cp /opt/tally-codex/codex/config.toml "$CODEX_HOME/config.toml"
fi

if [ -n "${CODEX_ACCESS_TOKEN:-}" ] && [ ! -f "$CODEX_HOME/auth.json" ]; then
  printenv CODEX_ACCESS_TOKEN | codex login --with-access-token >/dev/null
fi

if [ "$#" -eq 0 ]; then
  set -- codex
fi

case "$1" in
  bash|sh|python|python3|/bin/bash|/bin/sh)
    exec "$@"
    ;;
  codex)
    shift
    exec /opt/tally-codex/bin/tally-codex "$@"
    ;;
  *)
    exec /opt/tally-codex/bin/tally-codex "$@"
    ;;
esac
