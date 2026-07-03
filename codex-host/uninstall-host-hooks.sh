#!/usr/bin/env bash
set -Eeuo pipefail

CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
HOOKS_PATH="${CODEX_HOOKS_PATH:-$CODEX_HOME/hooks.json}"

export HOOKS_PATH

python3 <<'PY'
from __future__ import annotations

import json
import os
import shutil
from datetime import datetime, timezone
from pathlib import Path

hooks_path = Path(os.environ["HOOKS_PATH"]).expanduser()
if not hooks_path.exists():
    print(f"No hooks file found at {hooks_path}")
    raise SystemExit(0)

try:
    config = json.loads(hooks_path.read_text(encoding="utf-8"))
except json.JSONDecodeError as exc:
    raise SystemExit(f"Refusing to modify invalid JSON at {hooks_path}: {exc}") from exc

if not isinstance(config, dict) or not isinstance(config.get("hooks"), dict):
    raise SystemExit(f"Refusing to modify {hooks_path}: unexpected hooks file shape")

stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
backup = hooks_path.with_name(f"{hooks_path.name}.backup-{stamp}")
shutil.copy2(hooks_path, backup)


def is_tally_hook(hook: object) -> bool:
    if not isinstance(hook, dict):
        return False
    return "tally-host-hook" in str(hook.get("command", ""))


removed = 0
hooks = config["hooks"]
for event, groups in list(hooks.items()):
    if not isinstance(groups, list):
        continue
    kept_groups = []
    for group in groups:
        if not isinstance(group, dict) or not isinstance(group.get("hooks"), list):
            kept_groups.append(group)
            continue
        kept_handlers = []
        for handler in group["hooks"]:
            if is_tally_hook(handler):
                removed += 1
            else:
                kept_handlers.append(handler)
        if kept_handlers:
            new_group = dict(group)
            new_group["hooks"] = kept_handlers
            kept_groups.append(new_group)
    if kept_groups:
        hooks[event] = kept_groups
    else:
        hooks.pop(event, None)

hooks_path.write_text(json.dumps(config, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(f"Removed {removed} Tally hook handler(s) from {hooks_path}")
print(f"Backed up previous hooks file to {backup}")
PY
