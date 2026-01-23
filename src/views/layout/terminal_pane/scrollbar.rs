//! Scrollbar component for terminal pane.
//!
//! An Entity with Render that handles scrollbar display and drag interactions.

use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use gpui::*;
use std::sync::Arc;
use std::time::Instant;

/// Scrollbar view that handles display and drag interactions.
pub struct Scrollbar {
    /// Reference to the terminal for scroll info
    terminal: Option<Arc<Terminal>>,
    /// Whether currently dragging
    dragging: bool,
    /// Y position where drag started
    drag_start_y: Option<f32>,
    /// Scroll offset when drag started
    drag_start_offset: Option<usize>,
    /// Last scroll activity time for auto-hide
    last_activity: Instant,
    /// Element bounds for calculations
    element_bounds: Option<Bounds<Pixels>>,
}

impl Scrollbar {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            terminal: None,
            dragging: false,
            drag_start_y: None,
            drag_start_offset: None,
            last_activity: Instant::now(),
            element_bounds: None,
        }
    }

    /// Set the terminal reference.
    pub fn set_terminal(&mut self, terminal: Option<Arc<Terminal>>) {
        self.terminal = terminal;
    }

    /// Check if currently dragging.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Mark scroll activity (for auto-hide timer).
    pub fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if scrollbar should be visible.
    fn should_show(&self) -> bool {
        if self.dragging {
            return true;
        }
        self.last_activity.elapsed().as_millis() < 1500
    }

    /// Check if there's scrollable content.
    fn has_scroll_content(&self) -> bool {
        self.terminal
            .as_ref()
            .map(|t| {
                let (total, visible, _) = t.scroll_info();
                total > visible
            })
            .unwrap_or(false)
    }

    /// Calculate scrollbar thumb geometry.
    fn calculate_geometry(&self, content_height: f32) -> Option<(f32, f32)> {
        let terminal = self.terminal.as_ref()?;
        let (total_lines, visible_lines, display_offset) = terminal.scroll_info();

        if total_lines <= visible_lines {
            return None;
        }

        let scrollable_lines = total_lines - visible_lines;
        let thumb_height = (visible_lines as f32 / total_lines as f32 * content_height).max(20.0);
        let available_space = content_height - thumb_height;
        let scroll_ratio = display_offset as f32 / scrollable_lines as f32;
        let thumb_y = (1.0 - scroll_ratio) * available_space;

        Some((thumb_y, thumb_height))
    }

    /// Start scrollbar drag.
    pub fn start_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            self.dragging = true;
            self.drag_start_y = Some(y);
            self.drag_start_offset = Some(terminal.display_offset());
            self.last_activity = Instant::now();
            cx.notify();
        }
    }

    /// Update scrollbar during drag.
    pub fn update_drag(&mut self, y: f32, content_height: f32, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }

        if let (Some(start_y), Some(start_offset), Some(ref terminal)) =
            (self.drag_start_y, self.drag_start_offset, &self.terminal)
        {
            let (total_lines, visible_lines, _) = terminal.scroll_info();
            if total_lines <= visible_lines {
                return;
            }

            let scrollable_lines = total_lines - visible_lines;
            let delta_y = y - start_y;
            let lines_per_pixel = scrollable_lines as f32 / content_height;
            let delta_lines = (-delta_y * lines_per_pixel).round() as i32;

            let new_offset =
                (start_offset as i32 + delta_lines).clamp(0, scrollable_lines as i32) as usize;
            terminal.scroll_to(new_offset);

            self.last_activity = Instant::now();
            cx.notify();
        }
    }

    /// End scrollbar drag.
    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.dragging = false;
        self.drag_start_y = None;
        self.drag_start_offset = None;
        cx.notify();
    }

    /// Handle scrollbar track click (jump to position).
    pub fn handle_click(&mut self, y: f32, content_height: f32, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            let (total_lines, visible_lines, _) = terminal.scroll_info();
            if total_lines <= visible_lines {
                return;
            }

            let scrollable_lines = total_lines - visible_lines;
            let ratio = 1.0 - (y / content_height).clamp(0.0, 1.0);
            let new_offset = (ratio * scrollable_lines as f32).round() as usize;
            terminal.scroll_to(new_offset);

            self.last_activity = Instant::now();
            cx.notify();
        }
    }

    /// Handle mouse down on scrollbar.
    fn handle_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.last_activity = Instant::now();

        if let Some(bounds) = self.element_bounds {
            let relative_y = f32::from(event.position.y) - f32::from(bounds.origin.y);
            let content_height = f32::from(bounds.size.height);

            if let Some((thumb_y, thumb_height)) = self.calculate_geometry(content_height) {
                if relative_y >= thumb_y && relative_y <= thumb_y + thumb_height {
                    // Click on thumb - start drag
                    self.start_drag(f32::from(event.position.y), cx);
                } else {
                    // Click on track - jump to position
                    self.handle_click(relative_y, content_height, cx);
                }
            }
        }
    }
}

impl Render for Scrollbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.has_scroll_content() {
            return div().into_any_element();
        }

        let opacity = if self.should_show() { 1.0 } else { 0.0 };
        let scrollbar_color = if self.dragging {
            rgb(t.scrollbar_hover)
        } else {
            rgb(t.scrollbar)
        };
        let scrollbar_hover_color = rgb(t.scrollbar_hover);
        let terminal_clone = self.terminal.clone();
        let dragging = self.dragging;

        div()
            .id("scrollbar")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(10.0))
            .opacity(opacity)
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.handle_mouse_down(event, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.dragging {
                    if let Some(bounds) = this.element_bounds {
                        let content_height = f32::from(bounds.size.height);
                        this.update_drag(f32::from(event.position.y), content_height, cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.end_drag(cx);
                }),
            )
            .child(
                canvas(
                    {
                        let entity = cx.entity().downgrade();
                        move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |this, _| {
                                    this.element_bounds = Some(bounds);
                                });
                            }
                        }
                    },
                    move |bounds: Bounds<Pixels>, _state: (), window: &mut Window, _cx: &mut App| {
                        if let Some(ref terminal) = terminal_clone {
                            let (total_lines, visible_lines, display_offset) = terminal.scroll_info();
                            if total_lines > visible_lines {
                                let track_height = f32::from(bounds.size.height);
                                let scrollable_lines = total_lines - visible_lines;
                                let thumb_height =
                                    (visible_lines as f32 / total_lines as f32 * track_height).max(20.0);
                                let available_scroll_space = track_height - thumb_height;
                                let scroll_ratio = display_offset as f32 / scrollable_lines as f32;
                                let thumb_y = (1.0 - scroll_ratio) * available_scroll_space;

                                let thumb_color = if dragging {
                                    scrollbar_hover_color
                                } else {
                                    scrollbar_color
                                };

                                let thumb_bounds = Bounds {
                                    origin: point(bounds.origin.x + px(2.0), bounds.origin.y + px(thumb_y)),
                                    size: size(px(6.0), px(thumb_height)),
                                };
                                window.paint_quad(fill(thumb_bounds, thumb_color).corner_radii(px(3.0)));
                            }
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}
