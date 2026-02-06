# Homebrew Cask for Okena
# This file is automatically updated by CI on release.
# To install: brew tap contember/okena && brew install --cask okena

cask "okena" do
  arch arm: "arm64", intel: "x64"

  version "0.4.1"
  sha256 arm:   "54a370425096aec869e07a3ebd6001207904ea5b031946c35424c7b2eb6e0870",
         intel: "49298159dd5caf8fcad4e5d02823966185c9558bb38bb17f8f3c91cff962c112"

  url "https://github.com/contember/okena/releases/download/v#{version}/okena-macos-#{arch}.zip"
  name "Okena"
  desc "Terminal multiplexer for managing multiple terminal sessions"
  homepage "https://github.com/contember/okena"

  livecheck do
    url :url
    strategy :github_latest
  end

  app "Okena.app"

  zap trash: [
    "~/.config/okena",
    "~/Library/Application Support/okena",
    "~/Library/Caches/okena",
    "~/Library/Preferences/com.contember.okena.plist",
  ]
end
