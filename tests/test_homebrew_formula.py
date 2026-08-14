#!/usr/bin/env python3
"""Regression tests for the single-installer Homebrew formula."""

from __future__ import annotations

import hashlib
import subprocess
import sys
import tempfile
from pathlib import Path


ASSETS = (
    "tally-macos-arm64.dmg",
    "tally-macos-x86_64.dmg",
    "tally-linux-x86_64",
)


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tally-homebrew-test-") as directory:
        root = Path(directory)
        dist = root / "dist"
        dist.mkdir()
        for name in ASSETS:
            (dist / name).write_bytes(f"test artifact: {name}\n".encode("ascii"))

        formula = root / "Formula" / "tally.rb"
        subprocess.run(
            [sys.executable, str(repo / "scripts" / "generate_homebrew_formula.py"),
             "--dist", str(dist), "--output", str(formula),
             "--version", "9.8.7", "--release-tag", "v9.8.6"],
            cwd=repo,
            check=True,
        )
        rendered = formula.read_text(encoding="ascii")
        assert 'version "9.8.7"' in rendered
        assert "/releases/download/v9.8.6/" in rendered
        assert "resource" not in rendered
        assert "-cli" not in rendered
        assert "tally-codex" not in rendered
        assert "tally-claude" not in rendered
        assert '"Tally.app/Contents/MacOS/tally"' in rendered
        for name in ASSETS:
            assert hashlib.sha256((dist / name).read_bytes()).hexdigest() in rendered

    print("Homebrew formula tests passed.")


if __name__ == "__main__":
    main()
