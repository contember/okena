//! Tab bar rendering and management
//!
//! This module contains tab-related functionality for LayoutContainer:
//! - Tab bar rendering with drag support and animations
//! - TabActionContext: Helper for action button closures
//! - Uses PaneDrag for unified drag-and-drop (tabs + terminal panes)

mod shell_selector;

use crate::keybindings::Cancel;
use crate::action_dispatch::ActionDispatcher;
use crate::settings::settings;
use crate::theme::{theme, with_alpha};
use crate::views::chrome::header_buttons::{header_button_base, ButtonSize, HeaderAction};
use crate::views::components::{is_renaming, rename_input, SimpleInput};
use crate::views::layout::layout_container::LayoutContainer;
use crate::views::layout::pane_drag::{PaneDrag, PaneDragView};
use crate::views::layout::app_registry;
use crate::workspace::state::{LayoutNode, SplitDirection};
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;
use std::collections::HashSet;

/// Context for tab action button closures.
///
/// This struct consolidates the common values needed by tab action buttons,
/// reducing the number of clones needed in render_tabs().
#[derive(Clone)]
pub(super) struct TabActionContext {
    pub workspace: Entity<crate::workspace::state::Workspace>,
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub active_tab: usize,
    /// When true, this is a standalone terminal (not in a Tabs container).
    /// Actions use layout_path directly instead of layout_path + [active_tab].
    pub standalone: bool,
    /// Action dispatcher for routing terminal actions (local or remote).
    pub action_dispatcher: Option<ActionDispatcher>,
}


