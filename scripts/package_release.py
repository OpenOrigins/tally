#!/usr/bin/env python3
"""Create consistently named native release assets and checksums."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from dataclasses import dataclass
from pathlib import Path


MACOS_PRODUCTS = {
    "tally-codex": {
        "product_name": "Tally Codex",
        "bundle_id": "com.openorigins.tally.codex",
    },
    "tally-claude": {
        "product_name": "Tally Claude Code",
        "bundle_id": "com.openorigins.tally.claude",
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
        help="fail unless Developer ID signing and Apple notarization succeed",
    )
    args = parser.parse_args()

    if args.require_macos_notarization and not args.label.startswith("macos-"):
        raise SystemExit("macOS notarization can only be required for a macOS target")

    args.output.mkdir(parents=True, exist_ok=True)
    if args.label.startswith("macos-"):
        package_macos(args)
        return

    suffix = ".exe" if "windows" in args.label else ""
    source_dir = Path("target") / args.target / "release"
    for name in MACOS_PRODUCTS:
        source = source_dir / f"{name}{suffix}"
        destination = args.output / f"{name}-{args.label}{suffix}"
        shutil.copy2(source, destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR)
        write_checksum(destination)


def package_macos(args: argparse.Namespace) -> None:
    if args.require_macos_notarization and sys.platform != "darwin":
        raise SystemExit("macOS notarization must run on a macOS host")

    signing = MacSigning.from_environment(args.require_macos_notarization)
    source_dir = Path("target") / args.target / "release"
    version = workspace_version()

    for binary_name, metadata in MACOS_PRODUCTS.items():
        source = source_dir / binary_name
        if not source.is_file():
            raise SystemExit(f"missing release binary: {source}")

        if sys.platform == "darwin":
            sign_path(source, signing, hardened_runtime=True)

        with tempfile.TemporaryDirectory(prefix=f"{binary_name}-dmg-") as directory:
            staging = Path(directory)
            app = build_macos_app(
                source,
                staging,
                binary_name,
                version,
                args.label,
                metadata["product_name"],
                metadata["bundle_id"],
            )
            if sys.platform == "darwin":
                sign_path(app, signing, hardened_runtime=True)
                verify_signature(app, deep=True)
                dmg = args.output / f"{binary_name}-{args.label}.dmg"
                create_dmg(staging, metadata["product_name"], dmg)
                sign_path(dmg, signing, hardened_runtime=False)
                verify_signature(dmg)
                if args.require_macos_notarization:
                    signing.notarize_and_staple(dmg)
                write_checksum(dmg)
            else:
                app_archive = args.output / f"{binary_name}-{args.label}-app.zip"
                write_app_zip(app, app_archive)
                write_checksum(app_archive)

        cli_archive = args.output / f"{binary_name}-{args.label}-cli.tar.gz"
        write_cli_archive(source, cli_archive, binary_name)
        write_checksum(cli_archive)


def workspace_version() -> str:
    with Path("Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def build_macos_app(
    source: Path,
    directory: Path,
    binary_name: str,
    version: str,
    label: str,
    product_name: str,
    bundle_id: str,
) -> Path:
    app = directory / f"{product_name}.app"
    contents = app / "Contents"
    macos = contents / "MacOS"
    resources = contents / "Resources"
    macos.mkdir(parents=True)
    resources.mkdir()
    contents.chmod(0o755)
    macos.chmod(0o755)
    resources.chmod(0o755)

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
    (contents / "Info.plist").write_bytes(plist)
    shutil.copy2("LICENSE", resources / "LICENSE")
    executable = macos / binary_name
    shutil.copy2(source, executable)
    executable.chmod(0o755)
    return app


@dataclass(frozen=True)
class MacSigning:
    identity: str
    keychain: str | None
    notary_key_path: str | None
    notary_key_id: str | None
    notary_issuer_id: str | None

    @classmethod
    def from_environment(cls, required: bool) -> "MacSigning":
        identity = os.environ.get("APPLE_DEVELOPER_IDENTITY", "-")
        keychain = os.environ.get("APPLE_SIGNING_KEYCHAIN")
        notary_values = {
            "APPLE_NOTARY_KEY_PATH": os.environ.get("APPLE_NOTARY_KEY_PATH"),
            "APPLE_NOTARY_KEY_ID": os.environ.get("APPLE_NOTARY_KEY_ID"),
            "APPLE_NOTARY_ISSUER_ID": os.environ.get("APPLE_NOTARY_ISSUER_ID"),
        }
        missing = [name for name, value in notary_values.items() if not value]
        if required and identity == "-":
            missing.insert(0, "APPLE_DEVELOPER_IDENTITY")
        if required and missing:
            raise SystemExit(
                "missing macOS release signing value(s): " + ", ".join(missing)
            )
        if not required and len(missing) not in (0, len(notary_values)):
            raise SystemExit("partial Apple notarization configuration is not allowed")
        return cls(
            identity=identity,
            keychain=keychain,
            notary_key_path=notary_values["APPLE_NOTARY_KEY_PATH"],
            notary_key_id=notary_values["APPLE_NOTARY_KEY_ID"],
            notary_issuer_id=notary_values["APPLE_NOTARY_ISSUER_ID"],
        )

    def notarize_and_staple(self, dmg: Path) -> None:
        command = [
            "xcrun",
            "notarytool",
            "submit",
            str(dmg),
            "--key",
            self.notary_key_path,
            "--key-id",
            self.notary_key_id,
            "--issuer",
            self.notary_issuer_id,
            "--wait",
            "--output-format",
            "json",
        ]
        result = subprocess.run(command, check=True, text=True, capture_output=True)
        response = json.loads(result.stdout)
        submission_id = response.get("id", "unknown")
        status = response.get("status", "unknown")
        print(f"Apple notarization {submission_id}: {status}")
        if status != "Accepted":
            if submission_id != "unknown":
                subprocess.run(
                    [
                        "xcrun",
                        "notarytool",
                        "log",
                        submission_id,
                        "--key",
                        self.notary_key_path,
                        "--key-id",
                        self.notary_key_id,
                        "--issuer",
                        self.notary_issuer_id,
                    ],
                    check=False,
                )
            raise SystemExit(f"Apple notarization failed with status {status}")

        run(["xcrun", "stapler", "staple", "-v", str(dmg)])
        run(["xcrun", "stapler", "validate", "-v", str(dmg)])
        run(
            [
                "spctl",
                "--assess",
                "--type",
                "open",
                "--context",
                "context:primary-signature",
                "--verbose=2",
                str(dmg),
            ]
        )


def sign_path(path: Path, signing: MacSigning, hardened_runtime: bool) -> None:
    command = ["codesign", "--force"]
    if signing.identity == "-":
        command.append("--timestamp=none")
    else:
        command.append("--timestamp")
    if hardened_runtime:
        command.extend(["--options", "runtime"])
    if signing.keychain:
        command.extend(["--keychain", signing.keychain])
    command.extend(["--sign", signing.identity, str(path)])
    run(command)


def verify_signature(path: Path, deep: bool = False) -> None:
    command = ["codesign", "--verify", "--strict", "--verbose=2"]
    if deep:
        command.append("--deep")
    command.append(str(path))
    run(command)


def create_dmg(source_folder: Path, volume_name: str, destination: Path) -> None:
    hdiutil = shutil.which("hdiutil")
    if not hdiutil:
        raise SystemExit("hdiutil is required to package macOS release DMGs")
    applications = source_folder / "Applications"
    applications.symlink_to("/Applications", target_is_directory=True)
    if destination.exists():
        destination.unlink()
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
            "-ov",
            str(destination),
        ]
    )


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
        license_info = archive.gettarinfo("LICENSE", arcname="LICENSE")
        license_info.mode = 0o644
        with Path("LICENSE").open("rb") as handle:
            archive.addfile(license_info, handle)


def write_checksum(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(f"{path.name}.sha256").write_bytes(
        f"{digest}  {path.name}\n".encode("ascii")
    )


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=True, **kwargs)


if __name__ == "__main__":
    main()
