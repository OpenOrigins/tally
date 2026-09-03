#!/usr/bin/env bash
# Starts (or reuses) the webhook server, then starts a fresh cloudflared quick tunnel
# and prints the URL to paste into GitHub's webhook Payload URL field.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

if ! curl -s -m 2 http://localhost:8000/health > /dev/null 2>&1; then
  echo "Starting webhook server..."
  source myvenv/bin/activate
  nohup uvicorn github_monitor.webhook_server:app --port 8000 > /tmp/uvicorn.log 2>&1 &
  disown
  sleep 3
else
  echo "Webhook server already running on :8000."
fi

echo "Restarting tunnel..."
pkill -f "cloudflared tunnel" 2>/dev/null || true
sleep 1
rm -f /tmp/cloudflared.log

# --protocol http2 avoids QUIC/UDP, which was unreliable on this network.
nohup "$HOME/.local/bin/cloudflared" tunnel --protocol http2 --url http://localhost:8000 \
  > /tmp/cloudflared.log 2>&1 &
disown

echo -n "Waiting for tunnel URL"
for _ in $(seq 1 15); do
  sleep 1
  echo -n "."
  URL=$(grep -oE 'https://[a-zA-Z0-9.-]+\.trycloudflare\.com' /tmp/cloudflared.log | head -1 || true)
  if [[ -n "$URL" ]]; then
    break
  fi
done
echo

if [[ -z "${URL:-}" ]]; then
  echo "Failed to obtain a tunnel URL -- check /tmp/cloudflared.log"
  exit 1
fi

# Use DNS-over-HTTPS for this check -- this machine's default resolver has been seen
# to return stale NXDOMAIN for brand new trycloudflare.com subdomains for a bit, even
# when the tunnel itself is already live and GitHub can reach it fine.
if curl -s -m 8 --doh-url https://1.1.1.1/dns-query "$URL/health" > /dev/null 2>&1; then
  echo "Tunnel is live and reachable."
else
  echo "WARNING: tunnel URL printed but not yet responding -- check /tmp/cloudflared.log"
fi

echo
echo "Payload URL to paste into GitHub (Settings -> Webhooks -> Payload URL):"
echo "  $URL/webhook"
