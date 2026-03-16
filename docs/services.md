# Project Services

Okena can manage background services (dev servers, databases, watchers) alongside your terminals. Services are defined per-project in an `okena.yaml` file and/or auto-detected from Docker Compose.

## Overview

There are two kinds of services:

- **Okena services** -- shell commands that Okena spawns and manages in PTY processes. Defined in `okena.yaml`.
- **Docker Compose services** -- containers managed by Docker Compose. Auto-detected from compose files or configured in `okena.yaml`.

Both kinds appear in the sidebar under a "Services" group for each project, showing live status, detected ports, and controls for start/stop/restart.

## okena.yaml Configuration

Place an `okena.yaml` file in your project root. It has two top-level keys:

```yaml
services:
  - name: "Service Name"
    command: "npm run dev"
    cwd: "frontend"              # Relative to project root (default: ".")
    env:                         # Environment variables (default: none)
      NODE_ENV: development
      PORT: "3000"
    auto_start: true             # Start when project loads (default: false)
    restart_on_crash: true       # Auto-restart on non-zero exit (default: false)
    restart_delay_ms: 2000       # Delay before restart in ms (default: 1000)

docker_compose:                  # Optional, see below
  file: "docker-compose.yml"
  enabled: true
  services:
    - web
    - db
```

### Service Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | *required* | Display name shown in the sidebar |
| `command` | string | *required* | Shell command to run |
| `cwd` | string | `"."` | Working directory, relative to the project root |
| `env` | map | `{}` | Environment variables passed to the process |
| `auto_start` | bool | `false` | Automatically start when the project is opened |
| `restart_on_crash` | bool | `false` | Restart the service if it exits with a non-zero code |
| `restart_delay_ms` | int | `1000` | Milliseconds to wait before restarting after a crash |

## Docker Compose Integration

Okena detects and integrates Docker Compose services automatically.

### Auto-Detection

When a project is opened, Okena checks for compose files in this order:

1. `docker-compose.yml`
2. `docker-compose.yaml`
3. `compose.yml`
4. `compose.yaml`

If one is found and the `docker compose` CLI is available, Okena lists the services defined in it. Services with `deploy.replicas: 0` are excluded.

### Configuration

Use the `docker_compose` section in `okena.yaml` to customize behavior:

```yaml
docker_compose:
  file: "docker-compose.prod.yml"   # Explicit compose file (overrides auto-detect)
  enabled: false                     # Set to false to disable integration entirely
  services:                          # Filter to specific services (default: all)
    - web
    - db
```

- **`file`** -- Path to the compose file, relative to the project root. If omitted, Okena auto-detects.
- **`enabled`** -- Explicitly enable or disable Docker Compose integration. If omitted, integration is enabled when a compose file is found.
- **`services`** -- A list of service names to highlight. Services not in this list are still shown but marked as "extra" and grouped separately.

Docker Compose integration works even without an `okena.yaml` file -- Okena will auto-detect compose files in any project.

### Status Polling

Okena polls Docker service statuses every 5 seconds using `docker compose ps`. This updates each service's status and detected ports in the sidebar without manual refresh.

### Docker Actions

- **Start/Stop/Restart** -- Runs `docker compose start|stop|restart <service>`.
- **View Logs** -- Opens a PTY running `docker compose logs -f --tail 200 <service>`. This log viewer is ephemeral and does not persist across restarts.

## Service Lifecycle

### Status States

| Status | Description |
|--------|-------------|
| **Stopped** | Not running. For Docker: exited with code 0 or not yet started. |
| **Starting** | Spawn/start command issued, waiting for the process to initialize. |
| **Running** | Process is alive and active. |
| **Crashed** | Exited with a non-zero code (or Docker state `dead`/`exited` with error). Shows the exit code when available. |
| **Restarting** | Waiting to restart after a crash or manual restart. |

### Auto-Restart Behavior

When `restart_on_crash: true` is set for an Okena service:

