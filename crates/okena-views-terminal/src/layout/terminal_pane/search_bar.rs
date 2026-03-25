//! Search bar component for terminal pane.

use crate::elements::terminal_element::SearchMatch;
use crate::actions::CloseSearch;
use okena_terminal::terminal::Terminal;
use okena_files::theme::theme;
use okena_ui::tokens::ui_text_md;
use crate::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::simple_input::InputChangedEvent;
use okena_workspace::state::Workspace;
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_ui::icon_button::icon_button_sized;
use std::sync::Arc;

#[derive(Clone)]
pub enum SearchBarEvent {
    Closed,
    MatchesChanged(Arc<Vec<SearchMatch>>, Option<usize>),
}

impl EventEmitter<SearchBarEvent> for SearchBar {}

pub struct SearchBar {
    workspace: Entity<Workspace>,
    terminal: Option<Arc<Terminal>>,
    input: Option<Entity<SimpleInputState>>,
    matches: Arc<Vec<SearchMatch>>,
    current_match_index: Option<usize>,
    case_sensitive: bool,
    use_regex: bool,
    is_active: bool,
    last_search_generation: u64,
}

impl SearchBar {
    pub fn new(workspace: Entity<Workspace>, _cx: &mut Context<Self>) -> Self {
        Self {
            workspace,
            terminal: None,
            input: None,
            matches: Arc::new(Vec::new()),
            current_match_index: None,
            case_sensitive: false,
            use_regex: false,
            is_active: false,
            last_search_generation: 0,
        }
    }

    pub fn set_terminal(&mut self, terminal: Option<Arc<Terminal>>) {
        self.terminal = terminal;
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_active = true;
        let input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Search...")
                .icon("icons/search.svg")
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
        cx.subscribe(&input, |this: &mut Self, _, _: &InputChangedEvent, cx| {
            this.perform_search(cx);
        }).detach();
        self.input = Some(input);
        self.matches = Arc::new(Vec::new());
        self.current_match_index = None;

        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.is_active = false;
        self.input = None;
        self.matches = Arc::new(Vec::new());
        self.current_match_index = None;

        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });

        cx.emit(SearchBarEvent::Closed);
        cx.notify();
    }

    pub fn perform_search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();

        if let Some(ref terminal) = self.terminal {
            self.last_search_generation = terminal.content_generation();
            let matches = terminal.search_grid(&query, self.case_sensitive, self.use_regex);
            let search_matches: Vec<SearchMatch> = matches
                .into_iter()
                .map(|(line, col, len)| SearchMatch { line, col, len })
                .collect();

            self.current_match_index = if !search_matches.is_empty() { Some(0) } else { None };
            self.matches = Arc::new(search_matches);

            cx.emit(SearchBarEvent::MatchesChanged(
                self.matches.clone(),
                self.current_match_index,
            ));
        }
        cx.notify();
    }

    /// Re-run search if terminal content has changed since last search.
    pub fn refresh_if_needed(&mut self, cx: &mut Context<Self>) {
        if !self.is_active { return; }
        if let Some(ref terminal) = self.terminal {
            let current_gen = terminal.content_generation();
            if current_gen != self.last_search_generation {
                self.perform_search(cx);
            }
        }
    }

    fn toggle_case_sensitive(&mut self, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        self.perform_search(cx);
    }

    fn toggle_regex(&mut self, cx: &mut Context<Self>) {
        self.use_regex = !self.use_regex;
        self.perform_search(cx);
    }

    pub fn next_match(&mut self, cx: &mut Context<Self>) {
        if self.matches.is_empty() { return; }
        let next_idx = match self.current_match_index {
            Some(idx) => (idx + 1) % self.matches.len(),
            None => 0,
        };
        self.current_match_index = Some(next_idx);
        self.scroll_to_current_match();
        cx.emit(SearchBarEvent::MatchesChanged(self.matches.clone(), self.current_match_index));
        cx.notify();
    }

    pub fn prev_match(&mut self, cx: &mut Context<Self>) {
        if self.matches.is_empty() { return; }
        let prev_idx = match self.current_match_index {
            Some(idx) => { if idx == 0 { self.matches.len() - 1 } else { idx - 1 } }
            None => self.matches.len() - 1,
        };
        self.current_match_index = Some(prev_idx);
        self.scroll_to_current_match();
        cx.emit(SearchBarEvent::MatchesChanged(self.matches.clone(), self.current_match_index));
        cx.notify();
    }

    fn scroll_to_current_match(&self) {
        if let (Some(idx), Some(terminal)) = (self.current_match_index, &self.terminal) {
            if let Some(search_match) = self.matches.get(idx) {
                let screen_lines = terminal.screen_lines() as i32;
                let display_offset = terminal.display_offset() as i32;
                // Convert absolute grid line to visual line
                let visual_line = search_match.line + display_offset;
                if visual_line < 0 || visual_line >= screen_lines {
                    let target_visible_line = screen_lines / 2;
                    let scroll_delta = target_visible_line - visual_line;
                    if scroll_delta > 0 { terminal.scroll_up(scroll_delta); }
                    else if scroll_delta < 0 { terminal.scroll_down(-scroll_delta); }
                }
            }
        }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                if event.keystroke.modifiers.shift { self.prev_match(cx); }
                else { self.next_match(cx); }
            }
            _ => {}
        }
    }
}

