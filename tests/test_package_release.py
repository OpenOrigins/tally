#!/usr/bin/env python3
"""Regression tests for native release asset layout."""

from __future__ import annotations

import hashlib
import os
import plistlib
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tally-package-test-") as directory:
        root = Path(directory)
        (root / "Cargo.toml").write_text(
            '[workspace]\n[workspace.package]\nversion = "9.8.7"\n', encoding="ascii"
        )
        release_dir = root / "target" / "test-target" / "release"
        release_dir.mkdir(parents=True)
        for name in ("tally-codex", "tally-claude"):
            binary = release_dir / name
            binary.write_bytes(f"fake executable: {name}".encode("ascii"))
            binary.chmod(0o755)

        subprocess.run(
            [
                sys.executable,
                str(repo / "scripts" / "package_release.py"),
                "--target",
                "test-target",
                "--label",
                "macos-arm64",
            ],
            cwd=root,
            check=True,
        )

        for binary_name, product_name, bundle_suffix in (
            ("tally-codex", "Tally Codex", "codex"),
            ("tally-claude", "Tally Claude Code", "claude"),
        ):
            app_name = f"{product_name}.app"
            executable = f"{product_name}.app/Contents/MacOS/{binary_name}"
            app_archive = root / "dist" / f"{binary_name}-macos-arm64-app.zip"
            assert_checksum(app_archive)
            with zipfile.ZipFile(app_archive) as archive:
                names = archive.namelist()
                assert executable in names
                assert f"{app_name}/Contents/Info.plist" in names
                assert {name.split("/", 1)[0] for name in names} == {app_name}
                mode = archive.getinfo(executable).external_attr >> 16
                assert stat.S_IMODE(mode) == 0o755
                plist = plistlib.loads(archive.read(f"{app_name}/Contents/Info.plist"))
                assert plist["CFBundleExecutable"] == binary_name
                assert plist["CFBundleIdentifier"] == f"com.openorigins.tally.{bundle_suffix}"
                assert plist["CFBundleShortVersionString"] == "9.8.7"
                assert plist["LSMinimumSystemVersion"] == "11.0"
                assert plist["LSUIElement"] is True

            cli_archive = root / "dist" / f"{binary_name}-macos-arm64-cli.tar.gz"
            assert_checksum(cli_archive)
            with tarfile.open(cli_archive, "r:gz") as archive:
                assert archive.getnames() == [binary_name]
                assert stat.S_IMODE(archive.getmember(binary_name).mode) == 0o755

            if sys.platform == "darwin":
                extract_dir = root / f"extract-{bundle_suffix}"
                subprocess.run(
                    ["ditto", "-x", "-k", str(app_archive), str(extract_dir)],
                    check=True,
                )
                extracted_app = extract_dir / app_name
                subprocess.run(
                    [
                        "codesign",
                        "--verify",
                        "--deep",
                        "--strict",
                        "--verbose=2",
                        str(extracted_app),
                    ],
                    check=True,
                )
                assert os.access(
                    extracted_app / "Contents" / "MacOS" / binary_name, os.X_OK
                )

    print("Release packaging tests passed.")


def assert_checksum(path: Path) -> None:
    checksum_path = path.with_name(f"{path.name}.sha256")
    assert path.exists()
    expected_checksum = hashlib.sha256(path.read_bytes()).hexdigest()
    assert checksum_path.read_bytes() == (
        f"{expected_checksum}  {path.name}\n".encode("ascii")
    )


if __name__ == "__main__":
    main()