impl LayoutContainer {
    /// Start drop animation for a tab at the given index
    /// Uses fewer steps with easing for smoother visual feedback
    pub(super) fn start_drop_animation(&mut self, tab_index: usize, cx: &mut Context<Self>) {
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

    /// Render action buttons for the tab bar.
    ///
    /// This helper method extracts the action buttons from render_tabs() for better readability.
    pub(super) fn render_tab_action_buttons(
        &self,
        ctx: TabActionContext,
        terminal_id: Option<String>,
        app_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let id_suffix = format!("tabs-{:?}", ctx.layout_path);

        // Check if buffer capture is supported
        let supports_buffer_capture = self.backend.supports_buffer_capture();
        let backend_for_export = self.backend.clone();
        let terminal_id_for_export = terminal_id.clone();
        let terminal_id_for_close = terminal_id.clone();
        let app_id_for_close = app_id.clone();
        let terminal_id_for_fullscreen = terminal_id.clone();

        // Clone context for each action - much cleaner than individual clones
        let ctx_split_v = ctx.clone();
        let ctx_split_h = ctx.clone();
        let ctx_add_tab = ctx.clone();
        let ctx_minimize = ctx.clone();
        let ctx_fullscreen = ctx.clone();
        let ctx_detach = ctx.clone();
        let ctx_close = ctx.clone();

        let standalone = ctx.standalone;
        let is_remote = self.backend.is_remote();

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
                        let child_path = if ctx_split_v.standalone {
                            ctx_split_v.layout_path.clone()
                        } else {
                            let mut p = ctx_split_v.layout_path.clone();
                            p.push(ctx_split_v.active_tab);
                            p
                        };
                        if let Some(ref dispatcher) = ctx_split_v.action_dispatcher {
                            dispatcher.dispatch(okena_core::api::ActionRequest::SplitTerminal {
                                project_id: ctx_split_v.project_id.clone(),
                                path: child_path,
                                direction: SplitDirection::Vertical,
                            }, cx);
                        }
                    }),
            )
            // Split Horizontal
            .child(
                header_button_base(HeaderAction::SplitHorizontal, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        let child_path = if ctx_split_h.standalone {
                            ctx_split_h.layout_path.clone()
                        } else {
                            let mut p = ctx_split_h.layout_path.clone();
                            p.push(ctx_split_h.active_tab);
                            p
                        };
                        if let Some(ref dispatcher) = ctx_split_h.action_dispatcher {
                            dispatcher.dispatch(okena_core::api::ActionRequest::SplitTerminal {
                                project_id: ctx_split_h.project_id.clone(),
                                path: child_path,
                                direction: SplitDirection::Horizontal,
                            }, cx);
                        }
                    }),
            )
            // Add Tab
            .child(
                header_button_base(HeaderAction::AddTab, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click(move |_, _window, cx| {
                        if let Some(ref dispatcher) = ctx_add_tab.action_dispatcher {
                            dispatcher.add_tab(
                                &ctx_add_tab.project_id,
                                &ctx_add_tab.layout_path,
                                !ctx_add_tab.standalone,
                                cx,
                            );
                        }
                    }),
            )
            // Minimize
            .child(
                header_button_base(HeaderAction::Minimize, &id_suffix, ButtonSize::COMPACT, &t, None)
                    .on_click({
                        let terminal_id_for_minimize = terminal_id.clone();
                        move |_, _window, cx| {
                            if let Some(ref tid) = terminal_id_for_minimize {
                                if let Some(ref dispatcher) = ctx_minimize.action_dispatcher {
                                    dispatcher.dispatch(okena_core::api::ActionRequest::ToggleMinimized {
                                        project_id: ctx_minimize.project_id.clone(),
                                        terminal_id: tid.clone(),
                                    }, cx);
                                }
                            }
                        }
                    }),
            )
            // Export Buffer (conditional)
            .when(supports_buffer_capture, |el| {
                el.child(
                    header_button_base(HeaderAction::ExportBuffer, &id_suffix, ButtonSize::COMPACT, &t, None)
                        .on_click(move |_, _window, cx| {
                            if let Some(ref tid) = terminal_id_for_export {
                                if let Some(path) = backend_for_export.capture_buffer(tid) {
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
                            if let Some(ref dispatcher) = ctx_fullscreen.action_dispatcher {
                                dispatcher.dispatch(okena_core::api::ActionRequest::SetFullscreen {
                                    project_id: ctx_fullscreen.project_id.clone(),
                                    terminal_id: Some(tid.clone()),
                                }, cx);
                            }
                        }
                    }),
            )
            // Detach
            .when(!is_remote, |el| {
                el.child(
                    header_button_base(HeaderAction::Detach, &id_suffix, ButtonSize::COMPACT, &t, None)
                        .on_click(move |_, _window, cx| {
                            let full_path = if ctx_detach.standalone {
                                ctx_detach.layout_path.clone()
                            } else {
                                let mut p = ctx_detach.layout_path.clone();
                                p.push(ctx_detach.active_tab);
                                p
                            };
                            ctx_detach.workspace.update(cx, |ws, cx| {
                                ws.detach_terminal(&ctx_detach.project_id, &full_path, cx);
                            });
                        }),
                )
            })
            // Close Tab
            .child({
                header_button_base(HeaderAction::Close, &id_suffix, ButtonSize::COMPACT, &t, Some(if standalone { "Close" } else { "Close Tab" }))
                    .on_click(move |_, _window, cx| {
                        if let Some(ref dispatcher) = ctx_close.action_dispatcher {
                            if let Some(ref aid) = app_id_for_close {
                                dispatcher.dispatch(okena_core::api::ActionRequest::CloseApp {
                                    project_id: ctx_close.project_id.clone(),
                                    app_id: aid.clone(),
                                }, cx);
                            } else if let Some(ref tid) = terminal_id_for_close {
                                dispatcher.dispatch(okena_core::api::ActionRequest::CloseTerminal {
                                    project_id: ctx_close.project_id.clone(),
                                    terminal_id: tid.clone(),
                                }, cx);
                            }
                        }
                    })
            })
    }

    pub(super) fn render_tabs(
        &mut self,
        children: &[LayoutNode],
        active_tab: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // If a zoomed terminal exists in one of our children, render only that child at full size
        if let Some(zoomed_idx) = self.find_zoomed_child_index(children, cx) {
            let mut child_path = self.layout_path.clone();
            child_path.push(zoomed_idx);

            let container = self
                .child_containers
                .entry(child_path.clone())
                .or_insert_with(|| {
                    cx.new(|_cx| {
                        LayoutContainer::new(
                            self.workspace.clone(),
                            self.request_broker.clone(),
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.action_dispatcher.clone(),
                            self.app_broadcaster.clone(),
                        )
                    })
                })
                .clone();

            return v_flex()
                .size_full()
                .child(container);
        }

        // Clean up stale child containers (e.g., when a tab was removed)
        let num_children = children.len();
        let valid_paths: HashSet<Vec<usize>> = (0..num_children)
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();
        self.child_containers.retain(|path, _| valid_paths.contains(path));

        // Shared reference to container bounds (updated by canvas during prepaint)
        let container_bounds_ref = self.container_bounds_ref.clone();

        v_flex()
            .size_full()
            .relative()
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
            .child(self.render_tab_bar(children, active_tab, false, cx))
            .child(
                // Active tab content
                div().flex_1().min_h_0().child({
                    let mut child_path = self.layout_path.clone();
                    child_path.push(active_tab);

                    let container = self.child_containers
                        .entry(child_path.clone())
                        .or_insert_with(|| {
                            cx.new(|_cx| {
                                LayoutContainer::new(
                                    self.workspace.clone(),
                                    self.request_broker.clone(),
                                    self.project_id.clone(),
                                    self.project_path.clone(),
                                    child_path.clone(),
                                    self.backend.clone(),
                                    self.terminals.clone(),
                                    self.active_drag.clone(),
                                    self.action_dispatcher.clone(),
                                    self.app_broadcaster.clone(),
                                )
                            })
                        })
                        .clone();

                    container
                }),
            )
    }

    /// Render a tab bar for a standalone pane (not inside a Tabs container).
    /// Delegates to the shared `render_tab_bar` with the current node.
    pub(super) fn render_standalone_tab_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let node = {
            let ws = self.workspace.read(cx);
            self.get_layout(&ws).cloned()
        };

        let children: &[LayoutNode] = match node {
            Some(ref n @ LayoutNode::Terminal { .. }) => std::slice::from_ref(n),
            Some(ref n @ LayoutNode::App { .. }) => std::slice::from_ref(n),
            _ => &[],
        };

        self.render_tab_bar(children, 0, true, cx)
    }

    /// Shared tab bar rendering used by both multi-tab and standalone modes.
    fn render_tab_bar(
        &mut self,
        children: &[LayoutNode],
        active_tab: usize,
        standalone: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();
        let num_children = children.len();

        // Check for active drop animation
        let drop_animation = self.drop_animation;

        // Get terminal names for tabs from the terminals registry
        let terminals = self.terminals.clone();
        let workspace_reader = self.workspace.read(cx);
        let project = workspace_reader.project(&self.project_id);
        let project_for_names = project.cloned();

        // Check if the focused terminal is within this tab group
        let is_pane_focused = workspace_reader.focus_manager
            .focused_terminal_state()
            .map_or(false, |f| {
                f.project_id == self.project_id
                    && f.layout_path.starts_with(&self.layout_path)
            });

        // Build tab elements
        let tab_elements: Vec<_> = children.iter().enumerate().map(|(i, child)| {
            let is_active = i == active_tab;
            let workspace = workspace.clone();
            let project_id = project_id.clone();
            let project_id_for_drag = project_id.clone();
            let project_id_for_drop = project_id.clone();
            let layout_path = layout_path.clone();
            let layout_path_for_drag = layout_path.clone();
            let layout_path_for_drop = layout_path.clone();

            // Detect terminal vs app child and extract pane info
            let (pane_id, pane_icon, is_app) = match child {
                LayoutNode::Terminal { terminal_id: Some(id), .. } => (Some(id.clone()), "icons/terminal.svg", false),
                LayoutNode::App { app_id, app_kind, .. } => {
                    let icon = app_registry::find_app(app_kind)
                        .map(|def| def.icon_path)
                        .unwrap_or("icons/terminal.svg");
                    (app_id.clone(), icon, true)
                }
                _ => (None, "icons/terminal.svg", false),
            };

            // Terminal-specific: get terminal_id for idle detection and rename
            let terminal_id = match child {
                LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.clone()),
                _ => None,
            };

            // Check cached waiting state and idle duration (terminals only)
            let (is_waiting, idle_label) = terminal_id.as_ref().map_or((false, None), |tid| {
                let guard = terminals.lock();
                guard.get(tid).map_or((false, None), |t| {
                    if t.is_waiting_for_input() {
                        (true, Some(t.idle_duration_display()))
                    } else {
                        (false, None)
                    }
                })
            });

            // Get tab label
            let tab_label = if is_app {
                match child {
                    LayoutNode::App { app_kind, .. } => {
                        app_registry::find_app(app_kind)
                            .map(|def| def.display_name.to_string())
                            .unwrap_or_else(|| format!("Tab {}", i + 1))
                    }
                    _ => format!("Tab {}", i + 1),
                }
            } else if let Some(ref tid) = terminal_id {
                if let Some(ref p) = project_for_names {
                    let osc_title = terminals.lock().get(tid).and_then(|t| t.title());
                    p.terminal_display_name(tid, osc_title)
                } else {
                    format!("Tab {}", i + 1)
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
                .pt(px(4.0))
                .pb(px(4.0))
                .border_r_1()
                .border_color(rgb(t.border))
                .text_size(px(12.0))
                .items_center()
                .when(is_active && is_pane_focused, |d| {
                    d.bg(rgb(t.term_background))
                        .text_color(rgb(t.text_primary))
                })
                .when(is_active && !is_pane_focused, |d| {
                    d.bg(rgb(t.term_background_unfocused))
                        .text_color(rgb(t.text_primary))
                })
                .when(!is_active && is_pane_focused, |d| {
                    d.bg(rgb(t.term_background_unfocused))
                        .text_color(rgb(t.text_secondary))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                })
                .when(!is_active && !is_pane_focused, |d| {
                    d.bg(rgb(t.bg_header))
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
                // Tab content with icon and label (or rename input)
                .child({
                    let is_renaming_this = terminal_id.as_ref().map_or(false, |tid| {
                        is_renaming(&self.tab_rename_state, tid)
                    });
                    if !is_app && is_renaming_this {
                        if let Some(input) = rename_input(&self.tab_rename_state) {
                            div()
                                .id(format!("tab-rename-{}", i))
                                .key_context("TerminalRename")
                                .flex_1()
                                .min_w(px(80.0))
                                .bg(rgb(t.bg_secondary))
                                .border_1()
                                .border_color(rgb(t.border_active))
                                .rounded(px(4.0))
                                .child(SimpleInput::new(input).text_size(px(12.0)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(|_, _window, cx| {
                                    cx.stop_propagation();
                                })
                                .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                                    this.cancel_tab_rename(cx);
                                }))
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    if event.keystroke.key.as_str() == "enter" {
                                        this.finish_tab_rename(cx);
                                    }
                                }))
                                .into_any_element()
                        } else {
                            let icon_color = if is_waiting { rgb(t.border_idle) } else if is_active { rgb(t.success) } else { rgb(t.text_muted) };
                            h_flex()
                                .gap(px(6.0))
                                .child(svg().path(pane_icon).size(px(12.0)).text_color(icon_color))
                                .child(tab_label.clone())
                                .children(idle_label.as_ref().map(|d| {
                                    div().text_size(px(10.0)).text_color(rgb(t.border_idle)).child(d.clone())
                                }))
                                .into_any_element()
                        }
                    } else {
                        let icon_color = if is_app {
                            if is_active { rgb(t.success) } else { rgb(t.text_muted) }
                        } else if is_waiting {
                            rgb(t.border_idle)
                        } else if is_active {
                            rgb(t.success)
                        } else {
                            rgb(t.text_muted)
                        };
                        h_flex()
                            .gap(px(6.0))
                            .child(svg().path(pane_icon).size(px(12.0)).text_color(icon_color))
                            .child(tab_label.clone())
                            .children(idle_label.as_ref().map(|d| {
                                div().text_size(px(10.0)).text_color(rgb(t.border_idle)).child(d.clone())
                            }))
                            .into_any_element()
                    }
                })
                // Right-click for context menu
                .on_mouse_down(MouseButton::Right, {
                    let project_id = project_id.clone();
                    let layout_path = layout_path.clone();
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        this.request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(
                                crate::workspace::requests::OverlayRequest::TabContextMenu {
                                    tab_index: i,
                                    num_tabs: num_children,
                                    project_id: project_id.clone(),
                                    layout_path: layout_path.clone(),
                                    position: event.position,
                                },
                                cx,
                            );
                        });
                        cx.stop_propagation();
                    })
                })
                // Middle-click to close tab
                .on_mouse_down(MouseButton::Middle, {
                    let project_id = project_id.clone();
                    let pane_id = pane_id.clone();
                    let is_app = is_app;
                    let action_dispatcher = self.action_dispatcher.clone();
                    cx.listener(move |_this, _event: &MouseDownEvent, _window, cx| {
                        if let Some(ref id) = pane_id {
                            if let Some(ref dispatcher) = action_dispatcher {
                                if is_app {
                                    dispatcher.dispatch(okena_core::api::ActionRequest::CloseApp {
                                        project_id: project_id.clone(),
                                        app_id: id.clone(),
                                    }, cx);
                                } else {
                                    dispatcher.dispatch(okena_core::api::ActionRequest::CloseTerminal {
                                        project_id: project_id.clone(),
                                        terminal_id: id.clone(),
                                    }, cx);
                                }
                            }
                        }
                        cx.stop_propagation();
                    })
                })
                // Drag source (works for both terminals and apps)
                .when_some(pane_id.clone(), |el, pid| {
                    let pane_path = if standalone {
                        layout_path_for_drag.clone()
                    } else {
                        let mut p = layout_path_for_drag.clone();
                        p.push(i);
                        p
                    };
                    el.on_drag(
                        PaneDrag {
                            project_id: project_id_for_drag.clone(),
                            layout_path: pane_path,
                            pane_id: pid,
                            pane_name: tab_label.clone(),
                            icon_path: pane_icon.to_string(),
                        },
                        move |drag, _position, _window, cx| {
                            cx.new(|_| PaneDragView::new(drag.pane_name.clone(), drag.icon_path.clone()))
                        },
                    )
                })
                // Drop target indicator
                .when(!standalone, |el| {
                    el.drag_over::<PaneDrag>({
                        let active_drag = self.active_drag.clone();
                        move |style, _, _, _| {
                            if active_drag.borrow().is_some() {
                                return style;
                            }
                            style
                                .border_l(px(3.0))
                                .border_color(rgb(t.border_active))
                                .bg(with_alpha(t.border_active, 0.15))
                        }
                    })
                    .on_drop(cx.listener({
                        let active_drag = self.active_drag.clone();
                        let dispatcher_for_drop = self.action_dispatcher.clone();
                        move |this, drag: &PaneDrag, _window, cx| {
                            if active_drag.borrow().is_some() {
                                return;
                            }
                            if drag.project_id != project_id_for_drop {
                                return;
                            }

                            let drag_parent = &drag.layout_path[..drag.layout_path.len().saturating_sub(1)];
                            let drag_tab_index = drag.layout_path.last().copied();

                            if drag_parent == layout_path_for_drop.as_slice() {
                                if let Some(from_index) = drag_tab_index {
                                    if from_index != i {
                                        let target_index = if from_index < i { i - 1 } else { i };
                                        if let Some(ref dispatcher) = dispatcher_for_drop {
                                            dispatcher.dispatch(okena_core::api::ActionRequest::MoveTab {
                                                project_id: project_id_for_drop.clone(),
                                                path: layout_path_for_drop.clone(),
                                                from_index,
                                                to_index: i,
                                            }, cx);
                                        }
                                        this.start_drop_animation(target_index, cx);
                                    }
                                }
                            } else {
                                if let Some(ref dispatcher) = dispatcher_for_drop {
                                    dispatcher.dispatch(okena_core::api::ActionRequest::MoveTerminalToTabGroup {
                                        project_id: drag.project_id.clone(),
                                        terminal_id: drag.pane_id.clone(),
                                        target_path: layout_path_for_drop.clone(),
                                        position: Some(i),
                                    }, cx);
                                }
                            }
                        }
                    }))
                })
                .on_click({
                    let workspace = workspace.clone();
                    let project_id = project_id.clone();
                    let layout_path = layout_path.clone();
                    let terminal_id = terminal_id.clone();
                    let pane_id = pane_id.clone();
                    let is_app = is_app;
                    let tab_label = tab_label.clone();
                    let dispatcher_for_click = self.action_dispatcher.clone();
                    cx.listener(move |this, _, window, cx| {
                        let is_double_click = this.tab_click_detector.check(i);

                        // Cancel any active rename if clicking a different tab
                        if this.tab_rename_state.is_some() && !is_double_click {
                            let is_renaming_this = terminal_id.as_ref().map_or(false, |tid| {
                                is_renaming(&this.tab_rename_state, tid)
                            });
                            if !is_renaming_this {
                                this.cancel_tab_rename(cx);
                            }
                        }

                        if !standalone {
                            // Switch to clicked tab
                            if let Some(ref dispatcher) = dispatcher_for_click {
                                dispatcher.dispatch(okena_core::api::ActionRequest::SetActiveTab {
                                    project_id: project_id.clone(),
                                    path: layout_path.clone(),
                                    index: i,
                                }, cx);
                            }
                        }

                        // Focus the pane in the clicked tab
                        if pane_id.is_some() {
                            let pane_path = if standalone {
                                layout_path.clone()
                            } else {
                                let mut p = layout_path.clone();
                                p.push(i);
                                p
                            };
                            workspace.update(cx, |ws, cx| {
                                ws.set_focused_terminal(project_id.clone(), pane_path, cx);
                            });
                        }

                        // Double-click → start rename (terminals only, not apps)
                        if is_double_click && !is_app {
                            if let Some(ref tid) = terminal_id {
                                this.start_tab_rename(tid.clone(), tab_label.clone(), window, cx);
                            }
                        }
                    })
                })
        }).collect();

        // End drop zone / empty area
        let project_id_for_new = self.project_id.clone();
        let layout_path_for_new = self.layout_path.clone();
        let dispatcher_for_new = self.action_dispatcher.clone();

        let mut end_drop_zone = div()
            .id(ElementId::Name(format!("tab-end-drop-{:?}", self.layout_path).into()))
            .flex_1()
            .h_full()
            .min_w(px(20.0))
            .on_click(cx.listener(move |this, _, _window, cx| {
                if this.empty_area_click_detector.check(()) {
                    if let Some(ref dispatcher) = dispatcher_for_new {
                        dispatcher.add_tab(
                            &project_id_for_new,
                            &layout_path_for_new,
                            !standalone,
                            cx,
                        );
                    }
                }
            }));

        if !standalone {
            let active_drag_for_end_hover = self.active_drag.clone();
            let active_drag_for_end_drop = self.active_drag.clone();
            let project_id_for_end = self.project_id.clone();
            let layout_path_for_end = self.layout_path.clone();
            let dispatcher_for_end = self.action_dispatcher.clone();

            end_drop_zone = end_drop_zone
                .drag_over::<PaneDrag>(move |style, _, _, _| {
                    if active_drag_for_end_hover.borrow().is_some() {
                        return style;
                    }
                    style
                        .border_l(px(3.0))
                        .border_color(rgb(t.border_active))
                        .bg(with_alpha(t.border_active, 0.1))
                })
                .on_drop(cx.listener(move |this, drag: &PaneDrag, _window, cx| {
                    if active_drag_for_end_drop.borrow().is_some() {
                        return;
                    }
                    if drag.project_id != project_id_for_end {
                        return;
                    }

                    let drag_parent = &drag.layout_path[..drag.layout_path.len().saturating_sub(1)];
                    let drag_tab_index = drag.layout_path.last().copied();

                    if drag_parent == layout_path_for_end.as_slice() {
                        if let Some(from_index) = drag_tab_index {
                            let target_index = num_children;
                            if from_index != target_index - 1 {
                                if let Some(ref dispatcher) = dispatcher_for_end {
                                    dispatcher.dispatch(okena_core::api::ActionRequest::MoveTab {
                                        project_id: project_id_for_end.clone(),
                                        path: layout_path_for_end.clone(),
                                        from_index,
                                        to_index: target_index,
                                    }, cx);
                                }
                                this.start_drop_animation(num_children - 1, cx);
                            }
                        }
                    } else {
                        if let Some(ref dispatcher) = dispatcher_for_end {
                            dispatcher.dispatch(okena_core::api::ActionRequest::MoveTerminalToTabGroup {
                                project_id: drag.project_id.clone(),
                                terminal_id: drag.pane_id.clone(),
                                target_path: layout_path_for_end.clone(),
                                position: None,
                            }, cx);
                        }
                    }
                }));
        }

        // Build action context for tab buttons
        let action_ctx = TabActionContext {
            workspace: self.workspace.clone(),
            project_id: self.project_id.clone(),
            layout_path: self.layout_path.clone(),
            active_tab,
            standalone,
            action_dispatcher: self.action_dispatcher.clone(),
        };

        // Get terminal_id and app_id for actions
        let (terminal_id_for_actions, app_id_for_actions) = if standalone {
            match children.first() {
                Some(LayoutNode::Terminal { terminal_id, .. }) => (terminal_id.clone(), None),
                Some(LayoutNode::App { app_id, .. }) => (None, app_id.clone()),
                _ => (None, None),
            }
        } else {
            (self.get_active_terminal_id(active_tab, cx), self.get_active_app_id(active_tab, cx))
        };

        let action_buttons = self.render_tab_action_buttons(action_ctx, terminal_id_for_actions.clone(), app_id_for_actions, cx);

        // Check shell selector visibility
        let show_shell = settings(cx).show_shell_selector && !self.backend.is_remote();

        // Tab bar
        div()
            .group("tab-bar-row")
            .h(px(28.0))
            .px(px(0.0))
            .flex()
            .items_center()
            .gap(px(0.0))
            .bg(rgb(if is_pane_focused { t.term_background_unfocused } else { t.bg_header }))
            .children(tab_elements)
            .child(end_drop_zone)
            .child(
                h_flex()
                    .opacity(0.0)
                    .group_hover("tab-bar-row", |s| s.opacity(1.0))
                    .when(show_shell, |el| {
                        el.child(self.render_shell_indicator(active_tab, cx))
                    })
                    .child(action_buttons),
            )
    }
}
