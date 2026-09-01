cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.12"
  sha256 arm:   "620d3bb0f537805d2b04174e402348601fd3c5a630c192203244ca4ffee185f8",
         intel: "1878a92138e22414c877e70e1a75cdb541674822388851390e642dd4f6f8fa5d"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
