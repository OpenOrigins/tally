#!/usr/bin/env python3
"""End-user installation and audit-record smoke test for native binaries."""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
import stat
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

EVENTS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]
EXPECTED_TYPES = [
    "ACTION_TAKEN",
    "INSTRUCTION_RECEIVED",
    "RESULT_RECEIVED",
    "SESSION_END",
    "SESSION_START",
]


def run(
    binary: Path,
    *args: str,
    env: dict[str, str],
    payload: dict | None = None,
    expected_code: int = 0,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        [str(binary), *args],
        input=None if payload is None else json.dumps(payload),
        text=True,
        env=env,
        capture_output=True,
        timeout=30,
    )
    if completed.returncode != expected_code:
        raise AssertionError(
            f"{binary.name} {' '.join(args)} failed ({completed.returncode})\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


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


class CaptureServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, expected_files: list[Path]):
        super().__init__(("127.0.0.1", 0), CaptureHandler)
        self.expected_files = expected_files
        self.requests: list[dict] = []
        self.response_status = 204
        self.lock = threading.Lock()

    @property
    def api_url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}/v1/tally/logs"

    def recorded(self, path: str) -> list[dict]:
        with self.lock:
            return [request for request in self.requests if request["path"] == path]

    def wait_for(self, path: str, count: int = 1, timeout: float = 10) -> list[dict]:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            requests = self.recorded(path)
            if len(requests) >= count:
                return requests
            time.sleep(0.05)
        raise AssertionError(f"timed out waiting for {count} request(s) to {path}")


class CaptureHandler(BaseHTTPRequestHandler):
    server: CaptureServer

    def do_POST(self) -> None:  # noqa: N802 - required by BaseHTTPRequestHandler
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length).decode("utf-8")
        request = {
            "path": self.path,
            "headers": list(self.headers.raw_items()),
            "body": json.loads(body),
            "local_install_complete": all(path.exists() for path in self.server.expected_files)
            and "tally-" in self.server.expected_files[0].read_text(encoding="utf-8"),
        }
        with self.server.lock:
            self.server.requests.append(request)
            status = self.server.response_status
        self.send_response(status)
        self.end_headers()

    def log_message(self, _format: str, *_args: object) -> None:
        pass


def header(request: dict, name: str) -> str | None:
    return next((value for key, value in request["headers"] if key == name), None)


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
    state_dir = config_path.parent / "tally" / "logs" / ".state"
    api_key_path = state_dir / "api_key.txt"
    api_config_path = state_dir / "config.json"
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
    env.pop("TALLY_STATE_DIR", None)

    api_key = secrets.token_urlsafe(32)
    server = CaptureServer([config_path, api_key_path, api_config_path])
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    try:
        installed = run(
            binary,
            "install",
            "--api-key",
            api_key,
            "--api-url",
            server.api_url,
            env=env,
        )
        assert api_key not in installed.stdout
        assert api_key not in installed.stderr
        assert "dashboard connection confirmed" in installed.stdout

        handshakes = server.wait_for("/v1/tally/onboarding/client-connected")
        handshake = handshakes[-1]
        assert handshake["local_install_complete"], "handshake ran before local installation"
        assert header(handshake, "x-api-key") == api_key
        assert handshake["body"] == {
            "source": "codex" if agent == "codex" else "claude-code"
        }

        assert api_key_path.read_text(encoding="utf-8") == api_key
        assert api_key.encode("utf-8") not in binary.read_bytes()
        assert json.loads(api_config_path.read_text(encoding="utf-8")) == {
            "apiUrl": server.api_url
        }
        if os.name != "nt":
            assert stat.S_IMODE(api_key_path.stat().st_mode) == 0o600
            assert stat.S_IMODE(api_config_path.stat().st_mode) == 0o600

        config = json.loads(config_path.read_text(encoding="utf-8"))
        assert config["theme"] == "dark"
        assert "echo keep" in json.dumps(config)
        assert api_key not in json.dumps(config)
        assert len(tally_commands(config)) == 10
        assert all(str(binary) in command for command in tally_commands(config))

        run(
            binary,
            "install",
            f"--api-key={api_key}",
            f"--api-url={server.api_url}",
            env=env,
        )
        config = json.loads(config_path.read_text(encoding="utf-8"))
        assert len(tally_commands(config)) == 10, "reinstall duplicated hook handlers"

        session_start = next(
            command
            for command in tally_commands(config)
            if command.endswith("hook SessionStart")
        )
        run_installed_hook(
            session_start,
            env,
            {"session_id": "native-forwarding-session"},
        )
        forwarded = server.wait_for("/v1/tally/logs")[-1]
        assert header(forwarded, "x-api-key") == api_key
        assert forwarded["body"]["record_type"] == "SESSION_START"

        stable_hooks = config_path.read_bytes()
        stable_key = api_key_path.read_bytes()
        stable_api_config = api_config_path.read_bytes()
        with server.lock:
            server.response_status = 503
        failed_handshake = run(
            binary,
            "install",
            "--api-key",
            api_key,
            "--api-url",
            server.api_url,
            env=env,
        )
        assert "Automatic dashboard connection failed" in failed_handshake.stderr
        assert "Mark connected manually" in failed_handshake.stderr
        assert config_path.read_bytes() == stable_hooks
        assert api_key_path.read_bytes() == stable_key
        assert api_config_path.read_bytes() == stable_api_config
        assert len(tally_commands(json.loads(stable_hooks))) == 10
    finally:
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=5)

    shutil.rmtree(log_root, ignore_errors=True)
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
    offline_env = env.copy()
    offline_env["TALLY_FORWARDING_ENABLED"] = "0"
    run_installed_hook(session_start, offline_env, payloads["SessionStart"])
    for event in EVENTS[1:]:
        run(binary, "hook", event, env=offline_env, payload=payloads[event])

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
    assert not api_key_path.exists()
    assert not api_config_path.exists()


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
