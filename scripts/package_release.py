#!/usr/bin/env python3
"""Create consistently named native release assets and checksums."""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import zipfile
from pathlib import Path


MACOS_PRODUCTS = {
    "tally-codex": {
        "display_name": "Tally Codex Installer",
        "bundle_id": "com.openorigins.tally.codex.installer",
    },
    "tally-claude": {
        "display_name": "Tally Claude Installer",
        "bundle_id": "com.openorigins.tally.claude.installer",
    },
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    parser.add_argument(
        "--require-macos-notarization",
        action="store_true",
        help="fail macOS packaging unless Developer ID signing and notarization succeeds",
    )
    args = parser.parse_args()

    args.output.mkdir(parents=True, exist_ok=True)
    if is_macos_label(args.label):
        package_macos_apps(args)
        return

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


def is_macos_label(label: str) -> bool:
    return label.startswith("macos-")


def package_macos_apps(args: argparse.Namespace) -> None:
    if args.require_macos_notarization and sys.platform != "darwin":
        raise SystemExit("macOS notarization must run on a macOS host")

    source_dir = Path("target") / args.target / "release"
    version = workspace_version()
    staging_root = args.output / f".{args.label}-apps"
    if staging_root.exists():
        shutil.rmtree(staging_root)
    staging_root.mkdir(parents=True)

    signing = None
    try:
        if args.require_macos_notarization:
            signing = MacSigningContext.from_environment()
        for binary_name, metadata in MACOS_PRODUCTS.items():
            product_root = staging_root / binary_name
            product_root.mkdir()
            app_path = product_root / f"{metadata['display_name']}.app"
            create_macos_app(
                source_dir / binary_name,
                app_path,
                binary_name,
                metadata["display_name"],
                metadata["bundle_id"],
                version,
            )
            if signing is not None:
                signing.sign_and_notarize_app(app_path, binary_name)

            destination = args.output / f"{binary_name}-{args.label}.dmg"
            create_dmg(product_root, metadata["display_name"], destination)
            if signing is not None:
                signing.sign_and_notarize_dmg(destination)
            write_checksum(destination)
    finally:
        if signing is not None:
            signing.cleanup()
        if staging_root.exists():
            shutil.rmtree(staging_root)


def workspace_version() -> str:
    cargo_toml = Path("Cargo.toml").read_text(encoding="utf-8")
    in_workspace_package = False
    for raw_line in cargo_toml.splitlines():
        line = raw_line.strip()
        if line == "[workspace.package]":
            in_workspace_package = True
            continue
        if line.startswith("[") and line.endswith("]"):
            in_workspace_package = False
        if in_workspace_package and line.startswith("version"):
            return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit("could not find workspace package version in Cargo.toml")


def create_macos_app(
    source: Path,
    app_path: Path,
    binary_name: str,
    display_name: str,
    bundle_id: str,
    version: str,
) -> None:
    if not source.exists():
        raise SystemExit(f"missing release binary: {source}")

    macos_dir = app_path / "Contents" / "MacOS"
    resources_dir = app_path / "Contents" / "Resources"
    macos_dir.mkdir(parents=True)
    resources_dir.mkdir(parents=True)

    bundled_binary = resources_dir / binary_name
    shutil.copy2(source, bundled_binary)
    bundled_binary.chmod(bundled_binary.stat().st_mode | stat.S_IXUSR)

    launcher_name = f"{binary_name}-installer"
    launcher_path = macos_dir / launcher_name
    launcher_path.write_text(
        launcher_script(binary_name, display_name),
        encoding="utf-8",
    )
    launcher_path.chmod(0o755)

    plist = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": display_name,
        "CFBundleExecutable": launcher_name,
        "CFBundleIdentifier": bundle_id,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": display_name,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": "10.15",
        "LSUIElement": True,
        "NSHighResolutionCapable": True,
    }
    with (app_path / "Contents" / "Info.plist").open("wb") as handle:
        plistlib.dump(plist, handle, sort_keys=True)


