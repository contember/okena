//! Keyboard handling for the review workspace — spec §11.
//!
//! [`dispatch`] and [`cancel_step`] are pure: the key table and the Esc ladder
//! are decided without touching GPUI, and the `impl` below only runs the result.

mod footer;
mod help;

use super::super::DiffViewer;
use super::model::AttentionTarget;
use super::state::{ContentView, FocusRegion, NavRowId, NavigatorMode};
use gpui::{App, ClipboardItem, Context, KeyDownEvent, Modifiers, Window};
use okena_core::review::ComparisonSide;

/// Everything the key table branches on, gathered once per event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyContext {
    pub screen: ContentView,
    pub focus: FocusRegion,
    /// The navigator filter or the in-page search field has focus.
    pub input_focused: bool,
    pub search_open: bool,
    /// The open file has at least one changed symbol.
    pub has_symbols: bool,
    pub split_available: bool,
    /// A commit list is open, so `[` / `]` belong to the legacy commit bar (§3).
    pub has_commits: bool,
    pub modifiers: Modifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorMove {
    Prev,
    Next,
    First,
    Last,
}

/// One resolved keystroke. Every review action the keyboard can reach is here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewCommand {
    /// Handled on purpose but does nothing — keeps the key out of the legacy path.
    Swallow,
    SetNavigator(NavigatorMode),
    FocusFilter,
    ToggleRoles,
    OpenOverview,
    StepQueue(i32),
    StepSymbol(i32),
    PrevCommit,
    NextCommit,
    ToggleDetails,
    ToggleSplit,
    ToggleWhitespace,
    OpenSearch,
    SearchNext,
    SearchPrev,
    CopyPathLine,
    CopyNavigatorRow,
    CopySelection,
    ToggleHelp,
    FocusRegionSwitch(FocusRegion),
    CycleRegion,
    MoveCursor(CursorMove),
    ExpandCursor,
    CollapseCursor,
    ToggleCursorNode,
    ActivateCursor,
}

/// The key table of spec §11. `None` means the legacy diff handler may run.
pub(crate) fn dispatch(ctx: KeyContext, key: &str) -> Option<ReviewCommand> {
    let modifiers = ctx.modifiers;
    let accelerator = modifiers.platform || modifiers.control;

    // Region switching stays reachable from inside a text field.
    if key == "f6" {
        return Some(ReviewCommand::CycleRegion);
    }
    if modifiers.control && key == "1" {
        return Some(ReviewCommand::FocusRegionSwitch(FocusRegion::Navigator));
    }
    if modifiers.control && key == "2" {
        return Some(ReviewCommand::FocusRegionSwitch(FocusRegion::Content));
    }
    // A focused field owns the key; swallowing stops the legacy single-letter
    // shortcuts from firing while the user types.
    if ctx.input_focused {
        return Some(ReviewCommand::Swallow);
    }
    // Shifted punctuation arrives as the literal character; accept the
    // unshifted form too for layouts that report it that way.
    if key == "?" || (key == "/" && modifiers.shift) {
        return Some(ReviewCommand::ToggleHelp);
    }
    if key == "}" || (key == "]" && modifiers.shift) {
        return ctx.has_symbols.then_some(ReviewCommand::StepSymbol(1));
    }
    if key == "{" || (key == "[" && modifiers.shift) {
        return ctx.has_symbols.then_some(ReviewCommand::StepSymbol(-1));
    }
    if accelerator {
        return accelerator_key(ctx, key);
    }

    match key {
        "1" => Some(ReviewCommand::SetNavigator(NavigatorMode::Files)),
        "2" => Some(ReviewCommand::SetNavigator(NavigatorMode::Attention)),
        "/" => Some(ReviewCommand::FocusFilter),
        "r" => Some(ReviewCommand::ToggleRoles),
        "o" => Some(ReviewCommand::OpenOverview),
        "d" => Some(ReviewCommand::ToggleDetails),
        "w" => Some(ReviewCommand::ToggleWhitespace),
        "y" => Some(ReviewCommand::CopyPathLine),
        "s" => ctx.split_available.then_some(ReviewCommand::ToggleSplit),
        "n" if ctx.search_open => Some(if modifiers.shift {
            ReviewCommand::SearchPrev
        } else {
            ReviewCommand::SearchNext
        }),
        "]" => Some(if ctx.has_commits {
            ReviewCommand::NextCommit
        } else {
            ReviewCommand::StepQueue(1)
        }),
        "[" => Some(if ctx.has_commits {
            ReviewCommand::PrevCommit
        } else {
            ReviewCommand::StepQueue(-1)
        }),
        _ => navigator_key(ctx, key),
    }
}

