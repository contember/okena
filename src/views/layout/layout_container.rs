use crate::terminal::pty_manager::PtyManager;
use crate::terminal::shell_config::{available_shells, AvailableShell, ShellType};
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::theme::{theme, with_alpha};
use crate::views::header_buttons::{header_button_base, ButtonSize, HeaderAction};
use crate::views::root::TerminalsRegistry;
use crate::views::split_pane::render_split_divider;
use crate::views::terminal_pane::TerminalPane;
use crate::workspace::state::{LayoutNode, SplitDirection, Workspace};
use gpui_component::tooltip::Tooltip;
use gpui::*;
use gpui::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// Drag payload for tab reordering
#[derive(Clone)]
struct TabDrag {
    project_id: String,
    layout_path: Vec<usize>,
    tab_index: usize,
    tab_label: String,
}

/// Drag preview view for tabs - shows a polished ghost image during drag
struct TabDragView {
    label: String,
}

/// Context for tab action button closures.
///
/// This struct consolidates the common values needed by tab action buttons,
/// reducing the number of clones needed in render_tabs().
#[derive(Clone)]
struct TabActionContext {
    workspace: Entity<Workspace>,
    project_id: String,
    layout_path: Vec<usize>,
    active_tab: usize,
}

impl Render for TabDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Ghost tab with enhanced visual feedback
        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(with_alpha(t.bg_primary, 0.95))
            .border_1()
            .border_color(rgb(t.border_active))
            .rounded(px(6.0))
            .shadow_xl()
            .text_size(px(12.0))
            .text_color(rgb(t.text_primary))
            .font_weight(FontWeight::MEDIUM)
            // Add a subtle glow effect using the active border color
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    // Shell icon
                    .child(
                        svg()
                            .path("icons/shell.svg")
                            .size(px(12.0))
                            .text_color(rgb(t.success))
                    )
                    .child(self.label.clone())
            )
    }
}

/// Recursive layout container that renders terminal/split/tabs nodes
pub struct LayoutContainer {
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    layout_path: Vec<usize>,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    /// Stored terminal pane entity (for single terminal case)
    terminal_pane: Option<Entity<TerminalPane>>,
    /// Cached child layout containers keyed by their layout path.
    /// Without this, split/tabs would recreate entities every render, which breaks focus.
    child_containers: HashMap<Vec<usize>, Entity<LayoutContainer>>,
    /// Shared bounds of this container (updated during prepaint via canvas)
    container_bounds_ref: Rc<RefCell<Bounds<Pixels>>>,
    /// Tab context menu state: (tab_index, position, num_tabs)
    tab_context_menu: Option<(usize, Point<Pixels>, usize)>,
    /// Animation state for recently dropped tab (tab_index, animation_progress)
    /// progress goes from 1.0 (just dropped) to 0.0 (animation complete)
    drop_animation: Option<(usize, f32)>,
    /// Shell dropdown state for tab groups
    shell_dropdown_open: bool,
    /// Available shells for switching
    available_shells: Vec<AvailableShell>,
}

