# Homebrew Cask for Term Manager
# This file is automatically updated by CI on release.
# To install: brew tap contember/tap && brew install --cask term-manager

cask "term-manager" do
  arch arm: "arm64", intel: "x64"

  version "0.0.0"
  sha256 arm:   "PLACEHOLDER_ARM64_SHA256",
         intel: "PLACEHOLDER_X64_SHA256"

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