def launcher_script(binary_name: str, display_name: str) -> str:
    return textwrap.dedent(
        f"""\
        #!/bin/sh
        set -u

        APP_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
        BIN="$APP_DIR/Resources/{binary_name}"
        LOG_FILE="$(/usr/bin/mktemp "${{TMPDIR:-/tmp}}/{binary_name}.install.XXXXXX")" || exit 1

        "$BIN" install >"$LOG_FILE" 2>&1
        STATUS=$?
        OUTPUT="$(/bin/cat "$LOG_FILE")"
        /bin/rm -f "$LOG_FILE"

        if [ "$STATUS" -eq 0 ]; then
          /usr/bin/osascript - "$OUTPUT" <<'APPLESCRIPT'
        on run argv
          set messageText to item 1 of argv
          if messageText is "" then set messageText to "Tally installation completed."
          display dialog messageText buttons {{"Done"}} default button "Done" with title "{display_name}"
        end run
        APPLESCRIPT
        else
          /usr/bin/osascript - "$OUTPUT" <<'APPLESCRIPT'
        on run argv
          set messageText to item 1 of argv
          if messageText is "" then set messageText to "Tally installation failed."
          display dialog messageText buttons {{"Done"}} default button "Done" with title "{display_name}" with icon caution
        end run
        APPLESCRIPT
          exit "$STATUS"
        fi
        """
    )


class MacSigningContext:
    def __init__(self, temp_dir: Path, keychain_path: Path, identity: str) -> None:
        self.temp_dir = temp_dir
        self.keychain_path = keychain_path
        self.identity = identity

    @classmethod
    def from_environment(cls) -> "MacSigningContext":
        required = [
            "APPLE_DEVELOPER_ID_APPLICATION_CERTIFICATE_BASE64",
            "APPLE_DEVELOPER_ID_APPLICATION_CERTIFICATE_PASSWORD",
            "APPLE_ID",
            "APPLE_TEAM_ID",
            "APPLE_APP_SPECIFIC_PASSWORD",
        ]
        missing = [name for name in required if not os.environ.get(name)]
        if missing:
            raise SystemExit(
                "missing macOS release signing secret(s): " + ", ".join(missing)
            )

        temp_dir = Path(tempfile.mkdtemp(prefix="tally-macos-signing-"))
        keychain_path = temp_dir / "signing.keychain-db"
        keychain_password = os.environ.get("APPLE_KEYCHAIN_PASSWORD", "temporary-keychain")
        certificate_path = temp_dir / "developer-id-application.p12"
        certificate_path.write_bytes(
            base64.b64decode(
                os.environ["APPLE_DEVELOPER_ID_APPLICATION_CERTIFICATE_BASE64"]
            )
        )

        run(["security", "create-keychain", "-p", keychain_password, str(keychain_path)])
        run(
            [
                "security",
                "set-keychain-settings",
                "-lut",
                "21600",
                str(keychain_path),
            ]
        )
        run(["security", "unlock-keychain", "-p", keychain_password, str(keychain_path)])
        run(
            [
                "security",
                "import",
                str(certificate_path),
                "-k",
                str(keychain_path),
                "-P",
                os.environ["APPLE_DEVELOPER_ID_APPLICATION_CERTIFICATE_PASSWORD"],
                "-T",
                "/usr/bin/codesign",
            ]
        )
        run(
            [
                "security",
                "set-key-partition-list",
                "-S",
                "apple-tool:,apple:,codesign:",
                "-s",
                "-k",
                keychain_password,
                str(keychain_path),
            ]
        )

        identity = os.environ.get("APPLE_DEVELOPER_ID_APPLICATION_IDENTITY")
        if not identity:
            result = run(
                ["security", "find-identity", "-v", "-p", "codesigning", str(keychain_path)],
                capture_output=True,
            )
            identity = first_developer_id_identity(result.stdout)
        return cls(temp_dir, keychain_path, identity)

    def sign_and_notarize_app(self, app_path: Path, binary_name: str) -> None:
        run(
            [
                "codesign",
                "--force",
                "--timestamp",
                "--options",
                "runtime",
                "--sign",
                self.identity,
                str(app_path / "Contents" / "Resources" / binary_name),
            ]
        )
        run(
            [
                "codesign",
                "--force",
                "--timestamp",
                "--options",
                "runtime",
                "--sign",
                self.identity,
                str(app_path),
            ]
        )
        run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app_path)])

        notary_zip = self.temp_dir / f"{app_path.stem}.zip"
        zip_app(app_path, notary_zip)
        run(
            [
                "xcrun",
                "notarytool",
                "submit",
                str(notary_zip),
                "--apple-id",
                os.environ["APPLE_ID"],
                "--team-id",
                os.environ["APPLE_TEAM_ID"],
                "--password",
                os.environ["APPLE_APP_SPECIFIC_PASSWORD"],
                "--wait",
            ]
        )
        run(["xcrun", "stapler", "staple", str(app_path)])
        run(["xcrun", "stapler", "validate", str(app_path)])
        run(["spctl", "-a", "-t", "execute", "-vv", str(app_path)])

    def sign_and_notarize_dmg(self, dmg_path: Path) -> None:
        run(
            [
                "codesign",
                "--force",
                "--timestamp",
                "--sign",
                self.identity,
                str(dmg_path),
            ]
        )
        run(
            [
                "xcrun",
                "notarytool",
                "submit",
                str(dmg_path),
                "--apple-id",
                os.environ["APPLE_ID"],
                "--team-id",
                os.environ["APPLE_TEAM_ID"],
                "--password",
                os.environ["APPLE_APP_SPECIFIC_PASSWORD"],
                "--wait",
            ]
        )
        run(["xcrun", "stapler", "staple", str(dmg_path)])
        run(["xcrun", "stapler", "validate", str(dmg_path)])
        run(
            [
                "spctl",
                "-a",
                "-t",
                "open",
                "--context",
                "context:primary-signature",
                "-vv",
                str(dmg_path),
            ]
        )

    def cleanup(self) -> None:
        run(["security", "delete-keychain", str(self.keychain_path)], check=False)
        shutil.rmtree(self.temp_dir, ignore_errors=True)


