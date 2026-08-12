# Homebrew distribution

The Homebrew formula is an additional release channel. It installs the same
`tally-codex` and `tally-claude` executables that are published as standalone
release artifacts, plus the `tally` launcher from `scripts/tally`.

## User flow

The Tally repository can be registered as a tap even though its name does not
start with `homebrew-` by supplying the repository URL:

```sh
brew tap openorigins/tally https://github.com/OpenOrigins/tally
brew install tally
tally
```

The first `tally` run detects an installed Codex or Claude Code client. If the
choice is ambiguous, it asks the user which client to configure, then opens the
existing browser installer for the Agent API key.

## Publishing a version

A tagged native release still publishes every existing standalone binary and
macOS app. After all matrix artifacts have been merged and their checksums have
been verified, the release workflow also generates `tally.rb` and attaches it
to the GitHub release.

Promote that generated file to `Formula/tally.rb` on the default branch, then
verify the tap on both supported macOS architectures and Linux:

```sh
brew style openorigins/tally/tally
brew audit --strict openorigins/tally/tally
brew install openorigins/tally/tally
brew test openorigins/tally/tally
```

Do not replace published release assets after promoting a formula. Homebrew
pins their SHA-256 checksums, so changing an asset at the same URL breaks fresh
installs. Publish a new version and update the formula instead.

Installing by the short name on a completely clean machine requires either the
one-time `brew tap` command above or acceptance into `homebrew/core`.
