cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.3"
  sha256 arm:   "01c2e554cffa44e354dd5de12baed48178f9532f9ae5a6d5c2a5f6330e453537",
         intel: "efb8916c730df5fa41bcd0b0b19ad15f9ab46e8d02cc929b8f8dfdd72cf391a0"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
