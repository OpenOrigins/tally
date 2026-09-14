# Changelog

## 0.1.0

- Initial supported `tally-langgraph` package.
- Durable ordered SQLite outbox with retry, leases, idempotency, and dead letters.
- Tally `0.2` lifecycle, instruction, action, result, handoff, turn, session, and
  heartbeat records.
- Bounded/redacted server evidence with local SHA-256-addressed private evidence.
- Synchronous and asynchronous explicit handoff helpers based on public LangChain APIs.
- Session-level token usage totals from LangChain model callbacks.
