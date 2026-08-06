#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOST_CLAUDE_JSON="${HOME}/.claude.json"

if [ ! -f "$HOST_CLAUDE_JSON" ]; then
  cat >&2 <<EOF
No Claude Code session file found at:
  $HOST_CLAUDE_JSON

Run 'claude' locally and complete login first, or authenticate inside the
container instead with an API key:
  docker compose -f "$SCRIPT_DIR/compose.yaml" run --rm \\
    -e ANTHROPIC_API_KEY=sk-ant-... \\
    claude claude -p "Respond exactly: host-auth-ok"
EOF
  exit 1
fi

docker compose -f "$SCRIPT_DIR/compose.yaml" run --rm -T --entrypoint /bin/bash claude -lc '
  set -Eeuo pipefail
  umask 077
  cat > "$HOME/.claude.json"
  chmod 600 "$HOME/.claude.json"
  printf "Imported Claude Code session into $HOME/.claude.json\\n"
' < "$HOST_CLAUDE_JSON"
