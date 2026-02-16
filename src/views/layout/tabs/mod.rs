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
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let id_suffix = format!("tabs-{:?}", ctx.layout_path);

        // Check if buffer capture is supported
        let supports_buffer_capture = self.backend.supports_buffer_capture();
        let backend_for_export = self.backend.clone();
        let terminal_id_for_export = terminal_id.clone();
        let terminal_id_for_close = terminal_id.clone();
        let terminal_id_for_fullscreen = terminal_id;

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
                    .on_click(move |_, _window, cx| {
                        let full_path = if ctx_minimize.standalone {
                            ctx_minimize.layout_path.clone()
                        } else {
                            let mut p = ctx_minimize.layout_path.clone();
                            p.push(ctx_minimize.active_tab);
                            p
                        };
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
                            ctx_fullscreen.workspace.update(cx, |ws, cx| {
                                ws.set_fullscreen_terminal(ctx_fullscreen.project_id.clone(), tid.clone(), cx);
                            });
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
                        if let Some(ref tid) = terminal_id_for_close {
                            if let Some(ref dispatcher) = ctx_close.action_dispatcher {
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
                .pt(px(4.0))
                .border_r_1()
                .border_color(rgb(t.border))
                .text_size(px(12.0))
                .items_center()
                .when(is_active, |d| {
                    d.bg(rgb(t.term_background))
                        .text_color(rgb(t.text_primary))
                        .pb(px(0.0))
                })
                .when(!is_active, |d| {
                    d.bg(rgb(t.term_background_unfocused))
                        .text_color(rgb(t.text_secondary))
                        .pb(px(4.0))
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
                    if is_renaming_this {
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
                            h_flex()
                                .gap(px(6.0))
                                .child(svg().path("icons/terminal.svg").size(px(12.0)).text_color(if is_active { rgb(t.success) } else { rgb(t.text_muted) }))
                                .child(tab_label.clone())
                                .into_any_element()
                        }
                    } else {
                        h_flex()
                            .gap(px(6.0))
                            .child(svg().path("icons/terminal.svg").size(px(12.0)).text_color(if is_active { rgb(t.success) } else { rgb(t.text_muted) }))
                            .child(tab_label.clone())
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
                    let terminal_id = terminal_id.clone();
                    let action_dispatcher = self.action_dispatcher.clone();
                    cx.listener(move |_this, _event: &MouseDownEvent, _window, cx| {
                        if let Some(ref tid) = terminal_id {
                            if let Some(ref dispatcher) = action_dispatcher {
                                dispatcher.dispatch(okena_core::api::ActionRequest::CloseTerminal {
                                    project_id: project_id.clone(),
                                    terminal_id: tid.clone(),
                                }, cx);
                            }
                        }
                        cx.stop_propagation();
                    })
                })
                // Drag source — use PaneDrag so tabs can be dropped onto pane edge zones too
                .when_some(terminal_id.clone(), |el, tid| {
                    let mut terminal_path = layout_path_for_drag.clone();
                    terminal_path.push(i);
                    el.on_drag(
                        PaneDrag {
                            project_id: project_id_for_drag.clone(),
                            layout_path: terminal_path,
                            terminal_id: tid,
                            terminal_name: tab_label.clone(),
                        },
                        move |drag, _position, _window, cx| {
                            cx.new(|_| PaneDragView::new(drag.terminal_name.clone()))
                        },
                    )
                })
                // Enhanced drop target - show prominent indicator with glow
                .drag_over::<PaneDrag>({
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
                            // Same tab group → reorder
                            if let Some(from_index) = drag_tab_index {
                                if from_index != i {
                                    let target_index = if from_index < i { i - 1 } else { i };
                                    workspace_for_drop.update(cx, |ws, cx| {
                                        ws.move_tab(&project_id_for_drop, &layout_path_for_drop, from_index, i, cx);
                                    });
                                    this.start_drop_animation(target_index, cx);
                                }
                            }
                        } else {
                            // Cross-container → insert into this tab group
                            workspace_for_drop.update(cx, |ws, cx| {
                                ws.move_terminal_to_tab_group(
                                    &drag.project_id, &drag.terminal_id,
                                    &layout_path_for_drop, Some(i), cx,
                                );
                            });
                        }
                    }
                }))
                .on_click({
                    let workspace = workspace.clone();
                    let project_id = project_id.clone();
                    let layout_path = layout_path.clone();
                    let terminal_id = terminal_id.clone();
                    let tab_label = tab_label.clone();
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

                        // Switch to clicked tab
                        workspace.update(cx, |ws, cx| {
                            ws.set_active_tab(&project_id, &layout_path, i, cx);
                        });

                        // Double-click → start rename
                        if is_double_click {
                            if let Some(ref tid) = terminal_id {
                                this.start_tab_rename(tid.clone(), tab_label.clone(), window, cx);
                            }
                        }
                    })
                })
        }).collect();

        // Create end drop zone for dropping after the last tab
        let workspace_for_end = self.workspace.clone();
        let project_id_for_end = self.project_id.clone();
        let layout_path_for_end = self.layout_path.clone();
        let active_drag_for_end_hover = self.active_drag.clone();
        let active_drag_for_end_drop = self.active_drag.clone();
        // Double-click on empty area creates new tab
        let workspace_for_new = self.workspace.clone();
        let project_id_for_new = self.project_id.clone();
        let layout_path_for_new = self.layout_path.clone();

        let end_drop_zone = div()
            .id(ElementId::Name(format!("tab-end-drop-{:?}", self.layout_path).into()))
            .flex_1()
            .h_full()
            .min_w(px(20.0))
            .on_click(cx.listener(move |this, _, _window, cx| {
                if this.empty_area_click_detector.check(()) {
                    workspace_for_new.update(cx, |ws, cx| {
                        ws.add_tab_to_group(&project_id_for_new, &layout_path_for_new, cx);
                    });
                }
            }))
            // Enhanced drop zone indicator
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
                    // Same tab group → reorder to end
                    if let Some(from_index) = drag_tab_index {
                        let target_index = num_children;
                        if from_index != target_index - 1 {
                            workspace_for_end.update(cx, |ws, cx| {
                                ws.move_tab(&project_id_for_end, &layout_path_for_end, from_index, target_index, cx);
                            });
                            this.start_drop_animation(num_children - 1, cx);
                        }
                    }
                } else {
                    // Cross-container → append to this tab group
                    workspace_for_end.update(cx, |ws, cx| {
                        ws.move_terminal_to_tab_group(
                            &drag.project_id, &drag.terminal_id,
                            &layout_path_for_end, None, cx,
                        );
                    });
                }
            }));

        // Build action context for tab buttons
        let action_ctx = TabActionContext {
            workspace: self.workspace.clone(),
            project_id: self.project_id.clone(),
            layout_path: self.layout_path.clone(),
            active_tab,
            standalone: false,
            action_dispatcher: self.action_dispatcher.clone(),
        };

        // Get terminal_id for actions that need it
        let terminal_id_for_actions = self.get_active_terminal_id(active_tab, cx);

        // Render action buttons using helper method
        let action_buttons = self.render_tab_action_buttons(action_ctx, terminal_id_for_actions, cx);

        // Check shell selector visibility
        let show_shell = settings(cx).show_shell_selector && !self.backend.is_remote();

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
            .child(
                // Tab bar
                div()
                    .group("tab-bar-row")
                    .h(px(28.0))
                    .px(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(0.0))
                    .bg(rgb(t.term_background_unfocused))
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
                    ),
            )
            .child(
                // Active tab content
                div().flex_1().child({
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
                                )
                            })
                        })
                        .clone();

                    container
                }),
            )
    }

    /// Render a tab bar for a standalone terminal (not inside a Tabs container).
    /// This gives every terminal a consistent tab bar UI.
    pub(super) fn render_standalone_tab_bar(
        &mut self,
        terminal_id: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();

        // Get terminal name
        let terminals = self.terminals.clone();
        let workspace_reader = self.workspace.read(cx);
        let project = workspace_reader.project(&self.project_id);
        let terminal_names_map = project.map(|p| p.terminal_names.clone());

        let tab_label = if let Some(ref tid) = terminal_id {
            let custom_name = terminal_names_map.as_ref().and_then(|m| m.get(tid).cloned());
            if let Some(name) = custom_name {
                name
            } else {
                let terminals_guard = terminals.lock();
                if let Some(terminal) = terminals_guard.get(tid) {
                    terminal.title().unwrap_or_else(|| "Terminal".to_string())
                } else {
                    "Terminal".to_string()
                }
            }
        } else {
            "Terminal".to_string()
        };

        // Check if drag is possible (more than one terminal in project)
        let can_drag = terminal_id.is_some() && {
            project
                .and_then(|p| p.layout.as_ref())
                .map(|l| l.collect_terminal_ids().len() > 1)
                .unwrap_or(false)
        };

        // Check for active rename
        let is_renaming_this = terminal_id.as_ref().map_or(false, |tid| {
            is_renaming(&self.tab_rename_state, tid)
        });

        // Build the single tab element
        let tab_element = {
            let terminal_id_for_drag = terminal_id.clone();
            let tab_label_for_drag = tab_label.clone();
            let layout_path_for_drag = layout_path.clone();
            let project_id_for_drag = project_id.clone();

            let mut tab = div()
                .id(ElementId::Name(format!("standalone-tab-{:?}", layout_path).into()))
                .cursor_pointer()
                .relative()
                .px(px(8.0))
                .pt(px(4.0))
                .pb(px(0.0))
                .border_r_1()
                .border_color(rgb(t.border))
                .text_size(px(12.0))
                .items_center()
                .bg(rgb(t.term_background))
                .text_color(rgb(t.text_primary))
                .child({
                    if is_renaming_this {
                        if let Some(input) = rename_input(&self.tab_rename_state) {
                            div()
                                .id("standalone-tab-rename")
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
                            h_flex()
                                .gap(px(6.0))
                                .child(svg().path("icons/terminal.svg").size(px(12.0)).text_color(rgb(t.success)))
                                .child(tab_label.clone())
                                .into_any_element()
                        }
                    } else {
                        h_flex()
                            .gap(px(6.0))
                            .child(svg().path("icons/terminal.svg").size(px(12.0)).text_color(rgb(t.success)))
                            .child(tab_label.clone())
                            .into_any_element()
                    }
                })
                .on_click({
                    let terminal_id = terminal_id.clone();
                    let tab_label = tab_label.clone();
                    cx.listener(move |this, _, window, cx| {
                        let is_double_click = this.tab_click_detector.check(0);
                        if is_double_click {
                            if let Some(ref tid) = terminal_id {
                                this.start_tab_rename(tid.clone(), tab_label.clone(), window, cx);
                            }
                        }
                    })
                });

            // Add drag support
            if can_drag {
                if let Some(ref tid) = terminal_id_for_drag {
                    tab = tab.on_drag(
                        PaneDrag {
                            project_id: project_id_for_drag,
                            layout_path: layout_path_for_drag,
                            terminal_id: tid.clone(),
                            terminal_name: tab_label_for_drag,
                        },
                        move |drag, _position, _window, cx| {
                            cx.new(|_| PaneDragView::new(drag.terminal_name.clone()))
                        },
                    );
                }
            }

            tab
        };

        // Empty area: double-click creates new tab
        let workspace_for_new = self.workspace.clone();
        let project_id_for_new = self.project_id.clone();
        let layout_path_for_new = self.layout_path.clone();

        let empty_area = div()
            .id(ElementId::Name(format!("standalone-tab-empty-{:?}", self.layout_path).into()))
            .flex_1()
            .h_full()
            .min_w(px(20.0))
            .on_click(cx.listener(move |this, _, _window, cx| {
                if this.empty_area_click_detector.check(()) {
                    workspace_for_new.update(cx, |ws, cx| {
                        ws.add_tab(&project_id_for_new, &layout_path_for_new, cx);
                    });
                }
            }));

        // Build action context for buttons
        let action_ctx = TabActionContext {
            workspace: self.workspace.clone(),
            project_id: self.project_id.clone(),
            layout_path: self.layout_path.clone(),
            active_tab: 0,
            standalone: true,
            action_dispatcher: self.action_dispatcher.clone(),
        };

        let action_buttons = self.render_tab_action_buttons(action_ctx, terminal_id.clone(), cx);

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
            .bg(rgb(t.term_background_unfocused))
            .child(tab_element)
            .child(empty_area)
            .child(
                h_flex()
                    .opacity(0.0)
                    .group_hover("tab-bar-row", |s| s.opacity(1.0))
                    .when(show_shell, |el| {
                        el.child(self.render_standalone_shell_indicator(terminal_id.clone(), cx))
                    })
                    .child(action_buttons),
            )
    }
}
