use super::LayoutContainer;
use crate::ActionDispatch;
use crate::layout::pane_drag::PaneMoveState;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use okena_core::api::ActionRequest;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::TerminalTransport;
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{LayoutNode, SplitDirection, WindowId, Workspace, WorkspaceData};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};

#[derive(Clone, Default)]
struct RecordedActions(Arc<parking_lot::Mutex<Vec<ActionRequest>>>);

impl ActionDispatch for RecordedActions {
    fn dispatch(&self, action: ActionRequest, _: &mut App) {
        self.0.lock().push(action);
    }
    fn shares_local_filesystem(&self) -> bool {
        false
    }
    fn split_terminal(&self, _: &str, _: &[usize], _: SplitDirection, _: &mut App) {
        panic!("unexpected split");
    }
    fn add_tab(&self, _: &str, _: &[usize], _: bool, _: &mut App) {
        panic!("unexpected add tab");
    }
}

// The real header is rendered, but no terminal process, daemon or profile is used.
struct NoBackend;
impl TerminalBackend for NoBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        panic!("unexpected transport")
    }
    fn create_terminal(&self, _: &str, _: Option<&ShellType>) -> anyhow::Result<String> {
        panic!("unexpected terminal creation")
    }
    fn reconnect_terminal(
        &self,
        _: &str,
        _: &str,
        _: Option<&ShellType>,
    ) -> anyhow::Result<String> {
        panic!("unexpected terminal reconnection")
    }
    fn kill(&self, _: &str) {
        panic!("unexpected terminal kill")
    }
    fn capture_buffer(&self, _: &str) -> Option<PathBuf> {
        None
    }
    fn supports_buffer_capture(&self) -> bool {
        false
    }
    fn is_remote(&self) -> bool {
        true
    }
    fn get_shell_pid(&self, _: &str) -> Option<u32> {
        None
    }
    fn get_service_pids(&self, _: &str) -> Vec<u32> {
        vec![]
    }
}

struct Header {
    container: Entity<LayoutContainer<RecordedActions>>,
    children: Vec<LayoutNode>,
}