impl LayoutContainer {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        pty_manager: Arc<PtyManager>,
        terminals: TerminalsRegistry,
    ) -> Self {
        Self {
            workspace,
            project_id,
            project_path,
            layout_path,
            pty_manager,
            terminals,
            terminal_pane: None,
            child_containers: HashMap::new(),
            container_bounds_ref: Rc::new(RefCell::new(Bounds {
                origin: Point::default(),
                size: Size { width: px(800.0), height: px(600.0) },
            })),
            tab_context_menu: None,
            drop_animation: None,
            shell_dropdown_open: false,
            available_shells: available_shells(),
        }
    }

    /// Start drop animation for a tab at the given index
    /// Uses fewer steps with easing for smoother visual feedback
    fn start_drop_animation(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        self.drop_animation = Some((tab_index, 1.0));
        cx.notify();

        // Animate the drop effect with eased fade-out
        cx.spawn(async move |this: WeakEntity<LayoutContainer>, cx| {
            let duration_ms = 200;
            let frame_time_ms = 33; // ~30fps for subtle effect (doesn't need 60fps)
            let steps = duration_ms / frame_time_ms;
            let step_duration = std::time::Duration::from_millis(frame_time_ms as u64);

            for i in 1..=steps {
                smol::Timer::after(step_duration).await;

                // Ease-out for smooth fade
                let t = i as f32 / steps as f32;
                let progress = 1.0 - t * t; // quadratic ease-out

                let result = this.update(cx, |this, cx| {
                    if let Some((idx, _)) = this.drop_animation {
                        this.drop_animation = Some((idx, progress));
                        cx.notify();
                    }
                });
                if result.is_err() {
                    break;
                }
            }

            // Clear the animation
            let _ = this.update(cx, |this, cx| {
                this.drop_animation = None;
                cx.notify();
            });
        }).detach();
    }

    fn ensure_terminal_pane(
        &mut self,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        cx: &mut Context<Self>,
    ) {
        // Check if we need to create a new pane or update existing one
        let needs_new_pane = match &self.terminal_pane {
            None => true,
            Some(pane) => {
                // Check if terminal_id matches - if not, we need a new pane
                let current_id = pane.read(cx).terminal_id();
                current_id != terminal_id
            }
        };

        if needs_new_pane {
            let workspace = self.workspace.clone();
            let project_id = self.project_id.clone();
            let project_path = self.project_path.clone();
            let layout_path = self.layout_path.clone();
            let pty_manager = self.pty_manager.clone();
            let terminals = self.terminals.clone();

            self.terminal_pane = Some(cx.new(move |cx| {
                TerminalPane::new(
                    workspace,
                    project_id,
                    project_path,
                    layout_path,
                    terminal_id,
                    minimized,
                    detached,
                    pty_manager,
                    terminals,
                    cx,
                )
            }));
        } else if let Some(pane) = &self.terminal_pane {
            // Update minimized and detached states if they changed
            pane.update(cx, |pane, cx| {
                pane.set_minimized(minimized, cx);
                pane.set_detached(detached, cx);
            });
        }
    }

    fn get_layout<'a>(&self, workspace: &'a Workspace) -> Option<&'a LayoutNode> {
        let project = workspace.project(&self.project_id)?;
        project.layout.as_ref()?.get_at_path(&self.layout_path)
    }

    /// Get the terminal_id for the active tab in this Tabs container.
    fn get_active_terminal_id(&self, active_tab: usize, cx: &Context<Self>) -> Option<String> {
        let ws = self.workspace.read(cx);
        if let Some(LayoutNode::Tabs { children, .. }) = self.get_layout(&ws) {
            if let Some(LayoutNode::Terminal { terminal_id, .. }) = children.get(active_tab) {
                return terminal_id.clone();
            }
        }
        None
    }

    /// Get the shell type for the active tab in this Tabs container.
    fn get_active_shell_type(&self, active_tab: usize, cx: &Context<Self>) -> ShellType {
        let ws = self.workspace.read(cx);
        if let Some(LayoutNode::Tabs { children, .. }) = self.get_layout(&ws) {
            if let Some(LayoutNode::Terminal { shell_type, .. }) = children.get(active_tab) {
                return shell_type.clone();
            }
        }
        ShellType::Default
    }

    /// Get the display name for a shell type.
    fn get_shell_display_name(&self, shell_type: &ShellType) -> String {
        shell_type.display_name()
    }

    /// Toggle the shell dropdown for tab groups.
    fn toggle_shell_dropdown(&mut self, cx: &mut Context<Self>) {
        self.shell_dropdown_open = !self.shell_dropdown_open;
        cx.notify();
    }

    /// Switch the shell for the active tab.
    fn switch_shell(&mut self, active_tab: usize, shell_type: ShellType, cx: &mut Context<Self>) {
        self.shell_dropdown_open = false;

        // Get the terminal_id for the active tab
        let terminal_id = self.get_active_terminal_id(active_tab, cx);

        // Get the current shell type
        let current_shell = self.get_active_shell_type(active_tab, cx);
        if current_shell == shell_type {
            cx.notify();
            return;
        }

        // Kill the old terminal if it exists
        if let Some(ref tid) = terminal_id {
            self.pty_manager.kill(tid);
        }

        // Update the shell type in workspace state
        let mut full_path = self.layout_path.clone();
        full_path.push(active_tab);
        let project_id = self.project_id.clone();
        let shell_for_save = shell_type.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.set_terminal_shell(&project_id, &full_path, shell_for_save, cx);
        });

        // Create a new terminal with the new shell
        match self.pty_manager.create_terminal_with_shell(&self.project_path, Some(&shell_type)) {
            Ok(new_terminal_id) => {
                // Update the terminal_id in workspace state
                let new_id = new_terminal_id.clone();
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(&project_id, &full_path, new_id.clone(), cx);
                });

                // Create Terminal wrapper and register it
                let size = TerminalSize::default();
                let terminal = Arc::new(Terminal::new(new_terminal_id.clone(), size, self.pty_manager.clone()));
                self.terminals.lock().insert(new_terminal_id.clone(), terminal);

                log::info!("Switched tab {} to shell {:?}, new terminal_id: {}", active_tab, shell_type, new_terminal_id);
            }
            Err(e) => {
                log::error!("Failed to create terminal with new shell: {}", e);
            }
        }

        cx.notify();
    }

    /// Render the shell indicator button for tab groups.
    fn render_shell_indicator(&mut self, active_tab: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let shell_type = self.get_active_shell_type(active_tab, cx);
        let shell_name = self.get_shell_display_name(&shell_type);
        let id_suffix = format!("tabs-{:?}", self.layout_path);

        div()
            .id(format!("shell-indicator-{}", id_suffix))
            .cursor_pointer()
            .px(px(6.0))
            .h(px(18.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                cx.stop_propagation();
                this.toggle_shell_dropdown(cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                            .child(shell_name)
                    )
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                    )
            )
            .tooltip(|_window, cx| Tooltip::new("Switch Shell").build(_window, cx))
    }

    /// Render the shell dropdown modal for tab groups.
    fn render_shell_dropdown(&mut self, active_tab: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.shell_dropdown_open {
            return div().into_any_element();
        }

        let shells = self.available_shells.clone();
        let current_shell = self.get_active_shell_type(active_tab, cx);

        // Full-screen backdrop + centered modal
        div()
            .id("shell-modal-backdrop-tabs")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.shell_dropdown_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .id("shell-modal-tabs")
                    .w(px(200.0))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        // Modal header
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(t.text_primary))
                                    .child("Switch Shell")
                            )
                    )
                    .child(
                        // Shell list
                        div()
                            .py(px(2.0))
                            .children(shells.into_iter().filter(|s| s.available).map(|shell| {
                                let shell_type = shell.shell_type.clone();
                                let is_current = shell_type == current_shell;
                                let name = shell.name.clone();

                                div()
                                    .id(format!("shell-option-tabs-{}", name.replace(" ", "-").to_lowercase()))
                                    .w_full()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .cursor_pointer()
                                    .bg(if is_current { rgb(t.bg_hover) } else { rgb(t.bg_secondary) })
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                        this.switch_shell(active_tab, shell_type.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .child(name)
                                            )
                                            .when(is_current, |d| {
                                                d.child(
                                                    svg()
                                                        .path("icons/check.svg")
                                                        .size(px(12.0))
                                                        .text_color(rgb(t.success))
                                                )
                                            })
                                    )
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_terminal(
        &mut self,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Ensure terminal pane exists (created once, not every render)
        self.ensure_terminal_pane(terminal_id, minimized, detached, cx);

        div()
            .size_full()
            .min_h_0()
            .child(self.terminal_pane.clone().unwrap())
    }

    fn render_split(
        &mut self,
        direction: SplitDirection,
        sizes: &[f32],
        children: &[LayoutNode],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_horizontal = direction == SplitDirection::Horizontal;
        let num_children = children.len();
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();

        // Clean up stale child containers (e.g., when a child was removed)
        let valid_paths: std::collections::HashSet<Vec<usize>> = (0..num_children)
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();
        self.child_containers.retain(|path, _| valid_paths.contains(path));

        // Shared reference to container bounds (updated by canvas during prepaint)
        let container_bounds_ref = self.container_bounds_ref.clone();

        // Check which children are hidden (minimized or detached) and collect sizes for visible ones
        let mut visible_children_info: Vec<(usize, f32)> = Vec::new();
        for (i, child) in children.iter().enumerate() {
            let is_hidden = match child {
                LayoutNode::Terminal { minimized, detached, .. } => *minimized || *detached,
                _ => false,
            };
            if !is_hidden {
                let size = sizes.get(i).copied().unwrap_or(100.0 / num_children as f32);
                visible_children_info.push((i, size));
            }
        }

        // Normalize visible sizes to sum to 100%
        let total_visible_size: f32 = visible_children_info.iter().map(|(_, s)| s).sum();
        let normalized_sizes: Vec<f32> = if total_visible_size > 0.0 {
            visible_children_info.iter().map(|(_, s)| s / total_visible_size * 100.0).collect()
        } else {
            vec![100.0 / visible_children_info.len().max(1) as f32; visible_children_info.len()]
        };

        // Build interleaved children and dividers
        let mut elements: Vec<AnyElement> = Vec::new();

        for (visible_idx, (original_idx, _)) in visible_children_info.iter().enumerate() {
            let mut child_path = self.layout_path.clone();
            child_path.push(*original_idx);

            let container = self
                .child_containers
                .entry(child_path.clone())
                .or_insert_with(|| {
                    cx.new(|_cx| {
                        LayoutContainer::new(
                            self.workspace.clone(),
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.pty_manager.clone(),
                            self.terminals.clone(),
                        )
                    })
                })
                .clone();

            // Add divider before this child (if not first visible child)
            if visible_idx > 0 {
                let divider = render_split_divider(
                    self.project_id.clone(),
                    visible_idx - 1,
                    direction,
                    self.layout_path.clone(),
                    container_bounds_ref.clone(),
                    cx,
                );
                elements.push(divider.into_any_element());
            }

            let size_percent = normalized_sizes[visible_idx];
            let child_element = div()
                .flex_basis(relative(size_percent / 100.0))
                .min_w_0()
                .min_h_0()
                .child(container)
                .into_any_element();

            elements.push(child_element);
        }

        div()
            .id(ElementId::Name(format!("split-container-{}-{:?}", project_id, layout_path).into()))
            // Use a canvas to capture the container bounds during prepaint
            .child(canvas(
                {
                    let container_bounds_ref = container_bounds_ref.clone();
                    move |bounds, _window, _cx| {
                        *container_bounds_ref.borrow_mut() = bounds;
                    }
                },
                |_bounds, _prepaint, _window, _cx| {},
            ).absolute().size_full())
            // Mouse handlers are on RootView - no need to duplicate here
            .flex()
            .when(is_horizontal, |d| d.flex_col())
            .flex_nowrap()
            .size_full()
            .min_h_0()
            .min_w_0()
            .children(elements)
    }

    fn show_tab_context_menu(&mut self, tab_index: usize, position: Point<Pixels>, num_tabs: usize, cx: &mut Context<Self>) {
        self.tab_context_menu = Some((tab_index, position, num_tabs));
        cx.notify();
    }

    fn hide_tab_context_menu(&mut self, cx: &mut Context<Self>) {
        self.tab_context_menu = None;
        cx.notify();
    }

    fn render_tab_context_menu(&self, tab_index: usize, position: Point<Pixels>, num_tabs: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();
        let has_tabs_to_right = tab_index < num_tabs.saturating_sub(1);
        let has_other_tabs = num_tabs > 1;

        div()
            .id("tab-context-menu")
            .absolute()
            .left(position.x)
            .top(position.y)
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(4.0))
            .shadow_lg()
            .py(px(4.0))
            .min_w(px(140.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
            // Close tab
            .child(
                div()
                    .id("tab-menu-close")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .child("Close")
                    .on_click({
                        let workspace = workspace.clone();
                        let project_id = project_id.clone();
                        let layout_path = layout_path.clone();
                        cx.listener(move |this, _, _window, cx| {
                            workspace.update(cx, |ws, cx| {
                                ws.close_tab(&project_id, &layout_path, tab_index, cx);
                            });
                            this.hide_tab_context_menu(cx);
                        })
                    }),
            )
            // Close Others
            .child({
                let base = div()
                    .id("tab-menu-close-others")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(if has_other_tabs { rgb(t.text_primary) } else { rgb(t.text_muted) })
                    .cursor(if has_other_tabs { CursorStyle::PointingHand } else { CursorStyle::Arrow })
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(if has_other_tabs { rgb(t.text_secondary) } else { rgb(t.text_muted) })
                    )
                    .child("Close Others");
                if has_other_tabs {
                    base.hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click({
                            let workspace = workspace.clone();
                            let project_id = project_id.clone();
                            let layout_path = layout_path.clone();
                            cx.listener(move |this, _, _window, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.close_other_tabs(&project_id, &layout_path, tab_index, cx);
                                });
                                this.hide_tab_context_menu(cx);
                            })
                        })
                } else {
                    base
                }
            })
            // Close to Right
            .child({
                let base = div()
                    .id("tab-menu-close-to-right")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(if has_tabs_to_right { rgb(t.text_primary) } else { rgb(t.text_muted) })
                    .cursor(if has_tabs_to_right { CursorStyle::PointingHand } else { CursorStyle::Arrow })
                    .child(
                        svg()
                            .path("icons/chevron-right.svg")
                            .size(px(14.0))
                            .text_color(if has_tabs_to_right { rgb(t.text_secondary) } else { rgb(t.text_muted) })
                    )
                    .child("Close to Right");
                if has_tabs_to_right {
                    base.hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click({
                            let workspace = workspace.clone();
                            let project_id = project_id.clone();
                            let layout_path = layout_path.clone();
                            cx.listener(move |this, _, _window, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.close_tabs_to_right(&project_id, &layout_path, tab_index, cx);
                                });
                                this.hide_tab_context_menu(cx);
                            })
                        })
                } else {
                    base
                }
            })
    }

    /// Render action buttons for the tab bar.
    ///
    /// This helper method extracts the action buttons from render_tabs() for better readability.
    fn render_tab_action_buttons(
        &self,
        ctx: TabActionContext,
        terminal_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let id_suffix = format!("tabs-{:?}", ctx.layout_path);

        // Check if buffer capture is supported
        let supports_buffer_capture = self.pty_manager.supports_buffer_capture();
        let pty_manager_for_export = self.pty_manager.clone();
        let terminal_id_for_export = terminal_id.clone();
        let terminal_id_for_fullscreen = terminal_id;

        // Clone context for each action - much cleaner than individual clones
        let ctx_split_v = ctx.clone();
        let ctx_split_h = ctx.clone();
        let ctx_add_tab = ctx.clone();
        let ctx_minimize = ctx.clone();
        let ctx_fullscreen = ctx.clone();
        let ctx_detach = ctx.clone();
        let ctx_close = ctx.clone();

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .px(px(4.0))
            // Split Vertical
            .child(
                header_button_base(HeaderAction::SplitVertical, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        let mut child_path = ctx_split_v.layout_path.clone();
                        child_path.push(ctx_split_v.active_tab);
                        ctx_split_v.workspace.update(cx, |ws, cx| {
                            ws.split_terminal(&ctx_split_v.project_id, &child_path, SplitDirection::Vertical, cx);
                        });
                    }),
            )
            // Split Horizontal
            .child(
                header_button_base(HeaderAction::SplitHorizontal, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        let mut child_path = ctx_split_h.layout_path.clone();
                        child_path.push(ctx_split_h.active_tab);
                        ctx_split_h.workspace.update(cx, |ws, cx| {
                            ws.split_terminal(&ctx_split_h.project_id, &child_path, SplitDirection::Horizontal, cx);
                        });
                    }),
            )
            // Add Tab
            .child(
                header_button_base(HeaderAction::AddTab, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        ctx_add_tab.workspace.update(cx, |ws, cx| {
                            ws.add_tab_to_group(&ctx_add_tab.project_id, &ctx_add_tab.layout_path, cx);
                        });
                    }),
            )
            // Minimize
            .child(
                header_button_base(HeaderAction::Minimize, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        let mut full_path = ctx_minimize.layout_path.clone();
                        full_path.push(ctx_minimize.active_tab);
                        ctx_minimize.workspace.update(cx, |ws, cx| {
                            ws.toggle_terminal_minimized(&ctx_minimize.project_id, &full_path, cx);
                        });
                    }),
            )
            // Export Buffer (conditional)
            .when(supports_buffer_capture, |el| {
                el.child(
                    header_button_base(HeaderAction::ExportBuffer, &id_suffix, ButtonSize::COMPACT, &t, None)
                        .on_click(move |_, _window, cx| {
                            if let Some(ref tid) = terminal_id_for_export {
                                if let Some(path) = pty_manager_for_export.capture_buffer(tid) {
                                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                                    log::info!("Buffer exported to {} (path copied to clipboard)", path.display());
                                }
                            }
                        }),
                )
            })
            // Fullscreen
            .child(
                header_button_base(HeaderAction::Fullscreen, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        if let Some(ref tid) = terminal_id_for_fullscreen {
                            ctx_fullscreen.workspace.update(cx, |ws, cx| {
                                ws.set_fullscreen_terminal(ctx_fullscreen.project_id.clone(), tid.clone(), cx);
                            });
                        }
                    }),
            )
            // Detach
            .child(
                header_button_base(HeaderAction::Detach, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        let mut full_path = ctx_detach.layout_path.clone();
                        full_path.push(ctx_detach.active_tab);
                        ctx_detach.workspace.update(cx, |ws, cx| {
                            ws.detach_terminal(&ctx_detach.project_id, &full_path, cx);
                        });
                    }),
            )
            // Close Tab
            .child(
                header_button_base(HeaderAction::Close, &id_suffix, ButtonSize::COMPACT, &t, Some("Close Tab"))
                    .on_click(move |_, _window, cx| {
                        ctx_close.workspace.update(cx, |ws, cx| {
                            ws.close_tab(&ctx_close.project_id, &ctx_close.layout_path, ctx_close.active_tab, cx);
                        });
                    }),
            )
    }

    fn render_tabs(
        &mut self,
        children: &[LayoutNode],
        active_tab: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();
        let num_children = children.len();

        // Clean up stale child containers (e.g., when a tab was removed)
        let valid_paths: HashSet<Vec<usize>> = (0..num_children)
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();
        self.child_containers.retain(|path, _| valid_paths.contains(path));

        // Check for active drop animation
        let drop_animation = self.drop_animation;

        // Get terminal names for tabs from the terminals registry
        let terminals = self.terminals.clone();
        let workspace_reader = self.workspace.read(cx);
        let project = workspace_reader.project(&self.project_id);
        let terminal_names_map = project.map(|p| p.terminal_names.clone());

        // Build tab elements with right-click handlers
        let tab_elements: Vec<_> = children.iter().enumerate().map(|(i, child)| {
            let is_active = i == active_tab;
            let workspace = workspace.clone();
            let workspace_for_drop = workspace.clone();
            let project_id = project_id.clone();
            let project_id_for_drag = project_id.clone();
            let project_id_for_drop = project_id.clone();
            let layout_path = layout_path.clone();
            let layout_path_for_drag = layout_path.clone();
            let layout_path_for_drop = layout_path.clone();

            // Get terminal ID from the child node if it's a terminal
            let terminal_id = match child {
                LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.clone()),
                _ => None,
            };

            // Get tab label: custom name > OSC title > "Tab N"
            let tab_label = if let Some(ref tid) = terminal_id {
                // Check for custom name first
                let custom_name = terminal_names_map.as_ref().and_then(|m| m.get(tid).cloned());
                if let Some(name) = custom_name {
                    name
                } else {
                    // Check for OSC title from terminal
                    let terminals_guard = terminals.lock();
                    if let Some(terminal) = terminals_guard.get(tid) {
                        terminal.title().unwrap_or_else(|| format!("Tab {}", i + 1))
                    } else {
                        format!("Tab {}", i + 1)
                    }
                }
            } else {
                format!("Tab {}", i + 1)
            };

            // Check if this tab has an active drop animation
            let has_drop_animation = drop_animation.map(|(idx, _)| idx == i).unwrap_or(false);
            let animation_progress = drop_animation
                .filter(|(idx, _)| *idx == i)
                .map(|(_, p)| p)
                .unwrap_or(0.0);

            div()
                .id(ElementId::Name(format!("tab-{}-{:?}", i, layout_path).into()))
                .cursor_pointer()
                .relative()
                .px(px(8.0))
                .py(px(4.0))
                .rounded_t(px(4.0))
                .text_size(px(12.0))
                .when(is_active, |d| {
                    d.bg(rgb(t.bg_primary))
                        .text_color(rgb(t.text_primary))
                })
                .when(!is_active, |d| {
                    d.bg(rgb(t.bg_secondary))
                        .text_color(rgb(t.text_secondary))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                })
                // Drop animation effect - glow highlight
                .when(has_drop_animation, |d| {
                    let glow_alpha = animation_progress * 0.5;
                    d.bg(with_alpha(t.border_active, glow_alpha))
                        .border_1()
                        .border_color(with_alpha(t.border_active, animation_progress * 0.9))
                        .rounded(px(4.0))
                })
                // Tab content with icon and label
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        // Shell icon
                        .child(
                            svg()
                                .path("icons/shell.svg")
                                .size(px(12.0))
                                .text_color(if is_active { rgb(t.success) } else { rgb(t.text_muted) })
                        )
                        .child(tab_label.clone())
                )
                // Right-click for context menu
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.show_tab_context_menu(i, event.position, num_children, cx);
                    cx.stop_propagation();
                }))
                // Drag source for tab reordering
                .on_drag(
                    TabDrag {
                        project_id: project_id_for_drag,
                        layout_path: layout_path_for_drag,
                        tab_index: i,
                        tab_label: tab_label.clone(),
                    },
                    move |drag, _position, _window, cx| {
                        cx.new(|_| TabDragView { label: drag.tab_label.clone() })
                    },
                )
                // Enhanced drop target - show prominent indicator with glow
                .drag_over::<TabDrag>(move |style, _, _, _| {
                    style
                        .border_l(px(3.0))
                        .border_color(rgb(t.border_active))
                        .bg(with_alpha(t.border_active, 0.15))
                })
                .on_drop(cx.listener(move |this, drag: &TabDrag, _window, cx| {
                    // Only allow reordering within the same tabs container
                    if drag.project_id == project_id_for_drop
                        && drag.layout_path == layout_path_for_drop
                        && drag.tab_index != i
                    {
                        // Calculate the target index after move
                        let target_index = if drag.tab_index < i { i - 1 } else { i };

                        workspace_for_drop.update(cx, |ws, cx| {
                            ws.move_tab(&project_id_for_drop, &layout_path_for_drop, drag.tab_index, i, cx);
                        });

                        // Start drop animation on the moved tab
                        this.start_drop_animation(target_index, cx);
                    }
                }))
                .on_click(move |_, _window, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.set_active_tab(&project_id, &layout_path, i, cx);
                    });
                })
        }).collect();

        // Create end drop zone for dropping after the last tab
        let workspace_for_end = self.workspace.clone();
        let project_id_for_end = self.project_id.clone();
        let layout_path_for_end = self.layout_path.clone();
        let end_drop_zone = div()
            .id(ElementId::Name(format!("tab-end-drop-{:?}", self.layout_path).into()))
            .flex_1()
            .h_full()
            .min_w(px(20.0))
            // Enhanced drop zone indicator
            .drag_over::<TabDrag>(move |style, _, _, _| {
                style
                    .border_l(px(3.0))
                    .border_color(rgb(t.border_active))
                    .bg(with_alpha(t.border_active, 0.1))
            })
            .on_drop(cx.listener(move |this, drag: &TabDrag, _window, cx| {
                // Only allow reordering within the same tabs container
                if drag.project_id == project_id_for_end
                    && drag.layout_path == layout_path_for_end
                {
                    let target_index = num_children;
                    if drag.tab_index != target_index - 1 {
                        workspace_for_end.update(cx, |ws, cx| {
                            ws.move_tab(&project_id_for_end, &layout_path_for_end, drag.tab_index, target_index, cx);
                        });
                        // Start animation on the last tab (now at num_children - 1)
                        this.start_drop_animation(num_children - 1, cx);
                    }
                }
            }));

        // Render context menu if visible
        let context_menu = self.tab_context_menu.map(|(tab_idx, pos, num)| {
            self.render_tab_context_menu(tab_idx, pos, num, cx)
        });

        // Build action context for tab buttons
        let action_ctx = TabActionContext {
            workspace: self.workspace.clone(),
            project_id: self.project_id.clone(),
            layout_path: self.layout_path.clone(),
            active_tab,
        };

        // Get terminal_id for actions that need it
        let terminal_id_for_actions = self.get_active_terminal_id(active_tab, cx);

        // Render action buttons using helper method
        let action_buttons = self.render_tab_action_buttons(action_ctx, terminal_id_for_actions, cx);

        // Render shell indicator and dropdown
        let shell_indicator = self.render_shell_indicator(active_tab, cx);
        let shell_dropdown = self.render_shell_dropdown(active_tab, cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            // Close context menu on left-click anywhere in tabs area
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                if this.tab_context_menu.is_some() {
                    this.hide_tab_context_menu(cx);
                }
            }))
            .child(
                // Tab bar
                div()
                    .h(px(28.0))
                    .px(px(4.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .bg(rgb(t.bg_header))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .children(tab_elements)
                    .child(end_drop_zone)
                    .child(shell_indicator)
                    .child(action_buttons),
            )
            .children(context_menu)
            .child(
                // Active tab content
                div().flex_1().child({
                    let mut child_path = self.layout_path.clone();
                    child_path.push(active_tab);

                    self.child_containers
                        .entry(child_path.clone())
                        .or_insert_with(|| {
                            cx.new(|_cx| {
                                LayoutContainer::new(
                                    self.workspace.clone(),
                                    self.project_id.clone(),
                                    self.project_path.clone(),
                                    child_path.clone(),
                                    self.pty_manager.clone(),
                                    self.terminals.clone(),
                                )
                            })
                        })
                        .clone()
                }),
            )
            // Shell dropdown modal (rendered last to be on top)
            .child(shell_dropdown)
    }
}

