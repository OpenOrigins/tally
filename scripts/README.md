# Push test logs

Generate random [Tally](../Tally-SPEC.md) v0.2 records and `POST` them to OpenOrigins ingest (`/v1/tally/logs`).

Uses stdlib only — no pip install.

## Quick start

```bash
cd tally
./scripts/push_test_logs.py --env dev2 --api-key "$OO_API_KEY" --count 12
```

Expect `HTTP 200` and a JSON body like:

```json
{
  "ingest_id": "...",
  "record_count": 12,
  "remaining_allowance": 999988,
  "status": "accepted"
}
```

## Options

| Flag | Default | Notes |
|------|---------|--------|
| `--env` | — | `dev1` / `dev2` / `dev3` / `prod` (sets URL) |
| `--url` | — | Full ingest URL (overrides `--env`) |
| `--api-key` | **required** | Org API key (`x-api-key` header, lowercase) |
| `-n` / `--count` | `5` | Records per run (1–50) |
| `--include-invalid` | off | ~25% intentionally non-compliant (for pipeline testing) |
| `--dry-run` | off | Print payload only; no HTTP call |
| `--one-per-request` | off | One POST per record instead of a `{ "records": [...] }` batch |
| `--source` | `push-test-logs` | `x-oo-tally-source` |
| `--ingest-path` | `scripts/push_test_logs.py` | `x-oo-tally-ingest-path` |
| `--seed` | — | Reproducible RNG |
| `--timeout` | `30` | Seconds |

## Examples

```bash
# Preview payload
./scripts/push_test_logs.py --env dev2 --api-key "$OO_API_KEY" --count 5 --dry-run

# Mix in bad records
./scripts/push_test_logs.py --env dev2 --api-key "$OO_API_KEY" --count 20 --include-invalid

# Custom endpoint
./scripts/push_test_logs.py \
  --url https://api.dev2.openorigins.com/v1/tally/logs \
  --api-key "$OO_API_KEY" \
  --count 8
```

## Notes

- Auth is the **org API key**, not a Cognito token (same as SDK ingest).
- Do **not** use Python `urllib` against this Gateway with a title-cased `X-Api-Key` — the authorizer identity source only accepts lowercase `x-api-key`. This script uses `http.client` and sends the correct casing.
- Without `--include-invalid`, records are shaped to pass `oo-tally` validation (schema `0.2`).
- Quota is reserved at ingest; confirmed after `tally-process-service` stores the logs.
