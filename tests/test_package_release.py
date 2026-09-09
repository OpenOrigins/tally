#!/usr/bin/env python3
"""Regression tests for the single native installer layout."""

from __future__ import annotations

import os
import plistlib
import shutil
import stat
import subprocess
import sys
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
        (root / "LICENSE").write_text("test license\n", encoding="ascii")
        assets = root / "assets"
        assets.mkdir()
        shutil.copy2(repo / "assets" / "Tally.icns", assets / "Tally.icns")
        release_dir = root / "target" / "test-target" / "release"
        release_dir.mkdir(parents=True)
        binary = release_dir / "tally"
        if sys.platform == "darwin":
            shutil.copyfile("/usr/bin/true", binary)
        else:
            binary.write_bytes(b"fake Tally installer")
        binary.chmod(0o755)

        subprocess.run(
            [sys.executable, str(repo / "scripts" / "package_release.py"),
             "--target", "test-target", "--label", "macos-arm64"],
            cwd=root,
            check=True,
        )

        if sys.platform == "darwin":
            dmg = root / "dist" / "tally-macos-arm64.dmg"
            assert dmg.is_file()
            subprocess.run(["codesign", "--verify", "--strict", "--verbose=2", str(dmg)], check=True)
            mount = root / "mount"
            mount.mkdir()
            subprocess.run(["hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", str(mount), str(dmg)], check=True, capture_output=True)
            try:
                assert (mount / "Applications").is_symlink()
                assert_app_layout(mount / "Tally.app")
            finally:
                subprocess.run(["hdiutil", "detach", str(mount)], check=True)
        else:
            archive_path = root / "dist" / "tally-macos-arm64-app.zip"
            with zipfile.ZipFile(archive_path) as archive:
                executable = "Tally.app/Contents/MacOS/tally"
                helper = "Tally.app/Contents/Helpers/tally-hook"
                icon = "Tally.app/Contents/Resources/Tally.icns"
                assert executable in archive.namelist()
                assert helper in archive.namelist()
                assert archive.read(icon) == (repo / "assets" / "Tally.icns").read_bytes()
                assert stat.S_IMODE(archive.getinfo(executable).external_attr >> 16) == 0o755
                assert stat.S_IMODE(archive.getinfo(helper).external_attr >> 16) == 0o755
                plist = plistlib.loads(archive.read("Tally.app/Contents/Info.plist"))
                assert_plist(plist)

        assert sorted(path.name for path in (root / "dist").iterdir()) == [
            "tally-macos-arm64.dmg" if sys.platform == "darwin" else "tally-macos-arm64-app.zip"
        ]

    print("Release packaging tests passed.")


def assert_app_layout(app: Path) -> None:
    executable = app / "Contents" / "MacOS" / "tally"
    helper = app / "Contents" / "Helpers" / "tally-hook"
    assert executable.is_file() and os.access(executable, os.X_OK)
    assert helper.is_file() and os.access(helper, os.X_OK)
    assert_plist(plistlib.loads((app / "Contents" / "Info.plist").read_bytes()))
    assert (app / "Contents" / "Resources" / "LICENSE").read_bytes() == b"test license\n"
    assert (app / "Contents" / "Resources" / "Tally.icns").is_file()
    assert_finder_uses_custom_icon(app)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)], check=True)
    subprocess.run(["codesign", "--verify", "--strict", "--verbose=2", str(helper)], check=True)
    with tempfile.TemporaryDirectory(prefix="tally-standalone-hook-") as directory:
        installed_hook = Path(directory) / "tally-codex"
        shutil.copy2(helper, installed_hook)
        installed_hook.chmod(0o755)
        subprocess.run(
            ["codesign", "--verify", "--strict", "--verbose=2", str(installed_hook)],
            check=True,
        )
        subprocess.run([str(installed_hook)], check=True)


def assert_finder_uses_custom_icon(app: Path) -> None:
    swift = r'''import AppKit
import UniformTypeIdentifiers

func png(_ image: NSImage) -> Data {
    let size = 256
    let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: size,
        pixelsHigh: size,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    )!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    image.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
    NSGraphicsContext.restoreGraphicsState()
    return bitmap.representation(using: .png, properties: [:])!
}

let appIcon = png(NSWorkspace.shared.icon(forFile: CommandLine.arguments[1]))
let placeholder = png(NSWorkspace.shared.icon(for: .applicationBundle))
precondition(appIcon != placeholder, "Finder resolved the generic application icon")
'''
    subprocess.run(["swift", "-e", swift, str(app)], check=True)


def assert_plist(plist: dict) -> None:
    assert plist["CFBundleExecutable"] == "tally"
    assert plist["CFBundleIdentifier"] == "com.openorigins.tally"
    assert plist["CFBundleDisplayName"] == "Tally"
    assert plist["CFBundleIconFile"] == "Tally.icns"
    assert plist["CFBundleShortVersionString"] == "9.8.7"
    assert plist["LSMinimumSystemVersion"] == "11.0"
    assert plist["LSUIElement"] is True


if __name__ == "__main__":
    main()
