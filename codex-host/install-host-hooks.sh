#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK_BIN="$SCRIPT_DIR/bin/tally-host-hook"
CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
HOOKS_PATH="${CODEX_HOOKS_PATH:-$CODEX_HOME/hooks.json}"
TALLY_LOG_ROOT="${TALLY_LOG_ROOT:-$HOME/.tally-codex/logs}"

mkdir -p "$CODEX_HOME" "$TALLY_LOG_ROOT"
chmod +x "$HOOK_BIN"

export HOOKS_PATH HOOK_BIN TALLY_LOG_ROOT

python3 <<'PY'
from __future__ import annotations

import json
import os
import shlex
import shutil
from datetime import datetime, timezone
from pathlib import Path

hooks_path = Path(os.environ["HOOKS_PATH"]).expanduser()
hook_bin = str(Path(os.environ["HOOK_BIN"]).resolve())
log_root = Path(os.environ["TALLY_LOG_ROOT"]).expanduser()

events = [
    ("SessionStart", "*", "Tally: recording Codex Desktop session start"),
    ("UserPromptSubmit", None, "Tally: recording user prompt"),
    ("PreToolUse", "*", "Tally: recording pre-tool action"),
    ("PermissionRequest", "*", "Tally: recording permission request"),
    ("PostToolUse", "*", "Tally: recording post-tool result"),
    ("PreCompact", "*", "Tally: recording pre-compact"),
    ("PostCompact", "*", "Tally: recording post-compact"),
    ("SubagentStart", "*", "Tally: recording subagent start"),
    ("SubagentStop", "*", "Tally: recording subagent stop"),
    ("Stop", None, "Tally: recording Codex Desktop stop"),
]


def empty_config() -> dict:
    return {"hooks": {}}


def load_config() -> dict:
    if not hooks_path.exists():
        return empty_config()
    try:
        data = json.loads(hooks_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise SystemExit(f"Refusing to modify invalid JSON at {hooks_path}: {exc}") from exc
    if not isinstance(data, dict):
        raise SystemExit(f"Refusing to modify non-object JSON at {hooks_path}")
    hooks = data.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        raise SystemExit(f"Refusing to modify {hooks_path}: top-level hooks is not an object")
    return data


def is_tally_hook(hook: object) -> bool:
    if not isinstance(hook, dict):
        return False
    command = str(hook.get("command", ""))
    return "tally-host-hook" in command


def remove_existing_tally_hooks(config: dict) -> None:
    hooks = config.setdefault("hooks", {})
    for event, groups in list(hooks.items()):
        if not isinstance(groups, list):
            continue
        kept_groups = []
        for group in groups:
            if not isinstance(group, dict):
                kept_groups.append(group)
                continue
            handlers = group.get("hooks")
            if not isinstance(handlers, list):
                kept_groups.append(group)
                continue
            kept_handlers = [handler for handler in handlers if not is_tally_hook(handler)]
            if kept_handlers:
                new_group = dict(group)
                new_group["hooks"] = kept_handlers
                kept_groups.append(new_group)
        if kept_groups:
            hooks[event] = kept_groups
        else:
            hooks.pop(event, None)


def tally_group(event: str, matcher: str | None, status: str) -> dict:
    group = {}
    if matcher is not None:
        group["matcher"] = matcher
    group["hooks"] = [
        {
            "type": "command",
            "command": f"{shlex.quote(hook_bin)} {shlex.quote(event)}",
            "timeout": 15,
            "statusMessage": status,
        }
    ]
    return group


config = load_config()
if hooks_path.exists():
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    backup = hooks_path.with_name(f"{hooks_path.name}.backup-{stamp}")
    shutil.copy2(hooks_path, backup)
else:
    backup = None

remove_existing_tally_hooks(config)
hooks = config.setdefault("hooks", {})
for event, matcher, status in events:
    groups = hooks.setdefault(event, [])
    if not isinstance(groups, list):
        raise SystemExit(f"Refusing to modify {hooks_path}: hooks.{event} is not a list")
    groups.append(tally_group(event, matcher, status))

hooks_path.write_text(json.dumps(config, indent=2, sort_keys=True) + "\n", encoding="utf-8")

print(f"Installed Tally Codex Desktop hooks into {hooks_path}")
if backup:
    print(f"Backed up previous hooks file to {backup}")
print(f"Tally logs will be written under {log_root}")
print("Open Codex Desktop and review/trust the hooks if prompted.")
PY
