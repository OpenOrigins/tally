# Tally

Tally connects Codex and Claude Code to OpenOrigins. It records structured audit
logs locally and forwards them using an Agent API key from your OpenOrigins
dashboard.

## Install

First, open **Connect a client** in the OpenOrigins dashboard and generate an
Agent API key. Keep that page open so you can paste the key into Tally.

### macOS

1. Open the [latest Tally release](https://github.com/OpenOrigins/tally/releases/latest).
2. Under **Assets**, download `tally-macos-arm64.dmg` for a Mac with an Apple
   chip, or `tally-macos-x86_64.dmg` for an Intel Mac. **About This Mac** shows
   whether your computer has a **Chip** or an Intel **Processor**.
3. Open the downloaded `.dmg`, drag **Tally** to **Applications**, and open Tally.
4. Choose Codex, Claude Code, or both. Paste the Agent API key and select
   **Install Tally**.

The macOS app and disk image are signed by OpenOrigins and notarized by Apple.

### Windows

1. Open the [latest Tally release](https://github.com/OpenOrigins/tally/releases/latest).
2. Under **Assets**, download and open `tally-windows-x86_64.exe`.
3. Choose Codex, Claude Code, or both. Paste the Agent API key and select
   **Install Tally**.

The Windows installer is not yet publisher-signed. SmartScreen may show
**Windows protected your PC**; select **More info**, then **Run anyway**. A
work-managed computer may require approval from its administrator.

### Homebrew (macOS)

Homebrew installs the same signed and notarized `Tally.app` as the DMG:

```sh
brew tap openorigins/tally https://github.com/OpenOrigins/tally
brew install --cask tally
```

Open **Tally** from Applications after installation.

### Linux download

Download `tally-linux-x86_64` from the
[latest release](https://github.com/OpenOrigins/tally/releases/latest), then:

```sh
chmod +x tally-linux-x86_64
./tally-linux-x86_64
```

Tally opens its setup page in your browser. The page only talks to the Tally
process running on your computer.

## After Installation

Tally stores the key and API URL beside each selected client's configuration,
installs its hooks, and asks the dashboard to mark that client as connected.
If confirmation fails, the installer keeps local logging available and shows a
warning. Correct the key and select **Try another key**, or use **Mark connected
manually** in the dashboard.

Open Tally again to update settings or uninstall selected integrations. Advanced
settings allow each client's configuration path and the ingest API URL to be
changed. New installations default to the OpenOrigins Production ingest API;
development endpoints are used only when explicitly entered.

After upgrading from Tally 0.1.3 on macOS, open Tally and select **Install
Tally** once. This replaces the older hook copies with independently signed
helpers while preserving the selected configuration paths.

More help is available in the [account setup guide](SETUP.md). Technical release
verification is documented under [docs](docs/).

## From Source

Install [Rust](https://www.rust-lang.org/tools/install), download this repository,
and run these commands in its folder:

```sh
cargo build --locked --release --package tally
./target/release/tally
```

On Windows, open `target\release\tally.exe` after the build.

Licensed under the [Apache License 2.0](LICENSE).
