cask "tally" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.5"
  sha256 arm:   "dc2912e9e79187d33234f82c4c41ba73dbeabb0a6834c54faab18a8ae34ba2e3",
         intel: "3444338fcc22abd1e3ba4bddc876516e2ee3e3aa45defdd0110c5f99235c58c3"

  url "https://github.com/OpenOrigins/tally/releases/download/v#{version}/tally-macos-#{arch}.dmg"
  name "Tally"
  desc "Install audit logging for Codex and Claude Code"
  homepage "https://github.com/OpenOrigins/tally"

  depends_on :macos

  app "Tally.app"
end
