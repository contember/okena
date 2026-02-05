---
description: Prepare and publish a new release
allowed-tools: Bash(git:*), Read, Edit
---

## Release Process

1. **Find the last release tag and show changes since then:**

!git describe --tags --abbrev=0

!git log $(git describe --tags --abbrev=0)..HEAD --oneline

2. **Analyze the changes above and determine the version bump type:**
   - **MAJOR** (x.0.0): Breaking changes, incompatible API changes
   - **MINOR** (0.x.0): New features, functionality additions (backwards compatible)
   - **PATCH** (0.0.x): Bug fixes, small improvements, refactoring

3. **Read current version from Cargo.toml** and calculate the new version.

4. **Update version in Cargo.toml**.

5. **Commit, push, and create the tag:**
   ```
   git add Cargo.toml
   git commit -m "chore: bump version to <new-version>"
   git push
   git tag v<new-version>
   git push origin v<new-version>
   ```

6. **Report success** with link to GitHub Actions where the build is running.

## Notes

- CI pipeline automatically builds for Linux, macOS (ARM64/Intel), and Windows
- GitHub Release is created with auto-generated release notes
- Homebrew cask is automatically updated with new SHA256 hashes
