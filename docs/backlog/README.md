# Maintenance backlog

Findings from the 2026-05-20 maintenance review (large files, Rust bad practices,
god classes, concurrency, render-path perf, clippy). One markdown per issue.

The first maintenance sprint resolved 10 of the original items (diff scrollbar
char-width, file-viewer render-thread I/O, updater orchestration, the workspace
clippy gate, PtyHandle Drop, dtach kill SAFETY/TOCTOU docs, shared SyntaxSet,
cached-file-viewer eviction, swallowed save error, and worktree stash_pop
recovery toasts). The items below remain.

## High

- [Markdown preview: full re-render per frame + no virtualization](markdown-preview-rerender-and-virtualization.md) — perf, rebuild every frame

## Medium

- [OverlayManager: collapse event-passthrough boilerplate](refactor-overlay-manager-event-passthrough.md) — refactor, 32-variant event enum
- [Extract worktree lifecycle out of actions/project.rs](extract-worktree-actions-from-project.md) — refactor
- [Split execute_action (900-line match, 40+ arms)](split-execute-action-dispatcher.md) — refactor
- [RootView god object + remote-sync logic in the view layer](rootview-god-object-and-remote-sync.md) — refactor

## Low


## Context

Overall the codebase is in good shape: god-objects were previously decomposed by
composition, error handling in git/auth is disciplined, async work runs off the main
thread, and there is essentially no TODO/FIXME debt. The remaining items are
structural debt concentrated in four oversized files plus a couple of concrete bugs.
