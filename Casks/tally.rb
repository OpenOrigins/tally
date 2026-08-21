cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.7"
  sha256 arm:   "194b9a4f4e9c46580b144e8f9e8af81feb6fc7898170116df029ed275b13ee5e",
         intel: "51c8e4e70406866c139a6ddceb31c89c7f915e67f1086bd52ad287cb207837b7"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
