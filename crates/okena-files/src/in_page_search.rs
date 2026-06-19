//! Shared, view-agnostic in-page search ("search in page" / find-in-content).
//!
//! Used by the file viewer, the git diff viewer, and the content-search preview
//! pane. The engine is deliberately dumb: it searches a flat list of **cells**
//! (each cell is a line of text identified by an opaque `usize` chosen by the
//! host) and tracks the current match. The host owns all view semantics — what a
//! cell maps to (a line index, or a side-by-side row/column) and how to scroll
//! to it.
//!
//! Typical host wiring:
//! 1. On open: `InPageSearch::new(...)`, then `cx.subscribe(&search.input, ...)`
//!    to a `recompute` wrapper, and run `recompute` once.
//! 2. `recompute`: read the query from `search.input`, call [`compute_matches`]
//!    over the host's cell texts into a local `Vec`, then [`InPageSearch::set_matches`]
//!    (this two-step keeps the `search` borrow separate from the content borrow).
//! 3. Per rendered line/cell: merge [`InPageSearch::ranges_for_cell`] into the
//!    background ranges passed to `build_styled_text_with_backgrounds`.
//! 4. Render [`render_search_bar`] above the content with [`SearchBarCallbacks`].

use std::ops::Range;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::icon_button::icon_button_sized;
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::tokens::ui_text_md;

/// A single match location: byte range `[start, end)` within cell `cell`.
pub struct SearchMatch {
    pub cell: usize,
    pub start: usize,
    pub end: usize,
}

/// View-agnostic in-page search state.
pub struct InPageSearch {
    /// The query input box. Owned here; the host subscribes to its
    /// `InputChangedEvent` and re-runs its `recompute`.
    pub input: Entity<SimpleInputState>,
    matches: Vec<SearchMatch>,
    current: Option<usize>,
    case_sensitive: bool,
}

impl InPageSearch {
    /// Create the search state, building & focusing the query input.
    ///
    /// `prefill` pre-populates the query (e.g. from the current text selection);
    /// only its first line is used.
    pub fn new<V: 'static>(
        prefill: Option<&str>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut state = SimpleInputState::new(cx)
                .placeholder("Search...")
                .icon("icons/search.svg");
            if let Some(text) = prefill {
                let first_line = text.lines().next().unwrap_or("");
                if !first_line.is_empty() {
                    state.set_value(first_line, cx);
                }
            }
            state
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            input,
            matches: Vec::new(),
            current: None,
            case_sensitive: false,
        }
    }

    /// Replace the match set (after the query, case mode, or content changed).
    /// Resets the current match to the first one.
    pub fn set_matches(&mut self, matches: Vec<SearchMatch>) {
        self.current = if matches.is_empty() { None } else { Some(0) };
        self.matches = matches;
    }

    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Toggle case sensitivity. The host is responsible for recomputing matches.
    pub fn toggle_case(&mut self) {
        self.case_sensitive = !self.case_sensitive;
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// 1-based index of the current match (0 when there are none), for the
    /// "n/N" counter.
    pub fn current_1based(&self) -> usize {
        self.current.map(|i| i + 1).unwrap_or(0)
    }

    /// Cell of the current match, if any. The host scrolls to it.
    pub fn current_cell(&self) -> Option<usize> {
        self.current
            .and_then(|i| self.matches.get(i))
            .map(|m| m.cell)
    }

    /// Advance to the next match (wrapping); returns the new current cell.
    pub fn next_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let next = match self.current {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        };
        self.current = Some(next);
        self.current_cell()
    }

    /// Step to the previous match (wrapping); returns the new current cell.
    pub fn prev_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let prev = match self.current {
            Some(0) => self.matches.len() - 1,
            Some(i) => i - 1,
            None => self.matches.len() - 1,
        };
        self.current = Some(prev);
        self.current_cell()
    }

    /// Background highlight ranges for `cell` — bright for the current match,
    /// dim for the rest. Feed the result to `build_styled_text_with_backgrounds`.
    pub fn ranges_for_cell(&self, cell: usize, t: &ThemeColors) -> Vec<(Range<usize>, Hsla)> {
        let match_bg = color_with_alpha(t.search_match_bg, 0.4);
        let current_bg = color_with_alpha(t.search_current_bg, 0.7);
        let current = self.current;
        self.matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.cell == cell)
            .map(|(i, m)| {
                let color = if Some(i) == current {
                    current_bg
                } else {
                    match_bg
                };
                (m.start..m.end, color)
            })
            .collect()
    }
}

