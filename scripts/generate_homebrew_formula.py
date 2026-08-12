#!/usr/bin/env python3
"""Generate the Tally Homebrew formula from verified release artifacts."""

from __future__ import annotations

import argparse
import hashlib
import re
import tomllib
from pathlib import Path


REPOSITORY = "OpenOrigins/tally"
PLATFORMS = (
    ("mac", "arm", "macos-arm64", ".tar.gz"),
    ("mac", "intel", "macos-x86_64", ".tar.gz"),
    ("linux", "intel", "linux-x86_64", ""),
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
    launcher = Path(__file__).with_name("tally").read_text(encoding="ascii")
    formula = render_formula(version, release_tag, checksums, launcher)
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
    for os_name, _, label, suffix in PLATFORMS:
        for agent in ("codex", "claude"):
            archive_suffix = f"-cli{suffix}" if os_name == "mac" else suffix
            name = f"tally-{agent}-{label}{archive_suffix}"
            path = dist / name
            if not path.is_file():
                raise SystemExit(f"missing release artifact: {path}")
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            checksum_path = dist / f"{name}.sha256"
            if not checksum_path.is_file():
                raise SystemExit(f"missing release checksum: {checksum_path}")
            recorded = checksum_path.read_text(encoding="ascii").split()[0]
            if recorded != digest:
                raise SystemExit(f"checksum does not match release artifact: {path}")
            checksums[name] = digest
    return checksums


def render_formula(
    version: str, release_tag: str, checksums: dict[str, str], launcher: str
) -> str:
    blocks = []
    for os_name, cpu, label, suffix in PLATFORMS:
        archive_suffix = f"-cli{suffix}" if os_name == "mac" else suffix
        codex_name = f"tally-codex-{label}{archive_suffix}"
        claude_name = f"tally-claude-{label}{archive_suffix}"
        nounzip = ", using: :nounzip" if not suffix else ""
        blocks.append(
            f'''  if OS.{os_name}? && Hardware::CPU.{cpu}?
    url "https://github.com/{REPOSITORY}/releases/download/{release_tag}/{codex_name}"{nounzip}
    sha256 "{checksums[codex_name]}"

    resource "tally-claude" do
      url "https://github.com/{REPOSITORY}/releases/download/{release_tag}/{claude_name}"{nounzip}
      sha256 "{checksums[claude_name]}"
    end
  end'''
        )

    indented_launcher = "".join(
        f"      {line}" if line.strip() else line
        for line in launcher.splitlines(keepends=True)
    )
    version_line = "" if release_tag == f"v{version}" else f'  version "{version}"\n'
    return f'''class Tally < Formula
  desc "Record verifiable Codex and Claude Code activity"
  homepage "https://github.com/{REPOSITORY}"
{version_line.rstrip()}
  license "Apache-2.0"

{chr(10).join(blocks)}

  def install
    codex_source = Dir["tally-codex*"].fetch(0)
    bin.install codex_source => "tally-codex"
    resource("tally-claude").stage do
      claude_source = Dir["tally-claude*"].fetch(0)
      bin.install claude_source => "tally-claude"
    end
    (bin/"tally").write <<~SH
{indented_launcher}    SH
  end

  def caveats
    <<~EOS
      Run `tally` to choose Codex or Claude Code and paste your Agent API key.
      You can also start a specific installer with `tally codex` or `tally claude`.
    EOS
  end

  test do
    assert_match "tally #{{version}}", shell_output("#{{bin}}/tally --version")
    assert_match "tally-codex #{{version}}", shell_output("#{{bin}}/tally-codex --version")
    assert_match "tally-claude #{{version}}", shell_output("#{{bin}}/tally-claude --version")
  end
end
'''


if __name__ == "__main__":
    main()
