//! Color picker component for folder colors

use crate::theme::{theme, FolderColor};
use gpui::*;
use gpui::prelude::*;

use super::Sidebar;

/// Render the color swatch grid shared by project and folder color pickers.
fn color_swatch_grid(
    id_prefix: &str,
    current_color: FolderColor,
    t: &crate::theme::ThemeColors,
    cx: &mut Context<Sidebar>,
    on_select: impl Fn(&mut Sidebar, FolderColor, &mut Window, &mut Context<Sidebar>) + 'static,
) -> Div {
    let colors: Vec<(FolderColor, u32)> = FolderColor::all()
        .iter()
        .map(|&color| (color, t.get_folder_color(color)))
        .collect();

    let on_select = std::rc::Rc::new(on_select);
    let prefix = id_prefix.to_string();

    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .w(px(126.0))
        .children(colors.into_iter().map(|( color, hex)| {
            let is_selected = color == current_color;

            div()
                .id(ElementId::Name(format!("{}-{:?}", prefix, color).into()))
                .w(px(24.0))
                .h(px(24.0))
                .rounded(px(4.0))
                .bg(rgb(hex))
                .cursor_pointer()
                .when(is_selected, |d| {
                    d.border_2().border_color(rgb(t.text_primary))
                })
                .when(!is_selected, |d| {
                    d.border_1().border_color(rgb(t.border))
                })
                .hover(|s| s.opacity(0.8))
                .on_mouse_down(MouseButton::Left, {
                    let on_select = on_select.clone();
                    cx.listener(move |this, _, _window, cx| {
                        on_select(this, color, _window, cx);
                    })
                })
        }))
}

impl Sidebar {
    pub(super) fn render_color_picker(&self, project_id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let ws = self.workspace.read(cx);

        let (current_color, has_color_override) = ws.project(project_id)
            .map(|p| {
                let color = ws.effective_folder_color(p);
                let has_override = p.worktree_info.as_ref()
                    .and_then(|wt| wt.color_override)
                    .is_some();
                (color, has_override)
            })
            .unwrap_or_default();

        let project_id_owned = project_id.to_string();

        okena_ui::popover::popover_panel("color-picker-panel", &t)
            .child({
                let pid = project_id_owned.clone();
                color_swatch_grid("color", current_color, &t, cx, move |this, color, _window, cx| {
                    this.set_folder_color(&pid, color, cx);
                })
            })
            .when(has_color_override, |panel| {
                let project_id_clone = project_id_owned.clone();
                panel.child(
                    div()
                        .id("reset-worktree-color")
                        .mt(px(6.0))
                        .pt(px(6.0))
                        .border_t_1()
                        .border_color(rgb(t.border))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("reset-worktree-color-btn")
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_secondary))
                                .hover(|s| s.text_color(rgb(t.text_primary)).bg(rgb(t.bg_hover)))
                                .child("Reset to parent")
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                    this.reset_worktree_color(&project_id_clone, cx);
                                }))
                        )
                )
            })
    }

    pub(super) fn render_folder_color_picker(&self, folder_id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let current_color = self.workspace.read(cx)
            .folder(folder_id)
            .map(|f| f.folder_color)
            .unwrap_or_default();

        let folder_id_owned = folder_id.to_string();

        okena_ui::popover::popover_panel("folder-color-picker-panel", &t)
            .child({
                let fid = folder_id_owned.clone();
                color_swatch_grid("folder-color", current_color, &t, cx, move |this, color, _window, cx| {
                    this.set_folder_item_color(&fid, color, cx);
                })
            })
    }
}
