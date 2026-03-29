# CLI

Okena provides a command-line interface for interacting with a running instance. The CLI communicates with the remote server via the local HTTP API.

## Commands

### `okena`

Start the Okena GUI application.

```bash
okena                              # Normal launch
okena --remote                     # Launch with remote server enabled
okena --listen <address>           # Launch with remote server on specific address
okena --headless --listen <addr>   # Launch in headless mode (no GUI)
```

### `okena health`

Check the health of a running Okena instance.

```bash
okena health           # Tab-separated output
okena health --json    # JSON output
```

Returns status, version, and uptime.

### `okena state`

Print the full workspace state as JSON.

```bash
okena state
```

Returns projects, layouts, terminals, services, and focus state.

### `okena pair`

Generate a pairing code for remote authentication.

```bash
okena pair
```

The code is valid for 60 seconds and single-use. Used by remote clients to obtain a bearer token.

### `okena services`

List all services and their status.

```bash
okena services                   # All services (tab-separated)
okena services "my-project"      # Filter by project name
okena services --json            # JSON output
```

Output columns: project name, service name, status, kind (okena/docker), ports.

### `okena service`

Control an individual service.

```bash
okena service start <name> [project]
okena service stop <name> [project]
okena service restart <name> [project]
okena service start backend --json
```

Waits for the service to reach the target status before returning.

### `okena action`

Execute a raw workspace action.

```bash
okena action '{"action":"run_command","terminal_id":"...","command":"ls -la"}'
```

See [Remote Control API](remote.md) for available actions.

### `okena whoami`

Identify the current terminal and project (must be run inside an Okena terminal).

```bash
okena whoami           # Tab-separated output
okena whoami --json    # JSON output
```

Returns terminal ID, project ID, project name, and project path. Uses the `OKENA_TERMINAL_ID` environment variable.

## Output Formats

- **Default:** tab-separated (grep/awk friendly)
- **`--json` flag:** structured JSON for machine parsing

## Authentication

The CLI auto-registers with the running Okena instance on first use. The token is stored in `~/.config/okena/cli.json` and refreshed automatically.

## Discovery

The CLI finds the running instance via `~/.config/okena/remote.json`, which is written by the server on startup and removed on shutdown.
