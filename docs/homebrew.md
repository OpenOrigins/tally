# Homebrew Distribution

Homebrew installs the same single Tally executable used by the graphical
installer. It does not install separate Codex, Claude Code, or command-line
editions.

```sh
brew tap openorigins/tally https://github.com/OpenOrigins/tally
brew install tally
tally
```

Running `tally` opens the installer, where the user chooses Codex, Claude Code,
or both and pastes the dashboard Agent API key. Upgrade later with:

```sh
brew upgrade tally
```

The release workflow generates `Formula/tally.rb` from the final macOS DMGs and
Linux installer. Before promotion, verify it with:

```sh
brew style openorigins/tally/tally
brew audit --strict openorigins/tally/tally
brew fetch openorigins/tally/tally
brew install openorigins/tally/tally
brew test openorigins/tally/tally
```

The repository URL is required in the one-time `brew tap` command because this
repository is not named `homebrew-tally` and the formula is not in Homebrew Core.