impl Render for SearchBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let match_count = self.matches.len();
        let current_idx = self.current_match_index.map(|i| i + 1).unwrap_or(0);
        let match_text = if match_count > 0 { format!("{}/{}", current_idx, match_count) } else { "0/0".to_string() };
        let case_sensitive = self.case_sensitive;
        let is_regex = self.use_regex;

        div()
            .id("search-bar")
            .h(px(36.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .bg(rgb(t.bg_header))
            .child(
                if let Some(ref input) = self.input {
                    div()
                        .id("search-input-wrapper")
                        .key_context("SearchBar")
                        .flex_1()
                        .min_w(px(100.0))
                        .max_w(px(300.0))
                        .bg(rgb(t.bg_secondary))
                        .border_1()
                        .border_color(rgb(t.border_active))
                        .rounded(px(4.0))
                        .child(SimpleInput::new(input).text_size(ui_text_md(cx)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                        .on_action(cx.listener(|this, _: &CloseSearch, _window, cx| { this.close(cx); }))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.handle_key_down(event, cx);
                        }))
                        .into_any_element()
                } else {
                    div().flex_1().into_any_element()
                },
            )
            .child(
                div().id("search-case-sensitive-btn").cursor_pointer().w(px(24.0)).h(px(24.0)).flex().items_center().justify_center().rounded(px(4.0))
                    .when(case_sensitive, |s| s.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                    .on_click(cx.listener(|this, _, _window, cx| { this.toggle_case_sensitive(cx); }))
                    .child(div().text_size(ui_text_md(cx)).font_weight(FontWeight::BOLD).text_color(if case_sensitive { rgb(t.text_primary) } else { rgb(t.text_secondary) }).child("Aa")),
            )
            .child(
                div().id("search-regex-btn").cursor_pointer().w(px(24.0)).h(px(24.0)).flex().items_center().justify_center().rounded(px(4.0))
                    .when(is_regex, |s| s.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                    .on_click(cx.listener(|this, _, _window, cx| { this.toggle_regex(cx); }))
                    .child(div().text_size(ui_text_md(cx)).font_weight(FontWeight::BOLD).text_color(if is_regex { rgb(t.text_primary) } else { rgb(t.text_secondary) }).child(".*")),
            )
            .child(div().text_size(ui_text_md(cx)).text_color(rgb(t.text_secondary)).min_w(px(40.0)).child(match_text))
            .child(icon_button_sized("search-prev-btn", "icons/chevron-up.svg", 24.0, 14.0, &t).on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); }).on_click(cx.listener(|this, _, _window, cx| { this.prev_match(cx); })))
            .child(icon_button_sized("search-next-btn", "icons/chevron-down.svg", 24.0, 14.0, &t).on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); }).on_click(cx.listener(|this, _, _window, cx| { this.next_match(cx); })))
            .child(icon_button_sized("search-close-btn", "icons/close.svg", 24.0, 14.0, &t).hover(|s| s.bg(rgba(0xf14c4c99))).on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); }).on_click(cx.listener(|this, _, _window, cx| { this.close(cx); })))
    }
}
