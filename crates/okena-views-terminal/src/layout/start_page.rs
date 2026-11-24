use crate::ActionDispatch;
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::api::ActionRequest;
use okena_core::shell::ShellType;
use okena_files::theme::theme;
use okena_ui::tokens::{ui_text_md, ui_text_xl};
use okena_workspace::settings::StartPageButton;

pub fn render_start_page_content<D: ActionDispatch>(
    project_id: &str,
    page_id: Option<&str>,
    buttons: Option<&[StartPageButton]>,
    dispatcher: Option<&D>,
    tip: AnyView,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    v_flex()
        .items_center()
        .justify_center()
        .w_full()
        .min_h(relative(1.0))
        .py(px(12.0))
        .gap(px(16.0))
        .child(
            svg()
                .path("icons/folder.svg")
                .size(px(48.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(
            div()
                .text_size(ui_text_xl(cx))
                .text_color(rgb(t.text_muted))
                .child(if page_id.is_some() {
                    "Start Page"
                } else {
                    "No terminal attached"
                }),
        )
        .child(render_start_page_buttons(
            project_id, page_id, buttons, dispatcher, cx,
        ))
        .child(tip)
}

pub(super) fn launch_request(
    project_id: &str,
    page_id: Option<&str>,
    shell: ShellType,
    command: Option<String>,
) -> ActionRequest {
    ActionRequest::LaunchStartPage {
        project_id: project_id.to_owned(),
        page_id: page_id.map(str::to_owned),
        shell,
        command,
    }
}

/// Shared launcher for a pending pane and the existing empty-project screen.
/// A missing button list keeps the empty project's original direct-start action.
fn render_start_page_buttons<D: ActionDispatch>(
    project_id: &str,
    page_id: Option<&str>,
    buttons: Option<&[StartPageButton]>,
    dispatcher: Option<&D>,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let launcher = |id: SharedString, label: SharedString, shell, command: Option<String>| {
        let primary = command.is_none();
        let action = if buttons.is_some() {
            launch_request(project_id, page_id, shell, command)
        } else {
            ActionRequest::CreateTerminal {
                project_id: project_id.to_owned(),
                start_page: false,
            }
        };
        let dispatcher = dispatcher.cloned();
        let (fg, bg, hover) = if primary {
            (
                t.button_primary_fg,
                t.button_primary_bg,
                t.button_primary_hover,
            )
        } else {
            (t.text_primary, t.bg_secondary, t.bg_hover)
        };

        div()
            .id(id.clone())
            .debug_selector(move || id.to_string())
            .focusable()
            .tab_stop(true)
            .focus_visible(move |style| style.bg(rgb(hover)).underline())
            .cursor_pointer()
            .max_w_full()
            .px(px(16.))
            .py(px(8.))
            .rounded(px(6.))
            .bg(rgb(bg))
            .hover(move |style| style.bg(rgb(hover)))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_color(rgb(fg))
            .text_size(ui_text_md(cx))
            .font_weight(FontWeight::MEDIUM)
            .when(primary, |button| {
                button.child(
                    svg()
                        .path("icons/terminal.svg")
                        .size(px(14.))
                        .flex_shrink_0()
                        .text_color(rgb(fg)),
                )
            })
            .child(div().min_w_0().child(label))
            .on_click(move |_, _, cx| {
                if let Some(dispatcher) = &dispatcher {
                    dispatcher.dispatch(action.clone(), cx);
                }
                cx.stop_propagation();
            })
    };

    div()
        .flex()
        .flex_wrap()
        .justify_center()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .child(launcher(
            "start-terminal-btn".into(),
            "Start Terminal".into(),
            ShellType::Default,
            None,
        ))
        .children(
            buttons
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, button)| {
                    launcher(
                        format!("start-page-button-{index}").into(),
                        button.name.clone().into(),
                        button.shell.clone().unwrap_or_default(),
                        Some(button.command.clone()),
                    )
                }),
        )
}
