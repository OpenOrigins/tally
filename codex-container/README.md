# Tally Codex Audit Container

This container runs Codex inside Docker through a small wrapper that installs
Codex lifecycle hooks and writes audit logs to a host-mounted directory.

The first milestone is intentionally small:

- wrap the `codex` CLI
- copy a known `hooks.json` into `CODEX_HOME`
- record Codex hook events as JSONL
- write Tally-style records for session/prompt/tool/stop events
- emit a hook-based heartbeat while a Codex session is active
- tee `codex exec` stdout/stderr into the same log directory

Logs are written under `.tally-codex-logs/` at the repository root when using
the included Compose file.

## Build

```bash
cd codex-container
docker compose build
```

The Dockerfile installs the current Codex CLI with OpenAI's standalone installer.

## Run Interactive Codex

```bash
docker compose run --rm codex
```

## Use This Laptop's Codex Auth

If this laptop is already signed in with the Codex CLI and has a file-based
`~/.codex/auth.json`, import it into the container's `codex-home` volume:

```bash
./import-host-auth.sh
docker compose run --rm codex codex login status
```

The script copies the auth cache through stdin and does not print token
contents.

For a headless login:

```bash
docker compose run --rm codex login --device-auth
```

For non-interactive API-key usage:

```bash
CODEX_API_KEY=... docker compose run --rm -e CODEX_API_KEY codex exec --json "summarize this repository"
```

## Log Layout

```text
.tally-codex-logs/
  codex-native/      # Codex's own plaintext diagnostics when enabled by config
  codex-stdio/       # stdout/stderr tee for `codex exec`
  jsonl/             # append-only operational JSONL streams
  private/           # raw hook and outside-observation payloads
  tally/             # Tally-style JSON records
  state/             # local counters and monitor state
```

The important streams are:

- `jsonl/codex-hooks.jsonl`
- `jsonl/hook-heartbeat.jsonl`
- `tally/codex-hooks/*.json`
- `tally/hook-heartbeat/*.json`

## Controls

- `TALLY_OVERWRITE_CODEX_HOOKS=0` keeps an existing `$CODEX_HOME/hooks.json`.
- `TALLY_OVERWRITE_CODEX_CONFIG=1` replaces an existing `$CODEX_HOME/config.toml`.
- `TALLY_HOOK_HEARTBEAT_ENABLED=0` disables hook-driven heartbeat records.
- `TALLY_HOOK_HEARTBEAT_SECONDS=60` controls the periodic hook heartbeat daemon.
- `TALLY_HOOK_HEARTBEAT_IDLE_SECONDS=300` stops the hook heartbeat daemon after hook inactivity.
- `TALLY_BYPASS_HOOK_TRUST=0` stops the wrapper from adding `--dangerously-bypass-hook-trust`.

## Hook Heartbeat

Hooks emit immediate `hook-event` heartbeat records whenever Codex fires a hook.
On `SessionStart`, the hook logger starts a small `hook-heartbeat` daemon that
emits periodic heartbeats until `Stop` or until the idle timeout is reached.

## Security Notes

The container records Codex-visible events, but it is not a complete security
boundary by itself. Run it with a dedicated Docker user, mount only the workspace
you intend Codex to touch, and send `.tally-codex-logs/tally/` through Atlas or
another independent anchor quickly if you need tamper evidence beyond the local
machine.
