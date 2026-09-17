"""``tally-langgraph`` command-line entry point for one-time installation setup."""

from __future__ import annotations

import argparse
import os
import sys
import uuid
from pathlib import Path

from .config import DEFAULT_API_URL, DEFAULT_STATE_DIR
from .journal import Journal
from .onboarding import OnboardingError, notify_client_connected

_DEFAULT_ENV_FILE = Path(".env")
_DEFAULT_SOURCE = "langgraph"


def _read_env_lines(path: Path) -> list[str]:
    if not path.exists():
        return []
    return path.read_text(encoding="utf-8").splitlines()


def _env_value(lines: list[str], key: str) -> str | None:
    prefix = f"{key}="
    for line in lines:
        if line.startswith(prefix):
            return line[len(prefix) :]
    return None


def _upsert_env_file(path: Path, values: dict[str, str]) -> None:
    """Write ``values`` into ``path``, replacing matching keys and preserving everything else."""

    lines = _read_env_lines(path)
    remaining = dict(values)
    updated: list[str] = []
    for line in lines:
        key = line.split("=", 1)[0] if "=" in line else None
        if key in remaining:
            updated.append(f"{key}={remaining.pop(key)}")
        else:
            updated.append(line)
    for key, value in values.items():
        if key in remaining:
            updated.append(f"{key}={value}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(updated) + "\n", encoding="utf-8")
    if os.name == "posix":
        path.chmod(0o600)


def _cmd_connect(args: argparse.Namespace) -> int:
    env_path = Path(args.env_file)
    existing_lines = _read_env_lines(env_path)

    api_key = (
        args.api_key
        or _env_value(existing_lines, "TALLY_API_KEY")
        or os.environ.get("TALLY_API_KEY")
    )
    if not api_key:
        print(
            f"error: no API key provided and none found in {env_path} or TALLY_API_KEY; "
            "pass --api-key",
            file=sys.stderr,
        )
        return 1

    api_url = (
        args.api_url
        or _env_value(existing_lines, "TALLY_API_URL")
        or os.environ.get("TALLY_API_URL")
        or DEFAULT_API_URL
    )

    to_write = {"TALLY_API_KEY": api_key}
    if args.api_url:
        to_write["TALLY_API_URL"] = api_url
    _upsert_env_file(env_path, to_write)
    print(f"Saved TALLY_API_KEY to {env_path}")

    state_dir = Path(args.state_dir or os.environ.get("TALLY_STATE_DIR") or DEFAULT_STATE_DIR)
    journal = Journal(state_dir, max_record_bytes=16 * 1024 * 1024)
    agent_id = journal.get_or_create_metadata("agent_id", lambda: f"agent:{uuid.uuid4()}")
    print(f"Agent ID: {agent_id}")
    print(f"State directory: {state_dir}")

    try:
        notify_client_connected(api_key=api_key, api_url=api_url, source=args.source)
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
    connect.add_argument("--api-key", help="Agent API key issued by the OpenOrigins dashboard")
    connect.add_argument("--api-url", help=f"Ingest URL (default: {DEFAULT_API_URL})")
    connect.add_argument(
        "--source",
        default=_DEFAULT_SOURCE,
        help=f"Client identifier sent with the handshake (default: {_DEFAULT_SOURCE})",
    )
    connect.add_argument(
        "--env-file",
        default=str(_DEFAULT_ENV_FILE),
        help="Path to the .env file to update (default: .env)",
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
