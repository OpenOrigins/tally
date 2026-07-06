# Tally

A spec and standard for trustworthy agent interactions.

## Codex Audit Wrapper

Tally includes a minimal Codex audit wrapper with two deployment modes:

- **Docker container mode** runs Codex inside Docker and writes hook logs to a
  host-mounted log directory.
- **Desktop host mode** keeps Codex running directly on the Mac, including
  Codex Desktop, and installs user-level hooks that write logs on the host.

Both modes use the same Rust binary, `tally-codex`, to capture Codex hook
events, write JSONL streams, emit Tally-style JSON records, tee `codex exec`
output when requested, and produce heartbeat records while a session is active.

```text
codex-audit/       # Rust source for the shared tally-codex binary
codex-container/   # Docker packaging, compose file, and container Codex config
codex-host/        # Desktop install/uninstall helper scripts
```

## System Shape

| Mode | Codex runs in | Hook installation | Default logs | Best for |
| --- | --- | --- | --- | --- |
| Docker container | Docker container | Container-owned `$CODEX_HOME/hooks.json` | `.tally-codex-logs/` | Stronger packaging/control story; demos where the workspace mount is the boundary |
| Desktop host | Local Mac process | User-level `~/.codex/hooks.json` | `~/.tally-codex/logs/` | Codex Desktop UI and normal local Codex CLI workflows |

```mermaid
flowchart LR
    subgraph DockerMode["Mode A: Docker container"]
        Workspace["Mounted workspace"]
        Container["Codex CLI inside Docker"]
        ContainerHooks["Container hooks"]
        ContainerBinary["Rust tally-codex binary"]
        ContainerLogs["Host-mounted .tally-codex-logs"]
        Workspace --> Container --> ContainerHooks --> ContainerBinary --> ContainerLogs
    end

    subgraph HostMode["Mode B: Desktop host"]
        Desktop["Codex Desktop or local CLI"]
        HostHooks["User hooks in ~/.codex/hooks.json"]
        HostBinary["Rust tally-codex binary"]
        HostLogs["Host logs in ~/.tally-codex/logs"]
        Desktop --> HostHooks --> HostBinary --> HostLogs
    end

    ContainerLogs --> Atlas["Atlas Lite / Tally validation"]
    HostLogs --> Atlas
```

## Shared Rust Binary

Build locally:

```bash
cargo build --release --manifest-path codex-audit/Cargo.toml --bin tally-codex
```

Command surface:

```bash
tally-codex wrap [codex-args...]
tally-codex hook SessionStart
tally-codex heartbeat-daemon
tally-codex install-desktop-hooks
tally-codex uninstall-desktop-hooks
```

- `wrap` runs Codex and optionally tees `codex exec` stdout/stderr.
- `hook` reads a Codex hook payload from stdin and writes JSONL, private
  payload, Tally record, and heartbeat outputs.
- `heartbeat-daemon` emits periodic heartbeat records for the current run.
- `install-desktop-hooks` and `uninstall-desktop-hooks` manage user-level
  `~/.codex/hooks.json` entries.

## Docker Container Mode

Build:

```bash
docker compose -f codex-container/compose.yaml build
```

Run interactive Codex:

```bash
docker compose -f codex-container/compose.yaml run --rm codex
```

Run a read-only prompt:

```bash
docker compose -f codex-container/compose.yaml run --rm \
  -e TALLY_RUN_ID=summary-demo \
  codex codex exec --json -s read-only \
  -c approval_policy='"never"' \
  "Summarize this repository in 5 bullets."
```

Run an edit prompt using Docker as the workspace boundary:

```bash
docker compose -f codex-container/compose.yaml run --rm \
  -e TALLY_RUN_ID=edit-demo \
  codex codex exec --json -s danger-full-access \
  -c approval_policy='"never"' \
  "Create hello_from_codex.txt with one sentence."
```

`workspace-write` can fail inside Docker because Codex's inner filesystem
sandbox uses Linux namespace features that are often unavailable inside an
unprivileged container. With this wrapper, the mounted project directory is the
workspace boundary: only mount folders you want Codex to access.

Use this laptop's file-based Codex auth:

```bash
./codex-container/import-host-auth.sh
docker compose -f codex-container/compose.yaml run --rm codex codex login status
```

The auth import script copies the auth cache through stdin and does not print
token contents.

## Desktop Host Mode

Install hooks for Codex Desktop and the local Codex CLI:

```bash
./codex-host/install-host-hooks.sh
```

The installer builds:

```text
~/.tally-codex/bin/tally-codex
```

It backs up any existing `~/.codex/hooks.json`, removes older Tally hook
handlers, and merges in the current `tally-codex hook ...` handlers.

Use Codex Desktop normally after installation. If Codex asks you to review
hooks, trust the Tally hooks.

One-off local CLI smoke test:

```bash
codex --dangerously-bypass-hook-trust exec --json -s read-only \
  -c approval_policy='"never"' \
  "Respond exactly: host-hook-ok"
```

Uninstall:

```bash
./codex-host/uninstall-host-hooks.sh
```

## Logs

Docker mode writes logs under:

```text
.tally-codex-logs/
```

Desktop host mode writes logs under:

```text
~/.tally-codex/logs/
```

Log layout:

```text
jsonl/codex-hooks.jsonl       # hook events observed from Codex
jsonl/hook-heartbeat.jsonl    # immediate and periodic heartbeat events
private/                      # raw hook payloads by run/source
tally/codex-hooks/*.json      # Tally-style session/instruction/action/result records
tally/hook-heartbeat/*.json   # heartbeat records
state/                        # counters and heartbeat daemon state
codex-stdio/                  # stdout/stderr tee for `codex exec`
codex-native/                 # Codex's own diagnostics when enabled by config
```

Useful checks:

```bash
cat ~/.tally-codex/logs/jsonl/codex-hooks.jsonl
cat ~/.tally-codex/logs/jsonl/hook-heartbeat.jsonl
find ~/.tally-codex/logs/tally -type f | sort
```

## Controls

- `TALLY_RUN_ID` sets the run/session grouping key.
- `TALLY_LOG_ROOT` sets the log root.
- `TALLY_WORKSPACE` sets the workspace path used in metadata.
- `TALLY_HOOK_HEARTBEAT_ENABLED=0` disables hook-driven heartbeat records.
- `TALLY_HOOK_HEARTBEAT_SECONDS=60` controls the heartbeat interval.
- `TALLY_HOOK_HEARTBEAT_IDLE_SECONDS=300` stops heartbeat after hook inactivity.
- `TALLY_TEE_CODEX_STDIO=0` disables `codex exec` stdout/stderr teeing.
- `TALLY_BYPASS_HOOK_TRUST=0` stops Docker mode from adding
  `--dangerously-bypass-hook-trust`.
- `TALLY_OVERWRITE_CODEX_HOOKS=0` keeps an existing container
  `$CODEX_HOME/hooks.json`.
- `TALLY_OVERWRITE_CODEX_CONFIG=1` replaces an existing container
  `$CODEX_HOME/config.toml`.

## Security Notes

Docker mode is the stronger packaging story because Codex, the hook config, and
the `tally-codex` runtime are delivered together. The mounted workspace remains
the practical boundary, so mount only folders Codex should access.

Desktop host mode is more ergonomic for local UI use, but the user controls the
host configuration and can modify or remove hooks. Send `tally/` records through
Atlas or another independent anchor quickly if you need tamper evidence beyond
the local machine.
