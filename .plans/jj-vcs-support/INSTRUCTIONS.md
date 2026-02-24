# Add Jujutsu (jj) VCS Support

> Source: (conversation — plan mode output)

## Context

Okena currently has Git-only VCS integration (status, diffs, worktrees, branches). The goal is to add Jujutsu support alongside Git, sharing as much code as possible, with jj preferred when both are available in a project.

The current Git integration is in `src/git/` (4 files) and uses subprocess calls to the `git` binary.

## Current Architecture

### Module: `src/git/mod.rs` (197 lines)

Public types:
- `GitStatus { branch: Option<String>, lines_added: usize, lines_removed: usize }` — with `has_changes()` method
- `FileDiffSummary { path: String, added: usize, removed: usize, is_new: bool }`
- Global 5-second cache: `CACHE: Mutex<Option<HashMap<PathBuf, CacheEntry>>>`

Public functions:
- `get_git_status(path: &Path) -> Option<GitStatus>` — cached, delegates to `repository::get_status`
- `invalidate_cache(path: &Path)` — `#[allow(dead_code)]`
- `get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary>` — `git diff --numstat HEAD` + `git ls-files --others`

### Module: `src/git/diff.rs` (731 lines)

Re-exports: `DiffMode` from `okena_core::types::DiffMode` (line 79)

Key types: `DiffLineType`, `DiffLine`, `DiffHunk`, `FileDiff`, `DiffResult`

Public functions:
- `get_diff_with_options(path, mode, ignore_whitespace) -> Result<DiffResult, String>` — `git diff` / `git diff --cached`
- `is_git_repo(path) -> bool` — `git rev-parse --is-inside-work-tree`
- `get_file_contents_for_diff(repo_path, file_path, mode) -> (Option<String>, Option<String>)` — `git show` for old, filesystem for new
- `parse_unified_diff(output: &str) -> DiffResult` — **pure parser, VCS-agnostic, reusable directly**

### Module: `src/git/repository.rs` (587 lines)

All git-specific operations (worktree, branch, merge, stash, rebase, fetch, push). These stay git-only.

### Module: `src/git/watcher.rs` (104 lines)

`GitStatusWatcher` — polls `git::get_git_status()` every 5 seconds for all visible non-remote projects.

## Call Sites to Update

| File | Line(s) | Current Call | New Call |
|------|---------|-------------|---------|
| `src/git/watcher.rs` | 67 | `git::get_git_status(Path::new(&path))` | `vcs::get_vcs_status(Path::new(&path))` |
| `src/views/overlays/diff_viewer/mod.rs` | 12 | `use crate::git::{get_diff_with_options, is_git_repo, ...}` | `use crate::vcs` + `use crate::git::{DiffMode, DiffResult, FileDiff}` |
| `src/views/overlays/diff_viewer/mod.rs` | 131 | `is_git_repo(path)` | `vcs::is_vcs_repo(path)` |
| `src/views/overlays/diff_viewer/mod.rs` | 132 | `"Not a git repository"` | `"Not a version-controlled repository"` |
| `src/views/overlays/diff_viewer/mod.rs` | 173 | `get_diff_with_options(path, mode, ...)` | `vcs::get_diff_with_options(path, mode, ...)` |
| `src/views/overlays/diff_viewer/syntax.rs` | 4 | `use crate::git::{get_file_contents_for_diff, ...}` | `use crate::vcs` |
| `src/views/overlays/diff_viewer/syntax.rs` | 69 | `get_file_contents_for_diff(...)` | `vcs::get_file_contents_for_diff(...)` |
| `src/views/panels/project_column.rs` | 1 | `use crate::git::{self, FileDiffSummary}` | `use crate::vcs; use crate::git::FileDiffSummary;` |
| `src/views/panels/project_column.rs` | 108 | `git::get_diff_file_summary(Path::new(&project_path))` | `vcs::get_diff_file_summary(Path::new(&project_path))` |
| `src/views/overlays/context_menu.rs` | 128 | `git::get_git_status(path).is_some()` | `vcs::get_vcs_status(path).is_some()` for general VCS; gate worktrees on `vcs::detect_vcs(path) == Some(Git)` |
| `src/views/overlays/context_menu.rs` | 167,179 | `is_git_repo` gating | Gate worktree items on `is_git_repo` (Git backend only) |
| `src/views/root/terminal_actions.rs` | 119 | `crate::git::get_git_status(path).is_some()` | `crate::vcs::detect_vcs(path) == Some(VcsBackend::Git)` for worktree gating |
| `src/workspace/actions/execute.rs` | 252,262,272,294 | `crate::git::get_git_status/get_diff_*` | `crate::vcs::` equivalents; keep `GitBranches` as git-only |

## Approach: Thin VCS Dispatch Layer

Create `src/vcs.rs` dispatch module + `src/jj/` backend module. Keep `src/git/` unchanged. Swap call sites from `crate::git::` to `crate::vcs::` where VCS-agnostic behavior is needed.

### Key Design Decisions

1. **jj preferred over git** when both `.jj/` and `.git/` exist (common with colocated repos)
2. **`parse_unified_diff` reused directly** — `jj diff --git` produces standard unified diff format
3. **No staging area in jj** — `DiffMode::Staged` and `DiffMode::WorkingTree` both show working copy vs parent
4. **jj status** — bookmark via `jj log -r @ --no-graph -T 'separate(" ", bookmarks)'`, fallback to change ID short
5. **jj file contents** — old from `jj file show --revision @- <file>`, new from filesystem
6. **Git-only features stay untouched** — worktrees, stash, rebase, merge, branches, push
7. **Separate cache for jj** in `src/vcs.rs` (5-second TTL, same pattern as git)

## Testing Strategy

- Unit tests for `is_jj_repo` with non-existent/non-jj paths
- Unit tests for `detect_vcs` and `is_vcs_repo` with non-repo paths
- Test that sample `jj diff --git` output parses correctly through `parse_unified_diff`
- All existing git tests pass unchanged
- `cargo build` compiles without errors
- `cargo test` passes all tests
