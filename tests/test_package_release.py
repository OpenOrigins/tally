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
            archive_path = root / "dist" / f"{binary_name}-macos-arm64.tar.gz"
            checksum_path = archive_path.with_name(f"{archive_path.name}.sha256")
            assert archive_path.exists()
            expected_checksum = hashlib.sha256(archive_path.read_bytes()).hexdigest()
            assert checksum_path.read_text(encoding="ascii") == (
                f"{expected_checksum}  {archive_path.name}\n"
            )

            executable = f"{product_name}.app/Contents/MacOS/{binary_name}"
            with tarfile.open(archive_path, "r:gz") as archive:
                names = archive.getnames()
                assert executable in names
                assert f"{product_name}.app/Contents/Info.plist" in names
                link = archive.getmember(binary_name)
                assert link.issym()
                assert link.linkname == executable
                assert stat.S_IMODE(archive.getmember(executable).mode) == 0o755
                plist = plistlib.loads(
                    archive.extractfile(f"{product_name}.app/Contents/Info.plist").read()
                )
                assert plist["CFBundleExecutable"] == binary_name
                assert plist["CFBundleIdentifier"] == f"com.openorigins.tally.{bundle_suffix}"
                assert plist["CFBundleShortVersionString"] == "9.8.7"
                assert plist["LSMinimumSystemVersion"] == "11.0"
                assert plist["LSUIElement"] is True

            extract_dir = root / f"extract-{bundle_suffix}"
            extract_dir.mkdir()
            with tarfile.open(archive_path, "r:gz") as archive:
                archive.extractall(extract_dir, filter="data")
            extracted = extract_dir / binary_name
            assert extracted.is_symlink()
            assert extracted.resolve().is_file()
            if os.name != "nt":
                assert os.access(extracted, os.X_OK)

    print("Release packaging tests passed.")


if __name__ == "__main__":
    main()
