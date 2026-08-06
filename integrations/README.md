# Claude Code Installers

This directory contains the cross-platform Claude Code installer source and
prebuilt development artifacts.

## Layout

- `src-windows/` contains the installer, uninstaller, hook, forwarder, and GUI
  source used for future builds.
- `Windows/` contains the Windows installer and hook executables.
- `Tally Anchor Setup.zip` and `Uninstall Tally Anchor.zip` contain macOS app
  bundles.

## Release Status

Treat the checked-in executables and app bundles as development artifacts, not
release-ready downloads. The macOS apps are ad-hoc signed and are not notarized,
so Gatekeeper rejects them. The Windows executables must be rebuilt, signed, and
tested on Windows before distribution.

The checked-in macOS package also fails two end-to-end acceptance checks:

- Its bundled forwarder initializes at the end of the local log, so records
  written before the forwarder starts are skipped instead of delivered.
- Its onboarding request does not honor the configured API origin.

Rebuild the macOS package from the current source before testing or distributing
it. Do not use the bundled package with production credentials.

The source can be newer than the prebuilt artifacts. Rebuild all packages after
source changes and verify their signatures and end-to-end install behavior on
the target operating system.
