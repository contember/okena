use crate::keybindings::{Quit, ShowCommandPalette, ShowKeybindings, ShowSettings, ShowThemeSelector, ToggleSidebar};
use crate::theme::theme;
use crate::views::components::menu_item;
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;

/// Window control button types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControlType {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Title bar with window controls and sidebar toggle
pub struct TitleBar {
    title: SharedString,
    menu_open: bool,
    sidebar_open: bool,
    /// HWND cached for Win32 drag operations (Windows only)
    #[cfg(target_os = "windows")]
    hwnd: Option<isize>,
}

impl TitleBar {
    pub fn new(
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            menu_open: false,
            sidebar_open: true,
            #[cfg(target_os = "windows")]
            hwnd: None,
        }
    }

    pub fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            cx.notify();
        }
    }

    pub fn is_menu_open(&self) -> bool {
        self.menu_open
    }

    fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = false;
        cx.notify();
    }

    /// Render the app dropdown menu overlay (must be called from a parent with full window coverage).
    pub fn render_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let traffic_light_padding = if cfg!(target_os = "macos") {
            px(80.0)
        } else {
            px(8.0)
        };

        div()
            .id("app-menu-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                cx.stop_propagation();
                this.close_menu(cx);
            }))
            .on_mouse_move(|_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                // Menu panel
                div()
                    .absolute()
                    .top(px(32.0))
                    .left(traffic_light_padding + px(40.0))
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(4.0))
                    .shadow_xl()
                    .min_w(px(200.0))
                    .py(px(4.0))
                    .id("app-menu-panel")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Settings
                    .child(
                        menu_item("app-menu-settings", "icons/edit.svg", "Open Settings", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_menu(cx);
                                window.dispatch_action(Box::new(ShowSettings), cx);
                            })),
                    )
                    // Theme
                    .child(
                        menu_item("app-menu-theme", "icons/eye.svg", "Select Theme", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_menu(cx);
                                window.dispatch_action(Box::new(ShowThemeSelector), cx);
                            })),
                    )
                    // Command Palette
                    .child(
                        menu_item("app-menu-command-palette", "icons/search.svg", "Command Palette", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_menu(cx);
                                window.dispatch_action(Box::new(ShowCommandPalette), cx);
                            })),
                    )
                    // Keyboard Shortcuts
                    .child(
                        menu_item("app-menu-keybindings", "icons/keyboard.svg", "Keyboard Shortcuts", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_menu(cx);
                                window.dispatch_action(Box::new(ShowKeybindings), cx);
                            })),
                    )
                    // Separator
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(rgb(t.border)),
                    )
                    // Exit
                    .child(
                        menu_item("app-menu-exit", "icons/close.svg", "Exit", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_menu(cx);
                                window.dispatch_action(Box::new(Quit), cx);
                            })),
                    ),
            )
    }

    fn render_window_control(
        &self,
        control_type: WindowControlType,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let icon = match control_type {
            WindowControlType::Minimize => "─",
            WindowControlType::Maximize => "□",
            WindowControlType::Restore => "❐",
            WindowControlType::Close => "✕",
        };

        let is_close = control_type == WindowControlType::Close;

        // On Windows, use WindowControlArea to let the OS handle button clicks natively.
        // This ensures proper maximize/restore toggle via WM_NCHITTEST.
        // On other platforms, use on_click handlers.
        let control_area = if cfg!(target_os = "windows") {
            Some(match control_type {
                WindowControlType::Minimize => WindowControlArea::Min,
                WindowControlType::Maximize | WindowControlType::Restore => WindowControlArea::Max,
                WindowControlType::Close => WindowControlArea::Close,
            })
        } else {
            None
        };

        div()
            .id(ElementId::Name(format!("window-control-{:?}", control_type).into()))
            .cursor_pointer()
            .w(px(46.0)) // Windows standard caption button width
            .h(px(32.0)) // Match titlebar height
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.0))
            .text_color(rgb(t.text_secondary))
            .when(is_close, |d| {
                d.hover(|s| s.bg(rgb(0xE81123)).text_color(rgb(0xffffff)))
            })
            .when(!is_close, |d| {
                d.hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .child(icon)
            .when_some(control_area, |d, area| {
                // occlude() prevents parent Drag hitbox from shadowing button hit tests
                d.occlude().window_control_area(area)
            })
            .when(control_area.is_none(), |d| {
                d
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click({
                        let control_type = control_type;
                        move |_, window, cx| {
                            cx.stop_propagation();
                            match control_type {
                                WindowControlType::Minimize => window.minimize_window(),
                                WindowControlType::Maximize | WindowControlType::Restore => {
                                    window.zoom_window();
                                }
                                WindowControlType::Close => {
                                    cx.quit();
                                }
                            }
                        }
                    })
            })
    }
}

