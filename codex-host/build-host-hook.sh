#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_PATH="${TALLY_HOST_HOOK_BIN:-$HOME/.tally-codex/bin/tally-host-hook}"
CARGO_BIN="${CARGO:-}"
if [ -z "$CARGO_BIN" ]; then
  CARGO_BIN="$(command -v cargo || true)"
fi
if [ -z "$CARGO_BIN" ] && [ -x "$HOME/.cargo/bin/cargo" ]; then
  CARGO_BIN="$HOME/.cargo/bin/cargo"
fi
if [ -z "$CARGO_BIN" ]; then
  echo "Rust cargo is required to build tally-host-hook." >&2
  echo "Install Rust or set CARGO=/path/to/cargo." >&2
  exit 127
fi

mkdir -p "$(dirname "$BIN_PATH")"
cd "$SCRIPT_DIR"
"$CARGO_BIN" build --release --bin tally-host-hook
cp "$SCRIPT_DIR/target/release/tally-host-hook" "$BIN_PATH"
chmod +x "$BIN_PATH"
printf "Built Tally host hook binary at %s\n" "$BIN_PATH"
