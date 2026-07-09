#!/usr/bin/env bash
# This build environment's egress corrupts long-lived HTTPS transfers
# (confirmed via curl: "SSL_read: decryption failed or bad record mac" —
# TCP delivers the bytes intact but TLS record decryption fails partway
# through large downloads, worse under AES-NI; smaller/short-lived requests
# succeed reliably). npm's own fetch of the ~75MB platform-native
# claude-code binary package hits this every time and silently falls back
# to a stub. Downloading it ourselves in small independently-retried Range
# chunks sidesteps the failure mode entirely, then we hand the validated
# tarball to npm/install.cjs exactly as if npm had fetched it.
set -euo pipefail

PKG="$1"          # e.g. @anthropic-ai/claude-code-linux-x64-musl
VERSION="$2"      # e.g. 2.1.202
DEST_DIR="$3"     # e.g. /usr/local/lib/node_modules/@anthropic-ai/claude-code-linux-x64-musl

NAME="${PKG#@anthropic-ai/}"
URL="https://registry.npmjs.org/${PKG}/-/${NAME}-${VERSION}.tgz"
OUT="/tmp/${NAME}-${VERSION}.tgz"
CHUNK=$((2 * 1024 * 1024))

rm -f "$OUT"

TOTAL="$(curl -sSI -r 0-0 "$URL" | tr -d '\r' | awk -F'/' '/^[Cc]ontent-[Rr]ange/{print $2}')"
if [ -z "$TOTAL" ]; then
  echo "fetch_native_binary: could not determine size for $URL" >&2
  exit 1
fi
echo "fetch_native_binary: $PKG@$VERSION is $TOTAL bytes, fetching in $((CHUNK / 1024 / 1024))MB chunks..."

start=0
chunks=0
while [ "$start" -lt "$TOTAL" ]; do
  end=$((start + CHUNK - 1))
  if [ "$end" -ge "$TOTAL" ]; then end=$((TOTAL - 1)); fi
  ok=0
  for _ in $(seq 1 15); do
    if curl -sS --range "$start-$end" -o /tmp/chunk.bin "$URL" 2>/tmp/chunk_err.log; then
      got="$(stat -c%s /tmp/chunk.bin)"
      want=$((end - start + 1))
      if [ "$got" -eq "$want" ]; then
        ok=1
        break
      fi
    fi
  done
  if [ "$ok" -ne 1 ]; then
    echo "fetch_native_binary: failed range $start-$end after retries" >&2
    cat /tmp/chunk_err.log >&2 || true
    exit 1
  fi
  cat /tmp/chunk.bin >> "$OUT"
  start=$((end + 1))
  chunks=$((chunks + 1))
done

FINAL_SIZE="$(stat -c%s "$OUT")"
if [ "$FINAL_SIZE" -ne "$TOTAL" ]; then
  echo "fetch_native_binary: size mismatch, got $FINAL_SIZE want $TOTAL" >&2
  exit 1
fi
if ! tar tzf "$OUT" >/dev/null 2>&1; then
  echo "fetch_native_binary: downloaded archive failed integrity check" >&2
  exit 1
fi

echo "fetch_native_binary: downloaded $chunks chunks, $FINAL_SIZE bytes, archive verified OK"

mkdir -p "$DEST_DIR"
tar xzf "$OUT" -C "$DEST_DIR" --strip-components=1
rm -f "$OUT"
echo "fetch_native_binary: extracted to $DEST_DIR"
