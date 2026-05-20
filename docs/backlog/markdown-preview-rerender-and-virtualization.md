# Markdown preview: full re-render per frame + no virtualization

- **Severity:** High (perf)
- **Type:** perf
- **Area:** `okena-files`
- **Location:** `crates/okena-files/src/file_viewer/render.rs:790-801`, `1227-1521`

## Problem

In preview mode `render()` unconditionally calls `doc.render_nodes_with_offsets()`,
rebuilding every node into `Div`s and recomputing text-length offsets for the entire
document. Each selection-drag `on_mouse_move` fires `cx.notify()` (render.rs:1273,
1341, 1410, 1452), so a large markdown file is fully re-laid-out on every mouse-move
pixel. The preview is also not virtualized (unlike the source view, which uses
`uniform_list`), so big docs build thousands of elements per frame.

## Suggested fix

- Memoize `RenderedNode`s keyed by `(selection, theme, font_size)`.
- Precompute cumulative node offsets once at parse time instead of per render.
- Virtualize the preview node list, or at minimum gate rebuild behind a cache.
