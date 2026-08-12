#!/usr/bin/env python3
"""Regression tests for Homebrew formula generation and launcher routing."""

from __future__ import annotations

import hashlib
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


ASSETS = (
    "tally-codex-macos-arm64-cli.tar.gz",
    "tally-claude-macos-arm64-cli.tar.gz",
    "tally-codex-macos-x86_64-cli.tar.gz",
    "tally-claude-macos-x86_64-cli.tar.gz",
    "tally-codex-linux-x86_64",
    "tally-claude-linux-x86_64",
)


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tally-homebrew-test-") as directory:
        root = Path(directory)
        dist = root / "dist"
        dist.mkdir()
        for name in ASSETS:
            path = dist / name
            path.write_bytes(f"test artifact: {name}\n".encode("ascii"))
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            (dist / f"{name}.sha256").write_text(
                f"{digest}  {name}\n", encoding="ascii"
            )

        formula = root / "Formula" / "tally.rb"
        subprocess.run(
            [
                sys.executable,
                str(repo / "scripts" / "generate_homebrew_formula.py"),
                "--dist",
                str(dist),
                "--output",
                str(formula),
                "--version",
                "9.8.7",
                "--release-tag",
                "v9.8.6",
            ],
            cwd=repo,
            check=True,
        )
        rendered = formula.read_text(encoding="ascii")
        assert 'version "9.8.7"' in rendered
        assert "/releases/download/v9.8.6/" in rendered
        assert rendered.count('resource "tally-claude"') == 3
        assert "macos-arm64-cli.tar.gz" in rendered
        assert "macos-x86_64-cli.tar.gz" in rendered
        assert "linux-x86_64" in rendered
        for name in ASSETS:
            assert hashlib.sha256((dist / name).read_bytes()).hexdigest() in rendered

        test_launcher(repo / "scripts" / "tally", root)

    print("Homebrew formula tests passed.")


def test_launcher(source: Path, root: Path) -> None:
    bin_dir = root / "bin"
    bin_dir.mkdir()
    launcher = bin_dir / "tally"
    launcher.write_bytes(source.read_bytes())
    launcher.chmod(launcher.stat().st_mode | stat.S_IXUSR)
    for agent in ("codex", "claude"):
        binary = bin_dir / f"tally-{agent}"
        binary.write_text(
            f'#!/bin/sh\nprintf "{agent}:%s\\n" "$*"\n', encoding="ascii"
        )
        binary.chmod(binary.stat().st_mode | stat.S_IXUSR)

    env = os.environ.copy()
    env["PATH"] = f"{bin_dir}{os.pathsep}{os.defpath}"
    detected_client = bin_dir / "codex"
    detected_client.write_text("#!/bin/sh\n", encoding="ascii")
    detected_client.chmod(detected_client.stat().st_mode | stat.S_IXUSR)
    detected = subprocess.run(
        [str(launcher)], env=env, text=True, capture_output=True, check=True
    )
    assert detected.stdout == "codex:gui\n"
    codex = subprocess.run(
        [str(launcher), "codex"], env=env, text=True, capture_output=True, check=True
    )
    assert codex.stdout == "codex:gui\n"
    claude = subprocess.run(
        [str(launcher), "claude", "uninstall"],
        env=env,
        text=True,
        capture_output=True,
        check=True,
    )
    assert claude.stdout == "claude:uninstall\n"


if __name__ == "__main__":
    main()
