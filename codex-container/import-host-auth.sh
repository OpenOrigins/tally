#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOST_CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
HOST_AUTH_JSON="$HOST_CODEX_HOME/auth.json"

if [ ! -f "$HOST_AUTH_JSON" ]; then
  cat >&2 <<EOF
No file-based Codex auth cache found at:
  $HOST_AUTH_JSON

Run 'codex login' locally with file-based credential storage, or authenticate
inside the container with:
  docker compose -f "$SCRIPT_DIR/compose.yaml" run --rm codex login --device-auth
EOF
  exit 1
fi

docker compose -f "$SCRIPT_DIR/compose.yaml" run --rm --entrypoint /bin/bash codex -lc '
  set -Eeuo pipefail
  mkdir -p /codex-home
  umask 077
  cat > /codex-home/auth.json
  chmod 600 /codex-home/auth.json
  printf "Imported Codex auth into /codex-home/auth.json\\n"
' < "$HOST_AUTH_JSON"
