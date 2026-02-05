# Homebrew Cask for Term Manager
# This file is automatically updated by CI on release.
# To install: brew tap contember/term-manager && brew install --cask term-manager

cask "term-manager" do
  arch arm: "arm64", intel: "x64"

  version "0.2.0"
  sha256 arm:   "203ffb67bd6891f0f1fef1a82728841d3e403838927204d6bd4e8b0652d3758e",
         intel: "ef2966d2d2148033439b00f3776099876743b6cb698f040ff73605784aad7b5d"

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
