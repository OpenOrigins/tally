cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.14"
  sha256 arm:   "5968bc01ae6b51f05da1e99c389a111b76c999911d8f3bc8f2b19971e4eedb6c",
         intel: "2952b2defac82bf9b63231e60e821b97863f5de3525dc71ed546e7ecfeac278e"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
