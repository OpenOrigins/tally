#!/usr/bin/env python3
"""Regression tests for the single-installer Homebrew cask."""

from __future__ import annotations

import hashlib
import subprocess
import sys
import tempfile
from pathlib import Path


ASSETS = (
    "tally-macos-arm64.dmg",
    "tally-macos-x86_64.dmg",
)


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tally-homebrew-test-") as directory:
        root = Path(directory)
        dist = root / "dist"
        dist.mkdir()
        for name in ASSETS:
            (dist / name).write_bytes(f"test artifact: {name}\n".encode("ascii"))

        cask = root / "Casks" / "tally.rb"
        subprocess.run(
            [sys.executable, str(repo / "scripts" / "generate_homebrew_cask.py"),
             "--dist", str(dist), "--output", str(cask),
             "--version", "9.8.7", "--release-tag", "v9.8.6"],
            cwd=repo,
            check=True,
        )
        rendered = cask.read_text(encoding="ascii")
        assert 'version "9.8.7"' in rendered
        assert "/releases/download/v9.8.6/" in rendered
        assert 'cask "tally" do' in rendered
        assert 'arch arm: "arm64", intel: "x86_64"' in rendered
        assert "depends_on :macos" in rendered
        assert 'app "Tally.app"' in rendered
        assert "resource" not in rendered
        assert "-cli" not in rendered
        assert "tally-codex" not in rendered
        assert "tally-claude" not in rendered
        assert "tally-linux" not in rendered
        for name in ASSETS:
            assert hashlib.sha256((dist / name).read_bytes()).hexdigest() in rendered

    print("Homebrew cask tests passed.")


if __name__ == "__main__":
    main()
