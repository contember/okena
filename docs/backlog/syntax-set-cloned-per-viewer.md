# SyntaxSet cloned per FileViewer instance

- **Severity:** Low (memory)
- **Type:** perf
- **Area:** `okena-files`
- **Location:** `crates/okena-files/src/file_viewer/syntax.rs:24-28`, `mod.rs:238`

## Problem

`load_syntax_set()` returns `SYNTAX_SET.get_or_init(...).clone()` and each
`FileViewer` stores its own `syntax_set: SyntaxSet` clone (cloned in
`new`/`new_browse`). `SyntaxSet` is large; cloning it per viewer wastes memory.

## Suggested fix

Store `&'static SyntaxSet` (return a reference from the `OnceLock`) or an
`Arc<SyntaxSet>`.
