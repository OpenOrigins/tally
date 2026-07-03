# Tally Codex Audit Container

This container runs Codex inside Docker through the shared Rust `tally-codex`
binary. The binary wraps the Codex CLI, receives Codex lifecycle hooks, writes
audit logs to a host-mounted directory, and runs the hook heartbeat daemon.

The first milestone is:

- build and copy the shared `tally-codex` Rust binary
- wrap the `codex` CLI with `tally-codex wrap`
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

The Dockerfile uses a Rust builder stage for `tally-codex`, then installs the
current Codex CLI with OpenAI's standalone installer in the final image.

## Run Interactive Codex

```bash
docker compose run --rm codex
```

## Run A Prompt

For read-only prompts:

```bash
docker compose run --rm \
  -e TALLY_RUN_ID=summary-demo \
  codex codex exec --json -s read-only \
  -c approval_policy='"never"' \
  "Summarize this repository in 5 bullets."
```

For prompts that should edit files, use Docker as the sandbox boundary:

```bash
docker compose run --rm \
  -e TALLY_RUN_ID=edit-demo \
  codex codex exec --json -s danger-full-access \
  -c approval_policy='"never"' \
  "Create hello_from_codex.txt with one sentence."
```

`workspace-write` can fail inside Docker because Codex's inner filesystem
sandbox uses Linux namespace features that are often unavailable inside an
unprivileged container. With this wrapper, the mounted project directory is the
workspace boundary: only mount folders you want Codex to access.

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
On `SessionStart`, `tally-codex hook SessionStart` starts a `hook-heartbeat` daemon that
emits periodic heartbeats until `Stop` or until the idle timeout is reached.

## Security Notes

The container records Codex-visible events, but it is not a complete security
boundary by itself. Run it with a dedicated Docker user, mount only the workspace
you intend Codex to touch, and send `.tally-codex-logs/tally/` through Atlas or
another independent anchor quickly if you need tamper evidence beyond the local
machine.
