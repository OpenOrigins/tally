#!/usr/bin/env python3
"""End-user installation and audit-record smoke test for native binaries."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path

EVENTS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]
EXPECTED_TYPES = [
    "ACTION_TAKEN",
    "INSTRUCTION_RECEIVED",
    "RESULT_RECEIVED",
    "SESSION_END",
    "SESSION_START",
]


def run(binary: Path, *args: str, env: dict[str, str], payload: dict | None = None) -> None:
    completed = subprocess.run(
        [str(binary), *args],
        input=None if payload is None else json.dumps(payload),
        text=True,
        env=env,
        capture_output=True,
        timeout=30,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"{binary.name} {' '.join(args)} failed ({completed.returncode})\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


def run_installed_hook(command: str, env: dict[str, str], payload: dict) -> None:
    completed = subprocess.run(
        command,
        input=json.dumps(payload),
        text=True,
        env=env,
        shell=True,
        capture_output=True,
        timeout=30,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"installed hook failed ({completed.returncode}): {command}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


def tally_commands(config: dict) -> list[str]:
    commands: list[str] = []
    for groups in config.get("hooks", {}).values():
        for group in groups:
            for hook in group.get("hooks", []):
                command = hook.get("command", "")
                if "tally-" in command and " hook " in command:
                    commands.append(command)
    return commands


def smoke(source_binary: Path, agent: str, root: Path) -> None:
    binary_dir = root / f"{agent} binary with spaces"
    binary_dir.mkdir(parents=True)
    binary = binary_dir / source_binary.name
    shutil.copy2(source_binary, binary)
    binary.chmod(binary.stat().st_mode | stat.S_IXUSR)

    home = root / agent / "home with spaces"
    config_path = home / (".codex/hooks.json" if agent == "codex" else ".claude/settings.json")
    config_path.parent.mkdir(parents=True)
    config_path.write_text(
        json.dumps(
            {
                "theme": "dark",
                "hooks": {
                    "SessionStart": [
                        {"hooks": [{"type": "command", "command": "echo keep"}]}
                    ]
                },
            }
        ),
        encoding="utf-8",
    )
    log_root = root / agent / "logs"
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "TALLY_HOOK_HEARTBEAT_ENABLED": "0",
            "TALLY_LOG_ROOT": str(log_root),
            "TALLY_RUN_ID": f"native-{agent}-smoke",
            "TALLY_WORKSPACE": str(root),
        }
    )
    env.pop("CODEX_HOME", None)
    env.pop("CODEX_HOOKS_PATH", None)
    env.pop("TALLY_CLAUDE_SETTINGS_PATH", None)

    run(binary, env=env)
    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert config["theme"] == "dark"
    assert "echo keep" in json.dumps(config)
    assert len(tally_commands(config)) == 10
    assert all(str(binary) in command for command in tally_commands(config))

    run(binary, "install", env=env)
    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert len(tally_commands(config)) == 10, "reinstall duplicated hook handlers"

    payloads = {
        "SessionStart": {"session_id": "native-smoke-session"},
        "UserPromptSubmit": {
            "session_id": "native-smoke-session",
            "prompt": "test prompt",
        },
        "PreToolUse": {
            "session_id": "native-smoke-session",
            "tool_call_id": "tool-1",
            "tool_name": "Shell",
            "tool_input": {"command": "true"},
        },
        "PostToolUse": {
            "session_id": "native-smoke-session",
            "tool_call_id": "tool-1",
            "tool_response": {"stdout": ""},
        },
        "Stop": {"session_id": "native-smoke-session"},
    }
    session_start = next(
        command for command in tally_commands(config) if command.endswith("hook SessionStart")
    )
    run_installed_hook(session_start, env, payloads["SessionStart"])
    for event in EVENTS[1:]:
        run(binary, "hook", event, env=env, payload=payloads[event])

    record_dir = log_root / "tally" / f"{agent}-hooks"
    records = [json.loads(path.read_text(encoding="utf-8")) for path in record_dir.glob("*.json")]
    assert sorted(record["record_type"] for record in records) == EXPECTED_TYPES
    assert all(record["schema_version"] == "0.2" for record in records)
    action_ids = {
        record["record_type"]: record.get("action_id")
        for record in records
        if record["record_type"] in {"ACTION_TAKEN", "RESULT_RECEIVED"}
    }
    assert action_ids["ACTION_TAKEN"] == action_ids["RESULT_RECEIVED"]

    run(binary, "uninstall", env=env)
    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert "echo keep" in json.dumps(config)
    assert not tally_commands(config)
    assert list(config_path.parent.glob(f"{config_path.name}.backup-*"))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex", type=Path, required=True)
    parser.add_argument("--claude", type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="tally-native-smoke-") as directory:
        root = Path(directory)
        smoke(args.codex.resolve(), "codex", root)
        smoke(args.claude.resolve(), "claude", root)
    print("Native install smoke tests passed.")


if __name__ == "__main__":
    main()
