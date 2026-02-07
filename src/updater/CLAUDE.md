# updater/ — Self-Update System

Background update checking, downloading, and in-place binary replacement.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `UpdateStatus` enum, `UpdateInfo` struct. Cancel token pattern for clean shutdown. Homebrew detection (skips self-update if installed via brew). |
| `checker.rs` | GitHub Releases API check — fetches latest release, semver comparison against current version, selects platform-appropriate asset. |
| `downloader.rs` | Download with progress reporting. SHA256 verification of downloaded artifact. Cancellation support via token. |
| `installer.rs` | Binary replacement — rename-based swap (old → backup, new → current). Restart trigger. Cleanup of backup on next launch. |

## Key Patterns

- **Cancel token**: Shared `Arc<AtomicBool>` token allows the update check/download to be cancelled cleanly from the UI.
- **Rename-based install**: Binary replacement uses rename (not overwrite) to avoid corrupting the running executable. The old binary is kept as backup.
- **Homebrew skip**: If Homebrew is detected as the install method, self-update is disabled in favor of `brew upgrade`.
