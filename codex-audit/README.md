# Tally Codex Audit Binary

This crate builds the shared `tally-codex` Rust binary used by both deployment
modes.

## Commands

```bash
tally-codex wrap [codex-args...]
tally-codex hook SessionStart
tally-codex heartbeat-daemon
tally-codex install-desktop-hooks
tally-codex uninstall-desktop-hooks
```

- `wrap` runs the Codex CLI and optionally tees `codex exec` stdout/stderr.
- `hook` reads a Codex hook payload from stdin and writes JSONL, private payload,
  Tally record, and heartbeat outputs.
- `heartbeat-daemon` emits periodic heartbeat records for the current run.
- `install-desktop-hooks` and `uninstall-desktop-hooks` manage user-level
  `~/.codex/hooks.json` entries for Codex Desktop and local CLI use.
