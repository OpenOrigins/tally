#!/usr/bin/env python3
"""Create consistently named native release assets and checksums."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import stat
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()

    args.output.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if "windows" in args.label else ""
    source_dir = Path("target") / args.target / "release"
    for name in ("tally-codex", "tally-claude"):
        source = source_dir / f"{name}{suffix}"
        destination = args.output / f"{name}-{args.label}{suffix}"
        shutil.copy2(source, destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR)
        digest = hashlib.sha256(destination.read_bytes()).hexdigest()
        destination.with_name(f"{destination.name}.sha256").write_text(
            f"{digest}  {destination.name}\n", encoding="ascii"
        )


if __name__ == "__main__":
    main()
