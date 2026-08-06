# Tally

Tally records verifiable Codex and Claude Code agent activity as structured audit logs.

## Install

Download `tally-codex` or `tally-claude` for your operating system from
[Releases](https://github.com/OpenOrigins/tally/releases), make it executable on
macOS/Linux, and run it. The executable installs or updates its hooks automatically.

To remove the hooks, run the executable with `uninstall`.

## From Source

Install [Rust](https://www.rust-lang.org/tools/install), then run:

```sh
cargo build --locked --release --workspace
./target/release/tally-codex
./target/release/tally-claude
```

Windows binaries end in `.exe`. The [specification and examples](docs/) are in `docs/`.
