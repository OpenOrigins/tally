# Changelog

All notable changes to Tally are documented in this file.

## Unreleased

### Added

- Added a durable append-only segmented delivery journal with ordered sequence
  numbers, torn-tail repair, terminal outcome records, dead letters, and receipt
  persistence.
- Added idempotent ordered delivery, a reusable HTTP client, bounded exponential
  retries with jitter and `Retry-After`, and a coalesced single-worker model.
- Added bounded hook input, JSON traversal, Git capture, response-body, and local
  storage traversal limits, plus fault tests for journal recovery, transient and
  permanent delivery failures, corrupt heartbeat state, evidence integrity, and
  multi-client installer rollback.

### Changed

- Stage content-addressed private evidence in the journal and materialize it in
  the background, reducing synchronous hook durability work to one append.
- Share hook-to-record construction between Codex and Claude Code and capture
  one bounded Git status snapshot instead of running three Git processes.
- Give installations persistent random agent IDs, use 128-bit record IDs, emit
  real `HANDOFF` records, and explicitly mark unsupported signatures, declared
  intent, principal identity, and deviation evaluation as unavailable.

### Removed

- Removed pre-journal queue migration, retired hook command detection, and old
  installed-helper cleanup paths.

### Fixed

- Preserve capture order instead of hash-sorting records, prevent retry storms
  and poison-record blockage, retain server receipts, and close worker wakeup
  races during bursts.
- Validate evidence hash/URI pairs and ensure action parameters, pre-state,
  post-state, results, and raw hooks reference the correct private objects.
- Stop heartbeat daemons on corrupt or implausibly future state, sync parent
  directories after atomic filesystem changes, restrict Windows private ACLs,
  and roll back all selected GUI clients when any installation fails.
- Publish release artifacts from the workflow's exact commit SHA and ignore
  generated output directories.

## 0.1.9 - 2026-08-24

### Added

- Show the installed Tally version in the graphical installer and window title.
- State the exact Codex CLI/Desktop and Claude Code versions verified for the
  release, together with capability-based compatibility guidance.

### Fixed

- Register a Codex turn-completion callback so Codex Desktop forwards session,
  instruction, and turn records even when Desktop does not execute hooks.
- Preserve, chain, and restore an existing Codex notification command during
  install, update, and uninstall.
- Suppress callback records when command-line hooks already recorded the same
  turn, and deduplicate repeated Desktop callbacks.
- Added native regression coverage for Desktop records, duplicate suppression,
  existing notification chaining, and uninstall restoration.

## 0.1.7 - 2026-08-21

### Fixed

- Limit heartbeats to one per agent every 10 minutes across concurrent Codex
  and Claude Code sessions.
- Suppress heartbeats until the agent has been quiet for 10 minutes and add a
  stable record ID so forwarding retries can be deduplicated.
- Added an end-user regression test that races two sessions for each client and
  asserts that only one heartbeat is written.
- Replaced the misleading post-uninstall Cancel action with Close and report
  retained queued records and local logs explicitly.
- Prevented Windows heartbeat daemons and forwarding workers from inheriting
  hook-runner handles, which could make a completed hook appear to hang.
- Added a Windows regression check requiring the hook process to return while
  its ten-minute heartbeat daemon remains active.

### Added

- Added a confirmed full-removal option that deletes queued records and local
  Tally logs while preserving unrelated client settings.

## 0.1.6 - 2026-08-19

### Fixed

- Stopped emitting a synthetic heartbeat for every Codex hook event.
- Emit Codex heartbeats only after a quiet interval and prevent concurrent
  heartbeat daemons for the same session.
- Added end-user heartbeat regression coverage for both supported clients.

## 0.1.5 - 2026-08-19

### Fixed

- Changed the public installer default from Dev2 to the Production ingest API.
- Stopped emitting a synthetic heartbeat for every Claude Code hook event.
- Emit periodic Claude Code heartbeats only after a quiet interval and prevent
  concurrent heartbeat daemons for the same session.
- Added end-user regression coverage that asserts one Claude hook forwards one
  audit record and no immediate heartbeat.

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
