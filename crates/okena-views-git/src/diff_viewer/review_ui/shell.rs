//! Composes the review workspace: navigator column, content column, overlays.

use super::super::DiffViewer;
use super::diff_state::SmartDiffViewState;
use super::state::ContentView;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::resizable_sidebar::resizable_sidebar;
use std::sync::Arc;

/// What `render_diff_pane` needs; the render pass computes all of it.
pub(crate) struct DiffPaneArgs {
    pub is_binary: bool,
    pub file_path: String,
    pub line_count: usize,
    pub gutter_width: f32,
    pub theme_colors: Arc<ThemeColors>,
}

impl DiffViewer {
    pub(crate) fn render_review_shell(
        &mut self,
        t: &ThemeColors,
        diff_pane: DiffPaneArgs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let navigator = self.render_navigator(t, cx);
        let sidebar = self.render_navigator_column(t, navigator, cx);
        let content = match self.review_ui.content {
            ContentView::Overview => self.render_overview(t, cx),
            ContentView::File => self.render_file_content(t, diff_pane, cx),
        };
        let status_popover = self.render_status_popover(t, cx);
        let roles_menu = self.render_roles_menu(t, cx);
        let outline = self.render_outline_popover(t, cx);
        let help = self.render_help_overlay(t, cx);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // Overlays position themselves against this box.
            .relative()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(sidebar)
                    .child(content),
            )
            .children(status_popover)
            .children(roles_menu)
            .children(outline)
            .children(help)
            .into_any_element()
    }

    fn render_navigator_column(
        &self,
        t: &ThemeColors,
        navigator: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity().downgrade();
        let entity_for_end = entity.clone();
        resizable_sidebar(
            self.sidebar_resize.width(),
            t.bg_primary,
            t.border,
            t.border_active,
            vec![navigator],
            move |mouse_pos, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, _| {
                        this.sidebar_resize.start_resize(f32::from(mouse_pos.x));
                    });
                }
            },
            move |cx| {
                if let Some(entity) = entity_for_end.upgrade() {
                    entity.update(cx, |this, _| this.sidebar_resize.end_resize());
                }
            },
        )
        .into_any_element()
    }

    fn render_file_content(
        &mut self,
        t: &ThemeColors,
        diff_pane: DiffPaneArgs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = self.render_file_header(t, cx);
        let symbol_bar = self.render_symbol_bar(t, cx);
        let unavailable = self.render_navigation_unavailable(t, cx);
        let state = self.smart_diff_view_state();
        let body = if state == SmartDiffViewState::Ready {
            self.render_diff_pane(
                t,
                diff_pane.is_binary,
                diff_pane.file_path,
                diff_pane.line_count,
                diff_pane.gutter_width,
                diff_pane.theme_colors,
                cx,
            )
            .into_any_element()
        } else {
            self.render_smart_diff_state(state, t, cx)
        };

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .children(symbol_bar)
            .children(unavailable)
            .child(body)
            .into_any_element()
    }
}
