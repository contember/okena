# Maintenance backlog

Findings from the 2026-05-20 maintenance review (large files, Rust bad practices,
god classes, concurrency, render-path perf, clippy). One markdown per issue.

## High

- [Diff viewer: horizontal scrollbar char-width mismatch](diff-scrollbar-char-width-mismatch.md) — bug, scrollbar uses `0.6em` vs measured metrics
- [File viewer: blocking filesystem I/O on the render thread](file-viewer-blocking-io-in-render.md) — perf, sync fs in `render()`
- [Markdown preview: full re-render per frame + no virtualization](markdown-preview-rerender-and-virtualization.md) — perf, rebuild every frame
- [Updater orchestration embedded inside render()](updater-orchestration-in-render.md) — refactor, ~120 lines of async logic in `render()`

## Medium

- [Split okena-git/repository.rs (1846-line god module)](split-git-repository-rs.md) — refactor
- [OverlayManager: collapse event-passthrough boilerplate](refactor-overlay-manager-event-passthrough.md) — refactor, 32-variant event enum
- [Extract worktree lifecycle out of actions/project.rs](extract-worktree-actions-from-project.md) — refactor
- [Split execute_action (900-line match, 40+ arms)](split-execute-action-dispatcher.md) — refactor
- [RootView god object + remote-sync logic in the view layer](rootview-god-object-and-remote-sync.md) — refactor
- [PTY kill() spawns an unbounded detached thread per call](pty-kill-thread-per-call.md) — concurrency / resource scaling
- [PtyHandle has no Drop impl](pty-handle-missing-drop.md) — resource safety
- [session_backend kill_session: undocumented unsafe + PID TOCTOU](session-backend-unsafe-kill-toctou.md) — safety
- [Enable clippy in CI + fix 118 existing warnings](enable-clippy-in-ci-and-fix-warnings.md) — hygiene / CI

## Low

- [close_worktree_dialog: swallowed errors on recovery paths](close-worktree-dialog-swallowed-errors.md) — error-handling
- [Git diff parser mishandles renames](git-diff-rename-handling.md) — bug, edge case
- [SyntaxSet cloned per FileViewer instance](syntax-set-cloned-per-viewer.md) — memory
- [cached_file_viewers never evicts on project deletion](cached-file-viewers-no-eviction.md) — memory leak
- [recover_settings_from_json silently drops fields](settings-recover-from-json-incomplete.md) — data safety
- [Blocking save_settings / save_workspace on the main thread](synchronous-save-on-main-thread.md) — perf

## Context

Overall the codebase is in good shape: god-objects were previously decomposed by
composition, error handling in git/auth is disciplined, async work runs off the main
thread, and there is essentially no TODO/FIXME debt. The items above are structural
debt concentrated in four oversized files plus a handful of concrete bugs.
