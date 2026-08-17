//! Shortcut help overlay (`?`) — spec §11.

use super::super::super::DiffViewer;
use super::footer::{COPY_KEY, DOWN, ENTER, FIND_KEY, UP, key_chip};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::tokens::{ui_text, ui_text_ms};

/// One line of the spec §11 table.
struct HelpRow {
    keys: &'static [&'static str],
    action: &'static str,
}

/// Every binding the review answers to, in spec order.
const HELP_ROWS: &[HelpRow] = &[
    HelpRow {
        keys: &[UP, DOWN],
        action: "Move in the navigator; the row opens in the content area",
    },
    HelpRow {
        keys: &[ENTER],
        action: "Open the row and move focus to the content",
    },
    HelpRow {
        keys: &["\u{2190}", "\u{2192}", "Space"],
        action: "Collapse / expand / toggle a tree node",
    },
    HelpRow {
        keys: &["Home", "End"],
        action: "Jump to the first / last row",
    },
    HelpRow {
        keys: &["1", "2"],
        action: "Navigator mode: files / attention",
    },
    HelpRow {
        keys: &["/"],
        action: "Focus the filter box",
    },
    HelpRow {
        keys: &["r"],
        action: "Roles menu",
    },
    HelpRow {
        keys: &["o"],
        action: "Back to the overview",
    },
    HelpRow {
        keys: &["]", "["],
        action: "Next / previous item in the attention order",
    },
    HelpRow {
        keys: &["}", "{"],
        action: "Next / previous changed symbol in the open file",
    },
    HelpRow {
        keys: &["d"],
        action: "Expand / collapse symbol details",
    },
    HelpRow {
        keys: &["s"],
        action: "Split / unified diff",
    },
    HelpRow {
        keys: &["w"],
        action: "Ignore whitespace",
    },
    HelpRow {
        keys: &[FIND_KEY],
        action: "Find in the displayed diff; on the overview it focuses the filter",
    },
    HelpRow {
        keys: &["n", "N"],
        action: "Next / previous search match",
    },
    HelpRow {
        keys: &["y"],
        action: "Copy path:line of the current symbol",
    },
    HelpRow {
        keys: &[COPY_KEY],
        action: "Copy the diff selection, or the navigator row when it has focus",
    },
    HelpRow {
        keys: &["F6"],
        action: "Switch between the navigator and the content",
    },
    HelpRow {
        keys: &["?"],
        action: "This help",
    },
    HelpRow {
        keys: &["Esc"],
        action: "Close find, then back to the overview, then close the review",
    },
];

