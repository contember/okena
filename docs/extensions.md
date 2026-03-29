# Extensions

Okena supports status bar extensions that provide live information widgets.

## Built-in Extensions

### Claude Code Status

Shows the operational status of the Claude Code API.

- **Status indicator:** green (OK), yellow (degraded/partial outage), red (major outage), gray (maintenance)
- **Hover popover:** lists unresolved incidents with impact level and timestamps
- **Click:** opens [status.claude.com](https://status.claude.com)
- **Polling:** every 60 seconds

**Enable in settings:**

```json
{
  "claude_code_integration": true
}
```

When enabled, the extension also shows **Claude API usage**:

- 5-hour and 7-day rate limit usage (percentage bars)
- Per-model usage (Sonnet, Opus tiers)
- Extra usage credits ($spent / $limit)
- Color-coded bars: green (< 60%), yellow (60-80%), red (> 80%)
- Reset time displayed in local timezone

Usage data requires a Claude Code OAuth token (from `~/.claude/.credentials.json`).

### Auto Update

Checks for new Okena releases on GitHub.

- **Checks:** `github.com/contember/okena/releases/latest`
- **Platforms:** Linux (x64, ARM64), macOS (x64, ARM64), Windows (x64, ARM64)
- **Homebrew detection:** if installed via Homebrew, shows `brew upgrade okena` instead of self-update
- **Checksum verification:** validates SHA256 when available

Update flow:

1. "New version available" (click to download)
2. "Downloading v1.2.3... 45%"
3. "Restart to update" (click to restart)

**Configuration:**

```json
{
  "auto_update_enabled": true
}
```

Set to `false` to disable update checks entirely.

## Extension Management

Enable or disable extensions in **Settings > Extensions** (`Cmd+,` / `Ctrl+,`).

Extension widgets appear in the status bar at the bottom of the window.
