#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/../claude-audit" && pwd)"
BIN_PATH="${TALLY_CLAUDE_BIN:-$HOME/.tally-claude/bin/tally-claude}"
CARGO_BIN="${CARGO:-}"
if [ -z "$CARGO_BIN" ]; then
  CARGO_BIN="$(command -v cargo || true)"
fi
if [ -z "$CARGO_BIN" ] && [ -x "$HOME/.cargo/bin/cargo" ]; then
  CARGO_BIN="$HOME/.cargo/bin/cargo"
fi
if [ -z "$CARGO_BIN" ]; then
  echo "Rust cargo is required to build tally-claude." >&2
  echo "Install Rust or set CARGO=/path/to/cargo." >&2
  exit 127
fi

mkdir -p "$(dirname "$BIN_PATH")"
"$CARGO_BIN" build --release --manifest-path "$CRATE_DIR/Cargo.toml" --bin tally-claude
cp "$CRATE_DIR/target/release/tally-claude" "$BIN_PATH"
chmod +x "$BIN_PATH"
printf "Built Tally Claude Code binary at %s\n" "$BIN_PATH"
