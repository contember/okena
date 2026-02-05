# Homebrew Cask for Term Manager
# This file is automatically updated by CI on release.
# To install: brew tap contember/term-manager && brew install --cask term-manager

cask "term-manager" do
  arch arm: "arm64", intel: "x64"

  version "0.3.0"
  sha256 arm:   "4cdf398bfb815fef343cd157c240deae4a3ef778cc0bbb31f9c2020ca0399d7b",
         intel: "37efb512c95b54d0a6048b170af4d43b01955ad82c7853b1a59a7bbe418173be"

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
