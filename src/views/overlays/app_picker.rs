use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::views::components::{
    handle_list_overlay_key, keyboard_hints_footer, modal_backdrop, modal_content,
    search_input_area, substring_filter, ListOverlayAction, ListOverlayConfig, ListOverlayState,
};
use crate::views::layout::app_registry::{all_apps, AppDefinition};
use gpui::*;
use gpui::prelude::*;

/// Entry in the app picker list, derived from AppDefinition.
#[derive(Clone)]
struct AppEntry {
    kind: &'static str,
    display_name: &'static str,
    icon_path: &'static str,
    description: &'static str,
}

impl From<&AppDefinition> for AppEntry {
    fn from(def: &AppDefinition) -> Self {
        Self {
            kind: def.kind,
            display_name: def.display_name,
            icon_path: def.icon_path,
            description: def.description,
        }
    }
}

/// App picker overlay for selecting an app to open.
pub struct AppPickerOverlay {
    focus_handle: FocusHandle,
    state: ListOverlayState<AppEntry>,
}

impl AppPickerOverlay {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let apps: Vec<AppEntry> = all_apps().iter().map(AppEntry::from).collect();

        let config = ListOverlayConfig::new("Open App")
            .searchable("Type to search apps...")
            .size(450.0, 350.0)
            .empty_message("No apps found")
            .keyboard_hints(vec![
                ("Enter", "open as tab"),
                ("⇧+Enter", "replace pane"),
                ("Esc", "close"),
            ])
            .key_context("AppPicker");

        let state = ListOverlayState::new(apps, config, cx);
        let focus_handle = state.focus_handle.clone();

        Self { focus_handle, state }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(AppPickerEvent::Close);
    }

    fn open_as_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(filter_result) = self.state.filtered.get(index) {
            let app = &self.state.items[filter_result.index];
            cx.emit(AppPickerEvent::OpenAsTab {
                app_kind: app.kind.to_string(),
            });
        }
    }

    fn replace_pane(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(filter_result) = self.state.filtered.get(index) {
            let app = &self.state.items[filter_result.index];
            cx.emit(AppPickerEvent::ReplacePane {
                app_kind: app.kind.to_string(),
            });
        }
    }

    fn filter_apps(&mut self) {
        let filtered = substring_filter(&self.state.items, &self.state.search_query, |app| {
            vec![
                app.display_name.to_string(),
                app.description.to_string(),
                app.kind.to_string(),
            ]
        });
        self.state.set_filtered(filtered);
    }

    fn render_app_row(
        &self,
        filtered_index: usize,
        app_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let app = &self.state.items[app_index];
        let is_selected = filtered_index == self.state.selected_index;

        let name = app.display_name;
        let description = app.description;
        let icon_path = app.icon_path;

        div()
            .id(ElementId::Name(format!("app-{}", filtered_index).into()))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_as_tab(filtered_index, cx);
                }),
            )
            .child(
                svg()
                    .path(icon_path)
                    .size(px(20.0))
                    .text_color(rgb(t.text_secondary))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .child(description),
                    ),
            )
    }
}

pub enum AppPickerEvent {
    Close,
    OpenAsTab { app_kind: String },
    ReplacePane { app_kind: String },
}

impl EventEmitter<AppPickerEvent> for AppPickerOverlay {}

impl Render for AppPickerOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let search_query = self.state.search_query.clone();
        let config_width = self.state.config.width;
        let config_max_height = self.state.config.max_height;
        let search_placeholder = self.state.config.search_placeholder.clone().unwrap_or_default();
        let empty_message = self.state.config.empty_message.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("app-picker-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("AppPicker")
            .items_start()
            .pt(px(80.0))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // Intercept Shift+Enter for replace mode before standard handling
                if event.keystroke.key.as_str() == "enter" && event.keystroke.modifiers.shift {
                    let index = this.state.selected_index;
                    this.replace_pane(index, cx);
                    return;
                }

                match handle_list_overlay_key(&mut this.state, event, &[]) {
                    ListOverlayAction::Close => this.close(cx),
                    ListOverlayAction::SelectPrev | ListOverlayAction::SelectNext => {
                        this.state.scroll_to_selected();
                        cx.notify();
                    }
                    ListOverlayAction::Confirm => {
                        let index = this.state.selected_index;
                        this.open_as_tab(index, cx);
                    }
                    ListOverlayAction::QueryChanged => {
                        this.filter_apps();
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("app-picker-modal", &t)
                    .w(px(config_width))
                    .max_h(px(config_max_height))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(search_input_area(&search_query, &search_placeholder, &t))
                    .child(
                        div()
                            .id("app-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.state.scroll_handle)
                            .children(
                                self.state
                                    .filtered
                                    .iter()
                                    .enumerate()
                                    .map(|(i, filter_result)| {
                                        self.render_app_row(i, filter_result.index, cx)
                                    }),
                            )
                            .when(self.state.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(20.0))
                                        .text_size(px(13.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(empty_message.clone()),
                                )
                            }),
                    )
                    .child(keyboard_hints_footer(
                        &[
                            ("Enter", "open as tab"),
                            ("⇧+Enter", "replace pane"),
                            ("Esc", "close"),
                        ],
                        &t,
                    )),
            )
    }
}

impl_focusable!(AppPickerOverlay);
