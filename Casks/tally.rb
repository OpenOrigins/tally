cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.11"
  sha256 arm:   "1ac260713cfae34acc0fac59e923964e5da11199edb41c04afe681bd1108e41d",
         intel: "6c8998a548b0c2caef8e821d4948e1c8d1bb4d10dac886986c296371faa62072"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
