# cached_file_viewers never evicts on project deletion

- **Severity:** Low (memory leak)
- **Type:** bug
- **Area:** `src/` (desktop app)
- **Location:** `src/views/overlay_manager.rs:209`, `1312-1369`, `1418`

## Problem

The `cached_file_viewers` HashMap entries are only dropped on detach (line 1418);
closing a project leaves its `FileViewer` (and its `ProjectFs` / blame provider
`Arc`) cached indefinitely. Slow memory growth over a long session with many
opened/closed projects.

## Suggested fix

Prune the cache when a project is removed (observe workspace, or key the cache off
the set of visible projects).
