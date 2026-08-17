//! Footer hints — only keys that work on the current screen — spec §11.

use super::super::super::DiffViewer;
use super::super::state::ContentView;
use super::KeyContext;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::ui_text_ms;

/// Spec §3: the footer is 28 px and uses both halves.
const FOOTER_HEIGHT: f32 = 28.0;

const fn platform_key(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

pub(super) const FIND_KEY: &str = platform_key("\u{2318}F", "Ctrl+F");
pub(super) const COPY_KEY: &str = platform_key("\u{2318}C", "Ctrl+C");
pub(super) const UP: &str = "\u{2191}";
pub(super) const DOWN: &str = "\u{2193}";
pub(super) const ENTER: &str = "\u{21B5}";

/// One footer entry: the key chips that trigger it and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hint {
    pub keys: &'static [&'static str],
    pub action: &'static str,
}

/// The hints for the current screen, left half then right half.
pub(crate) fn footer_hints(ctx: KeyContext) -> (Vec<Hint>, Vec<Hint>) {
    match ctx.screen {
        ContentView::Overview => overview_hints(),
        ContentView::File => file_hints(ctx),
    }
}

fn overview_hints() -> (Vec<Hint>, Vec<Hint>) {
    (
        vec![
            Hint {
                keys: &[UP, DOWN],
                action: "navigate",
            },
            Hint {
                keys: &[ENTER],
                action: "open",
            },
            Hint {
                keys: &["1", "2"],
                action: "files \u{00B7} attention",
            },
            Hint {
                keys: &["/"],
                action: "filter",
            },
            Hint {
                keys: &["r"],
                action: "roles",
            },
            Hint {
                keys: &["?"],
                action: "keys",
            },
        ],
        vec![Hint {
            keys: &["Esc"],
            action: "close",
        }],
    )
}

fn file_hints(ctx: KeyContext) -> (Vec<Hint>, Vec<Hint>) {
    let mut left = Vec::new();
    if ctx.has_symbols {
        left.push(Hint {
            keys: &["}", "{"],
            action: "next / prev symbol",
        });
    }
    left.push(Hint {
        keys: &["]", "["],
        action: if ctx.has_commits {
            "next / prev commit"
        } else {
            "next / prev in queue"
        },
    });
    left.push(Hint {
        keys: &["d"],
        action: "details",
    });
    left.push(Hint {
        keys: &["o"],
        action: "overview",
    });
    if ctx.split_available {
        left.push(Hint {
            keys: &["s"],
            action: "split",
        });
    }
    left.push(Hint {
        keys: &["w"],
        action: "whitespace",
    });
    left.push(Hint {
        keys: &[FIND_KEY],
        action: "find",
    });

    let right = vec![
        Hint {
            keys: &["y"],
            action: "copy path:line",
        },
        Hint {
            keys: &[COPY_KEY],
            action: "copy",
        },
        Hint {
            keys: &["Esc"],
            action: "back",
        },
    ];
    (left, right)
}

impl DiffViewer {
    pub(crate) fn render_review_footer(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (left, right) = footer_hints(self.review_screen_context());
        div()
            .h(px(FOOTER_HEIGHT))
            .flex_shrink_0()
            .px(px(12.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(hint_row(&left, t, cx))
            .child(hint_row(&right, t, cx))
            .into_any_element()
    }
}

fn hint_row(hints: &[Hint], t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .gap(px(14.0))
        .children(hints.iter().map(|hint| render_hint_item(*hint, t, cx)))
        .into_any_element()
}

fn render_hint_item(hint: Hint, t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .gap(px(6.0))
        .child(
            h_flex()
                .gap(px(3.0))
                .children(hint.keys.iter().map(|key| key_chip(key, t, cx))),
        )
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(hint.action.to_string()),
        )
        .into_any_element()
}

/// A keycap. Shared with the help overlay so both spell keys the same way.
pub(super) fn key_chip(key: &str, t: &ThemeColors, cx: &App) -> AnyElement {
    div()
        .px(px(5.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(ui_text_ms(cx))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(t.text_muted))
        .child(key.to_string())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::super::super::state::ContentView;
    use super::super::KeyContext;
    use super::{FIND_KEY, Hint, footer_hints};

    fn actions(hints: &[Hint]) -> Vec<&'static str> {
        hints.iter().map(|hint| hint.action).collect()
    }

    #[test]
    fn the_overview_lists_the_navigator_keys_and_closes_on_the_right() {
        let (left, right) = footer_hints(KeyContext::default());
        assert_eq!(
            actions(&left),
            [
                "navigate",
                "open",
                "files \u{00B7} attention",
                "filter",
                "roles",
                "keys"
            ]
        );
        assert_eq!(actions(&right), ["close"]);
    }

    #[test]
    fn the_file_screen_lists_the_reading_keys_and_goes_back_on_the_right() {
        let ctx = KeyContext {
            screen: ContentView::File,
            has_symbols: true,
            split_available: true,
            ..KeyContext::default()
        };
        let (left, right) = footer_hints(ctx);
        assert_eq!(
            actions(&left),
            [
                "next / prev symbol",
                "next / prev in queue",
                "details",
                "overview",
                "split",
                "whitespace",
                "find"
            ]
        );
        assert_eq!(actions(&right), ["copy path:line", "copy", "back"]);
        assert!(
            left.iter().any(|hint| hint.keys == [FIND_KEY]),
            "find carries the platform accelerator"
        );
    }

    #[test]
    fn hints_whose_action_is_unavailable_are_dropped() {
        let ctx = KeyContext {
            screen: ContentView::File,
            ..KeyContext::default()
        };
        let (left, _) = footer_hints(ctx);
        assert_eq!(
            actions(&left),
            [
                "next / prev in queue",
                "details",
                "overview",
                "whitespace",
                "find"
            ]
        );
    }

    #[test]
    fn a_commit_list_renames_the_bracket_hint() {
        let ctx = KeyContext {
            screen: ContentView::File,
            has_commits: true,
            ..KeyContext::default()
        };
        let (left, _) = footer_hints(ctx);
        assert!(actions(&left).contains(&"next / prev commit"));
        assert!(!actions(&left).contains(&"next / prev in queue"));
    }
}
