# Changelog

All notable changes to Tally are documented in this file.

## Unreleased

### Added

- Rust audit wrappers for Codex and Claude Code.
- Docker and host installation modes for both wrappers.
- Multi-architecture Public ECR release workflows.
- End-to-end Docker smoke tests, including a real Claude Code tool-use turn
  against a local API double.

### Changed

- Pinned container base images, Codex CLI 0.146.1, Node.js 22.23.2, and Claude
  Code 2.1.223.
- Consolidated Rust package metadata, dependency locking, and host build logic.
- Correlated Codex pre-tool and post-tool records through stable action IDs.

### Removed

- Superseded JavaScript wrappers.
- Unsigned, stale macOS and Windows installer binaries.
- Generated runtime logs and obsolete implementation comparison notes.
