//! The bar above the grid: what it is showing, and how.
//!
//! Projects and agents are shown by the same grid, and both offer the same two
//! choices — how the columns are arranged, and whether they open on their
//! terminal or their info. Those lived only in the sidebar's "..." menu, away
//! from the grid they change; the bar puts them above it.
//!
//! Everything goes through `grid_layout_mode` / `grid_show_info`, which pick
//! whichever overview is on screen, so the bar always acts on what you are
//! looking at.

use crate::theme::theme;
use crate::ui::tokens::ui_text;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_ui::toggle::{Segment, segmented_control};
use okena_workspace::state::ProjectLayoutMode;

use super::WindowView;

/// What the bar calls the grid beneath it.
///
/// A folder's name beats "Projects": seeing the folder named is how you tell
/// the grid is filtered at all.
fn grid_title(agents_overview: bool, folder: Option<&str>) -> String {
    if agents_overview {
        "Agents".to_string()
    } else {
        folder.unwrap_or("Projects").to_string()
    }
}

impl WindowView {
    /// The view bar, or `None` while a project is zoomed to fill the window:
    /// zoom is for room, and every choice here is about the overview.
    pub(super) fn render_view_mode_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self
            .focus_manager
            .read(cx)
            .fullscreen_project_id()
            .is_some()
        {
            return None;
        }
        let t = theme(cx);
        let window_id = self.window_id;
        let (title, rows, show_info) = {
            let ws = self.workspace.read(cx);
            let agents = ws
                .data()
                .window(window_id)
                .is_some_and(|w| w.agents_overview);
            let folder = ws
                .active_folder_filter(window_id)
                .and_then(|id| ws.folder(id))
                .map(|f| f.name.clone());
            (
                grid_title(agents, folder.as_deref()),
                ws.grid_layout_mode(window_id).is_rows(),
                ws.grid_show_info(window_id),
            )
        };

        let layout_segments = [
            Segment {
                id: "columns".into(),
                label: "Columns",
                selected: !rows,
                disabled: false,
                tooltip: None,
            },
            Segment {
                id: "stacked".into(),
                label: "Stacked",
                selected: rows,
                disabled: false,
                tooltip: None,
            },
            // Offered before it exists, so the bar keeps its shape when it lands.
            Segment {
                id: "canvas".into(),
                label: "Canvas",
                selected: false,
                disabled: true,
                tooltip: Some("Canvas — coming soon".into()),
            },
        ];
        let workspace = self.workspace.clone();
        let layout = segmented_control(
            "view-layout",
            &layout_segments,
            &t,
            cx,
            move |i, _window, cx| {
                let mode = match i {
                    0 => ProjectLayoutMode::Columns,
                    1 => ProjectLayoutMode::Rows,
                    _ => return,
                };
                workspace.update(cx, |ws, cx| ws.set_grid_layout_mode(window_id, mode, cx));
            },
        );

        let content_segments = [
            Segment {
                id: "info".into(),
                label: "Info",
                selected: show_info,
                disabled: false,
                tooltip: None,
            },
            Segment {
                id: "terminals".into(),
                label: "Terminals",
                selected: !show_info,
                disabled: false,
                tooltip: None,
            },
        ];
        let workspace = self.workspace.clone();
        let content = segmented_control(
            "view-content",
            &content_segments,
            &t,
            cx,
            move |i, _window, cx| {
                workspace.update(cx, |ws, cx| ws.set_grid_show_info(window_id, i == 0, cx));
            },
        );

        Some(
            h_flex()
                .id("view-mode-bar")
                .w_full()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(4.0))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text(13.0, cx))
                        .text_color(rgb(t.text_primary))
                        .child(title),
                )
                .child(layout)
                .child(content)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::grid_title;

    #[test]
    fn the_bar_names_the_overview_on_screen() {
        assert_eq!(grid_title(false, None), "Projects");
        assert_eq!(grid_title(true, None), "Agents");
    }

    #[test]
    fn a_filtered_grid_is_named_by_its_folder() {
        assert_eq!(grid_title(false, Some("Clients")), "Clients");
        // The agents overview clears the folder filter, so a stale one must not
        // rename it.
        assert_eq!(grid_title(true, Some("Clients")), "Agents");
    }
}
