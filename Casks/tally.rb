cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.16"
  sha256 arm:   "ed2de166d14e31c6a8aa1f0a5d8e911c449a76a172ae5f6ba8566daf9c244e77",
         intel: "3e1e63d6a143749550ed253cc8ae9e48bfaf36b281872cfb529aadcefca6fc5f"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex, Claude Code, and Cursor"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
