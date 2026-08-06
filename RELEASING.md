# Releasing Tally

## Supported Targets

- Docker images: Linux `amd64` and `arm64`.
- Host hook installers: macOS and Linux systems with Rust 1.89 or newer.
- Windows packages are not currently produced or supported.

## Release Checks

Run the complete local acceptance suite:

```bash
./scripts/release-check.sh --docker
```

This checks Rust formatting, Clippy, unit tests, shell syntax, isolated host
installation and removal, both Compose files, both image builds, installed CLI
versions, synthetic hook lifecycles, record correlation, and a real Claude Code
tool-use session against a local API double. The suite does not need production
credentials.

## Checklist

1. Choose and add a repository `LICENSE` before public distribution.
2. Move the relevant entries in `CHANGELOG.md` from `Unreleased` to the new
   version and date.
3. Run `./scripts/release-check.sh --docker` from a clean checkout.
4. Run and review dependency and base-image vulnerability scans.
5. Merge to `dev`, then manually run both image release workflows with the
   same version.
6. Verify the `amd64`, `arm64`, version, commit, and optional `latest` tags in
   Public ECR.
7. Pull each multi-architecture image from Public ECR and repeat the smoke
   commands on clean hosts.
8. Tag the verified commit as `v<version>` and publish the changelog notes.

Do not publish host binaries built on a developer machine. The host installer
builds locally from the tagged source so its output matches the user's target.
