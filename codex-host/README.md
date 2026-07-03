# Tally Codex Desktop Hooks

This installs Tally hooks for Codex running directly on the Mac, including the
Codex Desktop app. It does not run Codex inside Docker. Instead, it adds
user-level hooks to `~/.codex/hooks.json`.

Codex Desktop uses the same local Codex configuration as the CLI and IDE
extension. User-level hooks load independently of project trust.

## Install

```bash
cd /Users/ekaterinapavlova/Desktop/tally-codex-wrapper-pr
./codex-host/install-host-hooks.sh
```

By default, logs go to:

```text
~/.tally-codex/logs/
```

The installer backs up any existing `~/.codex/hooks.json` before editing it and
merges Tally hooks with any existing hooks. It removes older Tally host hooks
before adding the current ones, so it is safe to rerun.

## Use With Codex Desktop

1. Open the Codex Desktop app.
2. Select a local project.
3. If Codex asks you to review hooks, trust the Tally hooks.
4. Run a prompt normally.

Then inspect logs:

```bash
cat ~/.tally-codex/logs/jsonl/codex-hooks.jsonl
cat ~/.tally-codex/logs/jsonl/hook-heartbeat.jsonl
find ~/.tally-codex/logs/tally -type f | sort
```

## Use With Local Codex CLI

For a one-off CLI test that bypasses hook trust review:

```bash
codex --dangerously-bypass-hook-trust exec --json -s read-only \
  -c approval_policy='"never"' \
  "Respond exactly: host-hook-ok"
```

## Configure

Set these before launching Codex if you want different defaults:

```bash
export TALLY_LOG_ROOT="$HOME/.tally-codex/logs"
export TALLY_HOOK_HEARTBEAT_SECONDS=60
export TALLY_HOOK_HEARTBEAT_IDLE_SECONDS=300
```

## Uninstall

```bash
./codex-host/uninstall-host-hooks.sh
```

The uninstaller removes only hook handlers whose command contains
`tally-host-hook`. It also backs up `~/.codex/hooks.json` before editing.
