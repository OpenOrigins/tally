"""Installer: `python -m anchor_hooks.cli init` scaffolds the directories and
config file a client needs to drop anchor hooks into their own LangGraph
agent. This is the piece that ships to the client as the "installation" step.
"""
from __future__ import annotations

import argparse
from pathlib import Path

CONFIG_TEMPLATE = """\
# Anchor hooks configuration. Values here can be overridden by environment
# variables (ANCHOR_AGENT_ID, ANCHOR_AGENT_VERSION, TALLY_USER_EMAIL,
# TALLY_API_URL, TALLY_API_KEY_FILE).
agent_id: local-agent:CHANGE-ME
agent_version: unknown  # set to your model/build version, or pass AnchorSession(agent_version=...)
principal_id: user:CHANGE-ME@example.com
log_dir: logs
heartbeat_interval_seconds: 60
heartbeat_max_hours: 6
forwarder:
  api_url: https://api.dev2.openorigins.com/v1/tally/logs
  api_key_file: logs/.anchor_state/api_key.txt
  max_hours: 24
"""

INTEGRATION_SNIPPET = """
Integration snippet -- add this around wherever you currently call
`app.invoke(...)` / `app.stream(...)` on your compiled graph:

    from anchor_hooks import AnchorSession

    session = AnchorSession()
    app = graph.compile()

    with session.track(source="startup") as s:
        session.emit_instruction_received(user_input_text)
        result = app.invoke(inputs, config={"callbacks": [s.callback_handler]})
"""


def init(target_dir: str = ".") -> None:
    root = Path(target_dir)
    (root / "logs" / ".state").mkdir(parents=True, exist_ok=True)
    (root / "logs" / ".anchor_state").mkdir(parents=True, exist_ok=True)

    config_path = root / "anchor_config.yaml"
    if not config_path.exists():
        config_path.write_text(CONFIG_TEMPLATE)
        print(f"Wrote {config_path}")
    else:
        print(f"{config_path} already exists, leaving it untouched")

    key_path = root / "logs" / ".anchor_state" / "api_key.txt"
    if not key_path.exists():
        key_path.write_text("")
        print(f"Created empty {key_path} -- replace with the real Tally API key")

    print(INTEGRATION_SNIPPET)


def main():
    parser = argparse.ArgumentParser(prog="anchor-hooks")
    sub = parser.add_subparsers(dest="command", required=True)
    init_parser = sub.add_parser("init", help="Scaffold anchor hooks config/dirs in the current project")
    init_parser.add_argument("--dir", default=".", help="Target project directory (default: current dir)")
    args = parser.parse_args()

    if args.command == "init":
        init(args.dir)


if __name__ == "__main__":
    main()
