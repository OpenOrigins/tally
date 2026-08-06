# Releasing Tally

Tags matching `v*` build and test native executables on:

- macOS arm64 (deployment target 11.0)
- macOS x86_64 (deployment target 10.15)
- Windows Server 2025 x86_64
- Linux x86_64 (static musl build)

The workflow publishes macOS `.tar.gz` archives, Linux/Windows executables, and
SHA-256 files for every release asset.

## Checklist

1. Confirm the repository license is appropriate for public binary distribution.
2. Run `./scripts/release-check.sh` from a clean checkout.
3. Update `CHANGELOG.md` and the workspace version in `Cargo.toml`.
4. Merge to `dev` and confirm all required PR checks pass.
5. Tag the tested commit as `v<version>` and push the tag.
6. Verify all macOS archives, Linux/Windows executables, and checksum files in the release.
7. Install each executable on a clean target and run one real agent session.

The workflow produces unsigned binaries. Configure platform signing and macOS
notarization credentials before treating downloads as a polished public release.
