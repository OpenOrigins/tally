cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.13"
  sha256 arm:   "08536d2166314b2b80934f5ef102a914875a4dbc7a4b932315ed9a85ab4067c7",
         intel: "d9c0f325ca3df0345431de391038687993308910cef465fca5ef76c1786bf338"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
