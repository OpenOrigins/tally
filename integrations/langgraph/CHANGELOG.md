# Changelog

## Unreleased

- Add a `tally-langgraph connect` CLI for one-time installation setup: saves
  `TALLY_API_KEY`/`TALLY_API_URL` to `.env` and performs the dashboard's
  `client-connected` handshake, matching the contract shared by the other Tally
  clients. The handshake also establishes and prints this installation's
  persisted `agent_id`.
- Bundle `certifi`'s CA store for all outbound HTTPS requests (handshake and log
  delivery alike), so TLS verification works even on Python installs that don't
  link to the platform certificate store (notably python.org builds on macOS,
  which otherwise fail every HTTPS request with
  `CERTIFICATE_VERIFY_FAILED: unable to get local issuer certificate`).

## 0.1.0

- Initial supported `tally-langgraph` package.
- Durable ordered SQLite outbox with retry, leases, idempotency, and dead letters.
- Tally `0.2` lifecycle, instruction, action, result, handoff, turn, session, and
  heartbeat records.
- Bounded/redacted server evidence with local SHA-256-addressed private evidence.
- Synchronous and asynchronous explicit handoff helpers based on public LangChain APIs.
