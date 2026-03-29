# Terminal Link Highlights

Okena automatically detects and highlights clickable links in terminal output.

## Detected Link Types

### URLs

Any URL with a recognized scheme is detected and highlighted:

`http`, `https`, `ftp`, `file`, `ssh`, `git`, `mailto`, `tel`, `magnet`, `ipfs`, `gemini`, `gopher`, `news`

Examples: `https://example.com`, `ssh://host`, `mailto:user@example.com`

### File Paths

Paths starting with explicit prefixes are detected:

- Absolute paths: `/home/user/file.rs`
- Home-relative: `~/projects/main.py`
- Relative: `./config.json`, `../lib/utils.ts`
- Dotfile directories: `.github/workflows/ci.yml`

File paths support optional line and column suffixes for editor integration:

```
/path/to/file.rs:42
/path/to/file.rs:42:10
```

Path existence is validated before displaying to prevent false positives.

## Interaction

- **Hover** over a link to see an underline and background highlight
- **Click** a link to open it

### URL Opening

URLs open in the system default browser/handler:

- Linux: `xdg-open`
- macOS: `open`
- Windows: `cmd /C start`

### File Path Opening

File paths open in your configured editor with line/column positioning:

| Editor | Command |
|--------|---------|
| VS Code | `code --goto file:line:col` |
| Cursor | `cursor --goto file:line:col` |
| Zed | `zed file:line:col` |
| Sublime Text | `subl file:line:col` |
| Vim / Neovim | `vim +line file` |

Configure your editor in settings:

```json
{
  "file_opener": "code"
}
```

If `file_opener` is empty, the system default handler is used.

## Wrapped Lines

Okena handles URLs that span multiple terminal rows (soft-wrapped lines). Multi-segment links are grouped so the entire URL highlights as one unit on hover.
