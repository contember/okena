# File Viewer and Search

Okena includes a built-in file search, content search, and file viewer with syntax highlighting.

## File Search

**Shortcut:** `Cmd+P` / `Ctrl+P`

Quickly open files in the current project using fuzzy matching.

- Type to filter files by name or path
- Results are ranked by relevance (exact matches, filename matches, and shorter paths score higher)
- Up/Down arrows to navigate, Enter to open, Esc to close
- Respects `.gitignore` and ignores common directories (`node_modules/`, `.git/`, `target/`, etc.)
- Scans up to 10,000 files per project

The search restores your last query and selection when reopened.

## Content Search (Find in Files)

**Shortcut:** `Cmd+Shift+F` / `Ctrl+Shift+F`

Search across all files in a project for text patterns.

### Search Modes

- **Literal** (default) -- plain text matching
- **Regex** -- full regular expression support
- **Fuzzy** -- approximate matching with relevance scoring

### Options

- **Case sensitivity** toggle
- **File glob filter** -- restrict search to specific file patterns (e.g., `*.rs`, `*.ts`)
- **Context lines** -- configurable lines of context before/after each match

### Results

- Grouped by file with match counts
- Syntax-highlighted previews
- Click a result to open the file at that line
- Maximum 1,000 results

## File Viewer

The file viewer opens when you select a file from file search or content search.

### Features

- **Syntax highlighting** for 100+ languages (Rust, TypeScript, Python, Go, Java, etc.)
- **Line numbers** with dynamic width
- **Tabs** for multiple open files (max 20)
- **Markdown preview** -- `.md` files automatically render as formatted markdown; press Tab to toggle between preview and source
- **Sidebar file tree** -- browse the project's file structure
- **Selection and copy** -- click and drag to select, `Cmd+C` / `Ctrl+C` to copy
- **Back/forward navigation** -- `Alt+Left` / `Alt+Right` to navigate history

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Tab | Toggle markdown preview/source (markdown files only) |
| Ctrl+Tab / Ctrl+Shift+Tab | Next/previous tab |
| b | Toggle sidebar |
| Cmd+C / Ctrl+C | Copy selection |
| Cmd+A / Ctrl+A | Select all |
| Cmd+W / Ctrl+W | Close current tab |
| Alt+Left / Alt+Right | Back/forward navigation |
| Escape | Close viewer |

### Limits

- Maximum file size: 5 MB
- Maximum displayed lines: 10,000
- Binary files are detected and show an error message

### Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `file_font_size` | `12.0` | Font size in the file viewer (8.0-24.0) |
| `file_opener` | `""` | Editor command for opening files externally (e.g., `"code"`, `"cursor"`, `"zed"`, `"vim"`) |
