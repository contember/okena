# Homebrew Cask for Term Manager
# This file is automatically updated by CI on release.
# To install: brew tap contember/term-manager && brew install --cask term-manager

cask "term-manager" do
  arch arm: "arm64", intel: "x64"

  version "0.1.5"
  sha256 arm:   "513dc61ac995f2bea474e21e67804bd246773f960178aa3e023194ca4c7fbe1a",
         intel: "a93741320e63c56568af95d3126a3c0c69fbeeb2ad26640098be4b37cdd4aba6"

  url "https://github.com/contember/term-manager/releases/download/v#{version}/term-manager-macos-#{arch}.zip"
  name "Term Manager"
  desc "Terminal multiplexer for managing multiple terminal sessions"
  homepage "https://github.com/contember/term-manager"

  livecheck do
    url :url
    strategy :github_latest
  end

  app "Term Manager.app"

  zap trash: [
    "~/.config/term-manager",
    "~/Library/Application Support/term-manager",
    "~/Library/Caches/term-manager",
    "~/Library/Preferences/com.contember.term-manager.plist",
  ]
end
