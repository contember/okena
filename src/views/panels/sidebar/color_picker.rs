//! Color picker component for folder colors

use crate::theme::{theme, FolderColor};
use gpui::*;
use gpui::prelude::*;

use super::Sidebar;

impl Sidebar {
    /// Compute the top position for the color picker relative to the sidebar container.
    /// Converts the stored window-Y click coordinate to sidebar-relative Y,
    /// then clamps so the picker stays visible.
    fn color_picker_top(&self) -> f32 {
        let title_bar_offset = if cfg!(target_os = "macos") { 28.0 } else { 32.0 };
        // Position the picker so its top aligns near the clicked item
        let y = self.color_picker_click_y - title_bar_offset - 4.0;
        // Clamp: minimum below sidebar headers (35 + 28 = 63), no less than 8
        y.max(8.0)
    }

    pub(super) fn render_color_picker(&self, project_id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Get current color for this project
        let current_color = self.workspace.read(cx)
            .project(project_id)
            .map(|p| p.folder_color)
            .unwrap_or_default();

        let project_id_owned = project_id.to_string();

        // Build color swatches
        let colors: Vec<(FolderColor, u32)> = FolderColor::all()
            .iter()
            .map(|&color| (color, t.get_folder_color(color)))
            .collect();

        let picker_top = self.color_picker_top();

        div()
            .absolute()
            .occlude()
            .top(px(picker_top))
            .left(px(30.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(6.0))
            .shadow_lg()
            .p(px(8.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                // Grid of color swatches (3 rows x 4 columns = 12 colors)
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .w(px(126.0))  // 4 swatches * 24px + 3 gaps * 6px + padding
                    .children(colors.into_iter().map(|(color, hex)| {
                        let is_selected = color == current_color;
                        let project_id_clone = project_id_owned.clone();

                        div()
                            .id(ElementId::Name(format!("color-{:?}", color).into()))
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
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                this.set_folder_color(&project_id_clone, color, cx);
                            }))
                    }))
            )
    }

    pub(super) fn render_folder_color_picker(&self, folder_id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Get current color for this folder
        let current_color = self.workspace.read(cx)
            .folder(folder_id)
            .map(|f| f.folder_color)
            .unwrap_or_default();

        let folder_id_owned = folder_id.to_string();

        // Build color swatches
        let colors: Vec<(FolderColor, u32)> = FolderColor::all()
            .iter()
            .map(|&color| (color, t.get_folder_color(color)))
            .collect();

        let picker_top = self.color_picker_top();

        div()
            .absolute()
            .occlude()
            .top(px(picker_top))
            .left(px(30.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(6.0))
            .shadow_lg()
            .p(px(8.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .w(px(126.0))
                    .children(colors.into_iter().map(|(color, hex)| {
                        let is_selected = color == current_color;
                        let folder_id_clone = folder_id_owned.clone();

                        div()
                            .id(ElementId::Name(format!("folder-color-{:?}", color).into()))
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
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                this.set_folder_item_color(&folder_id_clone, color, cx);
                            }))
                    }))
            )
    }
}
