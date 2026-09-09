# Homebrew Distribution

Homebrew installs the same signed and notarized macOS `Tally.app` distributed
in the release DMG. It does not install separate Codex, Claude Code, or
command-line editions.

```sh
brew tap openorigins/tally https://github.com/OpenOrigins/tally
brew install --cask tally
```

Open **Tally** from Applications. The installer lets the user choose Codex,
Claude Code, or both and paste the dashboard Agent API key. Upgrade later with:

```sh
brew upgrade --cask tally
```

The release workflow generates `Casks/tally.rb` from the final macOS DMGs.
Before promotion, verify it with:

```sh
brew style --cask openorigins/tally/tally
brew audit --strict --cask openorigins/tally/tally
brew fetch --cask openorigins/tally/tally
brew install --cask openorigins/tally/tally
```

The repository URL is required in the one-time `brew tap` command because this
repository is not named `homebrew-tally` and the cask is not in Homebrew Cask.
