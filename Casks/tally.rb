cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.4"
  sha256 arm:   "9d28cd8133d33e55ab4303f17c32db9dfe50e704f023b4f1bcec061104d0c050",
         intel: "488f376e266e3456abd50f88e6bc666ae75b0287dc579c09843e85c4fbfa35b7"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