impl Render for LayoutContainer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let layout = self.get_layout(workspace).cloned();

        // Clean up stale entities when layout type changes
        match &layout {
            Some(LayoutNode::Terminal { .. }) => {
                // When rendering a terminal, clear any cached child containers from previous split/tabs
                if !self.child_containers.is_empty() {
                    self.child_containers.clear();
                }
            }
            Some(LayoutNode::Split { .. }) | Some(LayoutNode::Tabs { .. }) => {
                // When rendering split/tabs, clear any cached terminal_pane from previous terminal
                if self.terminal_pane.is_some() {
                    self.terminal_pane = None;
                }
            }
            None => {
                // Clear everything when no layout
                self.terminal_pane = None;
                self.child_containers.clear();
            }
        }

        match layout {
            Some(LayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            }) => self
                .render_terminal(terminal_id.clone(), minimized, detached, window, cx)
                .into_any_element(),

            Some(LayoutNode::Split {
                direction,
                ref sizes,
                ref children,
            }) => self
                .render_split(direction, sizes, children, window, cx)
                .into_any_element(),

            Some(LayoutNode::Tabs {
                ref children,
                active_tab,
            }) => self
                .render_tabs(children, active_tab, window, cx)
                .into_any_element(),

            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.text_muted))
                .child("No layout")
                .into_any_element(),
        }
    }
}