fn accelerator_key(ctx: KeyContext, key: &str) -> Option<ReviewCommand> {
    match key {
        "f" => Some(if ctx.screen == ContentView::File {
            ReviewCommand::OpenSearch
        } else {
            ReviewCommand::FocusFilter
        }),
        "c" => Some(if ctx.focus == FocusRegion::Navigator {
            ReviewCommand::CopyNavigatorRow
        } else {
            ReviewCommand::CopySelection
        }),
        _ => None,
    }
}

/// Arrows belong to the navigator; in the content they keep scrolling the diff.
fn navigator_key(ctx: KeyContext, key: &str) -> Option<ReviewCommand> {
    if ctx.focus != FocusRegion::Navigator {
        return None;
    }
    match key {
        "up" => Some(ReviewCommand::MoveCursor(CursorMove::Prev)),
        "down" => Some(ReviewCommand::MoveCursor(CursorMove::Next)),
        "home" => Some(ReviewCommand::MoveCursor(CursorMove::First)),
        "end" => Some(ReviewCommand::MoveCursor(CursorMove::Last)),
        "left" => Some(ReviewCommand::CollapseCursor),
        "right" => Some(ReviewCommand::ExpandCursor),
        "space" => Some(ReviewCommand::ToggleCursorNode),
        "enter" => Some(ReviewCommand::ActivateCursor),
        _ => None,
    }
}

/// What `Esc` is currently for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CancelStep {
    CloseHelp,
    DismissMenu,
    ClearFilter,
    CloseSearch,
    DismissLegacy,
    BackToOverview,
    Unhandled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CancelFlags {
    pub help_open: bool,
    /// A review menu or popover is open (roles, status, outline).
    pub menu_open: bool,
    pub filter_focused: bool,
    pub search_open: bool,
    /// A legacy context menu or confirm dialog is open.
    pub legacy_transient: bool,
    pub content_is_file: bool,
}

/// The Esc ladder of spec §11 — never leaves an input without clearing it first.
pub(crate) fn cancel_step(flags: CancelFlags) -> CancelStep {
    if flags.help_open {
        CancelStep::CloseHelp
    } else if flags.menu_open {
        CancelStep::DismissMenu
    } else if flags.filter_focused {
        CancelStep::ClearFilter
    } else if flags.search_open {
        CancelStep::CloseSearch
    } else if flags.legacy_transient {
        CancelStep::DismissLegacy
    } else if flags.content_is_file {
        CancelStep::BackToOverview
    } else {
        CancelStep::Unhandled
    }
}

/// Clamped cursor step within `len` rows; no cursor starts at the first row.
fn next_cursor_index(len: usize, current: Option<usize>, movement: CursorMove) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len.saturating_sub(1);
    Some(match movement {
        CursorMove::First => 0,
        CursorMove::Last => last,
        CursorMove::Prev => current.map_or(0, |index| index.saturating_sub(1)),
        CursorMove::Next => current.map_or(0, |index| index.saturating_add(1).min(last)),
    })
}

