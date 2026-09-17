"""``tally-langgraph`` command-line entry point for one-time installation setup."""

from __future__ import annotations

import argparse
import getpass
import os
import re
import sys
import tempfile
import uuid
from pathlib import Path

from dotenv import dotenv_values

from .config import DEFAULT_API_URL, DEFAULT_STATE_DIR, TallyConfig
from .journal import Journal
from .onboarding import OnboardingError, normalize_source, notify_client_connected

_DEFAULT_ENV_FILE = Path(".env")
_DEFAULT_SOURCE = "langgraph"


def _read_env_lines(path: Path) -> list[str]:
    if path.is_symlink():
        raise ValueError(f"refusing to update symlinked environment file: {path}")
    if not path.exists():
        return []
    return path.read_text(encoding="utf-8").splitlines()


def _env_value(path: Path, key: str) -> str | None:
    value = dotenv_values(path).get(key) if path.exists() else None
    return value if isinstance(value, str) else None


def _serialized_env_value(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace("'", "\\'")
    return f"'{escaped}'"


def _upsert_env_file(path: Path, values: dict[str, str]) -> None:
    """Write ``values`` into ``path``, replacing matching keys and preserving everything else."""

    lines = _read_env_lines(path)
    remaining = dict(values)
    updated: list[str] = []
    for line in lines:
        match = re.match(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=", line)
        key = match.group(1) if match else None
        if key in remaining:
            updated.append(f"{key}={_serialized_env_value(remaining.pop(key))}")
        else:
            updated.append(line)
    for key, value in values.items():
        if key in remaining:
            updated.append(f"{key}={_serialized_env_value(value)}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            delete=False,
        ) as temporary:
            temporary_path = Path(temporary.name)
            if os.name == "posix":
                os.fchmod(temporary.fileno(), 0o600)
            temporary.write("\n".join(updated) + "\n")
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary_path, path)
        if os.name == "posix":
            path.chmod(0o600)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)


def _cmd_connect(args: argparse.Namespace) -> int:
    env_path = Path(args.env_file)
    try:
        _read_env_lines(env_path)
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    api_key = (
        args.api_key or os.environ.get("TALLY_API_KEY") or _env_value(env_path, "TALLY_API_KEY")
    )
    if not api_key and sys.stdin.isatty():
        api_key = getpass.getpass("Agent API key: ")
    if not api_key:
        print(
            f"error: no API key provided and none found in {env_path} or TALLY_API_KEY; "
            "run interactively or pass --api-key",
            file=sys.stderr,
        )
        return 1

    api_url = (
        args.api_url
        or os.environ.get("TALLY_API_URL")
        or _env_value(env_path, "TALLY_API_URL")
        or DEFAULT_API_URL
    )
    state_dir = Path(
        args.state_dir
        or os.environ.get("TALLY_STATE_DIR")
        or _env_value(env_path, "TALLY_STATE_DIR")
        or DEFAULT_STATE_DIR
    )

    try:
        resolved = TallyConfig.from_env(
            env_file=env_path,
            api_key=api_key,
            api_url=api_url,
            state_dir=state_dir,
        )
        source = normalize_source(args.source)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if resolved.api_key is None:
        print("error: Agent API key must not be empty", file=sys.stderr)
        return 2

    to_write = {"TALLY_API_KEY": resolved.api_key}
    if args.api_url:
        to_write["TALLY_API_URL"] = resolved.api_url
    try:
        _upsert_env_file(env_path, to_write)
    except (OSError, ValueError) as error:
        print(f"error: could not update {env_path}: {error}", file=sys.stderr)
        return 2
    print(f"Saved Tally configuration to {env_path}")

    journal = Journal(resolved.state_dir, max_record_bytes=resolved.max_record_bytes)
    agent_id = resolved.agent_id or journal.get_or_create_metadata(
        "agent_id", lambda: f"agent:{uuid.uuid4()}"
    )
    print(f"Agent ID: {agent_id}")
    print(f"State directory: {resolved.state_dir}")

    try:
        notify_client_connected(
            api_key=resolved.api_key,
            api_url=resolved.api_url,
            source=source,
        )
    except OnboardingError as error:
        print(
            f"Warning: automatic dashboard connection failed: {error}. Local logging is "
            "installed and will continue offline; log delivery does not depend on this "
            "handshake and will retry through the normal outbox once the server is reachable.",
            file=sys.stderr,
        )
        return 0
    print("OpenOrigins dashboard connection confirmed.")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="tally-langgraph")
    subparsers = parser.add_subparsers(dest="command", required=True)

    connect = subparsers.add_parser(
        "connect",
        help="Save the Agent API key and confirm this installation with the dashboard.",
    )
    connect.add_argument(
        "--api-key",
        help="Agent API key (prefer the interactive prompt or TALLY_API_KEY)",
    )
    connect.add_argument("--api-url", help=f"Ingest URL (default: {DEFAULT_API_URL})")
    connect.add_argument(
        "--source",
        default=_DEFAULT_SOURCE,
        help=f"Client identifier sent with the handshake (default: {_DEFAULT_SOURCE})",
    )
    connect.add_argument(
        "--env-file",
        default=os.environ.get("TALLY_ENV_FILE", str(_DEFAULT_ENV_FILE)),
        help="Path to the .env file to update (default: TALLY_ENV_FILE or .env)",
    )
    connect.add_argument(
        "--state-dir",
        help=f"Durable state directory (default: {DEFAULT_STATE_DIR})",
    )
    connect.set_defaults(func=_cmd_connect)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
