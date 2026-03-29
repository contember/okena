# Git Diff Viewer

Okena includes a built-in diff viewer for inspecting changes in your repository.

## Opening the Diff Viewer

The diff viewer opens from:

- The **git header** in a project column (click the branch/changes indicator)
- The **commit log** (select a commit to view its diff)
- The **command palette**

## Diff Modes

| Mode | Description |
|------|-------------|
| **Working Tree** | Unstaged changes (index vs. working directory) |
| **Staged** | Staged changes (index vs. HEAD) |
| **Commit** | Changes in a specific commit |
| **Branch Compare** | Three-dot diff between two branches |

Toggle between Working Tree and Staged with **Tab**.

If one mode is empty, the viewer automatically falls back to the other.

## View Modes

| Mode | Description |
|------|-------------|
| **Unified** | Traditional diff format with added/removed lines interleaved |
| **Side-by-Side** | Old file on the left, new file on the right |

Toggle with **S** key. The preference is persisted across sessions.

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Tab | Toggle Working Tree / Staged |
| S | Toggle Unified / Side-by-Side |
| W | Toggle ignore whitespace |
| Up / Down | Previous / next file |
| Left / Right | Scroll horizontally (40px per press) |
| [ / ] | Previous / next commit (when viewing commit list) |
| Cmd+C / Ctrl+C | Copy selected text |
| Escape | Close diff viewer |

## Features

- **Syntax highlighting** matching each file's language
- **File tree sidebar** with collapsible folders showing per-file stats (+/- line counts)
- **Horizontal scrolling** for wide lines
- **Selection and copy** support
- **Whitespace toggle** -- hide whitespace-only changes with **W**

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `diff_view_mode` | `"Unified"` | Default view mode: `"Unified"` or `"SideBySide"` |
| `diff_ignore_whitespace` | `false` | Ignore whitespace changes by default |
| `file_font_size` | `12.0` | Font size in the diff viewer |
