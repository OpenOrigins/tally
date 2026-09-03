"""Configuration loading for anchor hooks.

Precedence: environment variables > anchor_config.yaml > built-in defaults.
"""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

import yaml

DEFAULT_CONFIG_PATH = "anchor_config.yaml"


@dataclass
class ForwarderConfig:
    api_url: str = "https://api.dev2.openorigins.com/v1/tally/logs"
    api_key_file: str = "logs/.anchor_state/api_key.txt"
    max_hours: float = 24


@dataclass
class AnchorConfig:
    agent_id: str = "local-agent:langgraph-agent"
    agent_version: str = "unknown"
    principal_id: str = "user:unknown"
    log_dir: str = "logs"
    schema_version: str = "0.2"
    heartbeat_interval_seconds: float = 60
    heartbeat_max_hours: float = 6
    forwarder: ForwarderConfig = field(default_factory=ForwarderConfig)
    source_path: Optional[str] = None

    @property
    def log_jsonl_path(self) -> Path:
        return Path(self.log_dir) / "log.jsonl"

    @property
    def log_sqlite_path(self) -> Path:
        return Path(self.log_dir) / "log.sqlite"

    @property
    def state_dir(self) -> Path:
        return Path(self.log_dir) / ".state"

    @property
    def anchor_state_dir(self) -> Path:
        return Path(self.log_dir) / ".anchor_state"

    @property
    def forwarder_offset_path(self) -> Path:
        return self.state_dir / "forwarder_offset.txt"

    @property
    def forwarder_pid_path(self) -> Path:
        return self.state_dir / "forwarder.pid"

    def heartbeat_pid_path(self, session_id: str) -> Path:
        return self.state_dir / f"heartbeat_{session_id}.pid"


def load_config(path: Optional[str] = None) -> AnchorConfig:
    resolved_path = path or os.environ.get("ANCHOR_CONFIG_PATH", DEFAULT_CONFIG_PATH)
    data = {}
    config_file = Path(resolved_path)
    if config_file.exists():
        with config_file.open() as f:
            data = yaml.safe_load(f) or {}

    forwarder_data = data.get("forwarder", {}) or {}
    forwarder = ForwarderConfig(
        api_url=os.environ.get("TALLY_API_URL", forwarder_data.get("api_url", ForwarderConfig.api_url)),
        api_key_file=os.environ.get(
            "TALLY_API_KEY_FILE", forwarder_data.get("api_key_file", ForwarderConfig.api_key_file)
        ),
        max_hours=float(forwarder_data.get("max_hours", ForwarderConfig.max_hours)),
    )

    user_email = os.environ.get("TALLY_USER_EMAIL")
    if user_email:
        principal_id = f"user:{user_email}"
    else:
        principal_id = data.get("principal_id", AnchorConfig.principal_id)

    return AnchorConfig(
        agent_id=os.environ.get("ANCHOR_AGENT_ID", data.get("agent_id", AnchorConfig.agent_id)),
        agent_version=os.environ.get("ANCHOR_AGENT_VERSION", data.get("agent_version", AnchorConfig.agent_version)),
        principal_id=principal_id,
        log_dir=data.get("log_dir", AnchorConfig.log_dir),
        schema_version=data.get("schema_version", AnchorConfig.schema_version),
        heartbeat_interval_seconds=float(
            data.get("heartbeat_interval_seconds", AnchorConfig.heartbeat_interval_seconds)
        ),
        heartbeat_max_hours=float(data.get("heartbeat_max_hours", AnchorConfig.heartbeat_max_hours)),
        forwarder=forwarder,
        source_path=str(config_file) if config_file.exists() else None,
    )
