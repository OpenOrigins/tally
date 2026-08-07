#!/usr/bin/env python3
"""Create consistently named native release assets and checksums."""

from __future__ import annotations

import argparse
import hashlib
import plistlib
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
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
            with tempfile.TemporaryDirectory(prefix=f"{name}-app-") as directory:
                app = build_macos_app(
                    source, Path(directory), name, version, args.label
                )
                app_archive = args.output / f"{name}-{args.label}-app.zip"
                write_app_zip(app, app_archive)
            cli_archive = args.output / f"{name}-{args.label}-cli.tar.gz"
            write_cli_archive(source, cli_archive, name)
            write_checksum(app_archive)
            write_checksum(cli_archive)
            continue

        destination = args.output / f"{name}-{args.label}{suffix}"
        shutil.copy2(source, destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR)
        write_checksum(destination)


def workspace_version() -> str:
    with Path("Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def build_macos_app(
    source: Path,
    directory: Path,
    binary_name: str,
    version: str,
    label: str,
) -> Path:
    product_name = "Tally Codex" if binary_name == "tally-codex" else "Tally Claude Code"
    app_name = f"{product_name}.app"
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
    app = directory / app_name
    contents = app / "Contents"
    macos = contents / "MacOS"
    macos.mkdir(parents=True)
    contents.chmod(0o755)
    macos.chmod(0o755)
    (contents / "Info.plist").write_bytes(plist)
    executable = macos / binary_name
    shutil.copy2(source, executable)
    executable.chmod(0o755)

    if sys.platform == "darwin":
        subprocess.run(
            ["codesign", "--force", "--sign", "-", "--timestamp=none", str(app)],
            check=True,
        )
        subprocess.run(
            ["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)],
            check=True,
        )
    return app


def write_app_zip(app: Path, archive_path: Path) -> None:
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in (app, *sorted(app.rglob("*"))):
            archive.write(path, path.relative_to(app.parent))


def write_cli_archive(source: Path, archive_path: Path, binary_name: str) -> None:
    with tarfile.open(archive_path, "w:gz") as archive:
        info = archive.gettarinfo(str(source), arcname=binary_name)
        info.mode = 0o755
        with source.open("rb") as handle:
            archive.addfile(info, handle)


def write_checksum(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(f"{path.name}.sha256").write_bytes(
        f"{digest}  {path.name}\n".encode("ascii")
    )


if __name__ == "__main__":
    main()