1. The service exits with a non-zero code.
2. The old terminal is cleaned up.
3. The status changes to **Restarting**.
4. After `restart_delay_ms` milliseconds, Okena spawns a new process.
5. The restart counter increments.

Auto-restart stops after **5 consecutive crashes** (the max retry limit). At that point the service enters the **Crashed** state and the terminal output is preserved so you can inspect what went wrong.

A manual restart (from the sidebar) resets the restart counter to zero.

### Session Persistence

Okena services can reconnect to existing sessions across app restarts (when using a session backend like tmux). The terminal ID for each service is persisted in the workspace file. Docker log viewer PTYs are ephemeral and not persisted.

## Service Panel

Services appear in the sidebar under each project. The **Services** group header shows:

- **Start All** -- Start every service in the project.
- **Stop All** -- Stop every service.
- **Reload** -- Re-read `okena.yaml` and update services. New services are added, removed services are stopped, and unchanged running services keep running.

Each service row shows:

- A status indicator (color-coded by state)
- The service name
- Detected ports (if any)
- Start, stop, or restart buttons on hover

Clicking a running Okena service shows its terminal output. Clicking a Docker service opens its log viewer.

## Port Detection

Okena automatically detects TCP ports that a running service is listening on.

### How It Works

1. After a service starts, Okena waits 2 seconds for the process to bind its port.
2. It walks the process tree from the service's root PID to find all child processes.
3. It checks for listening TCP ports owned by those processes.
4. Polling repeats every 3 seconds, up to 10 times, to catch late-binding ports.
5. Once ports stabilize for 2 consecutive polls, detection stops.

### Platform Methods

| Platform | PID Discovery | Port Discovery |
|----------|--------------|----------------|
| Linux | `/proc` filesystem | `ss -tlnp` |
| macOS | `pgrep -P` | `lsof -iTCP -sTCP:LISTEN` |
| Windows | `wmic process` | `netstat -ano` |

### Filtering

- Ephemeral ports (>= 32768) are excluded.
- The Node.js debug port (9229) is excluded.
- Duplicate ports are deduplicated.

For Docker services, ports are read directly from the Docker API (`Publishers` field) rather than using OS-level detection.

## Examples

### Node.js Frontend + API Server

```yaml
services:
  - name: "Frontend"
    command: "npm run dev"
    cwd: "frontend"
    env:
      PORT: "3000"
    auto_start: true
    restart_on_crash: true

  - name: "API Server"
    command: "npm run start:dev"
    cwd: "backend"
    env:
      DATABASE_URL: "postgres://localhost:5432/myapp"
      NODE_ENV: development
    auto_start: true
    restart_on_crash: true
    restart_delay_ms: 2000
```

### Full Stack with Docker Compose

```yaml
services:
  - name: "Vite Dev"
    command: "npm run dev"
    auto_start: true
    restart_on_crash: true

docker_compose:
  services:
    - postgres
    - redis
```

This starts the Vite dev server as an Okena service and monitors `postgres` and `redis` containers from Docker Compose. Any other services in the compose file will appear in a separate "Other" group.

### Rust Project with Cargo Watch

```yaml
services:
  - name: "Cargo Watch"
    command: "cargo watch -x run"
    auto_start: true
    restart_on_crash: true
    restart_delay_ms: 3000

  - name: "Tailwind CSS"
    command: "npx tailwindcss -i input.css -o output.css --watch"
    cwd: "assets"
    auto_start: true
```

### Docker Compose Only (No okena.yaml Needed)

If your project has a `docker-compose.yml` and no `okena.yaml`, Okena will still auto-detect the compose file and show all services in the sidebar. No configuration needed.

To customize which Docker services are highlighted or to use a non-standard compose file path, add an `okena.yaml` with just the `docker_compose` section:

```yaml
services: []
docker_compose:
  file: "infra/docker-compose.dev.yml"
  services:
    - api
    - db
```
