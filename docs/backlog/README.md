# Maintenance backlog

Findings from the 2026-05-20 maintenance review (large files, Rust bad practices,
god classes, concurrency, render-path perf, clippy). One markdown per issue.

The first maintenance sprint resolved 10 of the original items (diff scrollbar
char-width, file-viewer render-thread I/O, updater orchestration, the workspace
clippy gate, PtyHandle Drop, dtach kill SAFETY/TOCTOU docs, shared SyntaxSet,
cached-file-viewer eviction, swallowed save error, and worktree stash_pop
recovery toasts). The items below remain.

## High

- [Markdown preview: virtualization (remaining)](markdown-preview-rerender-and-virtualization.md) — perf, per-frame/per-pixel waste fixed

## Medium

- [RootView god object — remaining column/scroll extraction](rootview-god-object-and-remote-sync.md) — refactor, remote-sync extracted

## Low


## Context

Overall the codebase is in good shape: god-objects were previously decomposed by
composition, error handling in git/auth is disciplined, async work runs off the main
thread, and there is essentially no TODO/FIXME debt. The remaining items are
structural debt concentrated in four oversized files plus a couple of concrete bugs.
