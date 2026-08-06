#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 <claude|codex>" >&2
  exit 64
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT="$1"

case "$AGENT" in
  claude)
    PACKAGE="tally-claude"
    BIN_PATH="${TALLY_CLAUDE_BIN:-$HOME/.tally-claude/bin/tally-claude}"
    DISPLAY_NAME="Tally Claude Code"
    ;;
  codex)
    PACKAGE="tally-codex"
    BIN_PATH="${TALLY_CODEX_BIN:-${TALLY_HOST_HOOK_BIN:-$HOME/.tally-codex/bin/tally-codex}}"
    DISPLAY_NAME="Tally Codex"
    ;;
  *)
    echo "Unsupported agent: $AGENT" >&2
    exit 64
    ;;
esac

CARGO_BIN="${CARGO:-$(command -v cargo || true)}"
if [ -z "$CARGO_BIN" ] && [ -x "$HOME/.cargo/bin/cargo" ]; then
  CARGO_BIN="$HOME/.cargo/bin/cargo"
fi
if [ -z "$CARGO_BIN" ]; then
  echo "Rust cargo is required to build $PACKAGE." >&2
  echo "Install Rust or set CARGO=/path/to/cargo." >&2
  exit 127
fi

mkdir -p "$(dirname "$BIN_PATH")"
"$CARGO_BIN" build --locked --release --manifest-path "$REPO_ROOT/Cargo.toml" --package "$PACKAGE"
cp "$REPO_ROOT/target/release/$PACKAGE" "$BIN_PATH"
chmod +x "$BIN_PATH"
printf "Built %s binary at %s\n" "$DISPLAY_NAME" "$BIN_PATH"
