#!/usr/bin/env bash
set -euo pipefail

if command -v musl-gcc >/dev/null 2>&1; then
  exit 0
fi

version_codename="$(sed -n 's/^VERSION_CODENAME=//p' /etc/os-release | tr -d '"')"
: "${version_codename:?Ubuntu version codename is unavailable}"

sources_file="$(mktemp --suffix=.sources)"
trap 'rm -f "$sources_file"' EXIT
printf '%s\n' \
  'Types: deb' \
  'URIs: https://archive.ubuntu.com/ubuntu' \
  "Suites: ${version_codename}" \
  'Components: main universe' \
  'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
  >"$sources_file"

apt_options=(
  -o "Dir::Etc::sourcelist=${sources_file}"
  -o Dir::Etc::sourceparts=-
  -o Acquire::Retries=2
  -o Acquire::http::Timeout=20
  -o Acquire::https::Timeout=20
)

for attempt in 1 2 3; do
  if sudo timeout 90s apt-get "${apt_options[@]}" update \
    && sudo timeout 90s apt-get "${apt_options[@]}" install \
      --yes --no-install-recommends musl-tools; then
    command -v musl-gcc >/dev/null
    exit 0
  fi

  echo "musl-tools installation attempt ${attempt} failed; retrying" >&2
  sleep $((attempt * 5))
done

echo "musl-tools installation failed after 3 attempts" >&2
exit 1
