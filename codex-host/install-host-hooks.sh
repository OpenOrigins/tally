#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/build-host-hook.sh"
"${TALLY_CODEX_BIN:-${TALLY_HOST_HOOK_BIN:-$HOME/.tally-codex/bin/tally-codex}}" install-desktop-hooks
