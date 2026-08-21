cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.8"
  sha256 arm:   "aea1b62b211f9440da985f6af609b5f47a0bd70cfd9ffde2afbf0898cc3abab7",
         intel: "a1de8d462badd0dd15ced6042ec01fe14722f4739d320f5c6da63e8653bf26cd"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
