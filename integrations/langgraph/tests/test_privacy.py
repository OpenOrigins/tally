import json
import subprocess
from pathlib import Path

from tally_langgraph import TallyClient, TallyConfig, privacy
from tally_langgraph.transport import HttpTransport


def test_blocks_known_path_and_new_github_owner(monkeypatch, tmp_path: Path) -> None:
    blocked = tmp_path / "blocked"
    blocked.mkdir()
    future = tmp_path / "future"
    future.mkdir()
    allowed = tmp_path / "allowed"
    allowed.mkdir()
    subprocess.run(["git", "init", "-q", str(future)], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(future),
            "remote",
            "add",
            "origin",
            "git@github.com:Metal-Minds-LLC/new.git",
        ],
        check=True,
    )
    subprocess.run(["git", "init", "-q", str(allowed)], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(allowed),
            "remote",
            "add",
            "origin",
            "https://github.com/OpenOrigins/tally.git",
        ],
        check=True,
    )
    policy = tmp_path / "privacy.json"
    policy.write_text(
        json.dumps(
            {
                "excluded_git_owners": ["Metal-Minds-LLC"],
                "excluded_paths": [str(blocked)],
            }
        )
    )
    monkeypatch.setattr(privacy, "POLICY_PATH", policy)

    assert privacy.capture_blocked(blocked)
    assert privacy.capture_blocked(future)
    assert privacy.delivery_blocked({"workspace": str(future)})
    assert privacy.delivery_blocked({})
    assert not privacy.capture_blocked(allowed)
    assert privacy.capture_blocked(tmp_path)


def test_blocked_workspace_never_reaches_transport(monkeypatch, tmp_path: Path) -> None:
    blocked = tmp_path / "blocked"
    blocked.mkdir()
    policy = tmp_path / "privacy.json"
    policy.write_text(json.dumps({"excluded_paths": [str(blocked)]}))
    monkeypatch.setattr(privacy, "POLICY_PATH", policy)
    monkeypatch.chdir(blocked)
    client = TallyClient(TallyConfig(state_dir=tmp_path / "state"), background=False)
    client.start_session("session-1", source="test")
    assert client.journal.pending_count() == 0
    result = HttpTransport(client.config).deliver("test", {"workspace": str(blocked)})
    assert result.disposition == "dead_letter"


def test_unclassifiable_remote_is_blocked(monkeypatch, tmp_path: Path) -> None:
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    subprocess.run(
        ["git", "-C", str(tmp_path), "remote", "add", "origin", "https://example.com/team/repo"],
        check=True,
    )
    policy = tmp_path / "privacy.json"
    policy.write_text(json.dumps({"excluded_git_owners": ["Metal-Minds-LLC"]}))
    monkeypatch.setattr(privacy, "POLICY_PATH", policy)
    assert privacy.capture_blocked(tmp_path)
