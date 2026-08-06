#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/build-host-hook.sh"
"${TALLY_CLAUDE_BIN:-$HOME/.tally-claude/bin/tally-claude}" install-desktop-hooks
