#!/usr/bin/env python3
"""Create consistently named native release assets and checksums."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import stat
import tarfile
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
        if args.label.startswith("macos-"):
            destination = args.output / f"{name}-{args.label}.tar.gz"
            write_macos_archive(source, destination, name)
            write_checksum(destination)
            continue

        destination = args.output / f"{name}-{args.label}{suffix}"
        shutil.copy2(source, destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR)
        write_checksum(destination)


def write_macos_archive(source: Path, archive_path: Path, archive_name: str) -> None:
    executable_mode = source.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    with tarfile.open(str(archive_path), "w:gz") as archive:
        info = archive.gettarinfo(str(source), arcname=archive_name)
        info.mode = executable_mode & 0o777
        with source.open("rb") as handle:
            archive.addfile(info, handle)


def write_checksum(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(f"{path.name}.sha256").write_text(
        f"{digest}  {path.name}\n", encoding="ascii"
    )


if __name__ == "__main__":
    main()
