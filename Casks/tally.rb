cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.10"
  sha256 arm:   "5a183a2037569c1f86abd8ce76b66c7ee30fc025dc2b0a2cc08d9dc0f70285b6",
         intel: "b93f1281ba642bbcb01ea4a76b2786b4eb95b473077582096e3ae589969ca794"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
