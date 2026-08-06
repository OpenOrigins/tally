#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PYTHON="${PYTHON:-python3}"
HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"

cd "$REPO_ROOT"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
"$PYTHON" -B tests/test_push_test_logs.py
cargo build --workspace --release --locked --target "$HOST_TARGET"
"$PYTHON" -B tests/native_install_smoke.py \
  --codex "target/$HOST_TARGET/release/tally-codex" \
  --claude "target/$HOST_TARGET/release/tally-claude"
"$PYTHON" -B scripts/package_release.py \
  --target "$HOST_TARGET" \
  --label local-smoke \
  --output target/release-assets-smoke
"$PYTHON" -B scripts/check_release_tag.py

printf 'Release checks passed.\n'
