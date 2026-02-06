# Homebrew Cask for Term Manager
# This file is automatically updated by CI on release.
# To install: brew tap contember/term-manager && brew install --cask term-manager

cask "term-manager" do
  arch arm: "arm64", intel: "x64"

  version "0.4.1"
  sha256 arm:   "54a370425096aec869e07a3ebd6001207904ea5b031946c35424c7b2eb6e0870",
         intel: "49298159dd5caf8fcad4e5d02823966185c9558bb38bb17f8f3c91cff962c112"

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