impl Render for Header {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.container.update(cx, |container, cx| {
                let active = container
                    .action_dispatcher
                    .as_ref()
                    .and_then(|a| {
                        a.0.lock().iter().rev().find_map(|a| {
                            if let ActionRequest::SetActiveTab { index, .. } = a {
                                Some(*index)
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);
                container.render_tab_bar(&self.children, active, false, cx)
            }))
    }
}

fn terminal(id: &str) -> LayoutNode {
    let mut node = LayoutNode::new_terminal();
    if let LayoutNode::Terminal { terminal_id, .. } = &mut node {
        *terminal_id = Some(id.into());
    }
    node
}

fn start_page(id: &str) -> LayoutNode {
    let mut node = LayoutNode::new_terminal();
    if let LayoutNode::Terminal { start_page, .. } = &mut node {
        *start_page = Some(okena_core::api::StartPage {
            id: id.into(),
            cwd: ".".into(),
        });
    }
    node
}

fn test_layout(
    cx: &mut TestAppContext,
    workspace: Entity<Workspace>,
    focus: Entity<FocusManager>,
    window_id: WindowId,
    actions: RecordedActions,
) -> Entity<LayoutContainer<RecordedActions>> {
    cx.new(|cx| {
        let broker = cx.new(|_| RequestBroker::new());
        let pane_move = cx.new(|_| PaneMoveState::default());
        LayoutContainer::new(
            workspace,
            focus,
            broker,
            window_id,
            "project".into(),
            String::new(),
            vec![],
            Arc::new(NoBackend),
            Default::default(),
            Rc::new(RefCell::new(None)),
            pane_move,
            Some(actions),
            |cx| cx.new(|_| gpui::Empty).into(),
            cx,
        )
    })
}

fn header(
    cx: &mut TestAppContext,
    width: f32,
    show_shell: bool,
) -> (
    Entity<LayoutContainer<RecordedActions>>,
    RecordedActions,
    VisualTestContext,
) {
    header_with_children(
        cx,
        width,
        show_shell,
        vec![terminal("first"), terminal("second")],
    )
}

fn header_with_children(
    cx: &mut TestAppContext,
    width: f32,
    show_shell: bool,
    children: Vec<LayoutNode>,
) -> (
    Entity<LayoutContainer<RecordedActions>>,
    RecordedActions,
    VisualTestContext,
) {
    cx.update(|cx| {
        cx.set_global(okena_ui::theme::GlobalThemeProvider(|_| {
            okena_ui::theme::DARK_THEME
        }));
        cx.set_global(okena_extensions::ExtensionSettingsStore::new(
            |_, _| None,
            |_, _, _| {},
        ));
        let mut settings = crate::terminal_view_settings(cx);
        settings.show_shell_selector = show_shell;
        let settings = serde_json::to_value(settings).unwrap();
        cx.set_global(okena_extensions::ExtensionSettingsStore::new(
            move |_, _| Some(settings.clone()),
            |_, _, _| {},
        ));
    });
    let actions = RecordedActions::default();
    let workspace = cx.new(|_| Workspace::new(WorkspaceData::empty()));
    let focus = cx.new(|_| FocusManager::new());
    let container = test_layout(cx, workspace, focus, WindowId::Main, actions.clone());
    let window = cx.open_window(size(px(width), px(240.0)), {
        let container = container.clone();
        move |_, cx| {
            cx.observe(&container, |_, _, cx| cx.notify()).detach();
            Header {
                container,
                children,
            }
        }
    });
    let visual = VisualTestContext::from_window(window.into(), cx);
    (container, actions, visual)
}

#[gpui::test]
fn hover_actions_keep_active_target_until_a_tab_is_clicked(cx: &mut TestAppContext) {
    let (container, actions, mut visual) = header(cx, 400.0, false);
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    let second = visual.debug_bounds("hover-source-1").unwrap();
    visual.simulate_mouse_move(second.center(), None, Default::default());
    assert!(actions.0.lock().is_empty());
    let close = visual.debug_bounds("close-btn-tabs-[]").unwrap();
    visual.simulate_click(close.center(), Default::default());
    assert!(
        matches!(actions.0.lock().last(), Some(ActionRequest::CloseTerminal {terminal_id, ..}) if terminal_id == "first")
    );
    visual.simulate_click(second.center(), Default::default());
    container.update(cx, |_, cx| cx.notify());
    visual.simulate_mouse_move(second.center(), None, Default::default());
    let close = visual.debug_bounds("close-btn-tabs-[]").unwrap();
    visual.simulate_click(close.center(), Default::default());
    assert!(
        matches!(actions.0.lock().last(), Some(ActionRequest::CloseTerminal {terminal_id, ..}) if terminal_id == "second")
    );
    assert!(container.read_with(cx, |c, _| c.tab_rename_state.is_none()));
}

#[gpui::test]
fn pending_tabs_keep_separate_close_targets(cx: &mut TestAppContext) {
    let (container, actions, mut visual) = header_with_children(
        cx,
        400.,
        true,
        vec![start_page("first-page"), start_page("second-page")],
    );

    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    let second = visual.debug_bounds("hover-source-1").unwrap();
    visual.simulate_mouse_move(second.center(), None, Default::default());
    let close = visual.debug_bounds("close-btn-tabs-[]").unwrap();
    visual.simulate_click(close.center(), Default::default());
    assert!(
        matches!(actions.0.lock().last(), Some(ActionRequest::CloseTerminal { terminal_id, .. }) if terminal_id == "first-page")
    );

    visual.simulate_click(second.center(), Default::default());
    assert!(matches!(
        actions.0.lock().last(),
        Some(ActionRequest::SetActiveTab { index: 1, .. })
    ));
    container.update(cx, |_, cx| cx.notify());
    visual.simulate_mouse_move(second.center(), None, Default::default());
    let close = visual.debug_bounds("close-btn-tabs-[]").unwrap();
    visual.simulate_click(close.center(), Default::default());
    assert!(
        matches!(actions.0.lock().last(), Some(ActionRequest::CloseTerminal { terminal_id, .. }) if terminal_id == "second-page")
    );
}

#[gpui::test]
fn pending_panes_navigate_and_launch_with_the_keyboard(cx: &mut TestAppContext) {
    use crate::actions::FocusNextTerminal;
    use crate::layout::navigation::get_pane_map;
    use okena_core::shell::StartPageShell;
    use okena_workspace::state::ProjectData;

    struct LayoutRoot(Entity<LayoutContainer<RecordedActions>>);

    impl Render for LayoutRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    cx.update(|cx| {
        cx.set_global(okena_ui::theme::GlobalThemeProvider(|_| {
            okena_ui::theme::DARK_THEME
        }));
        cx.set_global(okena_extensions::ExtensionSettingsStore::new(
            |key, _| {
                (key == "start_page_buttons").then(|| {
                    serde_json::json!([
                        { "name": "Run tests", "command": "cargo test" }
                    ])
                })
            },
            |_, _, _| {},
        ));
        cx.bind_keys([gpui::KeyBinding::new(
            "ctrl-tab",
            FocusNextTerminal,
            Some("TerminalPane"),
        )]);
    });

    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![50.0, 50.0],
        children: vec![start_page("first-page"), start_page("second-page")],
    };
    let project: ProjectData = serde_json::from_value(serde_json::json!({
        "id": "project",
        "name": "Keyboard fixture",
        "path": ".",
        "layout": layout,
    }))
    .unwrap();
    let mut data = WorkspaceData::empty();
    data.projects.push(project);

    let window_id = WindowId::Extra("6ce8c733-ce96-4a31-8267-73ceaf957944".parse().unwrap());
    let workspace = cx.new(|_| Workspace::new(data));
    let focus = cx.new(|_| FocusManager::new());
    focus.update(cx, |fm, cx| {
        workspace.update(cx, |ws, cx| {
            ws.set_focused_terminal(fm, "project".into(), vec![0], cx);
        });
    });
    let actions = RecordedActions::default();
    let container = test_layout(cx, workspace, focus.clone(), window_id, actions.clone());
    let window = cx.open_window(size(px(700.0), px(280.0)), move |_, _| {
        LayoutRoot(container)
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    settle_frames(&mut visual);
    let pane_map = get_pane_map(window_id);
    assert!(pane_map.find_pane("project", &[0]).is_some());
    assert!(pane_map.find_pane("project", &[1]).is_some());

    visual.simulate_keystrokes("ctrl-tab");
    assert!(focus.read_with(cx, |fm, _| fm.is_focused("project", &[1])));
    visual.simulate_keystrokes("enter");
    assert!(matches!(
        actions.0.lock().last(),
        Some(ActionRequest::LaunchStartPage {
            page_id: Some(id), shell: StartPageShell::Default, command: None, ..
        }) if id == "second-page"
    ));

    visual.simulate_keystrokes("tab");
    settle_frames(&mut visual);
    visual.simulate_keystrokes("tab");
    settle_frames(&mut visual);
    visual.simulate_keystrokes("enter");
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    assert!(matches!(
        actions.0.lock().last(),
        Some(ActionRequest::LaunchStartPage { page_id: Some(id), command: Some(command), .. })
            if id == "second-page" && command == "cargo test"
    ));
}

#[gpui::test]
fn controls_reveal_without_reflow_then_expand_inline(cx: &mut TestAppContext) {
    for (width, shell) in [(90.0, false), (140.0, false), (240.0, true), (800.0, true)] {
        let (_, actions, mut visual) = header(cx, width, shell);
        visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
        settle_frames(&mut visual);
        let before = visual.debug_bounds("hover-tab-scroll").unwrap();
        let reserved = visual.debug_bounds("terminal-header-actions").unwrap();
        assert!(reserved.size.width >= px(46.0));
        assert!(visual.debug_bounds("close-btn-tabs-[]").is_none());
        let first = visual.debug_bounds("hover-source-0").unwrap();
        visual.simulate_mouse_move(first.center(), None, Default::default());
        settle_frames(&mut visual);
        assert_eq!(before, visual.debug_bounds("hover-tab-scroll").unwrap());
        assert_eq!(
            reserved,
            visual.debug_bounds("terminal-header-actions").unwrap()
        );
        let more = visual
            .debug_bounds("more-terminal-actions-btn-tabs-[]")
            .unwrap();
        let close = visual.debug_bounds("close-btn-tabs-[]").unwrap();
        let baseline_extra = visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width;
        visual.simulate_mouse_move(more.center(), None, Default::default());
        settle_frames(&mut visual);
        let extra = visual.debug_bounds("terminal-header-extra").unwrap();
        assert!(extra.size.width >= baseline_extra);
        assert!(extra.top() >= px(0.0) && extra.bottom() <= px(28.0));
        assert_eq!(
            more,
            visual
                .debug_bounds("more-terminal-actions-btn-tabs-[]")
                .unwrap()
        );
        assert_eq!(close, visual.debug_bounds("close-btn-tabs-[]").unwrap());
        assert!(close.right() <= px(width));
        visual.simulate_mouse_move(extra.center(), None, Default::default());
        assert_eq!(extra, visual.debug_bounds("terminal-header-extra").unwrap());
        // Returning to tabs keeps controls visible but restores the reservation.
        let tabs = visual.debug_bounds("hover-tab-scroll").unwrap();
        visual.simulate_mouse_move(
            point(tabs.left() + px(1.0), tabs.center().y),
            None,
            Default::default(),
        );
        settle_frames(&mut visual);
        assert_eq!(before, visual.debug_bounds("hover-tab-scroll").unwrap());
        assert!(visual.debug_bounds("close-btn-tabs-[]").is_some());
        visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
        settle_frames(&mut visual);
        assert_eq!(
            reserved,
            visual.debug_bounds("terminal-header-actions").unwrap()
        );
        assert!(visual.debug_bounds("close-btn-tabs-[]").is_none());
        // A synthetic click without mouse movement cannot activate a hidden Close.
        visual.simulate_click(close.center(), Default::default());
        assert!(actions.0.lock().is_empty());
        // Direct entry through the transparent reserved area also reveals controls.
        visual.simulate_mouse_move(close.center(), None, Default::default());
        assert!(visual.debug_bounds("close-btn-tabs-[]").is_some());
    }
}

#[gpui::test]
fn partial_actions_keep_visible_icons_anchored_on_expansion(cx: &mut TestAppContext) {
    let (container, _, mut visual) = header(cx, 800.0, false);
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    settle_frames(&mut visual);
    let tab_width = container.read_with(cx, |c, _| {
        (0..2)
            .map(|i| c.tab_scroll_handle.bounds_for_item(i).unwrap().size.width)
            .sum::<gpui::Pixels>()
            + px(20.0)
    });
    // 46px for More/Close, 4px separating gap, and exactly one 20px action.
    visual.simulate_resize(size(tab_width + px(70.0), px(240.0)));
    visual.run_until_parked();
    settle_frames(&mut visual);
    let first = visual.debug_bounds("hover-source-0").unwrap();
    visual.simulate_mouse_move(first.center(), None, Default::default());
    settle_frames(&mut visual);
    let add = visual.debug_bounds("add-tab-btn-tabs-[]").unwrap();
    assert!(
        visual
            .debug_bounds("split-horizontal-btn-tabs-[]")
            .is_none()
    );
    assert!(visual.debug_bounds("split-vertical-btn-tabs-[]").is_none());
    let more = visual
        .debug_bounds("more-terminal-actions-btn-tabs-[]")
        .unwrap();
    visual.simulate_mouse_move(more.center(), None, Default::default());
    settle_frames(&mut visual);
    assert_eq!(add, visual.debug_bounds("add-tab-btn-tabs-[]").unwrap());
    assert!(visual.debug_bounds("split-vertical-btn-tabs-[]").is_some());
}

#[gpui::test]
fn tab_drag_keeps_actions_collapsed(cx: &mut TestAppContext) {
    let (_, _, mut visual) = header(cx, 180.0, false);
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    let first = visual.debug_bounds("hover-source-0").unwrap();
    visual.simulate_mouse_move(first.center(), None, Default::default());
    visual.simulate_event(gpui::MouseDownEvent {
        position: first.center(),
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    let second = visual.debug_bounds("hover-source-1").unwrap();
    visual.simulate_mouse_move(
        second.center(),
        Some(gpui::MouseButton::Left),
        Default::default(),
    );
    assert!(cx.update(|cx| cx.has_active_drag()));
    let more = visual.debug_bounds("terminal-header-actions").unwrap();
    visual.simulate_mouse_move(
        more.center(),
        Some(gpui::MouseButton::Left),
        Default::default(),
    );
    assert_eq!(
        visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width,
        px(0.0)
    );
    visual.simulate_event(gpui::MouseUpEvent {
        position: point(px(10.0), px(220.0)),
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
    });
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    visual.simulate_mouse_move(more.center(), None, Default::default());
    assert!(
        visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width
            > px(0.0)
    );
}

#[gpui::test]
fn compact_more_menu_keeps_primary_actions_available_in_narrow_panes(cx: &mut TestAppContext) {
    let (container, _, mut visual) = header(cx, 90.0, true);
    let area = visual.debug_bounds("terminal-header-actions").unwrap();
    visual.simulate_mouse_move(area.center(), None, Default::default());
    let more = visual
        .debug_bounds("more-terminal-actions-btn-tabs-[]")
        .unwrap();
    visual.simulate_mouse_move(more.center(), None, Default::default());
    visual.simulate_click(more.center(), Default::default());
    let broker = container.read_with(cx, |c, _| c.request_broker.clone());
    let requests = broker.update(cx, |broker, _| broker.drain_overlay_requests());
    assert!(requests.into_iter().any(|request| matches!(request,
        okena_workspace::requests::OverlayRequest::Project(okena_workspace::requests::ProjectOverlay {
            kind: okena_workspace::requests::ProjectOverlayKind::TerminalMenu {
                terminal_id, invocation: okena_workspace::requests::TerminalMenuInvocation::Header { include_primary_actions: true, split_layout_path: Some(ref split_path) }, ..
            }, ..
        }) if terminal_id == "first" && split_path.is_empty())));
}

#[gpui::test]
fn adaptive_actions_follow_actual_header_space_without_hover(cx: &mut TestAppContext) {
    let (_, _, mut visual) = header(cx, 800.0, true);
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    visual.run_until_parked();
    settle_frames(&mut visual);
    let full = visual
        .debug_bounds("terminal-header-extra")
        .unwrap()
        .size
        .width;
    assert!(full > px(0.0));
    // A resize alone switches mode, in both directions, without moving the pointer.
    for width in [90.0, 800.0, 140.0, 900.0] {
        visual.simulate_resize(size(px(width), px(240.0)));
        visual.run_until_parked();
        settle_frames(&mut visual);
        let extra = visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width;
        if width < 300.0 {
            assert_eq!(extra, px(0.0));
        } else {
            assert_eq!(extra, full);
        }
        assert!(visual.debug_bounds("close-btn-tabs-[]").is_none());
        assert!(
            visual
                .debug_bounds("terminal-header-actions")
                .unwrap()
                .right()
                <= px(width)
        );
    }
}

// TestPlatform has no display frame loop; deliver pending layout frames explicitly.
fn settle_frames(visual: &mut VisualTestContext) {
    for _ in 0..5 {
        if visual.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            return;
        }
        visual.run_until_parked();
    }
    panic!("adaptive header did not settle within five frames");
}

#[gpui::test]
fn adaptive_actions_react_to_added_tabs_without_resizing(cx: &mut TestAppContext) {
    let (_, _, mut visual) = header(cx, 800.0, true);
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    settle_frames(&mut visual);
    assert!(
        visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width
            > px(0.0)
    );
    for count in [16, 2, 16, 2] {
        visual.update(|window, cx| {
            window
                .root::<Header>()
                .unwrap()
                .unwrap()
                .update(cx, |header, cx| {
                    header.children = (0..count)
                        .map(|i| terminal(&format!("terminal-{i}")))
                        .collect();
                    cx.notify();
                });
        });
        visual.run_until_parked();
        settle_frames(&mut visual);
        let extra = visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width;
        assert_eq!(extra > px(0.0), count == 2);
    }
}

#[gpui::test]
fn hidden_shell_keeps_its_natural_width(cx: &mut TestAppContext) {
    let (_, _, mut visual) = header(cx, 140.0, true);
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    settle_frames(&mut visual);
    assert_eq!(
        visual
            .debug_bounds("terminal-header-extra")
            .unwrap()
            .size
            .width,
        px(0.0)
    );
    let natural = visual
        .debug_bounds("terminal-header-item-0")
        .unwrap()
        .size
        .width;
    assert!(natural > px(20.0));
    let area = visual.debug_bounds("terminal-header-actions").unwrap();
    visual.simulate_mouse_move(area.center(), None, Default::default());
    settle_frames(&mut visual);
    assert_eq!(
        natural,
        visual
            .debug_bounds("terminal-header-item-0")
            .unwrap()
            .size
            .width
    );
    visual.simulate_mouse_move(point(px(10.0), px(220.0)), None, Default::default());
    visual.simulate_resize(size(px(800.0), px(240.0)));
    visual.run_until_parked();
    settle_frames(&mut visual);
    assert_eq!(
        natural,
        visual
            .debug_bounds("terminal-header-item-0")
            .unwrap()
            .size
            .width
    );
}

#[gpui::test]
fn slight_mouse_movement_during_control_press_does_not_cancel_click(cx: &mut TestAppContext) {
    let (_, actions, mut visual) = header(cx, 180.0, false);
    let region = visual.debug_bounds("terminal-header-actions").unwrap();
    visual.simulate_mouse_move(region.center(), None, Default::default());
    settle_frames(&mut visual);
    let close = visual.debug_bounds("close-btn-tabs-[]").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: close.center(),
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    let moved = close.center() + point(px(1.0), px(0.0));
    visual.simulate_mouse_move(moved, Some(gpui::MouseButton::Left), Default::default());
    settle_frames(&mut visual);
    assert_eq!(Some(close), visual.debug_bounds("close-btn-tabs-[]"));
    visual.simulate_event(gpui::MouseUpEvent {
        position: moved,
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
    });
    assert!(
        matches!(actions.0.lock().last(), Some(ActionRequest::CloseTerminal { terminal_id, .. }) if terminal_id == "first")
    );
}
