#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_PATH="${TALLY_HOST_HOOK_BIN:-$HOME/.tally-codex/bin/tally-host-hook}"

if [ ! -x "$BIN_PATH" ]; then
  "$SCRIPT_DIR/build-host-hook.sh"
fi

"$BIN_PATH" uninstall
