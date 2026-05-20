# Diff viewer: horizontal scrollbar char-width mismatch

- **Severity:** High (user-visible bug)
- **Type:** bug
- **Area:** `okena-views-git`
- **Location:** `crates/okena-views-git/src/diff_viewer/scrollbar.rs:82,90` vs `line_render.rs:150` / `render.rs:921`

## Problem

The scrollbar geometry (`panel_gutter_width`, `max_text_width`) hardcodes
`char_width = file_font_size * 0.6`, but the actual diff text is laid out using
`measured_char_width` derived from real font metrics. For any monospace font
whose advance is not exactly `0.6em`, the horizontal scrollbar thumb size and
`max_scroll_x` diverge from the rendered content width — causing over/under-scroll.

## Suggested fix

Route both the scrollbar math and the line rendering through a single
`self.char_width()` based on measured font metrics.
