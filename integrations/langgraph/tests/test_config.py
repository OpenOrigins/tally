from pathlib import Path

import pytest

from tally_langgraph.config import DEFAULT_API_URL, TallyConfig


def test_from_env_and_explicit_override(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("TALLY_API_KEY", "  test-key  ")
    monkeypatch.setenv("TALLY_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("TALLY_SERVER_EVIDENCE_ENABLED", "false")
    monkeypatch.setenv("TALLY_AGENT_VERSION", "agent/1")

    config = TallyConfig.from_env(agent_id="agent:explicit")

    assert config.api_url == DEFAULT_API_URL
    assert config.api_key == "test-key"
    assert config.state_dir == tmp_path
    assert config.agent_id == "agent:explicit"
    assert config.agent_version == "agent/1"
    assert config.server_evidence_enabled is False


def test_from_env_loads_file_but_process_environment_wins(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env_file = tmp_path / "custom.env"
    env_file.write_text(
        "TALLY_API_KEY=file-key\n"
        "TALLY_AGENT_VERSION=file-version\n"
        "TALLY_FORWARDING_ENABLED=false\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("TALLY_API_KEY", "process-key")

    config = TallyConfig.from_env(env_file=env_file)

    assert config.api_key == "process-key"
    assert config.agent_version == "file-version"
    assert config.forwarding_enabled is False


def test_default_api_url_is_production() -> None:
    assert DEFAULT_API_URL == "https://api.prod.openorigins.com/v1/tally/logs"


@pytest.mark.parametrize(
    ("kwargs", "message"),
    [
        ({"api_url": "http://example.com/logs"}, "HTTPS"),
        ({"api_url": "not-a-url"}, "absolute URL"),
        ({"api_url": "https://user:password@example.com/logs"}, "credentials"),
        ({"api_key": "invalid\nkey"}, "invalid format"),
        ({"heartbeat_interval_seconds": 599}, "at least 600"),
        ({"server_evidence_max_chars": 128}, "between 256"),
        ({"retry_base_seconds": 2, "retry_max_seconds": 1}, "must not exceed"),
        ({"max_record_bytes": 1_023}, "at least 1024"),
        ({"worker_poll_seconds": 0}, "greater than zero"),
        ({"delivered_retention_days": 0}, "at least one"),
        ({"principal_type": "robot"}, "principal_type"),
    ],
)
def test_invalid_config_is_rejected(kwargs: dict[str, object], message: str) -> None:
    with pytest.raises(ValueError, match=message):
        TallyConfig(**kwargs)  # type: ignore[arg-type]


def test_local_http_is_allowed() -> None:
    assert TallyConfig(api_url="http://127.0.0.1:8080/logs").api_url.startswith("http://")


def test_invalid_environment_value_has_variable_name(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("TALLY_FORWARDING_ENABLED", "sometimes")
    with pytest.raises(ValueError, match="TALLY_FORWARDING_ENABLED"):
        TallyConfig.from_env()


@pytest.mark.parametrize(
    ("name", "value", "message"),
    [
        ("TALLY_MAX_RECORD_BYTES", "large", "integer"),
        ("TALLY_WORKER_POLL_SECONDS", "fast", "number"),
    ],
)
def test_invalid_numeric_environment_values(
    monkeypatch: pytest.MonkeyPatch, name: str, value: str, message: str
) -> None:
    monkeypatch.setenv(name, value)
    with pytest.raises(ValueError, match=message):
        TallyConfig.from_env()