impl DiffViewer {
    pub(crate) fn render_help_overlay(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.review_ui.help_open {
            return None;
        }
        Some(
            modal_backdrop("review-help-backdrop", t)
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.review_toggle_help(cx)),
                )
                .child(
                    modal_content("review-help", t)
                        .w(px(560.0))
                        .max_h(px(560.0))
                        .p(px(16.0))
                        .gap(px(12.0))
                        .child(
                            v_flex()
                                .flex_shrink_0()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(ui_text(15.0, cx))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(t.text_primary))
                                        .child("Keyboard"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("Esc closes this"),
                                ),
                        )
                        // The table is taller than the card on a short window.
                        .child(
                            div()
                                .id("review-help-rows")
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(v_flex().gap(px(6.0)).children(
                                    HELP_ROWS.iter().map(|row| render_help_row(row, t, cx)),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn render_help_row(row: &HelpRow, t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .gap(px(10.0))
        .items_start()
        .child(
            h_flex()
                .w(px(150.0))
                .flex_shrink_0()
                .gap(px(3.0))
                .children(row.keys.iter().map(|key| key_chip(key, t, cx))),
        )
        .child(
            div()
                .flex_1()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(row.action.to_string()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::super::super::state::{ContentView, FocusRegion};
    use super::super::footer::{COPY_KEY, DOWN, ENTER, FIND_KEY, UP, footer_hints};
    use super::super::{KeyContext, ReviewCommand, dispatch, normalize_key};
    use super::HELP_ROWS;
    use gpui::Modifiers;
    use std::collections::BTreeSet;

    /// Everything a user can press, so the sweep below misses nothing.
    const KEY_CORPUS: &[&str] = &[
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v", "w", "x", "y", "z", "1", "2", "3", "/", "?", "[", "]", "{", "}", "up",
        "down", "left", "right", "home", "end", "enter", "space", "escape", "tab", "f6",
    ];

    /// How the help table and the footer spell the keystroke.
    fn label(key: &str, modifiers: Modifiers) -> String {
        if key == "f6" {
            return "F6".to_string();
        }
        if modifiers.platform || modifiers.control {
            return match key {
                "f" => FIND_KEY,
                "c" => COPY_KEY,
                other => other,
            }
            .to_string();
        }
        let (key, _) = normalize_key(key, modifiers.shift);
        match key {
            "up" => UP,
            "down" => DOWN,
            "enter" => ENTER,
            "left" => "\u{2190}",
            "right" => "\u{2192}",
            "space" => "Space",
            "home" => "Home",
            "end" => "End",
            "f6" => "F6",
            other => other,
        }
        .to_string()
    }

    fn documented() -> BTreeSet<String> {
        HELP_ROWS
            .iter()
            .flat_map(|row| row.keys.iter().map(|key| (*key).to_string()))
            .collect()
    }

    /// Every keystroke that resolves to a command, over every screen state.
    fn reachable() -> BTreeSet<String> {
        let contexts = [
            KeyContext::default(),
            KeyContext {
                focus: FocusRegion::Content,
                ..KeyContext::default()
            },
            KeyContext {
                screen: ContentView::File,
                has_symbols: true,
                split_available: true,
                search_open: true,
                ..KeyContext::default()
            },
            KeyContext {
                screen: ContentView::File,
                focus: FocusRegion::Content,
                has_symbols: true,
                split_available: true,
                search_open: true,
                ..KeyContext::default()
            },
            KeyContext {
                screen: ContentView::File,
                has_commits: true,
                ..KeyContext::default()
            },
        ];
        let modifier_sets = [
            Modifiers::default(),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
            Modifiers {
                function: true,
                ..Modifiers::default()
            },
        ];
        // Esc is bound through the Cancel action, not the key table.
        let mut found = BTreeSet::from(["Esc".to_string()]);
        for base in contexts {
            for modifiers in modifier_sets {
                let ctx = KeyContext { modifiers, ..base };
                for key in KEY_CORPUS {
                    match dispatch(ctx, key) {
                        Some(ReviewCommand::Swallow) | None => {}
                        Some(_) => {
                            found.insert(label(key, modifiers));
                        }
                    }
                }
            }
        }
        found
    }

    #[test]
    fn the_help_table_lists_exactly_what_is_bound() {
        assert_eq!(
            documented(),
            reachable(),
            "the help overlay must document every binding and nothing else"
        );
    }

    #[test]
    fn every_footer_key_is_documented_in_the_help_table() {
        let documented = documented();
        let screens = [
            KeyContext::default(),
            KeyContext {
                screen: ContentView::File,
                has_symbols: true,
                split_available: true,
                ..KeyContext::default()
            },
        ];
        for ctx in screens {
            let (left, right) = footer_hints(ctx);
            for hint in left.iter().chain(right.iter()) {
                for key in hint.keys {
                    assert!(
                        documented.contains(*key),
                        "the footer offers {key} but the help table never explains it"
                    );
                }
            }
        }
    }

    #[test]
    fn no_help_row_is_empty() {
        for row in HELP_ROWS {
            assert!(!row.keys.is_empty(), "{} has no key", row.action);
            assert!(!row.action.is_empty());
        }
    }
}
