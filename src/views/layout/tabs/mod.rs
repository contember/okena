//! Tab bar rendering and management
//!
//! This module contains tab-related functionality for LayoutContainer:
//! - TabDrag, TabDragView: Drag-and-drop support for tab reordering
//! - TabActionContext: Helper for action button closures
//! - Tab bar rendering with drag support and animations

mod context_menu;
mod shell_selector;

use crate::theme::{theme, with_alpha};
use crate::views::header_buttons::{header_button_base, ButtonSize, HeaderAction};
use crate::views::layout::layout_container::LayoutContainer;
use crate::workspace::state::{LayoutNode, SplitDirection};
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;
use std::collections::HashSet;

/// Drag payload for tab reordering
#[derive(Clone)]
pub(super) struct TabDrag {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub tab_index: usize,
    pub tab_label: String,
}

/// Drag preview view for tabs - shows a polished ghost image during drag
pub(super) struct TabDragView {
    label: String,
}

impl TabDragView {
    pub fn new(label: String) -> Self {
        Self { label }
    }
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
                h_flex()
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
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.pty_manager.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                        )
                    })
                })
                .clone();

            return v_flex()
                .size_full()
                .child(container);
        }

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
                .border_r_1()
                .border_color(rgb(t.border))
                .text_size(px(12.0))
                .when(is_active, |d| {
                    d.bg(rgb(t.bg_hover))
                        .text_color(rgb(t.text_primary))
                })
                .when(!is_active, |d| {
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
                // Tab content with icon and label
                .child(
                    h_flex()
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
                // Middle-click to close tab
                .on_mouse_down(MouseButton::Middle, {
                    let workspace = workspace.clone();
                    let project_id = project_id.clone();
                    let layout_path = layout_path.clone();
                    cx.listener(move |_this, _event: &MouseDownEvent, _window, cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.close_tab(&project_id, &layout_path, i, cx);
                        });
                        cx.stop_propagation();
                    })
                })
                // Drag source for tab reordering
                .on_drag(
                    TabDrag {
                        project_id: project_id_for_drag,
                        layout_path: layout_path_for_drag,
                        tab_index: i,
                        tab_label: tab_label.clone(),
                    },
                    move |drag, _position, _window, cx| {
                        cx.new(|_| TabDragView::new(drag.tab_label.clone()))
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

        // Render shell indicator (dropdown is handled by overlay)
        let shell_indicator = self.render_shell_indicator(active_tab, cx);

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
            // Close context menu on left-click anywhere in tabs area
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                if this.tab_context_menu.is_some() {
                    this.hide_tab_context_menu(cx);
                }
            }))
            .child(
                // Tab bar
                div()
                    .group("tab-bar-row")
                    .h(px(28.0))
                    .px(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(0.0))
                    .bg(rgb(t.bg_header))
                    .children(tab_elements)
                    .child(end_drop_zone)
                    .child(
                        h_flex()
                            .opacity(0.0)
                            .group_hover("tab-bar-row", |s| s.opacity(1.0))
                            .child(shell_indicator)
                            .child(action_buttons),
                    ),
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
                                    self.active_drag.clone(),
                                )
                            })
                        })
                        .clone()
                }),
            )
    }
}
