class Tally < Formula
  desc "Record verifiable Codex and Claude Code activity"
  homepage "https://github.com/OpenOrigins/tally"

  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/OpenOrigins/tally/releases/download/v0.1.2/tally-codex-macos-arm64-cli.tar.gz"
    sha256 "83b69e228c2a94d5b4b4290e67c38b1b55555bde836a594378725fea6404f23f"

    resource "tally-claude" do
      url "https://github.com/OpenOrigins/tally/releases/download/v0.1.2/tally-claude-macos-arm64-cli.tar.gz"
      sha256 "bccaf1ac8a514269b470e0244aec0b5edb1c9e4a2079671c61d861e83a58cca3"
    end
  end
  if OS.mac? && Hardware::CPU.intel?
    url "https://github.com/OpenOrigins/tally/releases/download/v0.1.2/tally-codex-macos-x86_64-cli.tar.gz"
    sha256 "624554f51c40024c95b18df66fbedc602c64918d9646ed7c7ad1b6650111c2f1"

    resource "tally-claude" do
      url "https://github.com/OpenOrigins/tally/releases/download/v0.1.2/tally-claude-macos-x86_64-cli.tar.gz"
      sha256 "91a451b34d18b4afb70ca4987109f493b302ad60db069d898b84dc167a631823"
    end
  end
  if OS.linux? && Hardware::CPU.intel?
    url "https://github.com/OpenOrigins/tally/releases/download/v0.1.2/tally-codex-linux-x86_64", using: :nounzip
    sha256 "1a9174ef07bdbfd6648a2967e6fbc815489957caa5bd3e124d52da1ddfb7f281"

    resource "tally-claude" do
      url "https://github.com/OpenOrigins/tally/releases/download/v0.1.2/tally-claude-linux-x86_64", using: :nounzip
      sha256 "8272e04c0f96b77ca6465f7a9226a7943cc8af7809db7db9646ec09f05da0c2a"
    end
  end

  def install
    codex_source = Dir["tally-codex*"].fetch(0)
    bin.install codex_source => "tally-codex"
    resource("tally-claude").stage do
      claude_source = Dir["tally-claude*"].fetch(0)
      bin.install claude_source => "tally-claude"
    end
    (bin/"tally").write <<~SH
      #!/usr/bin/env bash
      set -euo pipefail

      usage() {
        cat <<'EOF'
      Usage:
        tally codex [COMMAND] [ARGS...]
        tally claude [COMMAND] [ARGS...]

      Run `tally codex` or `tally claude` without a command to open the installer.
      EOF
      }

      launcher_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

      run_client() {
        local client="$1"
        shift
        if [[ "$#" -eq 0 ]]; then
          set -- gui
        fi
        exec "$launcher_dir/tally-$client" "$@"
      }

      choose_client() {
        local has_codex=0
        local has_claude=0
        command -v codex >/dev/null 2>&1 && has_codex=1
        command -v claude >/dev/null 2>&1 && has_claude=1

        if [[ "$has_codex" -eq 1 && "$has_claude" -eq 0 ]]; then
          run_client codex
        fi
        if [[ "$has_claude" -eq 1 && "$has_codex" -eq 0 ]]; then
          run_client claude
        fi
        if [[ ! -t 0 ]]; then
          usage >&2
          exit 2
        fi

        printf 'Set up Tally for:\n  1) Codex\n  2) Claude Code\nChoose 1 or 2: '
        read -r choice
        case "$choice" in
          1 | codex | Codex) run_client codex ;;
          2 | claude | Claude) run_client claude ;;
          *)
            printf 'Unknown choice: %s\n' "$choice" >&2
            exit 2
            ;;
        esac
      }

      case "${1:-}" in
        codex)
          shift
          run_client codex "$@"
          ;;
        claude)
          shift
          run_client claude "$@"
          ;;
        --version | version)
          "$launcher_dir/tally-codex" --version | sed 's/^tally-codex /tally /'
          ;;
        --help | -h | help)
          usage
          ;;
        "")
          choose_client
          ;;
        *)
          printf 'Unknown Tally client: %s\n\n' "$1" >&2
          usage >&2
          exit 2
          ;;
      esac
    SH
  end

  def caveats
    <<~EOS
      Run `tally` to choose Codex or Claude Code and paste your Agent API key.
      You can also start a specific installer with `tally codex` or `tally claude`.
    EOS
  end

  test do
    assert_match "tally #{version}", shell_output("#{bin}/tally --version")
    assert_match "tally-codex #{version}", shell_output("#{bin}/tally-codex --version")
    assert_match "tally-claude #{version}", shell_output("#{bin}/tally-claude --version")
  end
end
