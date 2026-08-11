class Tally < Formula
  desc "Record verifiable Codex and Claude Code activity"
  homepage "https://github.com/OpenOrigins/tally"
  version "0.1.3"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/OpenOrigins/tally/releases/download/v0.1.0/tally-codex-macos-arm64-cli.tar.gz"
    sha256 "b3486272a778197ad73ae9b8b08c47f0c9ebf2e055f7f263e79095268ece49f0"

    resource "tally-claude" do
      url "https://github.com/OpenOrigins/tally/releases/download/v0.1.0/tally-claude-macos-arm64-cli.tar.gz"
      sha256 "0a5a43cdfd425bf17402cba9d88e79ed9189009f4e7aef6ba19d7d1b4014d597"
    end
  end
  if OS.mac? && Hardware::CPU.intel?
    url "https://github.com/OpenOrigins/tally/releases/download/v0.1.0/tally-codex-macos-x86_64-cli.tar.gz"
    sha256 "5e096d0569f25c20fe1c8cdd89d77f052328d3ef9d216bf0a4ad037692f2e42f"

    resource "tally-claude" do
      url "https://github.com/OpenOrigins/tally/releases/download/v0.1.0/tally-claude-macos-x86_64-cli.tar.gz"
      sha256 "3be9dc7c73675459bd3143a85e4af3e2f049a241ad87a2c046d1e9c7a911b8b6"
    end
  end
  if OS.linux? && Hardware::CPU.intel?
    url "https://github.com/OpenOrigins/tally/releases/download/v0.1.0/tally-codex-linux-x86_64", using: :nounzip
    sha256 "6efbbb2538d215482c27f7fd9c7470d791fbf5fdfd8d86372ef397e4b13d22ff"

    resource "tally-claude" do
      url "https://github.com/OpenOrigins/tally/releases/download/v0.1.0/tally-claude-linux-x86_64", using: :nounzip
      sha256 "5a0bbf69d29102861e39354cac805e0a3297bf7d065fdeedc6122aec93a95f5e"
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
