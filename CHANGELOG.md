# Changelog

All notable changes to Tally are documented in this file.

## Unreleased

### Added

- Restore a shared graphical installer for macOS and Windows with dashboard
  Agent API key entry, custom ingest URL support, and explicit handshake status.
- Package Finder-openable macOS app bundles alongside CLI symlinks in each
  release archive.

### Changed

- Copy each installer executable into stable local Tally storage before wiring
  hooks, so removing the downloaded installer does not break logging.

## 0.1.3 - 2026-08-06

### Changed

- Rolled back macOS DMG installer packaging and package macOS downloads as
  `.tar.gz` archives that preserve executable permissions.

## 0.1.2 - 2026-08-06

### Fixed

- Treat successful HTTP responses with Tally API error bodies as forwarding
  failures so local queues are retained and users see retryable ingest errors.

## 0.1.0 - 2026-08-06

### Added

- Native Codex and Claude Code executables for macOS arm64/x86_64, Windows
  x86_64, and Linux x86_64 releases.
- Cross-platform end-user installation, hook execution, audit-record, removal,
  and release-packaging tests on every pull request.
- Regression tests for the Tally API push behavior introduced in `31b44c5`,
  including exact lowercase `x-api-key` header casing.
- Dashboard-issued Agent API key onboarding, automatic post-install connection
  handshake, and retryable background log forwarding for both agent clients.

### Changed

- Running a native executable without arguments now prompts for the dashboard
  Agent API key and then installs or updates hooks.
- Consolidated six agent packaging directories into `codex/` and `claude/`.
- Moved the specification and examples under `docs/` and reduced the root
  README to release and source-install essentials.
- Aligned wrapper records with Tally schema version `0.2` and added Windows
  home-directory, command-quoting, process-liveness, and file-replacement support.

### Removed

- Redundant Docker, host-installer, and agent-specific image release paths.