def first_developer_id_identity(output: str) -> str:
    for line in output.splitlines():
        if "Developer ID Application" in line and '"' in line:
            return line.split('"', 2)[1]
    raise SystemExit("no Developer ID Application signing identity found")


def create_dmg(source_folder: Path, volume_name: str, destination: Path) -> None:
    if destination.exists():
        destination.unlink()
    hdiutil = shutil.which("hdiutil")
    if not hdiutil:
        raise SystemExit("hdiutil is required to package macOS release DMGs")
    run(
        [
            hdiutil,
            "create",
            "-fs",
            "HFS+",
            "-format",
            "UDZO",
            "-volname",
            volume_name,
            "-srcfolder",
            str(source_folder),
            str(destination),
        ]
    )


def zip_app(app_path: Path, destination: Path) -> None:
    if destination.exists():
        destination.unlink()
    destination.parent.mkdir(parents=True, exist_ok=True)
    ditto = shutil.which("ditto")
    if ditto:
        run(
            [
                ditto,
                "-c",
                "-k",
                "--sequesterRsrc",
                "--keepParent",
                app_path.name,
                str(destination),
            ],
            cwd=app_path.parent,
        )
        return
    zip_directory(app_path, destination)


def zip_directory(source: Path, destination: Path) -> None:
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(source.rglob("*")):
            relative = Path(source.name) / path.relative_to(source)
            info = zipfile.ZipInfo(str(relative))
            mode = path.stat().st_mode
            info.external_attr = (mode & 0xFFFF) << 16
            if path.is_dir():
                archive.writestr(info, b"")
            else:
                archive.writestr(info, path.read_bytes())


def write_checksum(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(f"{path.name}.sha256").write_text(
        f"{digest}  {path.name}\n", encoding="ascii"
    )


def run(
    command: list[str],
    *,
    cwd: Path | None = None,
    capture_output: bool = False,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        text=True,
        check=check,
        stdout=subprocess.PIPE if capture_output else None,
        stderr=subprocess.STDOUT if capture_output else None,
    )


if __name__ == "__main__":
    main()