impl DiffViewer {
    /// Returns true when the review handled the key and the legacy path must not run.
    pub(crate) fn handle_review_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ctx = self.review_key_context(event.keystroke.modifiers, window, cx);
        let Some(command) = dispatch(ctx, event.keystroke.key.as_str()) else {
            return false;
        };
        self.run_review_command(command, window, cx);
        true
    }

    /// The Esc ladder. Returns true when it consumed the key.
    pub(crate) fn handle_review_cancel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let flags = CancelFlags {
            help_open: self.review_ui.help_open,
            menu_open: self.review_ui.roles_menu_open
                || self.review_ui.status_popover_open
                || self.review_ui.outline_open,
            filter_focused: self.review_filter_focused(window, cx),
            search_open: self.search.is_some(),
            legacy_transient: self.has_legacy_transient(),
            content_is_file: self.review_ui.content == ContentView::File,
        };
        match cancel_step(flags) {
            CancelStep::CloseHelp => {
                self.review_toggle_help(cx);
                true
            }
            CancelStep::DismissMenu => self.review_dismiss_transient(cx),
            CancelStep::ClearFilter => {
                self.review_clear_filter(cx);
                window.focus(&self.focus_handle, cx);
                true
            }
            CancelStep::CloseSearch => {
                self.close_search(window, cx);
                true
            }
            CancelStep::DismissLegacy => self.dismiss_transient_ui(cx),
            CancelStep::BackToOverview => {
                self.review_open_overview(cx);
                true
            }
            CancelStep::Unhandled => false,
        }
    }

    fn review_key_context(&self, modifiers: Modifiers, window: &Window, cx: &App) -> KeyContext {
        KeyContext {
            screen: self.review_ui.content,
            focus: self.review_ui.focus_region,
            input_focused: self.review_input_focused(window, cx),
            search_open: self.search.is_some(),
            has_symbols: self.review_open_file_has_symbols(),
            split_available: self.review_show_split_toggle(),
            has_commits: self.has_commits(),
            modifiers,
        }
    }

    /// The context the footer describes; no event, so no modifiers.
    fn review_screen_context(&self) -> KeyContext {
        KeyContext {
            screen: self.review_ui.content,
            focus: self.review_ui.focus_region,
            input_focused: false,
            search_open: self.search.is_some(),
            has_symbols: self.review_open_file_has_symbols(),
            split_available: self.review_show_split_toggle(),
            has_commits: self.has_commits(),
            modifiers: Modifiers::default(),
        }
    }

    fn review_input_focused(&self, window: &Window, cx: &App) -> bool {
        if self.review_filter_focused(window, cx) {
            return true;
        }
        self.search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn review_open_file_has_symbols(&self) -> bool {
        let Some(model) = self.review_ui.model.as_ref() else {
            return false;
        };
        let Some(key) = self.smart_review.selected_file.as_ref() else {
            return false;
        };
        model
            .file_index(key)
            .and_then(|index| model.files.get(index))
            .is_some_and(|entry| !entry.symbols.is_empty())
    }

    /// Mirrors the set [`DiffViewer::dismiss_transient_ui`] closes.
    fn has_legacy_transient(&self) -> bool {
        self.delete_confirm.is_some()
            || self.discard_confirm.is_some()
            || self.context_menu.is_some()
            || self.commit_hash_menu.is_some()
            || self.selection_context_menu.is_some()
    }

    fn run_review_command(
        &mut self,
        command: ReviewCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            ReviewCommand::Swallow => {}
            ReviewCommand::SetNavigator(mode) => self.review_set_navigator(mode, cx),
            ReviewCommand::FocusFilter => self.review_focus_filter(window, cx),
            ReviewCommand::ToggleRoles => self.review_toggle_roles_menu(cx),
            ReviewCommand::OpenOverview => self.review_open_overview(cx),
            ReviewCommand::StepQueue(delta) => self.review_step_queue(delta, cx),
            ReviewCommand::StepSymbol(delta) => self.review_step_symbol(delta, cx),
            ReviewCommand::PrevCommit => self.prev_commit(cx),
            ReviewCommand::NextCommit => self.next_commit(cx),
            ReviewCommand::ToggleDetails => self.review_toggle_details(cx),
            ReviewCommand::ToggleSplit => self.toggle_view_mode(cx),
            ReviewCommand::ToggleWhitespace => self.toggle_ignore_whitespace(cx),
            ReviewCommand::OpenSearch => self.open_search(window, cx),
            ReviewCommand::SearchNext => self.next_search_match(cx),
            ReviewCommand::SearchPrev => self.prev_search_match(cx),
            ReviewCommand::CopyPathLine => self.review_copy_path_line(cx),
            ReviewCommand::CopyNavigatorRow => self.review_copy_cursor_row(cx),
            ReviewCommand::CopySelection => self.copy_selection(cx),
            ReviewCommand::ToggleHelp => self.review_toggle_help(cx),
            ReviewCommand::FocusRegionSwitch(region) => self.review_set_focus_region(region, cx),
            ReviewCommand::CycleRegion => {
                let next = match self.review_ui.focus_region {
                    FocusRegion::Navigator => FocusRegion::Content,
                    FocusRegion::Content => FocusRegion::Navigator,
                };
                self.review_set_focus_region(next, cx);
            }
            ReviewCommand::MoveCursor(movement) => self.review_move_cursor(movement, cx),
            ReviewCommand::ExpandCursor => self.review_set_cursor_dir(true, cx),
            ReviewCommand::CollapseCursor => self.review_set_cursor_dir(false, cx),
            ReviewCommand::ToggleCursorNode => self.review_toggle_cursor_dir(cx),
            ReviewCommand::ActivateCursor => self.review_activate_cursor(cx),
        }
    }

    /// Move the navigator cursor and open whatever it lands on.
    fn review_move_cursor(&mut self, movement: CursorMove, cx: &mut Context<Self>) {
        let rows = self.navigator_row_ids();
        let current = self
            .review_ui
            .nav_cursor
            .as_ref()
            .and_then(|cursor| rows.iter().position(|row| row == cursor));
        let Some(index) = next_cursor_index(rows.len(), current, movement) else {
            return;
        };
        let Some(row) = rows.get(index).cloned() else {
            return;
        };
        self.review_ui.nav_cursor = Some(row.clone());
        self.review_open_row(row, cx);
    }

    /// Directories only move the cursor; files and items open in the content.
    fn review_open_row(&mut self, row: NavRowId, cx: &mut Context<Self>) {
        match row {
            NavRowId::Dir(_) => cx.notify(),
            NavRowId::File(key) => self.review_open_file(key, cx),
            NavRowId::Item(target) => self.review_open_item(target, cx),
        }
    }

    fn review_activate_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.review_ui.nav_cursor.clone() else {
            return;
        };
        // A tree folder has nothing to show, so `↵` behaves like `Space`.
        if let NavRowId::Dir(path) = row {
            self.review_toggle_dir(&path, cx);
            return;
        }
        self.review_open_row(row, cx);
        self.review_set_focus_region(FocusRegion::Content, cx);
    }

    fn review_set_cursor_dir(&mut self, expand: bool, cx: &mut Context<Self>) {
        let Some(NavRowId::Dir(path)) = self.review_ui.nav_cursor.clone() else {
            return;
        };
        let changed = if expand {
            self.review_ui.expanded_dirs.insert(path)
        } else {
            self.review_ui.expanded_dirs.remove(&path)
        };
        if changed {
            cx.notify();
        }
    }

    fn review_toggle_cursor_dir(&mut self, cx: &mut Context<Self>) {
        let Some(NavRowId::Dir(path)) = self.review_ui.nav_cursor.clone() else {
            return;
        };
        self.review_toggle_dir(&path, cx);
    }

    fn review_copy_cursor_row(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.review_cursor_row_text() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn review_cursor_row_text(&self) -> Option<String> {
        match self.review_ui.nav_cursor.as_ref()? {
            NavRowId::Dir(path) | NavRowId::Item(AttentionTarget::Directory(path)) => {
                Some(path.clone())
            }
            NavRowId::File(key) | NavRowId::Item(AttentionTarget::File(key)) => {
                self.review_file_path(key)
            }
            NavRowId::Item(AttentionTarget::Symbol { file, change_index }) => {
                let model = self.review_ui.model.as_ref()?;
                let entry = model
                    .file_index(file)
                    .and_then(|index| model.files.get(index))?;
                entry
                    .symbols
                    .iter()
                    .find(|symbol| symbol.change_index == *change_index)
                    .map(|symbol| symbol.qualified.clone())
            }
        }
    }

    /// The side that still exists; renames copy as the head path.
    fn review_file_path(&self, key: &super::super::review::ReviewFileKey) -> Option<String> {
        key.path(ComparisonSide::Head)
            .or_else(|| key.path(ComparisonSide::Base))
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CancelFlags, CancelStep, ContentView, CursorMove, FocusRegion, KeyContext, NavigatorMode,
        ReviewCommand, cancel_step, dispatch, next_cursor_index,
    };
    use gpui::Modifiers;

    fn overview() -> KeyContext {
        KeyContext::default()
    }

    fn file_screen() -> KeyContext {
        KeyContext {
            screen: ContentView::File,
            has_symbols: true,
            split_available: true,
            ..KeyContext::default()
        }
    }

    fn control() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn the_overview_keys_reach_the_navigator_and_the_filters() {
        let ctx = overview();
        assert_eq!(
            dispatch(ctx, "1"),
            Some(ReviewCommand::SetNavigator(NavigatorMode::Files))
        );
        assert_eq!(
            dispatch(ctx, "2"),
            Some(ReviewCommand::SetNavigator(NavigatorMode::Attention))
        );
        assert_eq!(dispatch(ctx, "/"), Some(ReviewCommand::FocusFilter));
        assert_eq!(dispatch(ctx, "r"), Some(ReviewCommand::ToggleRoles));
        assert_eq!(dispatch(ctx, "o"), Some(ReviewCommand::OpenOverview));
        assert_eq!(dispatch(ctx, "?"), Some(ReviewCommand::ToggleHelp));
        assert_eq!(dispatch(ctx, "d"), Some(ReviewCommand::ToggleDetails));
        assert_eq!(dispatch(ctx, "w"), Some(ReviewCommand::ToggleWhitespace));
        assert_eq!(dispatch(ctx, "y"), Some(ReviewCommand::CopyPathLine));
    }

    #[test]
    fn the_file_screen_steps_symbols_and_the_attention_queue() {
        let ctx = file_screen();
        assert_eq!(dispatch(ctx, "}"), Some(ReviewCommand::StepSymbol(1)));
        assert_eq!(dispatch(ctx, "{"), Some(ReviewCommand::StepSymbol(-1)));
        assert_eq!(dispatch(ctx, "]"), Some(ReviewCommand::StepQueue(1)));
        assert_eq!(dispatch(ctx, "["), Some(ReviewCommand::StepQueue(-1)));
        assert_eq!(dispatch(ctx, "s"), Some(ReviewCommand::ToggleSplit));
    }

    #[test]
    fn a_commit_list_takes_the_brackets_back_for_the_commit_bar() {
        let ctx = KeyContext {
            has_commits: true,
            ..file_screen()
        };
        assert_eq!(dispatch(ctx, "]"), Some(ReviewCommand::NextCommit));
        assert_eq!(dispatch(ctx, "["), Some(ReviewCommand::PrevCommit));
        assert_eq!(
            dispatch(ctx, "}"),
            Some(ReviewCommand::StepSymbol(1)),
            "the braces still step symbols"
        );
    }

    #[test]
    fn shifted_punctuation_is_accepted_in_both_reported_forms() {
        let ctx = file_screen();
        let shifted = KeyContext {
            modifiers: shift(),
            ..ctx
        };
        assert_eq!(dispatch(shifted, "]"), Some(ReviewCommand::StepSymbol(1)));
        assert_eq!(dispatch(shifted, "["), Some(ReviewCommand::StepSymbol(-1)));
        assert_eq!(dispatch(shifted, "/"), Some(ReviewCommand::ToggleHelp));
    }

    #[test]
    fn unavailable_actions_are_never_dispatched() {
        let ctx = KeyContext {
            screen: ContentView::File,
            ..KeyContext::default()
        };
        assert_eq!(dispatch(ctx, "}"), None, "no symbols in the open file");
        assert_eq!(dispatch(ctx, "{"), None);
        assert_eq!(dispatch(ctx, "s"), None, "no split toggle on this screen");
        assert_eq!(dispatch(ctx, "n"), None, "search is closed");
    }

    #[test]
    fn search_stepping_needs_an_open_search() {
        let open = KeyContext {
            search_open: true,
            ..file_screen()
        };
        assert_eq!(dispatch(open, "n"), Some(ReviewCommand::SearchNext));
        let back = KeyContext {
            modifiers: shift(),
            ..open
        };
        assert_eq!(dispatch(back, "n"), Some(ReviewCommand::SearchPrev));
    }

    #[test]
    fn find_opens_the_diff_search_only_on_the_file_screen() {
        let on_file = KeyContext {
            modifiers: control(),
            ..file_screen()
        };
        assert_eq!(dispatch(on_file, "f"), Some(ReviewCommand::OpenSearch));

        let on_overview = KeyContext {
            modifiers: control(),
            ..overview()
        };
        assert_eq!(dispatch(on_overview, "f"), Some(ReviewCommand::FocusFilter));
    }

    #[test]
    fn copy_follows_the_focused_region() {
        let navigator = KeyContext {
            modifiers: control(),
            ..file_screen()
        };
        assert_eq!(
            dispatch(navigator, "c"),
            Some(ReviewCommand::CopyNavigatorRow)
        );

        let content = KeyContext {
            focus: FocusRegion::Content,
            ..navigator
        };
        assert_eq!(dispatch(content, "c"), Some(ReviewCommand::CopySelection));
    }

    #[test]
    fn arrows_move_the_cursor_only_while_the_navigator_has_focus() {
        let ctx = overview();
        assert_eq!(
            dispatch(ctx, "up"),
            Some(ReviewCommand::MoveCursor(CursorMove::Prev))
        );
        assert_eq!(
            dispatch(ctx, "down"),
            Some(ReviewCommand::MoveCursor(CursorMove::Next))
        );
        assert_eq!(
            dispatch(ctx, "home"),
            Some(ReviewCommand::MoveCursor(CursorMove::First))
        );
        assert_eq!(
            dispatch(ctx, "end"),
            Some(ReviewCommand::MoveCursor(CursorMove::Last))
        );
        assert_eq!(dispatch(ctx, "left"), Some(ReviewCommand::CollapseCursor));
        assert_eq!(dispatch(ctx, "right"), Some(ReviewCommand::ExpandCursor));
        assert_eq!(
            dispatch(ctx, "space"),
            Some(ReviewCommand::ToggleCursorNode)
        );
        assert_eq!(dispatch(ctx, "enter"), Some(ReviewCommand::ActivateCursor));

        let content = KeyContext {
            focus: FocusRegion::Content,
            ..ctx
        };
        for key in ["up", "down", "left", "right", "home", "end", "enter"] {
            assert_eq!(
                dispatch(content, key),
                None,
                "{key} must fall through to the diff pane"
            );
        }
    }

    #[test]
    fn a_focused_field_makes_every_shortcut_inert() {
        let ctx = KeyContext {
            input_focused: true,
            search_open: true,
            ..file_screen()
        };
        for key in [
            "1", "2", "r", "o", "d", "w", "s", "y", "n", "/", "?", "[", "]", "{", "}", "up",
            "down", "left", "right", "space", "enter",
        ] {
            assert_eq!(
                dispatch(ctx, key),
                Some(ReviewCommand::Swallow),
                "{key} must not act while a field has focus"
            );
        }
        let with_control = KeyContext {
            modifiers: control(),
            ..ctx
        };
        assert_eq!(
            dispatch(with_control, "f"),
            Some(ReviewCommand::Swallow),
            "even find stays out of a focused field"
        );
    }

    #[test]
    fn the_region_switches_work_from_anywhere() {
        for base in [overview(), file_screen()] {
            for input_focused in [false, true] {
                let ctx = KeyContext {
                    input_focused,
                    modifiers: control(),
                    ..base
                };
                assert_eq!(
                    dispatch(ctx, "1"),
                    Some(ReviewCommand::FocusRegionSwitch(FocusRegion::Navigator))
                );
                assert_eq!(
                    dispatch(ctx, "2"),
                    Some(ReviewCommand::FocusRegionSwitch(FocusRegion::Content))
                );
                let plain = KeyContext {
                    input_focused,
                    ..base
                };
                assert_eq!(dispatch(plain, "f6"), Some(ReviewCommand::CycleRegion));
            }
        }
    }

    #[test]
    fn the_navigator_modes_are_always_handled() {
        for screen in [ContentView::Overview, ContentView::File] {
            for focus in [FocusRegion::Navigator, FocusRegion::Content] {
                for input_focused in [false, true] {
                    let ctx = KeyContext {
                        screen,
                        focus,
                        input_focused,
                        ..KeyContext::default()
                    };
                    assert!(dispatch(ctx, "1").is_some(), "1 must never reach the app");
                    assert!(dispatch(ctx, "2").is_some(), "2 must never reach the app");
                }
            }
        }
    }

    #[test]
    fn unknown_keys_leave_the_legacy_path_alone() {
        let ctx = file_screen();
        assert_eq!(dispatch(ctx, "tab"), None);
        assert_eq!(dispatch(ctx, "escape"), None);
        assert_eq!(dispatch(ctx, "q"), None);
        let with_control = KeyContext {
            modifiers: control(),
            ..ctx
        };
        assert_eq!(
            dispatch(with_control, "a"),
            None,
            "select-all stays with the diff pane"
        );
    }

    #[test]
    fn the_esc_ladder_runs_in_order() {
        let all = CancelFlags {
            help_open: true,
            menu_open: true,
            filter_focused: true,
            search_open: true,
            legacy_transient: true,
            content_is_file: true,
        };
        assert_eq!(cancel_step(all), CancelStep::CloseHelp);
        let no_help = CancelFlags {
            help_open: false,
            ..all
        };
        assert_eq!(cancel_step(no_help), CancelStep::DismissMenu);
        let no_menu = CancelFlags {
            menu_open: false,
            ..no_help
        };
        assert_eq!(cancel_step(no_menu), CancelStep::ClearFilter);
        let blurred = CancelFlags {
            filter_focused: false,
            ..no_menu
        };
        assert_eq!(cancel_step(blurred), CancelStep::CloseSearch);
        let no_search = CancelFlags {
            search_open: false,
            ..blurred
        };
        assert_eq!(cancel_step(no_search), CancelStep::DismissLegacy);
        let no_legacy = CancelFlags {
            legacy_transient: false,
            ..no_search
        };
        assert_eq!(cancel_step(no_legacy), CancelStep::BackToOverview);
        let on_overview = CancelFlags {
            content_is_file: false,
            ..no_legacy
        };
        assert_eq!(cancel_step(on_overview), CancelStep::Unhandled);
    }

    #[test]
    fn esc_clears_a_focused_filter_before_it_closes_anything() {
        let flags = CancelFlags {
            filter_focused: true,
            search_open: true,
            content_is_file: true,
            ..CancelFlags::default()
        };
        assert_eq!(cancel_step(flags), CancelStep::ClearFilter);
    }

    #[test]
    fn cursor_steps_clamp_at_both_ends() {
        assert_eq!(next_cursor_index(0, None, CursorMove::Next), None);
        assert_eq!(next_cursor_index(3, None, CursorMove::Next), Some(0));
        assert_eq!(next_cursor_index(3, None, CursorMove::Prev), Some(0));
        assert_eq!(next_cursor_index(3, Some(0), CursorMove::Prev), Some(0));
        assert_eq!(next_cursor_index(3, Some(1), CursorMove::Next), Some(2));
        assert_eq!(next_cursor_index(3, Some(2), CursorMove::Next), Some(2));
        assert_eq!(next_cursor_index(3, Some(1), CursorMove::First), Some(0));
        assert_eq!(next_cursor_index(3, Some(1), CursorMove::Last), Some(2));
    }
}
