# Refactoring Issues

## High Priority

### ~~1. Extract generic filterable list overlay component~~ ✅ DONE
**Files:** `command_palette.rs`, `theme_selector.rs`, `file_search.rs`, `project_switcher.rs`, `shell_selector_overlay.rs`
**Solution:** Created `list_overlay.rs` with `ListOverlayConfig`, `ListOverlayState<T, M>`, `substring_filter()`. All overlays refactored to use shared infrastructure.

### ~~2. Extract keyboard navigation handler for list overlays~~ ✅ DONE
**Files:** `command_palette.rs`, `theme_selector.rs`, `file_search.rs`, `project_switcher.rs`, `shell_selector_overlay.rs`
**Solution:** Created `handle_list_overlay_key()` function with support for custom extra keys (e.g., Space for toggle in ProjectSwitcher).

### 3. Extract context menu item helper
**Files:** `overlays/context_menu.rs`, `layout/tabs/context_menu.rs`
**Similarity:** ~80%
**Description:** Menu items use identical styling: `div().px(12).py(6).flex().items_center().gap(8).cursor_pointer().text_size(12).text_color(...).hover(...).child(svg).child(label).on_click(...)`. Extract `menu_item(id, icon, label, theme, on_click)` helper. Each file has 3+ near-identical items.

## Medium Priority

### 4. Deduplicate shell selector display logic
**Files:** `terminal_pane/shell_selector.rs`, `tabs/shell_selector.rs`
**Similarity:** ~95%
**Description:** Both files implement identical `get_display_name()` matching on `ShellType` and nearly identical rendering (indicator chip with shell name + chevron icon). Move `get_display_name()` to `ShellType` impl in `shell_config.rs` and extract shared rendering function.

### 5. Extract standard button component
**Files:** 20+ occurrences across overlays and panels
**Description:** Recurring pattern: `div().cursor_pointer().px().py().rounded(4).bg(bg_secondary).hover(bg_hover).text_size().text_color().child(label).on_click(...)`. Text sizes vary (11-13px), padding varies slightly. Extract `button(id, label, theme, on_click)` helper to `ui_helpers.rs`.

### 6. Extract modal close trait
**Files:** All 6 overlay files
**Similarity:** 100%
**Description:** Every overlay defines `fn close(&self, cx) { cx.emit(Event::Close); }` with its own event enum containing a `Close` variant. Minor duplication but could use a shared `Closeable` trait or macro.

### ~~7. Deduplicate scroll-to-selected logic~~ ✅ DONE
**Files:** `command_palette.rs`, `file_search.rs`, `project_switcher.rs`
**Solution:** Now part of `ListOverlayState::scroll_to_selected()` method.

### 8. Deduplicate workspace action path-based mutation pattern
**Files:** `workspace/actions/terminal.rs`, `workspace/actions/layout.rs`
**Similarity:** ~70%
**Description:** Multiple methods follow the same pattern: take `project_id` + `path`, call `self.with_layout_node(...)`, match on `LayoutNode::Terminal { field, .. }`, mutate field, return bool. Could use a helper or macro for the common wrapper.

## Low Priority

### 9. Standardize UI spacing/sizing constants
**Files:** All UI overlay and component files
**Description:** Repeated magic numbers for padding (6, 8, 12), text sizes (10, 11, 12, 13), border radius (4), gap (4, 8). Consider defining named constants or a small design token module for consistency.

### 10. Deduplicate input field styling
**Files:** `sidebar/add_dialog.rs`, `overlays/worktree_dialog.rs`
**Similarity:** ~75%
**Description:** Both implement name/path input fields with labels, styled containers, and action buttons (Browse, Quick-add). Extract shared input field component.
