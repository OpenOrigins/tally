cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.15"
  sha256 arm:   "d4e2f34ae6d9c5f4032d7baf49220277e617e52043838d4b7c85cc9b4c41df40",
         intel: "70fc4db9a7731d8a5189553564bdcb265ea3d485cf86850225137f4f2cc739b9"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex, Claude Code, and Cursor"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
