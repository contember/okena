# Refactoring Issues

## High Priority

### ~~1. Extract generic filterable list overlay component~~ ✅ DONE
**Files:** `command_palette.rs`, `theme_selector.rs`, `file_search.rs`, `project_switcher.rs`, `shell_selector_overlay.rs`
**Solution:** Created `list_overlay.rs` with `ListOverlayConfig`, `ListOverlayState<T, M>`, `substring_filter()`. All overlays refactored to use shared infrastructure.

### ~~2. Extract keyboard navigation handler for list overlays~~ ✅ DONE
**Files:** `command_palette.rs`, `theme_selector.rs`, `file_search.rs`, `project_switcher.rs`, `shell_selector_overlay.rs`
**Solution:** Created `handle_list_overlay_key()` function with support for custom extra keys (e.g., Space for toggle in ProjectSwitcher).

### ~~3. Extract context menu item helper~~ ✅ DONE
**Files:** `overlays/context_menu.rs`, `layout/tabs/context_menu.rs`
**Solution:** Created `menu_item()`, `menu_item_with_color()`, and `menu_item_disabled()` helpers in `ui_helpers.rs`. Both context menu files refactored to use shared infrastructure.

## Medium Priority

### ~~4. Deduplicate shell selector display logic~~ ✅ DONE
**Files:** `terminal_pane/shell_selector.rs`, `tabs/shell_selector.rs`
**Solution:** Added `short_display_name()` to `ShellType` in `shell_config.rs` and created `shell_indicator_chip()` helper in `ui_helpers.rs`. Both shell selector files refactored to use shared infrastructure.

### ~~5. Extract standard button component~~ ✅ DONE
**Files:** 20+ occurrences across overlays and panels
**Solution:** Created `button()` and `button_primary()` helpers in `ui_helpers.rs`. Refactored `add_dialog.rs` and `worktree_dialog.rs` to use shared infrastructure. Other files can be migrated incrementally.

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
