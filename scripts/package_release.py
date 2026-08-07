#!/usr/bin/env python3
"""Create consistently named native release assets and checksums."""

from __future__ import annotations

import argparse
import hashlib
import io
import plistlib
import shutil
import stat
import tarfile
import tomllib
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
    version = workspace_version()
    for name in ("tally-codex", "tally-claude"):
        source = source_dir / f"{name}{suffix}"
        if args.label.startswith("macos-"):
            destination = args.output / f"{name}-{args.label}.tar.gz"
            write_macos_archive(source, destination, name, version, args.label)
            write_checksum(destination)
            continue

        destination = args.output / f"{name}-{args.label}{suffix}"
        shutil.copy2(source, destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR)
        write_checksum(destination)


def workspace_version() -> str:
    with Path("Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def write_macos_archive(
    source: Path,
    archive_path: Path,
    binary_name: str,
    version: str,
    label: str,
) -> None:
    product_name = "Tally Codex" if binary_name == "tally-codex" else "Tally Claude Code"
    app_name = f"{product_name}.app"
    executable_path = f"{app_name}/Contents/MacOS/{binary_name}"
    bundle_id = f"com.openorigins.tally.{'codex' if binary_name == 'tally-codex' else 'claude'}"
    minimum_version = "11.0" if label == "macos-arm64" else "10.15"
    plist = plistlib.dumps(
        {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleDisplayName": product_name,
            "CFBundleExecutable": binary_name,
            "CFBundleIdentifier": bundle_id,
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": product_name,
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "LSMinimumSystemVersion": minimum_version,
            "LSUIElement": True,
            "NSHighResolutionCapable": True,
        },
        fmt=plistlib.FMT_XML,
        sort_keys=True,
    )
    executable_mode = source.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    with tarfile.open(str(archive_path), "w:gz") as archive:
        for directory in (app_name, f"{app_name}/Contents", f"{app_name}/Contents/MacOS"):
            info = tarfile.TarInfo(directory)
            info.type = tarfile.DIRTYPE
            info.mode = 0o755
            archive.addfile(info)

        info = tarfile.TarInfo(f"{app_name}/Contents/Info.plist")
        info.mode = 0o644
        info.size = len(plist)
        archive.addfile(info, io.BytesIO(plist))

        info = archive.gettarinfo(str(source), arcname=executable_path)
        info.mode = executable_mode & 0o777
        with source.open("rb") as handle:
            archive.addfile(info, handle)

        link = tarfile.TarInfo(binary_name)
        link.type = tarfile.SYMTYPE
        link.mode = 0o755
        link.linkname = executable_path
        archive.addfile(link)


def write_checksum(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(f"{path.name}.sha256").write_bytes(
        f"{digest}  {path.name}\n".encode("ascii")
    )


if __name__ == "__main__":
    main()