/// Compute literal-substring matches over a sequence of cells.
///
/// `cells` yields the plain text of each cell in host order; the cell id is the
/// iteration index. Returns byte-offset ranges. An empty query yields no
/// matches.
pub fn compute_matches<'a>(
    query: &str,
    case_sensitive: bool,
    cells: impl Iterator<Item = &'a str>,
) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let mut matches = Vec::new();
    for (cell, text) in cells.enumerate() {
        let haystack = if case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };
        let mut start = 0;
        while let Some(pos) = haystack[start..].find(&needle) {
            let abs = start + pos;
            matches.push(SearchMatch {
                cell,
                start: abs,
                end: abs + query.len(),
            });
            start = abs + 1;
        }
    }
    matches
}

/// Convert a `0xRRGGBB` theme color to `Hsla` with the given alpha.
fn color_with_alpha(rgb: u32, alpha: f32) -> Hsla {
    Hsla::from(Rgba {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a: alpha,
    })
}

/// A search-bar callback invoked with the host view and its context.
pub type SearchCallback<V> = Rc<dyn Fn(&mut V, &mut Context<V>)>;
/// A search-bar callback that also needs the window (e.g. to move focus).
pub type SearchCallbackWin<V> = Rc<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;

/// Host callbacks for the shared search bar. Each closure receives the host
/// view; the host bridges to its own engine + scroll glue. `Rc` because some
/// callbacks are wired to both a keystroke and a button.
pub struct SearchBarCallbacks<V> {
    pub on_next: SearchCallback<V>,
    pub on_prev: SearchCallback<V>,
    pub on_toggle_case: SearchCallback<V>,
    pub on_close: SearchCallbackWin<V>,
}

/// Render the shared search bar (input + case toggle + "n/N" counter + prev/next
/// + close).
///
/// Handles Enter / Shift+Enter (next/prev) on the query input and stops key
/// propagation so typed keys never leak into host shortcuts. Escape is left to
/// the host's own root `Cancel` action (each host closes search first if open),
/// which keeps the bar free of any crate-specific action dependency.
pub fn render_search_bar<V: 'static>(
    search: &InPageSearch,
    t: &ThemeColors,
    cx: &mut Context<V>,
    cb: SearchBarCallbacks<V>,
) -> AnyElement {
    let match_count = search.match_count();
    let match_text = if match_count > 0 {
        format!("{}/{}", search.current_1based(), match_count)
    } else {
        "0/0".to_string()
    };
    let case_sensitive = search.case_sensitive();
    let input = &search.input;

    let on_next_key = cb.on_next.clone();
    let on_prev_key = cb.on_prev.clone();

    div()
        .id("in-page-search-bar")
        .h(px(36.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(rgb(t.bg_header))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .id("in-page-search-input-wrapper")
                .key_context("InPageSearch")
                .flex_1()
                .min_w(px(100.0))
                .max_w(px(300.0))
                .bg(rgb(t.bg_secondary))
                .border_1()
                .border_color(rgb(t.border_active))
                .rounded(px(4.0))
                .child(SimpleInput::new(input).text_size(ui_text_md(cx)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                    let key = event.keystroke.key.as_str();
                    // Let Escape bubble to the host's root handler (Cancel action
                    // or key handler) so it can close search. Everything else is
                    // stopped so typed keys never leak into host shortcuts.
                    if key == "escape" {
                        return;
                    }
                    cx.stop_propagation();
                    if key == "enter" {
                        if event.keystroke.modifiers.shift {
                            (on_prev_key)(this, cx);
                        } else {
                            (on_next_key)(this, cx);
                        }
                    }
                })),
        )
        .child({
            let on_toggle = cb.on_toggle_case.clone();
            div()
                .id("in-page-search-case-btn")
                .cursor_pointer()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .when(case_sensitive, |s| s.bg(rgb(t.bg_selection)))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _window, cx| (on_toggle)(this, cx)))
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if case_sensitive {
                            rgb(t.text_primary)
                        } else {
                            rgb(t.text_secondary)
                        })
                        .child("Aa"),
                )
        })
        .child(
            div()
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_secondary))
                .min_w(px(40.0))
                .child(match_text),
        )
        .child({
            let on_prev = cb.on_prev.clone();
            icon_button_sized("in-page-search-prev-btn", "icons/chevron-up.svg", 24.0, 14.0, t)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _window, cx| (on_prev)(this, cx)))
        })
        .child({
            let on_next = cb.on_next.clone();
            icon_button_sized(
                "in-page-search-next-btn",
                "icons/chevron-down.svg",
                24.0,
                14.0,
                t,
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _window, cx| (on_next)(this, cx)))
        })
        .child({
            let on_close = cb.on_close.clone();
            icon_button_sized("in-page-search-close-btn", "icons/close.svg", 24.0, 14.0, t)
                .hover(|s| s.bg(gpui::rgba(0xf14c4c99)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| (on_close)(this, window, cx)))
        })
        .into_any_element()
}
