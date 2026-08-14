#!/usr/bin/env python3
"""Generate the Tally Homebrew formula from verified release installers."""

from __future__ import annotations

import argparse
import hashlib
import re
import tomllib
from pathlib import Path


REPOSITORY = "OpenOrigins/tally"
PLATFORMS = (
    ("mac", "arm", "tally-macos-arm64.dmg", False),
    ("mac", "intel", "tally-macos-x86_64.dmg", False),
    ("linux", "intel", "tally-linux-x86_64", True),
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
    formula = render_formula(version, release_tag, checksums)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(formula, encoding="ascii")


def workspace_version() -> str:
    with Path("Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def validate_version(version: str) -> str:
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version):
        raise SystemExit(f"invalid package version: {version!r}")
    return version


def release_checksums(dist: Path) -> dict[str, str]:
    checksums: dict[str, str] = {}
    for _, _, name, _ in PLATFORMS:
        path = dist / name
        if not path.is_file():
            raise SystemExit(f"missing release artifact: {path}")
        checksums[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return checksums


def render_formula(version: str, release_tag: str, checksums: dict[str, str]) -> str:
    blocks = []
    for os_name, cpu, name, nounzip in PLATFORMS:
        using = ", using: :nounzip" if nounzip else ""
        blocks.append(
            f'''  if OS.{os_name}? && Hardware::CPU.{cpu}?
    url "https://github.com/{REPOSITORY}/releases/download/{release_tag}/{name}"{using}
    sha256 "{checksums[name]}"
  end'''
        )

    version_line = "" if release_tag == f"v{version}" else f'  version "{version}"\n'
    return f'''class Tally < Formula
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/{REPOSITORY}"
{version_line.rstrip()}
  license "Apache-2.0"

{chr(10).join(blocks)}

  def install
    source = if OS.mac?
      "Tally.app/Contents/MacOS/tally"
    else
      Dir["tally-linux-*"].fetch(0)
    end
    bin.install source => "tally"
  end

  def caveats
    <<~EOS
      Run `tally` to open the installer, choose Codex and/or Claude Code,
      and paste the Agent API key from the OpenOrigins dashboard.
    EOS
  end

  test do
    assert_match "Tally #{{version}}", shell_output("#{{bin}}/tally --version")
  end
end
'''


if __name__ == "__main__":
    main()
