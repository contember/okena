//! Okena terminal views crate.
//!
//! Contains custom GPUI elements for terminal rendering and the layout system
//! (split panes, tabs, terminal panes) used by the main application.

pub mod actions;
pub mod elements;
pub mod layout;

mod simple_input;

use okena_core::api::ActionRequest;
use okena_workspace::state::SplitDirection;

/// Trait for dispatching terminal actions (local or remote).
///
/// This abstracts the `ActionDispatcher` enum from the main application,
/// allowing the layout views to dispatch actions without knowing whether
/// the project is local or remote.
pub trait ActionDispatch: Clone + 'static {
    /// Dispatch a standard action.
    fn dispatch(&self, action: ActionRequest, cx: &mut gpui::App);

    /// Whether this dispatcher targets a remote project.
    fn is_remote(&self) -> bool;

    /// Split a terminal.
    fn split_terminal(
        &self,
        project_id: &str,
        layout_path: &[usize],
        direction: SplitDirection,
        cx: &mut gpui::App,
    );

    /// Add a tab.
    fn add_tab(
        &self,
        project_id: &str,
        layout_path: &[usize],
        in_group: bool,
        cx: &mut gpui::App,
    );
}

/// Settings needed by terminal views.
///
/// Extracted from the main app's global settings to avoid a direct dependency.
/// Callers should populate this from their settings system.
#[derive(Clone, Debug)]
pub struct TerminalViewSettings {
    pub font_size: f32,
    pub line_height: f32,
    pub font_family: String,
    pub cursor_style: okena_workspace::settings::CursorShape,
    pub cursor_blink: bool,
    pub show_focused_border: bool,
    pub show_shell_selector: bool,
    pub idle_timeout_secs: u32,
    pub color_tinted_background: bool,
    pub file_opener: String,
    pub default_shell: okena_terminal::shell_config::ShellType,
    pub hooks: okena_workspace::settings::HooksConfig,
}

/// Global settings wrapper for crate-wide access.
#[derive(Clone)]
pub struct GlobalTerminalViewSettings(pub gpui::Entity<TerminalViewSettingsState>);

impl gpui::Global for GlobalTerminalViewSettings {}

/// Settings state entity that can be observed.
pub struct TerminalViewSettingsState {
    pub settings: TerminalViewSettings,
}

/// Get the current terminal view settings from the global entity.
pub fn terminal_view_settings(cx: &gpui::App) -> TerminalViewSettings {
    cx.global::<GlobalTerminalViewSettings>()
        .0
        .read(cx)
        .settings
        .clone()
}

/// Get the terminal view settings entity.
pub fn terminal_view_settings_entity(cx: &gpui::App) -> gpui::Entity<TerminalViewSettingsState> {
    cx.global::<GlobalTerminalViewSettings>().0.clone()
}

/// Callback type for registering content panes for dirty notification.
pub type RegisterContentPaneFn = Box<dyn Fn(String, gpui::WeakEntity<layout::terminal_pane::TerminalContent>) + Send + Sync>;

/// Global content pane registration function.
static REGISTER_CONTENT_PANE_FN: std::sync::OnceLock<RegisterContentPaneFn> = std::sync::OnceLock::new();

/// Set the global content pane registration function.
/// Called once by the main app at startup.
pub fn set_register_content_pane_fn(f: RegisterContentPaneFn) {
    let _ = REGISTER_CONTENT_PANE_FN.set(f);
}

/// Register a terminal content pane for direct dirty notification.
pub fn register_content_pane(
    terminal_id: String,
    content: gpui::WeakEntity<layout::terminal_pane::TerminalContent>,
) {
    if let Some(f) = REGISTER_CONTENT_PANE_FN.get() {
        f(terminal_id, content);
    }
}

/// Callback type for showing toast notifications.
pub type ToastErrorFn = Box<dyn Fn(String, &mut gpui::App) + Send + Sync>;

/// Global toast error function.
static TOAST_ERROR_FN: std::sync::OnceLock<ToastErrorFn> = std::sync::OnceLock::new();

/// Set the global toast error function.
pub fn set_toast_error_fn(f: ToastErrorFn) {
    let _ = TOAST_ERROR_FN.set(f);
}

/// Show an error toast notification.
pub fn toast_error(msg: String, cx: &mut gpui::App) {
    if let Some(f) = TOAST_ERROR_FN.get() {
        f(msg, cx);
    } else {
        log::error!("{}", msg);
    }
}

/// Implement the `Focusable` trait for a type that has a `focus_handle` field.
macro_rules! impl_focusable {
    ($type:ty) => {
        impl gpui::Focusable for $type {
            fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
                self.focus_handle.clone()
            }
        }
    };
}
#[allow(unused_macros)]
pub(crate) use impl_focusable;
