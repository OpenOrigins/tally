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
"$PYTHON" -B tests/test_package_release.py
"$PYTHON" -B tests/test_homebrew_cask.py
cargo build --package tally --release --locked --target "$HOST_TARGET"
"$PYTHON" -B tests/native_install_smoke.py \
  --tally "target/$HOST_TARGET/release/tally"
"$PYTHON" -B scripts/package_release.py \
  --target "$HOST_TARGET" \
  --label local-smoke \
  --output target/release-assets-smoke
"$PYTHON" -B scripts/check_release_tag.py

LANGGRAPH_VENV="$REPO_ROOT/target/langgraph-release-venv"
LANGGRAPH_SMOKE_VENV="$REPO_ROOT/target/langgraph-wheel-smoke-venv"
rm -rf "$LANGGRAPH_VENV" "$LANGGRAPH_SMOKE_VENV" integrations/langgraph/dist
"$PYTHON" -m venv "$LANGGRAPH_VENV"
"$LANGGRAPH_VENV/bin/python" -m pip install --upgrade pip \
  "$REPO_ROOT/integrations/langgraph[test]"
(
  cd integrations/langgraph
  "$LANGGRAPH_VENV/bin/python" -m ruff format --check .
  "$LANGGRAPH_VENV/bin/python" -m ruff check .
  "$LANGGRAPH_VENV/bin/python" -m mypy --strict src/tally_langgraph
  "$LANGGRAPH_VENV/bin/python" -m pytest
  "$LANGGRAPH_VENV/bin/python" -m build --wheel
  "$LANGGRAPH_VENV/bin/python" -m twine check dist/*.whl
)
"$PYTHON" -m venv "$LANGGRAPH_SMOKE_VENV"
"$LANGGRAPH_SMOKE_VENV/bin/python" -m pip install \
  integrations/langgraph/dist/*.whl
"$LANGGRAPH_SMOKE_VENV/bin/python" -c \
  'import tally_langgraph; print(tally_langgraph.__version__)'

printf 'Release checks passed.\n'
