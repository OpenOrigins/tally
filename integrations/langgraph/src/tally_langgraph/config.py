"""Configuration for the Tally LangGraph client.

Secrets are accepted through constructor arguments or the process environment.
The library deliberately does not read or modify ``.env`` files.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

DEFAULT_API_URL = "https://api.dev2.openorigins.com/v1/tally/logs"
DEFAULT_STATE_DIR = Path(".tally") / "langgraph"
MIN_HEARTBEAT_SECONDS = 600


def _env_bool(name: str, default: bool) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise ValueError(f"{name} must be a boolean value")


def _env_int(name: str, default: int) -> int:
    value = os.environ.get(name)
    if value is None:
        return default
    try:
        return int(value)
    except ValueError as error:
        raise ValueError(f"{name} must be an integer") from error


def _env_float(name: str, default: float) -> float:
    value = os.environ.get(name)
    if value is None:
        return default
    try:
        return float(value)
    except ValueError as error:
        raise ValueError(f"{name} must be a number") from error


def _validate_api_url(value: str) -> str:
    parts = urlsplit(value)
    if not parts.scheme or not parts.hostname:
        raise ValueError("Tally API URL must be an absolute URL")
    if parts.username or parts.password:
        raise ValueError("Tally API URL must not contain credentials")
    is_loopback = parts.hostname in {"127.0.0.1", "::1", "localhost"}
    if parts.scheme != "https" and not (parts.scheme == "http" and is_loopback):
        raise ValueError("Tally API URL must use HTTPS (HTTP is allowed only for localhost)")
    return value


@dataclass(frozen=True, slots=True)
class TallyConfig:
    """Immutable runtime configuration.

    ``agent_id`` may be omitted. In that case the client creates a random ID once
    and persists it in the local journal.
    """

    api_url: str = DEFAULT_API_URL
    api_key: str | None = None
    state_dir: Path = DEFAULT_STATE_DIR
    agent_id: str | None = None
    agent_version: str = "unknown"
    principal_id: str | None = None
    principal_type: str | None = None
    forwarding_enabled: bool = True
    server_evidence_enabled: bool = True
    server_evidence_max_chars: int = 8_192
    max_record_bytes: int = 16 * 1024 * 1024
    heartbeat_interval_seconds: int = MIN_HEARTBEAT_SECONDS
    worker_poll_seconds: float = 1.0
    request_timeout_seconds: float = 5.0
    retry_base_seconds: float = 0.5
    retry_max_seconds: float = 30.0
    claim_lease_seconds: float = 30.0
    delivered_retention_days: int = 30

    def __post_init__(self) -> None:
        object.__setattr__(self, "api_url", _validate_api_url(self.api_url))
        object.__setattr__(self, "state_dir", Path(self.state_dir))
        if self.api_key is not None:
            normalized_key = self.api_key.strip()
            object.__setattr__(self, "api_key", normalized_key or None)
        if self.heartbeat_interval_seconds < MIN_HEARTBEAT_SECONDS:
            raise ValueError(f"heartbeat_interval_seconds must be at least {MIN_HEARTBEAT_SECONDS}")
        if not 256 <= self.server_evidence_max_chars <= 32_768:
            raise ValueError("server_evidence_max_chars must be between 256 and 32768")
        if self.max_record_bytes < 1_024:
            raise ValueError("max_record_bytes must be at least 1024")
        for name in (
            "worker_poll_seconds",
            "request_timeout_seconds",
            "retry_base_seconds",
            "retry_max_seconds",
            "claim_lease_seconds",
        ):
            if getattr(self, name) <= 0:
                raise ValueError(f"{name} must be greater than zero")
        if self.retry_base_seconds > self.retry_max_seconds:
            raise ValueError("retry_base_seconds must not exceed retry_max_seconds")
        if self.delivered_retention_days < 1:
            raise ValueError("delivered_retention_days must be at least one")
        if self.principal_type not in {None, "human", "organisation", "agent"}:
            raise ValueError("principal_type must be human, organisation, agent, or None")

    @classmethod
    def from_env(cls, **overrides: object) -> TallyConfig:
        """Build configuration from ``TALLY_*`` variables plus explicit overrides."""

        values: dict[str, object] = {
            "api_url": os.environ.get("TALLY_API_URL", DEFAULT_API_URL),
            "api_key": os.environ.get("TALLY_API_KEY"),
            "state_dir": Path(os.environ.get("TALLY_STATE_DIR", str(DEFAULT_STATE_DIR))),
            "agent_id": os.environ.get("TALLY_AGENT_ID"),
            "agent_version": os.environ.get("TALLY_AGENT_VERSION", "unknown"),
            "principal_id": os.environ.get("TALLY_PRINCIPAL_ID"),
            "principal_type": os.environ.get("TALLY_PRINCIPAL_TYPE"),
            "forwarding_enabled": _env_bool("TALLY_FORWARDING_ENABLED", True),
            "server_evidence_enabled": _env_bool("TALLY_SERVER_EVIDENCE_ENABLED", True),
            "server_evidence_max_chars": _env_int("TALLY_SERVER_EVIDENCE_MAX_CHARS", 8_192),
            "max_record_bytes": _env_int("TALLY_MAX_RECORD_BYTES", 16 * 1024 * 1024),
            "heartbeat_interval_seconds": _env_int(
                "TALLY_HEARTBEAT_SECONDS", MIN_HEARTBEAT_SECONDS
            ),
            "worker_poll_seconds": _env_float("TALLY_WORKER_POLL_SECONDS", 1.0),
            "request_timeout_seconds": _env_float("TALLY_REQUEST_TIMEOUT_SECONDS", 5.0),
            "retry_base_seconds": _env_float("TALLY_RETRY_BASE_SECONDS", 0.5),
            "retry_max_seconds": _env_float("TALLY_RETRY_MAX_SECONDS", 30.0),
            "claim_lease_seconds": _env_float("TALLY_CLAIM_LEASE_SECONDS", 30.0),
            "delivered_retention_days": _env_int("TALLY_DELIVERED_RETENTION_DAYS", 30),
        }
        values.update(overrides)
        return cls(**values)  # type: ignore[arg-type]
