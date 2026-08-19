#!/usr/bin/env python3
"""End-user installation and audit-record smoke test for native binaries."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlsplit
from urllib.request import Request, urlopen

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


def tally_handlers(config: dict) -> list[dict]:
    handlers: list[dict] = []
    for groups in config.get("hooks", {}).values():
        for group in groups:
            for hook in group.get("hooks", []):
                command = hook.get("command", "")
                if "tally-" in command and " hook " in command:
                    handlers.append(hook)
    return handlers


def tally_commands(config: dict) -> list[str]:
    return [handler["command"] for handler in tally_handlers(config)]


def command_references_path(command: str, path: Path) -> bool:
    """Compare Windows paths independently of slash style and path casing."""
    normalized_command = command.replace("\\", "/").casefold()
    normalized_path = str(path).replace("\\", "/").casefold()
    return normalized_path in normalized_command


def installed_binary_path(config_path: Path, agent: str, env: dict[str, str]) -> Path:
    name = f"tally-{agent}{'.exe' if os.name == 'nt' else ''}"
    if os.name != "nt":
        return config_path.parent / "tally" / "bin" / name
    normalized_config = str(config_path).replace("/", "\\").lower()
    config_id = hashlib.sha256(normalized_config.encode()).hexdigest()[:12]
    return (
        Path(env["LOCALAPPDATA"])
        / "Programs"
        / "OpenOrigins"
        / "Tally"
        / agent
        / config_id
        / name
    )


def expected_hook_source(binary: Path) -> Path:
    app = next((path for path in binary.parents if path.suffix == ".app"), None)
    if app is None or sys.platform != "darwin":
        return binary
    return app / "Contents" / "Helpers" / "tally-hook"


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

    def set_response_status(self, status: int) -> None:
        with self.lock:
            self.response_status = status

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


def gui_request(
    origin: str,
    token: str,
    path: str,
    body: dict,
    *,
    authorized: bool = True,
    expected_status: int = 200,
) -> tuple[dict, object]:
    headers = {"content-type": "application/json", "origin": origin}
    if authorized:
        headers["x-tally-installer-token"] = token
    request = Request(
        f"{origin}{path}",
        data=json.dumps(body).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        response = urlopen(request, timeout=10)
    except HTTPError as error:
        response = error
    response_body = json.loads(response.read().decode("utf-8"))
    assert response.status == expected_status, response_body
    return response_body, response.headers


def gui_install(
    binary: Path,
    agent: str | list[str],
    env: dict[str, str],
    root: Path,
    api_key: str,
    api_url: str,
    *,
    expect_connected: bool,
    config_path: Path | None = None,
    retry_after_warning: Callable[[], None] | None = None,
) -> dict:
    url_file = root / f"{binary.name}-{time.time_ns()}.gui-url"
    gui_env = env.copy()
    gui_env.update(
        {
            "TALLY_GUI_NO_OPEN": "1",
            "TALLY_GUI_URL_FILE": str(url_file),
        }
    )
    process = subprocess.Popen(
        [str(binary), "gui"],
        text=True,
        env=gui_env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 10
        while not url_file.exists() and time.monotonic() < deadline:
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                raise AssertionError(f"GUI exited before startup\n{stdout}\n{stderr}")
            time.sleep(0.05)
        assert url_file.exists(), "GUI did not publish its local URL"

        split = urlsplit(url_file.read_text(encoding="utf-8"))
        token = parse_qs(split.fragment).get("token", [""])[0]
        origin = f"{split.scheme}://{split.netloc}"
        assert split.hostname == "127.0.0.1"
        assert len(token) == 64
        assert api_key not in url_file.read_text(encoding="utf-8")

        with urlopen(f"{origin}/", timeout=10) as response:
            html = response.read().decode("utf-8")
            assert "Agent API key" in html
            assert "Advanced settings" in html
            assert "Try another key" in html
            assert "Cancel" in html
            assert api_key not in html
            assert "default-src 'self'" in response.headers["content-security-policy"]
            assert response.headers["cache-control"] == "no-store"

        unauthorized, _ = gui_request(
            origin,
            token,
            "/api/status",
            {},
            authorized=False,
            expected_status=403,
        )
        assert not unauthorized["ok"]

        status, _ = gui_request(origin, token, "/api/status", {})
        assert status["defaultApiUrl"] == (
            "https://api.prod.openorigins.com/v1/tally/logs"
        )
        assert "dev2" not in status["defaultApiUrl"]
        agent_ids = [agent] if isinstance(agent, str) else agent
        client_status = {
            client["id"]: client for client in status["clients"] if client["id"] in agent_ids
        }
        assert set(client_status) == set(agent_ids)
        body = {
            "apiKey": api_key,
            "apiUrl": api_url,
            "clients": [
                {
                    "id": agent_id,
                    "configPath": str(
                        config_path
                        if config_path is not None and len(agent_ids) == 1
                        else client_status[agent_id]["configPath"]
                    ),
                }
                for agent_id in agent_ids
            ],
        }
        result, _ = gui_request(origin, token, "/api/install", body)
        assert result["connected"] is expect_connected
        assert api_key not in json.dumps(result)
        assert {client["id"] for client in result["clients"]} == set(agent_ids)
        if config_path is not None and len(agent_ids) == 1:
            assert result["clients"][0]["configPath"] == str(config_path)
        if expect_connected:
            assert result["warning"] is None
        else:
            assert "Mark connected manually" in result["warning"]
            assert process.poll() is None, "GUI closed before the user could retry"

        if retry_after_warning is not None:
            retry_after_warning()
            retry_result, _ = gui_request(origin, token, "/api/install", body)
            assert retry_result["connected"] is True
            assert retry_result["warning"] is None
            result = retry_result
        elif not expect_connected:
            gui_request(origin, token, "/api/shutdown", {})

        stdout, stderr = process.communicate(timeout=10)
        assert process.returncode == 0, f"GUI failed\n{stdout}\n{stderr}"
        assert api_key not in stdout
        assert api_key not in stderr
        return result
    finally:
        if process.poll() is None:
            process.terminate()
            process.communicate(timeout=5)


def smoke(source_binary: Path, agent: str, root: Path) -> None:
    if expected_hook_source(source_binary) != source_binary:
        binary = source_binary
    else:
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
    if os.name == "nt":
        env["LOCALAPPDATA"] = str(home / "AppData" / "Local")
    env.pop("CODEX_HOME", None)
    env.pop("CODEX_HOOKS_PATH", None)
    env.pop("TALLY_CLAUDE_SETTINGS_PATH", None)
    env.pop("TALLY_STATE_DIR", None)
    installed_binary = installed_binary_path(config_path, agent, env)
    legacy_installed_binary = (
        config_path.parent
        / "tally"
        / "bin"
        / f"tally-{agent}{'.exe' if os.name == 'nt' else ''}"
    )
    if os.name == "nt":
        legacy_installed_binary.parent.mkdir(parents=True)
        legacy_installed_binary.write_bytes(b"legacy unsigned hook executable")

    api_key = secrets.token_urlsafe(32)
    server = CaptureServer([config_path, api_key_path, api_config_path, installed_binary])
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    try:
        installed = run(
            binary,
            agent,
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
        assert installed_binary.exists()
        hook_source = expected_hook_source(binary)
        assert hook_source.is_file()
        assert installed_binary.read_bytes() == hook_source.read_bytes()
        if os.name == "nt":
            assert config_path.parent not in installed_binary.parents
            assert "Programs" in installed_binary.parts
            assert not legacy_installed_binary.exists()
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
        commands = tally_commands(config)
        assert all(command_references_path(command, installed_binary) for command in commands), (
            f"hooks do not reference installed binary {installed_binary}: {commands}"
        )
        if os.name == "nt" and agent == "codex":
            assert all(
                handler.get("commandWindows") == handler["command"]
                for handler in tally_handlers(config)
            )

        run(
            binary,
            agent,
            "install",
            f"--api-key={api_key}",
            f"--api-url={server.api_url}",
            env=env,
        )
        config = json.loads(config_path.read_text(encoding="utf-8"))
        assert len(tally_commands(config)) == 10, "reinstall duplicated hook handlers"

        gui_install(
            binary,
            agent,
            env,
            root,
            api_key,
            server.api_url,
            expect_connected=True,
        )
        config = json.loads(config_path.read_text(encoding="utf-8"))
        assert len(tally_commands(config)) == 10, "GUI reinstall duplicated hook handlers"

        custom_config_path = (
            root
            / agent
            / "custom config with spaces"
            / ("hooks.json" if agent == "codex" else "settings.json")
        )
        custom_config_path.parent.mkdir(parents=True)
        custom_config_path.write_text(json.dumps({"hooks": {}}), encoding="utf-8")
        custom_state_dir = custom_config_path.parent / "tally" / "logs" / ".state"
        custom_api_key_path = custom_state_dir / "api_key.txt"
        custom_api_config_path = custom_state_dir / "config.json"
        custom_installed_binary = installed_binary_path(custom_config_path, agent, env)
        server.expected_files = [
            custom_config_path,
            custom_api_key_path,
            custom_api_config_path,
            custom_installed_binary,
        ]
        custom_installed = run(
            binary,
            agent,
            "install",
            "--api-key",
            api_key,
            "--api-url",
            server.api_url,
            "--config-path",
            str(custom_config_path),
            env=env,
        )
        assert str(custom_config_path) in custom_installed.stdout
        assert custom_api_key_path.read_text(encoding="utf-8") == api_key
        assert json.loads(custom_api_config_path.read_text(encoding="utf-8")) == {
            "apiUrl": server.api_url
        }
        if os.name != "nt":
            assert stat.S_IMODE(custom_api_key_path.stat().st_mode) == 0o600
            assert stat.S_IMODE(custom_api_config_path.stat().st_mode) == 0o600
        custom_config = json.loads(custom_config_path.read_text(encoding="utf-8"))
        custom_commands = tally_commands(custom_config)
        assert len(custom_commands) == 10
        assert all(
            command_references_path(command, custom_installed_binary)
            for command in custom_commands
        )
        assert all(
            command_references_path(command, custom_state_dir)
            for command in custom_commands
        )

        custom_gui_config_path = (
            root
            / agent
            / "custom gui config with spaces"
            / ("hooks.json" if agent == "codex" else "settings.json")
        )
        custom_gui_config_path.parent.mkdir(parents=True)
        custom_gui_config_path.write_text(json.dumps({"hooks": {}}), encoding="utf-8")
        custom_gui_state_dir = custom_gui_config_path.parent / "tally" / "logs" / ".state"
        custom_gui_installed_binary = installed_binary_path(
            custom_gui_config_path, agent, env
        )
        server.expected_files = [
            custom_gui_config_path,
            custom_gui_state_dir / "api_key.txt",
            custom_gui_state_dir / "config.json",
            custom_gui_installed_binary,
        ]
        gui_result = gui_install(
            binary,
            agent,
            env,
            root,
            api_key,
            server.api_url,
            expect_connected=True,
            config_path=custom_gui_config_path,
        )
        assert gui_result["clients"][0]["keyPath"] == str(custom_gui_state_dir / "api_key.txt")

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

        custom_session_start = next(
            command
            for command in custom_commands
            if command.endswith("hook SessionStart")
        )
        forwarded_count = len(server.recorded("/v1/tally/logs"))
        run_installed_hook(
            custom_session_start,
            env,
            {"session_id": "native-custom-config-forwarding-session"},
        )
        custom_forwarded = server.wait_for(
            "/v1/tally/logs", count=forwarded_count + 1
        )[-1]
        assert header(custom_forwarded, "x-api-key") == api_key
        assert custom_forwarded["body"]["record_type"] == "SESSION_START"

        heartbeat_env = env.copy()
        heartbeat_env["TALLY_HOOK_HEARTBEAT_ENABLED"] = "1"
        heartbeat_run_id = f"native-heartbeat-volume-{agent}"
        heartbeat_env["TALLY_RUN_ID"] = heartbeat_run_id
        forwarded_count = len(server.recorded("/v1/tally/logs"))
        user_prompt = next(
            command
            for command in tally_commands(config)
            if command.endswith("hook UserPromptSubmit")
        )
        run_installed_hook(
            user_prompt,
            heartbeat_env,
            {
                "session_id": heartbeat_run_id,
                "prompt": "one hook must produce one record",
            },
        )
        server.wait_for("/v1/tally/logs", count=forwarded_count + 1)
        time.sleep(0.5)
        emitted = server.recorded("/v1/tally/logs")[forwarded_count:]
        assert len(emitted) == 1, (
            f"one {agent} hook produced {len(emitted)} forwarded records"
        )
        assert emitted[0]["body"]["record_type"] == "INSTRUCTION_RECEIVED"
        heartbeat_records = log_root / "tally" / "hook-heartbeat"
        assert not list(heartbeat_records.glob("*.json")), (
            f"{agent} hook emitted an immediate heartbeat"
        )

        run(binary, agent, "uninstall", "--config-path", str(custom_config_path), env=env)
        custom_config = json.loads(custom_config_path.read_text(encoding="utf-8"))
        assert not tally_commands(custom_config)
        assert not custom_api_key_path.exists()
        assert not custom_api_config_path.exists()
        assert not custom_installed_binary.exists()

        server.expected_files = [config_path, api_key_path, api_config_path, installed_binary]

        stable_hooks = config_path.read_bytes()
        stable_key = api_key_path.read_bytes()
        stable_api_config = api_config_path.read_bytes()
        server.set_response_status(503)
        gui_install(
            binary,
            agent,
            env,
            root,
            api_key,
            server.api_url,
            expect_connected=False,
            retry_after_warning=lambda: server.set_response_status(204),
        )
        assert config_path.read_bytes() == stable_hooks
        assert api_key_path.read_bytes() == stable_key
        assert api_config_path.read_bytes() == stable_api_config

        server.set_response_status(503)
        failed_handshake = run(
            binary,
            agent,
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
        run(binary, agent, "hook", event, env=offline_env, payload=payloads[event])

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

    run(binary, agent, "uninstall", env=env)
    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert "echo keep" in json.dumps(config)
    assert not tally_commands(config)
    assert list(config_path.parent.glob(f"{config_path.name}.backup-*"))
    assert not api_key_path.exists()
    assert not api_config_path.exists()
    assert not installed_binary.exists()


def smoke_combined_install(source_binary: Path, root: Path) -> None:
    home = root / "combined" / "home"
    codex_config = home / ".codex" / "hooks.json"
    claude_config = home / ".claude" / "settings.json"
    for config in (codex_config, claude_config):
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text(json.dumps({"hooks": {}}), encoding="utf-8")

    env = os.environ.copy()
    env.update({
        "HOME": str(home),
        "USERPROFILE": str(home),
        "TALLY_HOOK_HEARTBEAT_ENABLED": "0",
        "TALLY_FORWARDING_ENABLED": "0",
    })
    if os.name == "nt":
        env["LOCALAPPDATA"] = str(home / "AppData" / "Local")

    codex_state = codex_config.parent / "tally" / "logs" / ".state"
    server = CaptureServer([
        codex_config,
        codex_state / "api_key.txt",
        codex_state / "config.json",
        installed_binary_path(codex_config, "codex", env),
    ])
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        key = secrets.token_urlsafe(32)
        gui_install(
            source_binary,
            ["codex", "claude"],
            env,
            root,
            key,
            server.api_url,
            expect_connected=True,
        )
        handshakes = server.wait_for("/v1/tally/onboarding/client-connected", count=2)
        assert {request["body"]["source"] for request in handshakes[-2:]} == {
            "codex", "claude-code"
        }
        assert len(tally_commands(json.loads(codex_config.read_text(encoding="utf-8")))) == 10
        assert len(tally_commands(json.loads(claude_config.read_text(encoding="utf-8")))) == 10
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def smoke_heartbeat_daemon(binary: Path, root: Path, agent: str) -> None:
    interval_seconds = 1
    daemon_root = root / f"heartbeat-daemon-{agent}"
    home = daemon_root / "home"
    log_root = daemon_root / "logs"
    run_id = f"native-heartbeat-daemon-{agent}"
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "TALLY_LOG_ROOT": str(log_root),
            "TALLY_RUN_ID": run_id,
            "TALLY_FORWARDING_ENABLED": "0",
            "TALLY_HOOK_HEARTBEAT_ENABLED": "1",
            "TALLY_HOOK_HEARTBEAT_SECONDS": str(interval_seconds),
            "TALLY_HOOK_HEARTBEAT_IDLE_SECONDS": "5",
        }
    )
    payload = {"session_id": run_id}
    heartbeat_dir = log_root / "tally" / "hook-heartbeat"
    state_dir = log_root / "state"

    run(binary, agent, "hook", "SessionStart", env=env, payload=payload)
    deadline = time.monotonic() + 5
    while not list(heartbeat_dir.glob("*.json")) and time.monotonic() < deadline:
        time.sleep(0.1)
    heartbeat_paths = list(heartbeat_dir.glob("*.json"))
    assert heartbeat_paths, (
        "heartbeat daemon did not emit after a quiet interval"
    )
    state = json.loads(
        (state_dir / f"hook-heartbeat.{run_id}.json").read_text(
            encoding="utf-8"
        )
    )
    first_heartbeat = min(
        (
            json.loads(path.read_text(encoding="utf-8"))
            for path in heartbeat_paths
        ),
        key=lambda record: record["timestamp"],
    )
    state_time = datetime.fromisoformat(state["updated_at"].replace("Z", "+00:00"))
    heartbeat_time = datetime.fromisoformat(
        first_heartbeat["timestamp"].replace("Z", "+00:00")
    )
    assert (heartbeat_time - state_time).total_seconds() >= interval_seconds, (
        "heartbeat daemon emitted before the first quiet interval"
    )

    run(binary, agent, "hook", "Stop", env=env, payload=payload)
    deadline = time.monotonic() + 5
    while list(state_dir.glob("hook-heartbeat.*.pid")) and time.monotonic() < deadline:
        time.sleep(0.1)
    assert not list(state_dir.glob("hook-heartbeat.*.pid")), (
        "heartbeat daemon did not stop after the Stop hook"
    )
    heartbeat_count = len(list(heartbeat_dir.glob("*.json")))
    time.sleep(1.25)
    assert len(list(heartbeat_dir.glob("*.json"))) == heartbeat_count, (
        "heartbeat daemon emitted after the Stop hook"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tally", type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="tally-native-smoke-") as directory:
        root = Path(directory)
        smoke(args.tally.resolve(), "codex", root)
        smoke(args.tally.resolve(), "claude", root)
        smoke_combined_install(args.tally.resolve(), root)
        smoke_heartbeat_daemon(args.tally.resolve(), root, "codex")
        smoke_heartbeat_daemon(args.tally.resolve(), root, "claude")
    print("Native install smoke tests passed.")


if __name__ == "__main__":
    main()
