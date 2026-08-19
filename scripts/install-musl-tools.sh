#!/usr/bin/env bash
set -euo pipefail

if command -v musl-gcc >/dev/null 2>&1; then
  exit 0
fi

for attempt in 1 2 3; do
  if sudo timeout 90s apt-get -o Acquire::Retries=2 update \
    && sudo timeout 90s apt-get -o Acquire::Retries=2 install --yes musl-tools; then
    command -v musl-gcc >/dev/null
    exit 0
  fi

  echo "musl-tools installation attempt ${attempt} failed; retrying" >&2
  sleep $((attempt * 5))
done

echo "musl-tools installation failed after 3 attempts" >&2
exit 1
