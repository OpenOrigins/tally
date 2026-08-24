cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.9"
  sha256 arm:   "254c9219613dc3860b7c505bde9b1c5c0b760b2ad0cb7f56d9b57c81a3ff1df5",
         intel: "c82032e10647d598dab35673835d529f9b7ea39d3a1eb7e673654f6d952dfb78"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
