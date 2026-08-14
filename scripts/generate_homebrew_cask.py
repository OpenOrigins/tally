#!/usr/bin/env python3
"""Generate the Tally Homebrew cask from verified macOS installers."""

from __future__ import annotations

import argparse
import hashlib
import re
import tomllib
from pathlib import Path


REPOSITORY = "OpenOrigins/tally"
PLATFORMS = (
    ("arm", "tally-macos-arm64.dmg"),
    ("intel", "tally-macos-x86_64.dmg"),
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", default=workspace_version())
    parser.add_argument("--release-tag")
    args = parser.parse_args()

    version = validate_version(args.version)
    release_tag = args.release_tag or f"v{version}"
    checksums = release_checksums(args.dist)
    cask = render_cask(version, release_tag, checksums)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(cask, encoding="ascii")


def workspace_version() -> str:
    with Path("Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def validate_version(version: str) -> str:
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version):
        raise SystemExit(f"invalid package version: {version!r}")
    return version


def release_checksums(dist: Path) -> dict[str, str]:
    checksums: dict[str, str] = {}
    for _, name in PLATFORMS:
        path = dist / name
        if not path.is_file():
            raise SystemExit(f"missing release artifact: {path}")
        checksums[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return checksums


def render_cask(version: str, release_tag: str, checksums: dict[str, str]) -> str:
    arm_checksum = checksums["tally-macos-arm64.dmg"]
    intel_checksum = checksums["tally-macos-x86_64.dmg"]
    tag_component = "v#{version}" if release_tag == f"v{version}" else release_tag
    return f'''cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "{version}"
  sha256 arm:   "{arm_checksum}",
         intel: "{intel_checksum}"

  url "https://github.com/{REPOSITORY}/releases/download/{tag_component}/tally-macos-#{{arch}}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/{REPOSITORY}"

  depends_on :macos

  app "Tally.app"
end
'''


if __name__ == "__main__":
    main()
