//! Color picker popover — standalone overlay entity.
//!
//! Shows a color swatch grid for project or folder color selection.
//! Rendered at WindowView level via OverlayManager, like context menus.

use okena_core::theme::FolderColor;
use okena_ui::overlay::CloseEvent;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;
use okena_workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;

use crate::Cancel;

/// What the color picker targets.
#[derive(Clone)]
pub enum ColorPickerTarget {
    Project { project_id: String },
    Folder { folder_id: String },
}

/// Event emitted by ColorPickerPopover.
pub enum ColorPickerPopoverEvent {
    Close,
    /// Color was set on a project — sidebar should handle remote sync.
    ProjectColorChanged { project_id: String, color: FolderColor },
}

impl CloseEvent for ColorPickerPopoverEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl EventEmitter<ColorPickerPopoverEvent> for ColorPickerPopover {}

/// Standalone color picker popover entity.
pub struct ColorPickerPopover {
    workspace: Entity<Workspace>,
    target: ColorPickerTarget,
    position: Point<Pixels>,
    focus_handle: FocusHandle,
}

impl ColorPickerPopover {
    pub fn new(
        workspace: Entity<Workspace>,
        target: ColorPickerTarget,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self { workspace, target, position, focus_handle }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ColorPickerPopoverEvent::Close);
    }
}

/// Render the color swatch grid.
fn color_swatch_grid(
    id_prefix: &str,
    current_color: FolderColor,
    t: &okena_core::theme::ThemeColors,
    cx: &mut Context<ColorPickerPopover>,
    on_select: impl Fn(&mut ColorPickerPopover, FolderColor, &mut Window, &mut Context<ColorPickerPopover>) + 'static,
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
        .children(colors.into_iter().map(|(color, hex)| {
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

impl Render for ColorPickerPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let panel = match &self.target {
            ColorPickerTarget::Project { project_id } => {
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

                let project_id_owned = project_id.clone();

                okena_ui::popover::popover_panel("color-picker-panel", &t)
                    .child({
                        let pid = project_id_owned.clone();
                        color_swatch_grid("color", current_color, &t, cx, move |this, color, _window, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.set_folder_color(&pid, color, cx);
                            });
                            cx.emit(ColorPickerPopoverEvent::ProjectColorChanged {
                                project_id: pid.clone(),
                                color,
                            });
                            this.close(cx);
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
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_secondary))
                                        .hover(|s| s.text_color(rgb(t.text_primary)).bg(rgb(t.bg_hover)))
                                        .child("Reset to parent")
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                            this.workspace.update(cx, |ws, cx| {
                                                ws.set_worktree_color_override(&project_id_clone, None, cx);
                                            });
                                            this.close(cx);
                                        }))
                                )
                        )
                    })
                    .into_any_element()
            }
            ColorPickerTarget::Folder { folder_id } => {
                let current_color = self.workspace.read(cx)
                    .folder(folder_id)
                    .map(|f| f.folder_color)
                    .unwrap_or_default();

                let folder_id_owned = folder_id.clone();

                okena_ui::popover::popover_panel("folder-color-picker-panel", &t)
                    .child({
                        let fid = folder_id_owned.clone();
                        color_swatch_grid("folder-color", current_color, &t, cx, move |this, color, _window, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.set_folder_item_color(&fid, color, cx);
                            });
                            this.close(cx);
                        })
                    })
                    .into_any_element()
            }
        };

        let position = self.position;

        div()
            .track_focus(&self.focus_handle)
            .key_context("ColorPickerPopover")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .occlude()
            .id("color-picker-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_scroll_wheel(|_, _, cx| { cx.stop_propagation(); })
            .child(deferred(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(panel)
            ))
    }
}
