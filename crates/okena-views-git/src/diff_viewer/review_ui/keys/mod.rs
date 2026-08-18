//! Keyboard handling for the review workspace — spec §11.
//!
//! [`dispatch`] and [`cancel_step`] are pure: the key table and the Esc ladder
//! are decided without touching GPUI, and the `impl` below only runs the result.

mod footer;
mod help;

use super::super::DiffViewer;
use super::super::review::ReviewFileKey;
use super::model::{AttentionTarget, ReviewModel};
use super::state::{ContentView, FocusRegion, NavRowId, NavigatorMode};
use gpui::{App, ClipboardItem, Context, KeyDownEvent, Modifiers, ScrollStrategy, Window};
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
    ToggleOutline,
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
    CycleRegion,
    MoveCursor(CursorMove),
    ExpandCursor,
    CollapseCursor,
    ToggleCursorNode,
    ActivateCursor,
}

/// Keys the review owns. A modified variant of one of them is unbound and must
/// not reach the legacy diff shortcuts, which match on the key alone.
const REVIEW_KEYS: &[&str] = &[
    "1", "2", "/", "?", "r", "e", "o", "d", "w", "y", "s", "n", "N", "[", "]", "{", "}",
];

/// Layouts report a shifted character either as the character itself or as the
/// unshifted key plus `shift`. Returns the canonical key and whether `shift`
/// was consumed by the mapping.
fn normalize_key(key: &str, shift: bool) -> (&str, bool) {
    if !shift {
        return (key, false);
    }
    match key {
        "/" => ("?", true),
        "]" => ("}", true),
        "[" => ("{", true),
        "n" => ("N", true),
        other => (other, false),
    }
}

