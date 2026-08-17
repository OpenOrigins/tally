# Changelog

All notable changes to Tally are documented in this file.

## 0.1.4 - 2026-08-17

### Fixed

- Added an independently signed macOS hook helper so installed hooks remain
  valid after being copied outside `Tally.app`.
- Verify macOS hook signatures during installation and roll back cleanly if
  verification fails.
- Added native DMG tests that install and execute Codex and Claude Code hooks
  from the packaged app on every pull request and release.

## 0.1.3 - 2026-08-14

### Added

- One graphical installer that configures Codex, Claude Code, or both.
- Separate editable configuration paths for each selected client.
- Combined-client end-user tests covering secure key storage, lowercase
  `x-api-key` handshakes, retry, forwarding, hook deduplication, and uninstall.

### Changed

- Reduced the release to four platform installers and one checksum manifest.
- Removed separate client downloads and CLI release archives.
- Updated macOS packaging to one signed and notarized `Tally.app` in each DMG.
- Added a Homebrew Cask for the same signed, notarized graphical installer.
- Reworked installation documentation for users unfamiliar with GitHub.

## 0.1.2 - 2026-08-06

### Added

- Signed and notarized macOS DMGs for Codex and Claude Code on Apple Silicon and Intel.
- A shared graphical installer with Agent API key entry, custom configuration
  paths, retry, cancel, and automatic dashboard handshake status.
- Native installers for macOS, Windows, and Linux plus Homebrew distribution.

### Fixed

- Treat successful HTTP responses with Tally API error bodies as forwarding
  failures so local queues are retained and users see retryable ingest errors.
- Store Windows hook executables outside client configuration directories.

## 0.1.0 - 2026-08-06

### Added

- Native Codex and Claude Code hook handlers and cross-platform installation tests.
- Regression tests for the API push behavior introduced in `31b44c5`, including
  exact lowercase `x-api-key` header casing.
- Dashboard-issued Agent API key onboarding, automatic post-install connection
  handshake, and retryable background log forwarding.
