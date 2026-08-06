#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RUN_DOCKER=0

if [ "${1:-}" = "--docker" ]; then
  RUN_DOCKER=1
elif [ "$#" -ne 0 ]; then
  echo "Usage: $0 [--docker]" >&2
  exit 64
fi

cd "$REPO_ROOT"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked

while IFS= read -r script; do
  bash -n "$script"
done < <(find claude-container claude-host codex-container codex-host scripts tests -type f -name '*.sh' | sort)

docker compose -f claude-container/compose.yaml config --quiet
docker compose -f codex-container/compose.yaml config --quiet
tests/host-smoke.sh

if [ "$RUN_DOCKER" = "1" ]; then
  tests/docker-smoke.sh
fi

printf 'Release checks passed.\n'