/// The key table of spec §11. `None` means the legacy diff handler may run.
pub(crate) fn dispatch(ctx: KeyContext, key: &str) -> Option<ReviewCommand> {
    let modifiers = ctx.modifiers;

    // The only region switch the review owns — `Ctrl+1` / `Ctrl+2` are global
    // app bindings. It answers from inside a field and under any modifier.
    if key == "f6" {
        return Some(ReviewCommand::CycleRegion);
    }
    // A focused field owns the key; swallowing stops the legacy single-letter
    // shortcuts from firing while the user types.
    if ctx.input_focused {
        return Some(ReviewCommand::Swallow);
    }
    // Accelerators resolve first, so `Cmd+?` / `Cmd+}` never hit the key table.
    if modifiers.platform || modifiers.control {
        return accelerator_key(ctx, key);
    }

    let (key, shift_used) = normalize_key(key, modifiers.shift);
    let modified = modifiers.alt || modifiers.function || (modifiers.shift && !shift_used);
    if !modified {
        if let Some(command) = single_key(ctx, key) {
            return Some(command);
        }
    } else if REVIEW_KEYS.contains(&key) {
        return Some(ReviewCommand::Swallow);
    }
    navigator_key(ctx, key)
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

/// The single-key table; keys that act on the open file stay on the file screen.
fn single_key(ctx: KeyContext, key: &str) -> Option<ReviewCommand> {
    let on_file = ctx.screen == ContentView::File;
    match key {
        "1" => Some(ReviewCommand::SetNavigator(NavigatorMode::Files)),
        "2" => Some(ReviewCommand::SetNavigator(NavigatorMode::Attention)),
        "/" => Some(ReviewCommand::FocusFilter),
        "?" => Some(ReviewCommand::ToggleHelp),
        "r" => Some(ReviewCommand::ToggleRoles),
        "e" => Some(ReviewCommand::ToggleOutline),
        "o" => Some(ReviewCommand::OpenOverview),
        "w" => Some(ReviewCommand::ToggleWhitespace),
        "d" => on_file.then_some(ReviewCommand::ToggleDetails),
        "y" => on_file.then_some(ReviewCommand::CopyPathLine),
        "s" => ctx.split_available.then_some(ReviewCommand::ToggleSplit),
        "}" => (on_file && ctx.has_symbols).then_some(ReviewCommand::StepSymbol(1)),
        "{" => (on_file && ctx.has_symbols).then_some(ReviewCommand::StepSymbol(-1)),
        "n" => ctx.search_open.then_some(ReviewCommand::SearchNext),
        "N" => ctx.search_open.then_some(ReviewCommand::SearchPrev),
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
        _ => None,
    }
}

/// Arrows belong to the navigator; in the content they keep scrolling the diff.
fn navigator_key(ctx: KeyContext, key: &str) -> Option<ReviewCommand> {
    let command = match key {
        "up" => ReviewCommand::MoveCursor(CursorMove::Prev),
        "down" => ReviewCommand::MoveCursor(CursorMove::Next),
        "home" => ReviewCommand::MoveCursor(CursorMove::First),
        "end" => ReviewCommand::MoveCursor(CursorMove::Last),
        "left" => ReviewCommand::CollapseCursor,
        "right" => ReviewCommand::ExpandCursor,
        "space" => ReviewCommand::ToggleCursorNode,
        "enter" => ReviewCommand::ActivateCursor,
        _ => return None,
    };
    // `Alt+↑` / `Alt+↓` are reserved for hunk stepping, which has no helper yet.
    // Swallowing keeps them from stepping files through the legacy handler.
    if ctx.modifiers.alt {
        return Some(ReviewCommand::Swallow);
    }
    if ctx.focus != FocusRegion::Navigator {
        return None;
    }
    Some(command)
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

/// Mirrors the set [`DiffViewer::dismiss_transient_ui`] closes; kept separate so
/// the ladder input is testable without a viewer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LegacyTransients {
    pub delete_confirm: bool,
    pub discard_confirm: bool,
    pub context_menu: bool,
    pub commit_hash_menu: bool,
    pub selection_context_menu: bool,
}

impl LegacyTransients {
    pub(crate) fn any_open(self) -> bool {
        self.delete_confirm
            || self.discard_confirm
            || self.context_menu
            || self.commit_hash_menu
            || self.selection_context_menu
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

/// The row the cursor lands on. A cursor the rows no longer contain restarts.
fn next_cursor(
    rows: &[NavRowId],
    cursor: Option<&NavRowId>,
    movement: CursorMove,
) -> Option<NavRowId> {
    let current = cursor.and_then(|cursor| rows.iter().position(|row| row == cursor));
    let index = next_cursor_index(rows.len(), current, movement)?;
    rows.get(index).cloned()
}

/// What `Ctrl+C` copies from a navigator row: a path, or a qualified symbol.
fn cursor_row_text(cursor: &NavRowId, model: Option<&ReviewModel>) -> Option<String> {
    match cursor {
        NavRowId::Dir(path) | NavRowId::Item(AttentionTarget::Directory(path)) => {
            Some(path.clone())
        }
        NavRowId::File(key) | NavRowId::Item(AttentionTarget::File(key)) => file_path(key),
        NavRowId::Item(AttentionTarget::Symbol { file, change_index }) => {
            let model = model?;
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
fn file_path(key: &ReviewFileKey) -> Option<String> {
    key.path(ComparisonSide::Head)
        .or_else(|| key.path(ComparisonSide::Base))
        .map(str::to_owned)
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
            legacy_transient: self.legacy_transients().any_open(),
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
                self.review_return_to_overview(cx);
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

    fn legacy_transients(&self) -> LegacyTransients {
        LegacyTransients {
            delete_confirm: self.delete_confirm.is_some(),
            discard_confirm: self.discard_confirm.is_some(),
            context_menu: self.context_menu.is_some(),
            commit_hash_menu: self.commit_hash_menu.is_some(),
            selection_context_menu: self.selection_context_menu.is_some(),
        }
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
            ReviewCommand::ToggleOutline => {
                let outline = self.review_ui.outline_inline;
                self.review_set_outline(!outline, cx);
            }
            ReviewCommand::OpenOverview => self.review_return_to_overview(cx),
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

    /// The overview is navigator-driven, so focus comes back with it.
    fn review_return_to_overview(&mut self, cx: &mut Context<Self>) {
        self.review_open_overview(cx);
        self.review_set_focus_region(FocusRegion::Navigator, cx);
    }

    /// Move the navigator cursor and open whatever it lands on.
    fn review_move_cursor(&mut self, movement: CursorMove, cx: &mut Context<Self>) {
        let rows = self.navigator_row_ids();
        let Some(row) = next_cursor(&rows, self.review_ui.nav_cursor.as_ref(), movement) else {
            return;
        };
        self.review_ui.nav_cursor = Some(row.clone());
        self.review_ui.nav_reveal = Some(ScrollStrategy::Nearest);
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
        let text = self
            .review_ui
            .nav_cursor
            .as_ref()
            .and_then(|cursor| cursor_row_text(cursor, self.review_ui.model.as_deref()));
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::model::AttentionTarget;
    use super::super::state::NavRowId;
    use super::{
        CancelFlags, CancelStep, ContentView, CursorMove, FocusRegion, KeyContext,
        LegacyTransients, NavigatorMode, ReviewCommand, ReviewFileKey, cancel_step,
        cursor_row_text, dispatch, next_cursor, next_cursor_index,
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

    fn with(modifiers: Modifiers, ctx: KeyContext) -> KeyContext {
        KeyContext { modifiers, ..ctx }
    }

    fn control() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    fn platform() -> Modifiers {
        Modifiers {
            platform: true,
            ..Modifiers::default()
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::default()
        }
    }

    fn function() -> Modifiers {
        Modifiers {
            function: true,
            ..Modifiers::default()
        }
    }

    fn key(path: &str) -> ReviewFileKey {
        ReviewFileKey {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
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
        assert_eq!(dispatch(ctx, "w"), Some(ReviewCommand::ToggleWhitespace));
    }

    #[test]
    fn the_file_screen_steps_symbols_and_the_attention_queue() {
        let ctx = file_screen();
        assert_eq!(dispatch(ctx, "}"), Some(ReviewCommand::StepSymbol(1)));
        assert_eq!(dispatch(ctx, "{"), Some(ReviewCommand::StepSymbol(-1)));
        assert_eq!(dispatch(ctx, "]"), Some(ReviewCommand::StepQueue(1)));
        assert_eq!(dispatch(ctx, "["), Some(ReviewCommand::StepQueue(-1)));
        assert_eq!(dispatch(ctx, "s"), Some(ReviewCommand::ToggleSplit));
        assert_eq!(dispatch(ctx, "d"), Some(ReviewCommand::ToggleDetails));
        assert_eq!(dispatch(ctx, "y"), Some(ReviewCommand::CopyPathLine));
    }

    #[test]
    fn keys_that_act_on_the_open_file_stay_on_the_file_screen() {
        // The overview must never act on whatever file was open last.
        let ctx = KeyContext {
            has_symbols: true,
            ..overview()
        };
        for pressed in ["d", "y", "}", "{"] {
            assert_eq!(
                dispatch(ctx, pressed),
                None,
                "{pressed} has no meaning on the overview"
            );
        }
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
        let ctx = with(shift(), file_screen());
        assert_eq!(dispatch(ctx, "]"), Some(ReviewCommand::StepSymbol(1)));
        assert_eq!(dispatch(ctx, "["), Some(ReviewCommand::StepSymbol(-1)));
        assert_eq!(dispatch(ctx, "/"), Some(ReviewCommand::ToggleHelp));
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
        assert_eq!(
            dispatch(with(shift(), open), "n"),
            Some(ReviewCommand::SearchPrev)
        );
    }

    #[test]
    fn find_opens_the_diff_search_only_on_the_file_screen() {
        assert_eq!(
            dispatch(with(control(), file_screen()), "f"),
            Some(ReviewCommand::OpenSearch)
        );
        assert_eq!(
            dispatch(with(platform(), file_screen()), "f"),
            Some(ReviewCommand::OpenSearch)
        );
        assert_eq!(
            dispatch(with(control(), overview()), "f"),
            Some(ReviewCommand::FocusFilter)
        );
    }

    #[test]
    fn copy_follows_the_focused_region() {
        let navigator = with(control(), file_screen());
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
    fn an_accelerator_never_triggers_a_plain_review_shortcut() {
        // `Cmd+?` and `Cmd+}` used to fall into the punctuation block.
        for modifiers in [control(), platform()] {
            let ctx = with(modifiers, file_screen());
            for pressed in [
                "?", "}", "{", "/", "]", "[", "1", "2", "r", "o", "d", "w", "y", "s",
            ] {
                assert_eq!(
                    dispatch(ctx, pressed),
                    None,
                    "an accelerator plus {pressed} is not a review binding"
                );
            }
        }
    }

    #[test]
    fn alt_and_function_variants_of_review_keys_do_nothing() {
        for modifiers in [alt(), function()] {
            let ctx = with(modifiers, file_screen());
            for pressed in [
                "w", "s", "]", "[", "}", "{", "1", "2", "r", "o", "d", "y", "/",
            ] {
                assert_eq!(
                    dispatch(ctx, pressed),
                    Some(ReviewCommand::Swallow),
                    "{pressed} must not reach the legacy shortcut"
                );
            }
        }
    }

    #[test]
    fn alt_arrows_are_reserved_and_never_move_the_cursor() {
        for focus in [FocusRegion::Navigator, FocusRegion::Content] {
            let ctx = KeyContext {
                focus,
                modifiers: alt(),
                ..file_screen()
            };
            for pressed in ["up", "down", "left", "right"] {
                assert_eq!(
                    dispatch(ctx, pressed),
                    Some(ReviewCommand::Swallow),
                    "{pressed} with alt must not step files either"
                );
            }
        }
    }

    #[test]
    fn shifted_letters_are_not_plain_shortcuts() {
        let ctx = with(shift(), file_screen());
        for pressed in ["w", "s", "d", "y", "r", "o", "1", "2"] {
            assert_eq!(
                dispatch(ctx, pressed),
                Some(ReviewCommand::Swallow),
                "shift plus {pressed} is unbound"
            );
        }
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
        for pressed in ["up", "down", "left", "right", "home", "end", "enter"] {
            assert_eq!(
                dispatch(content, pressed),
                None,
                "{pressed} must fall through to the diff pane"
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
        for pressed in [
            "1", "2", "r", "o", "d", "w", "s", "y", "n", "/", "?", "[", "]", "{", "}", "up",
            "down", "left", "right", "space", "enter",
        ] {
            assert_eq!(
                dispatch(ctx, pressed),
                Some(ReviewCommand::Swallow),
                "{pressed} must not act while a field has focus"
            );
        }
        assert_eq!(
            dispatch(with(control(), ctx), "f"),
            Some(ReviewCommand::Swallow),
            "even find stays out of a focused field"
        );
    }

    #[test]
    fn f6_is_the_only_region_switch_that_reaches_the_review() {
        for base in [overview(), file_screen()] {
            for input_focused in [false, true] {
                let ctx = KeyContext {
                    input_focused,
                    ..base
                };
                assert_eq!(
                    dispatch(ctx, "f6"),
                    Some(ReviewCommand::CycleRegion),
                    "F6 works even from inside a field"
                );
                // `Ctrl+1` / `Ctrl+2` are global app bindings; the review must
                // not claim them. A focused field still swallows them first.
                let expected = if input_focused {
                    Some(ReviewCommand::Swallow)
                } else {
                    None
                };
                assert_eq!(dispatch(with(control(), ctx), "1"), expected);
                assert_eq!(dispatch(with(control(), ctx), "2"), expected);
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
        assert_eq!(
            dispatch(with(control(), ctx), "a"),
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
    fn every_legacy_transient_reaches_the_ladder() {
        assert!(!LegacyTransients::default().any_open());
        let each = [
            LegacyTransients {
                delete_confirm: true,
                ..LegacyTransients::default()
            },
            LegacyTransients {
                discard_confirm: true,
                ..LegacyTransients::default()
            },
            LegacyTransients {
                context_menu: true,
                ..LegacyTransients::default()
            },
            LegacyTransients {
                commit_hash_menu: true,
                ..LegacyTransients::default()
            },
            LegacyTransients {
                selection_context_menu: true,
                ..LegacyTransients::default()
            },
        ];
        for transient in each {
            assert!(transient.any_open(), "{transient:?} must stop Esc");
            assert_eq!(
                cancel_step(CancelFlags {
                    legacy_transient: transient.any_open(),
                    ..CancelFlags::default()
                }),
                CancelStep::DismissLegacy
            );
        }
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

    #[test]
    fn a_cursor_the_rows_no_longer_hold_restarts_at_the_top() {
        let rows = vec![
            NavRowId::Dir("src".into()),
            NavRowId::File(key("src/a.rs")),
            NavRowId::File(key("src/b.rs")),
        ];
        let gone = NavRowId::File(key("dropped.rs"));
        assert_eq!(
            next_cursor(&rows, Some(&gone), CursorMove::Next),
            Some(rows[0].clone()),
            "a filtered-out cursor must not silently jump to the last row"
        );
        assert_eq!(
            next_cursor(&rows, Some(&gone), CursorMove::Prev),
            Some(rows[0].clone())
        );
        assert_eq!(
            next_cursor(&rows, Some(&rows[1]), CursorMove::Next),
            Some(rows[2].clone())
        );
        assert_eq!(next_cursor(&[], None, CursorMove::Next), None);
    }

    #[test]
    fn a_cursor_row_copies_its_path_or_its_qualified_symbol() {
        assert_eq!(
            cursor_row_text(&NavRowId::Dir("packages/core".into()), None).as_deref(),
            Some("packages/core")
        );
        assert_eq!(
            cursor_row_text(
                &NavRowId::Item(AttentionTarget::Directory("packages/core".into())),
                None
            )
            .as_deref(),
            Some("packages/core")
        );
        assert_eq!(
            cursor_row_text(&NavRowId::File(key("src/lib.rs")), None).as_deref(),
            Some("src/lib.rs"),
            "the head path, not the `old → new` label"
        );
        let deleted = ReviewFileKey {
            old_path: Some("src/gone.rs".into()),
            new_path: None,
        };
        assert_eq!(
            cursor_row_text(&NavRowId::Item(AttentionTarget::File(deleted)), None).as_deref(),
            Some("src/gone.rs"),
            "a deletion falls back to the base side"
        );
        assert_eq!(
            cursor_row_text(
                &NavRowId::Item(AttentionTarget::Symbol {
                    file: key("src/lib.rs"),
                    change_index: 0,
                }),
                None
            ),
            None,
            "a symbol needs the model to name it"
        );
    }

    #[test]
    fn a_symbol_row_copies_the_name_the_model_qualified() {
        let model = fixtures::model();
        let (file, symbol) = model
            .files
            .iter()
            .find_map(|entry| entry.symbols.first().map(|symbol| (entry, symbol)))
            .expect("the fixture reaches structure for at least one file");
        assert_eq!(
            cursor_row_text(
                &NavRowId::Item(AttentionTarget::Symbol {
                    file: file.key.clone(),
                    change_index: symbol.change_index,
                }),
                Some(&model)
            )
            .as_deref(),
            Some(symbol.qualified.as_str())
        );
        assert_eq!(
            cursor_row_text(
                &NavRowId::Item(AttentionTarget::Symbol {
                    file: file.key.clone(),
                    change_index: usize::MAX,
                }),
                Some(&model)
            ),
            None,
            "an index the file no longer has copies nothing"
        );
    }
}
