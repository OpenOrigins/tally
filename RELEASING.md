# Releasing Tally

A `v*` tag runs tests and builds the same graphical installer for Codex and
Claude Code on macOS arm64, macOS Intel, Windows x86_64, and Linux x86_64.

Each release contains only:

- `tally-macos-arm64.dmg`
- `tally-macos-x86_64.dmg`
- `tally-windows-x86_64.exe`
- `tally-linux-x86_64`
- `SHA256SUMS`

The macOS app and DMG are Developer ID signed, hardened, notarized, stapled, and
assessed by Gatekeeper before upload. Windows is currently unsigned.

## Checklist

1. Update `CHANGELOG.md` and the workspace version in `Cargo.toml`.
2. Run `./scripts/release-check.sh` from a clean checkout.
3. Merge to `dev` and wait for every CI job to pass.
4. Tag that exact commit as `v<version>` and push the tag.
5. Wait for all four native release jobs and the publish job to pass.
6. Download every asset and verify `sha256sum --check SHA256SUMS`.
7. Mount both DMGs and verify the app and DMG with `codesign`, `stapler`, and
   `spctl`. Test installation, retry, hook execution, and uninstall on Windows
   and Linux as well.
8. Promote the generated Homebrew formula only after the published bytes and
   checksums are final.

Never replace an asset after its checksum has been promoted to Homebrew. Publish
a new version instead.
