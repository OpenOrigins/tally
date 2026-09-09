#!/usr/bin/env python3
"""Exercise hook installation from the signed app inside a release DMG."""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dmg", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise SystemExit("the macOS DMG smoke test requires macOS")
    dmg = args.dmg.resolve()
    if not dmg.is_file():
        raise SystemExit(f"missing DMG: {dmg}")

    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tally-dmg-smoke-") as directory:
        mount = Path(directory) / "mount"
        mount.mkdir()
        subprocess.run(
            [
                "hdiutil",
                "attach",
                "-readonly",
                "-nobrowse",
                "-mountpoint",
                str(mount),
                str(dmg),
            ],
            check=True,
            capture_output=True,
        )
        try:
            app = mount / "Tally.app"
            helper = app / "Contents" / "Helpers" / "tally-hook"
            subprocess.run(
                ["codesign", "--verify", "--strict", "--verbose=2", str(helper)],
                check=True,
            )
            subprocess.run(
                [
                    sys.executable,
                    str(repo / "tests" / "native_install_smoke.py"),
                    "--tally",
                    str(app / "Contents" / "MacOS" / "tally"),
                ],
                cwd=repo,
                check=True,
            )
        finally:
            subprocess.run(["hdiutil", "detach", str(mount)], check=True)

    print("Packaged macOS hook installation smoke tests passed.")


if __name__ == "__main__":
    main()
