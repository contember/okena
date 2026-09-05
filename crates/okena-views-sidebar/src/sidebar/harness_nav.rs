//! Harness view nav, rendered above the project list.
//!
//! Clicking an entry pushes a `WorkbenchRequest` through the shared
//! `RequestBroker`; the window drains it and opens the view as a tab in the
//! main area. The sidebar never holds the window's entity, so the broker is the
//! only channel between them.

use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::harness::HarnessSection;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text, ui_text_ms};
use okena_workspace::harness_state::active_harness;
use okena_workspace::requests::WorkbenchRequest;

use super::Sidebar;

impl Sidebar {
    pub(super) fn render_harness_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        let active = active_harness(self.window_id, cx);

        let items: Vec<AnyElement> = HarnessSection::all()
            .into_iter()
            .map(|section| {
                let broker = self.request_broker.clone();
                let is_active = active == Some(section);
                div()
                    .id(SharedString::from(format!(
                        "harness-nav-{}",
                        section.slug()
                    )))
                    .cursor_pointer()
                    .h(px(24.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .when(is_active, |d| d.bg(rgb(t.bg_selection)))
                    .when(!is_active, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                    .text_size(ui_text(13.0, cx))
                    .text_color(if is_active {
                        rgb(t.text_primary)
                    } else {
                        rgb(t.text_secondary)
                    })
                    .child(section.label())
                    .on_click(move |_, _window, cx| {
                        broker.update(cx, |b, cx| {
                            b.push_workbench_request(
                                WorkbenchRequest::OpenHarnessView(section),
                                cx,
                            );
                        });
                    })
                    .into_any_element()
            })
            .collect();

        v_flex()
            .child(
                div()
                    .h(px(28.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("HARNESS"),
            )
            .children(items)
            .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
    }
}

impl Sidebar {
    /// Leave the harness view so the projects grid shows again.
    ///
    /// Called from every path that selects a project — click and keyboard
    /// alike. Without it, selecting a project while a harness view is up would
    /// change the focus but leave the view covering the whole main area, so
    /// nothing would appear to happen.
    pub(crate) fn leave_harness_view(&self, cx: &mut App) {
        okena_workspace::harness_state::set_active_harness(self.window_id, None, cx);
    }
}
