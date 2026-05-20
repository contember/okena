# Markdown preview: virtualization (remaining)

- **Severity:** Medium (perf) — downgraded from High after the per-frame/per-pixel waste was removed
- **Type:** perf
- **Area:** `okena-files`
- **Location:** `crates/okena-files/src/file_viewer/render.rs` (preview node list ~1231-1521)

## Done

- **Per-pixel re-layout on selection drag fixed**: all four preview `on_mouse_move`
  handlers (simple node, code-block line, table header, table row) now only assign
  and `cx.notify()` when the selection endpoint actually changes, instead of
  repainting the whole document on every mouse-move event.
- **Per-frame offset re-walk fixed**: each node's cumulative start offset is
  precomputed once at parse time (`MarkdownDocument::node_offsets`) instead of
  re-walking `node_text_length` over the whole document on every
  `render_nodes_with_offsets` call.
- Note: `RenderedNode` holds GPUI `Div`s, which are immediate-mode and cannot be
  cached across frames, so the original "memoize RenderedNodes" idea is infeasible
  as stated — the wins above target the actual re-computation instead.

## Remaining (deferred — behavior-sensitive)

The preview is still a non-virtualized `v_flex` of every node's `AnyElement` inside
an `overflow_y_scroll` div, so a very large markdown file builds all node elements
each frame. `uniform_list` doesn't fit (markdown nodes have variable heights);
virtualizing would require `gpui::list` + a per-tab `ListState` synced with
`markdown_doc`, moving the ~250-line node→element construction (and the four mouse
handlers) into a lazy per-index callback.

The blocker is selection: cross-node drag selection relies on each node's
`on_mouse_move` firing, but virtualized off-screen nodes aren't laid out, so dragging
over scrolled-out nodes would compute the endpoint differently. A virtualized preview
needs a selection model that doesn't depend on every node being laid out.
