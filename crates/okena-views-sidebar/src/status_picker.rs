//! Status picker popover — the list behind the status pill in a project
//! column's header. Standalone overlay entity rendered at WindowView level
//! via OverlayManager, like the color picker.

use gpui::prelude::*;
use gpui::*;
use okena_ui::overlay::CloseEvent;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_md, ui_text_xs};
use okena_workspace::state::Workspace;

use crate::Cancel;

pub enum StatusPickerEvent {
    Close,
    /// A column was picked (`None` = clear the status).
    StatusChanged {
        project_id: String,
        status: Option<String>,
    },
    /// "Edit columns…" — the host opens the settings panel.
    EditColumns,
}

impl CloseEvent for StatusPickerEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

impl EventEmitter<StatusPickerEvent> for StatusPickerPopover {}

pub struct StatusPickerPopover {
    workspace: Entity<Workspace>,
    project_id: String,
    position: Point<Pixels>,
    focus_handle: FocusHandle,
}

impl StatusPickerPopover {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            workspace,
            project_id,
            position,
            focus_handle: cx.focus_handle(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(StatusPickerEvent::Close);
    }

    fn pick(&self, status: Option<String>, cx: &mut Context<Self>) {
        cx.emit(StatusPickerEvent::StatusChanged {
            project_id: self.project_id.clone(),
            status,
        });
        self.close(cx);
    }
}

impl Render for StatusPickerPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let current = self
            .workspace
            .read(cx)
            .project(&self.project_id)
            .and_then(|p| p.status.clone());
        let columns = okena_ui::board::board_columns(cx).to_vec();
        let known = current
            .as_deref()
            .is_some_and(|s| columns.iter().any(|c| c.id == s));

        let row = |id: String,
                   dot: Option<u32>,
                   label: String,
                   selected: bool,
                   muted: bool,
                   cx: &mut Context<Self>| {
            div()
                .id(ElementId::Name(id.into()))
                .h(px(26.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .text_size(ui_text_md(cx))
                .text_color(rgb(if muted { t.text_muted } else { t.text_primary }))
                .when(selected, |d| d.bg(rgb(t.bg_hover)))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(match dot {
                    Some(hex) => div()
                        .size(px(7.0))
                        .rounded_full()
                        .bg(rgb(hex))
                        .flex_shrink_0(),
                    None => div().w(px(7.0)).flex_shrink_0(),
                })
                .child(div().flex_1().child(label))
                .when(selected, |d| {
                    d.child(
                        svg()
                            .path("icons/check.svg")
                            .size(px(11.0))
                            .text_color(rgb(t.text_muted)),
                    )
                })
        };

        let mut panel = okena_ui::popover::popover_panel("status-picker-panel", &t)
            .w(px(180.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(
                div()
                    .px(px(8.0))
                    .pt(px(4.0))
                    .pb(px(2.0))
                    .text_size(ui_text_xs(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_muted))
                    .child("STATUS"),
            );
        for column in &columns {
            let selected = current.as_deref() == Some(column.id.as_str());
            let status = Some(column.id.clone());
            let dot = match column.color {
                okena_core::theme::FolderColor::Default => t.text_muted,
                color => t.get_folder_color(color),
            };
            panel = panel.child(
                row(
                    format!("status-pick-{}", column.id),
                    Some(dot),
                    column.name.clone(),
                    selected,
                    false,
                    cx,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| this.pick(status.clone(), cx)),
                ),
            );
        }
        panel = panel
            .child(div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(rgb(t.border)))
            .child(
                row(
                    "status-pick-none".to_string(),
                    None,
                    "No status".to_string(),
                    !known,
                    true,
                    cx,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.pick(None, cx)),
                ),
            )
            .child(
                row(
                    "status-pick-edit".to_string(),
                    None,
                    "Edit columns…".to_string(),
                    false,
                    true,
                    cx,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.emit(StatusPickerEvent::EditColumns);
                        this.close(cx);
                    }),
                ),
            );

        let position = self.position;
        div()
            .track_focus(&self.focus_handle)
            .key_context("StatusPickerPopover")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
            .absolute()
            .inset_0()
            .occlude()
            .id("status-picker-backdrop")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(deferred(
                anchored().position(position).snap_to_window().child(panel),
            ))
    }
}
