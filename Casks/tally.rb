cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.6"
  sha256 arm:   "5b5eb989d40884c55630fce35f1e99520d1a61c0e1be64382cf8d1ef15ca30df",
         intel: "6a7ccb69c0f4aed6cf8290f06ac732da88330326ae5b29dc9a1b06a7d09235a8"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
