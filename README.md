# Tally

Tally records verifiable Codex and Claude Code agent activity as structured audit logs.

## Install

Generate an Agent API key in the OpenOrigins dashboard, then download
`tally-codex` or `tally-claude` for your operating system from
[Releases](https://github.com/OpenOrigins/tally/releases). Run the executable for
an interactive key prompt, or install directly:

```sh
tally-codex install --api-key <agent-api-key>
tally-claude install --api-key <agent-api-key>
```

Add `--api-url <url>` only when using a custom ingest endpoint. On macOS/Linux,
make the downloaded file executable first with `chmod +x <file>`.

To remove the hooks, run the executable with `uninstall`.

## From Source

Install [Rust](https://www.rust-lang.org/tools/install), then run:

```sh
cargo build --locked --release --workspace
./target/release/tally-codex install --api-key <agent-api-key>
./target/release/tally-claude install --api-key <agent-api-key>
```

Windows binaries end in `.exe`. The [specification and examples](docs/) are in `docs/`.