/// Get the HWND from a GPUI Window on Windows.
#[cfg(target_os = "windows")]
fn get_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::HasWindowHandle;
    // Use trait method explicitly since Window has its own window_handle() method
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(win32) => Some(win32.hwnd.get() as isize),
        _ => None,
    }
}

// --- Win32 timer-based window drag ---
// We use a Win32 timer to poll cursor position and move the window.
// This runs outside GPUI's event dispatch, avoiding RefCell re-entrancy.
// The timer fires every ~16ms (60fps) while the mouse button is held.

#[cfg(target_os = "windows")]
extern "system" {
    fn GetWindowRect(hwnd: isize, rect: *mut WinRect) -> i32;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn IsZoomed(hwnd: isize) -> i32;
    fn GetCursorPos(point: *mut WinPoint) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn SetTimer(hwnd: isize, id: usize, elapse: u32, func: Option<unsafe extern "system" fn(isize, u32, usize, u32)>) -> usize;
    fn KillTimer(hwnd: isize, id: usize) -> i32;
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct WinPoint {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
const DRAG_TIMER_ID: usize = 0xD8A6; // Unique timer ID

/// Thread-local drag state for the timer callback.
#[cfg(target_os = "windows")]
struct DragState {
    hwnd: isize,
    /// Screen cursor position at drag start
    start_cursor: WinPoint,
    /// Window position at drag start
    start_window: WinPoint,
}

#[cfg(target_os = "windows")]
thread_local! {
    static DRAG_STATE: std::cell::RefCell<Option<DragState>> = const { std::cell::RefCell::new(None) };
}

/// Timer callback - polls cursor and moves window. Runs in the message loop,
/// outside GPUI's event dispatch.
#[cfg(target_os = "windows")]
unsafe extern "system" fn drag_timer_proc(_hwnd: isize, _msg: u32, _id: usize, _time: u32) {
    const VK_LBUTTON: i32 = 0x01;
    // Check if left mouse button is still held
    if GetAsyncKeyState(VK_LBUTTON) >= 0 {
        // Button released - stop dragging
        stop_drag();
        return;
    }

    DRAG_STATE.with(|state| {
        let state = state.borrow();
        if let Some(ds) = state.as_ref() {
            let mut cursor = WinPoint { x: 0, y: 0 };
            if GetCursorPos(&mut cursor) != 0 {
                let dx = cursor.x - ds.start_cursor.x;
                let dy = cursor.y - ds.start_cursor.y;
                const SWP_NOSIZE: u32 = 0x0001;
                const SWP_NOZORDER: u32 = 0x0004;
                const SWP_NOACTIVATE: u32 = 0x0010;
                SetWindowPos(
                    ds.hwnd, 0,
                    ds.start_window.x + dx,
                    ds.start_window.y + dy,
                    0, 0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    });
}

/// Start window drag timer.
#[cfg(target_os = "windows")]
fn start_drag(hwnd: isize) {
    unsafe {
        let mut cursor = WinPoint { x: 0, y: 0 };
        let mut rect = WinRect { left: 0, top: 0, right: 0, bottom: 0 };
        if GetCursorPos(&mut cursor) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
            return;
        }
        DRAG_STATE.with(|state| {
            *state.borrow_mut() = Some(DragState {
                hwnd,
                start_cursor: cursor,
                start_window: WinPoint { x: rect.left, y: rect.top },
            });
        });
        SetTimer(hwnd, DRAG_TIMER_ID, 16, Some(drag_timer_proc));
    }
}

/// Stop window drag timer.
#[cfg(target_os = "windows")]
fn stop_drag() {
    DRAG_STATE.with(|state| {
        if let Some(ds) = state.borrow().as_ref() {
            unsafe { KillTimer(ds.hwnd, DRAG_TIMER_ID); }
        }
        *state.borrow_mut() = None;
    });
}

/// Toggle maximize/restore using ShowWindow.
#[cfg(target_os = "windows")]
fn toggle_maximize_hwnd(hwnd: isize) {
    const SW_MAXIMIZE: i32 = 3;
    const SW_RESTORE: i32 = 9;
    unsafe {
        let cmd = if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE };
        ShowWindow(hwnd, cmd);
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_maximized = window.is_maximized();
        // On Windows, always show custom window controls since we use a custom titlebar
        // On macOS, use native traffic lights (server decorations)
        // On Linux, check runtime decorations
        let needs_controls = if cfg!(target_os = "windows") {
            true
        } else if cfg!(target_os = "macos") {
            false
        } else {
            // Linux: check runtime decorations
            matches!(window.window_decorations(), Decorations::Client { .. })
        };

        // On macOS with server decorations, we need to leave space for traffic lights
        let traffic_light_padding = if cfg!(target_os = "macos") {
            px(80.0) // Space for macOS traffic lights (close, minimize, fullscreen)
        } else {
            px(8.0)
        };

        // On macOS, the title bar only provides space for traffic lights (no content)
        let title_bar_height = if cfg!(target_os = "macos") {
            px(28.0)
        } else {
            px(32.0)
        };

        div()
            .id("title-bar")
            .h(title_bar_height)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            // On Windows, use a Win32 timer to poll cursor position and move the window.
            // This avoids GPUI's RefCell re-entrancy and works even when the cursor
            // leaves the titlebar during fast drags.
            // On other platforms, use WindowControlArea::Drag for platform-native drag.
            .when(cfg!(target_os = "windows"), |d| {
                d
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, _cx| {
                        #[cfg(target_os = "windows")]
                        {
                            // Cache HWND on first use
                            if this.hwnd.is_none() {
                                this.hwnd = get_hwnd(window);
                            }
                            if let Some(hwnd) = this.hwnd {
                                start_drag(hwnd);
                            }
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            let _ = (this, window);
                        }
                    }))
                    .on_click(cx.listener(|this, event: &ClickEvent, _window, _cx| {
                        if event.click_count() == 2 {
                            #[cfg(target_os = "windows")]
                            {
                                if let Some(hwnd) = this.hwnd {
                                    toggle_maximize_hwnd(hwnd);
                                }
                            }
                            #[cfg(not(target_os = "windows"))]
                            {
                                let _ = this;
                            }
                        }
                    }))
            })
            .when(!cfg!(target_os = "windows"), |d| {
                d.window_control_area(WindowControlArea::Drag)
            })
            .child(
                // Left side - sidebar toggle + title
                h_flex()
                    .gap(px(8.0))
                    .pl(traffic_light_padding)
                    // On macOS, sidebar toggle lives in the sidebar footer instead
                    .when(!cfg!(target_os = "macos"), |d| {
                        d.child(
                            // Sidebar toggle
                            div()
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .text_size(px(14.0))
                                .text_color(if self.sidebar_open {
                                    rgb(t.term_blue)
                                } else {
                                    rgb(t.text_secondary)
                                })
                                .child("☰")
                                .id("sidebar-toggle")
                                // Stop propagation to prevent title bar drag from capturing the click
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.dispatch_action(Box::new(ToggleSidebar), cx);
                                }),
                        )
                    })
                    // On macOS, app menu items live in the native menu bar
                    .when(!cfg!(target_os = "macos"), |d| {
                        d.child({
                            let menu_open = self.menu_open;
                            let chevron = if menu_open { "▲" } else { "▼" };
                            div()
                                .id("app-menu-trigger")
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .when(menu_open, |d| d.bg(rgb(t.bg_hover)))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(t.text_primary))
                                        .child(self.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(8.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(chevron),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.toggle_menu(cx);
                                }))
                        })
                    }),
            )
            .child(
                // Center - spacer
                div().flex_1()
            )
            .child(
                // Right side - window controls
                h_flex()
                    .gap(px(8.0))
                    .pr(px(4.0))
                    .when(needs_controls, |d| {
                        d.child(
                            h_flex()
                                .gap(px(2.0))
                                .child(self.render_window_control(WindowControlType::Minimize, window, cx))
                                .child(if is_maximized {
                                    self.render_window_control(WindowControlType::Restore, window, cx).into_any_element()
                                } else {
                                    self.render_window_control(WindowControlType::Maximize, window, cx).into_any_element()
                                })
                                .child(self.render_window_control(WindowControlType::Close, window, cx))
                        )
                    }),
            )
    }
}
