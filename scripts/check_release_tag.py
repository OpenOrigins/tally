#!/usr/bin/env python3
"""Fail when a release tag does not match the workspace package version."""

from __future__ import annotations

import argparse
import tomllib
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("tag", nargs="?")
    args = parser.parse_args()
    cargo = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))
    version = cargo["workspace"]["package"]["version"]
    expected = f"v{version}"
    tag = args.tag or expected
    if tag != expected:
        raise SystemExit(f"release tag {tag!r} must match workspace version {expected!r}")


if __name__ == "__main__":
    main()
