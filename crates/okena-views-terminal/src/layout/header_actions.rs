//! Actions shared by normal and Focus terminal headers.
use crate::ActionDispatch;
use gpui::*;
use okena_files::theme::theme;
use okena_ui::header_buttons::{ButtonSize, HeaderAction, header_button_base};
use okena_workspace::{
    focus::FocusManager,
    request_broker::RequestBroker,
    state::{LayoutNode, SplitDirection, Workspace},
};

/// Context for tab action button closures.
#[derive(Clone)]
pub(crate) struct TabActionContext<D: ActionDispatch> {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub active_tab: usize,
    pub standalone: bool,
    pub action_dispatcher: Option<D>,
}

pub(crate) fn render_action_buttons<D: ActionDispatch + Send + Sync>(
    ctx: TabActionContext<D>,
    terminal_id: Option<String>,
    request_broker: Entity<RequestBroker>,
    can_export_buffer: bool,
    focus_manager: Entity<FocusManager>,
    workspace: Entity<Workspace>,
    cx: &App,
) -> (Vec<AnyElement>, Div) {
    let t = theme(cx);
    let id_suffix = format!("tabs-{:?}", ctx.layout_path);

    let terminal_id_for_close = terminal_id.clone();

    let ctx_split_v = ctx.clone();
    let ctx_split_h = ctx.clone();
    let ctx_add_tab = ctx.clone();
    let ctx_close = ctx.clone();

    let standalone = ctx.standalone;
    let terminal_layout_path = if standalone {
        ctx.layout_path.clone()
    } else {
        let mut path = ctx.layout_path.clone();
        path.push(ctx.active_tab);
        path
    };
    // Group split/add-tab callbacks address the group, while keyboard actions
    // address its focused leaf. Only standalone targets are equivalent.
    let shortcut_guard = || {
        let focus_manager = focus_manager.clone();
        let workspace = workspace.clone();
        let project_id = ctx.project_id.clone();
        let path = terminal_layout_path.clone();
        let terminal_id = terminal_id.clone();
        move |window: &Window, cx: &App| {
            window
                .context_stack()
                .iter()
                .any(|context| context.contains("TerminalPane"))
                && focus_manager.read(cx).is_focused(&project_id, &path)
                && workspace.read(cx).project(&project_id)
                    .and_then(|project| project.layout.as_ref())
                    .and_then(|layout| layout.get_at_path(&path))
                    .is_some_and(|node| matches!(node, LayoutNode::Terminal { terminal_id: Some(id), .. } if Some(id) == terminal_id.as_ref()))
        }
    };
    let more_button = crate::layout::layout_container::terminal_actions_button(
        HeaderAction::More,
        &id_suffix,
        ctx.project_id.clone(),
        request_broker,
        terminal_layout_path.clone(),
        terminal_id.clone(),
        can_export_buffer,
        true,
        (!standalone).then(|| ctx.layout_path.clone()),
        cx,
    );

    let extra = vec![
        header_button_base(
            HeaderAction::SplitVertical,
            &id_suffix,
            ButtonSize::COMPACT,
            &t,
            (!standalone).then_some("Split Tab Group Vertically"),
            standalone.then(|| {
                okena_ui::shortcuts::ActionShortcut::new(
                    crate::actions::SplitVertical,
                    shortcut_guard(),
                )
            }),
        )
        .on_click(move |_, _window, cx| {
            if let Some(ref dispatcher) = ctx_split_v.action_dispatcher {
                dispatcher.dispatch(
                    okena_core::api::ActionRequest::SplitTerminal {
                        project_id: ctx_split_v.project_id.clone(),
                        path: ctx_split_v.layout_path.clone(),
                        direction: SplitDirection::Vertical,
                    },
                    cx,
                );
            }
        })
        .into_any_element(),
        header_button_base(
            HeaderAction::SplitHorizontal,
            &id_suffix,
            ButtonSize::COMPACT,
            &t,
            (!standalone).then_some("Split Tab Group Horizontally"),
            standalone.then(|| {
                okena_ui::shortcuts::ActionShortcut::new(
                    crate::actions::SplitHorizontal,
                    shortcut_guard(),
                )
            }),
        )
        .on_click(move |_, _window, cx| {
            if let Some(ref dispatcher) = ctx_split_h.action_dispatcher {
                dispatcher.dispatch(
                    okena_core::api::ActionRequest::SplitTerminal {
                        project_id: ctx_split_h.project_id.clone(),
                        path: ctx_split_h.layout_path.clone(),
                        direction: SplitDirection::Horizontal,
                    },
                    cx,
                );
            }
        })
        .into_any_element(),
        header_button_base(
            HeaderAction::AddTab,
            &id_suffix,
            ButtonSize::COMPACT,
            &t,
            (!standalone).then_some("Add Tab to Group"),
            standalone.then(|| {
                okena_ui::shortcuts::ActionShortcut::new(crate::actions::AddTab, shortcut_guard())
            }),
        )
        .on_click(move |_, _window, cx| {
            if let Some(ref dispatcher) = ctx_add_tab.action_dispatcher {
                dispatcher.add_tab(
                    &ctx_add_tab.project_id,
                    &ctx_add_tab.layout_path,
                    !ctx_add_tab.standalone,
                    cx,
                );
            }
        })
        .into_any_element(),
    ];
    let always = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(2.0))
        .child(more_button)
        .child({
            header_button_base(
                HeaderAction::Close,
                &id_suffix,
                ButtonSize::COMPACT,
                &t,
                Some(if standalone { "Close" } else { "Close Tab" }),
                terminal_id.as_ref().map(|_| {
                    okena_ui::shortcuts::ActionShortcut::new(
                        crate::actions::CloseTerminal,
                        shortcut_guard(),
                    )
                }),
            )
            .on_click(move |_, _window, cx| {
                if let Some(ref tid) = terminal_id_for_close
                    && let Some(ref dispatcher) = ctx_close.action_dispatcher
                {
                    dispatcher.dispatch(
                        okena_core::api::ActionRequest::CloseTerminal {
                            project_id: ctx_close.project_id.clone(),
                            terminal_id: tid.clone(),
                        },
                        cx,
                    );
                }
            })
        });
    (extra, always)
}
