//! Settings panel for visual settings configuration
//!
//! Provides a Zed-style settings dialog with sidebar categories, project selector,
//! and hooks configuration.

use crate::settings::{open_settings_file, settings_entity, SettingsState};
use crate::terminal::session_backend::SessionBackend;
use crate::terminal::shell_config::{available_shells, AvailableShell, ShellType};
use crate::theme::{theme, ThemeColors};
use crate::views::components::{dropdown_button, dropdown_option, dropdown_overlay, modal_backdrop, modal_content};
use crate::views::components::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use crate::workspace::persistence::get_settings_path;
use crate::workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;

/// Available monospace font families
const FONT_FAMILIES: &[&str] = &[
    "JetBrains Mono",
    "Menlo",
    "SF Mono",
    "Monaco",
    "Fira Code",
    "Source Code Pro",
    "Consolas",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Hack",
];

// ============================================================================
// Settings Categories
// ============================================================================

#[derive(Clone, Copy, PartialEq)]
enum SettingsCategory {
    General,
    Font,
    Terminal,
    Hooks,
}

impl SettingsCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Font => "Font",
            Self::Terminal => "Terminal",
            Self::Hooks => "Hooks",
        }
    }

    fn all() -> &'static [SettingsCategory] {
        &[Self::General, Self::Font, Self::Terminal, Self::Hooks]
    }

    /// Categories available in project mode (only hooks for now)
    fn project_categories() -> &'static [SettingsCategory] {
        &[Self::Hooks]
    }
}

// ============================================================================
// Reusable UI Components
// ============================================================================

/// Render a section header
fn section_header(title: &str, t: &ThemeColors) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_muted))
        .child(title.to_uppercase())
}

/// Render a settings section container
fn section_container(t: &ThemeColors) -> Div {
    div()
        .mx(px(16.0))
        .mb(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
        .overflow_hidden()
}

/// Render a settings row container
fn settings_row(id: impl Into<SharedString>, label: &str, t: &ThemeColors, has_border: bool) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .child(label.to_string()),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Render a settings row with label and description
fn settings_row_with_desc(id: impl Into<SharedString>, label: &str, desc: &str, t: &ThemeColors, has_border: bool) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(t.text_primary))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_muted))
                        .child(desc.to_string()),
                ),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Render a +/- stepper button
fn stepper_button(id: impl Into<SharedString>, label: &str, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(24.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(px(14.0))
        .text_color(rgb(t.text_secondary))
        .child(label.to_string())
}

/// Render a value display box
fn value_display(value: String, width: f32, t: &ThemeColors) -> Div {
    div()
        .w(px(width))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .text_size(px(13.0))
        .font_family("monospace")
        .text_color(rgb(t.text_primary))
        .child(value)
}

/// Render a toggle switch
fn toggle_switch(id: impl Into<SharedString>, enabled: bool, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(40.0))
        .h(px(22.0))
        .rounded(px(11.0))
        .bg(if enabled { rgb(t.border_active) } else { rgb(t.bg_secondary) })
        .flex()
        .items_center()
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(t.text_primary))
                .ml(if enabled { px(20.0) } else { px(2.0) }),
        )
}

/// Render a hook input row with label, description, and text input
fn hook_input_row(
    id: impl Into<SharedString>,
    label: &str,
    desc: &str,
    input: &Entity<SimpleInputState>,
    placeholder: &str,
    t: &ThemeColors,
    has_border: bool,
) -> Stateful<Div> {
    let _ = placeholder; // placeholder is set on the entity itself
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(t.text_primary))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_muted))
                        .child(desc.to_string()),
                ),
        )
        .child(
            div()
                .bg(rgb(t.bg_secondary))
                .border_1()
                .border_color(rgb(t.border))
                .rounded(px(4.0))
                .child(SimpleInput::new(input).text_size(px(12.0))),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

// ============================================================================
// Settings Panel
// ============================================================================

/// Settings panel overlay for configuring app settings
pub struct SettingsPanel {
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    active_category: SettingsCategory,
    /// None = "User" (global settings), Some(id) = per-project
    selected_project_id: Option<String>,
    project_dropdown_open: bool,
    font_dropdown_open: bool,
    shell_dropdown_open: bool,
    session_backend_dropdown_open: bool,
    available_shells: Vec<AvailableShell>,
    // Global hook inputs
    hook_project_open: Entity<SimpleInputState>,
    hook_project_close: Entity<SimpleInputState>,
    hook_worktree_create: Entity<SimpleInputState>,
    hook_worktree_close: Entity<SimpleInputState>,
    // Per-project hook inputs
    project_hook_project_open: Entity<SimpleInputState>,
    project_hook_project_close: Entity<SimpleInputState>,
    project_hook_worktree_create: Entity<SimpleInputState>,
    project_hook_worktree_close: Entity<SimpleInputState>,
    // File opener input
    file_opener_input: Entity<SimpleInputState>,
}

impl SettingsPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::new_with_options(workspace, None, None, cx)
    }

    pub fn new_for_project(
        workspace: Entity<Workspace>,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_options(workspace, Some(project_id), Some(SettingsCategory::Hooks), cx)
    }

    fn new_with_options(
        workspace: Entity<Workspace>,
        project_id: Option<String>,
        category: Option<SettingsCategory>,
        cx: &mut Context<Self>,
    ) -> Self {
        let s = settings_entity(cx).read(cx).settings.clone();

        // Create global hook inputs
        let hook_project_open = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. echo \"opened $TERM_MANAGER_PROJECT_NAME\"");
            match s.hooks.on_project_open { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_project_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. echo \"closed $TERM_MANAGER_PROJECT_NAME\"");
            match s.hooks.on_project_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. npm install");
            match s.hooks.on_worktree_create { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. cleanup script");
            match s.hooks.on_worktree_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });

        // Subscribe to global hook input changes
        cx.subscribe(&hook_project_open, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_project_open(val, cx));
        }).detach();
        cx.subscribe(&hook_project_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_project_close(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_create, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_worktree_create(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_worktree_close(val, cx));
        }).detach();

        // Create per-project hook inputs (initialized for selected project)
        let project_hooks = project_id.as_ref().and_then(|pid| {
            workspace.read(cx).project(pid).map(|p| p.hooks.clone())
        });
        let global_hooks = &s.hooks;

        let project_hook_project_open = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_project_open.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_project_open.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_project_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_project_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_project_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_worktree_create.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_worktree_create.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_worktree_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_worktree_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });

        // Subscribe to per-project hook input changes
        let ws = workspace.clone();
        cx.subscribe(&project_hook_project_open, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        if let Some(p) = ws.project_mut(&pid) { p.hooks.on_project_open = val; cx.notify(); }
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_project_close, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        if let Some(p) = ws.project_mut(&pid) { p.hooks.on_project_close = val; cx.notify(); }
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_worktree_create, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        if let Some(p) = ws.project_mut(&pid) { p.hooks.on_worktree_create = val; cx.notify(); }
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_worktree_close, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        if let Some(p) = ws.project_mut(&pid) { p.hooks.on_worktree_close = val; cx.notify(); }
                    });
                }
            }
        }).detach();

        // File opener input
        let file_opener_input = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. code, cursor, zed, vim");
            if !s.file_opener.is_empty() { state.default_value(s.file_opener.clone()) } else { state }
        });
        cx.subscribe(&file_opener_input, |_this, entity, _: &InputChangedEvent, cx| {
            let val = entity.read(cx).value().to_string();
            settings_entity(cx).update(cx, |state, cx| state.set_file_opener(val, cx));
        }).detach();

        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            active_category: category.unwrap_or(SettingsCategory::General),
            selected_project_id: project_id,
            project_dropdown_open: false,
            font_dropdown_open: false,
            shell_dropdown_open: false,
            session_backend_dropdown_open: false,
            available_shells: available_shells(),
            hook_project_open,
            hook_project_close,
            hook_worktree_create,
            hook_worktree_close,
            project_hook_project_open,
            project_hook_project_close,
            project_hook_worktree_create,
            project_hook_worktree_close,
            file_opener_input,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SettingsPanelEvent::Close);
    }

    fn close_all_dropdowns(&mut self) {
        self.font_dropdown_open = false;
        self.shell_dropdown_open = false;
        self.session_backend_dropdown_open = false;
        self.project_dropdown_open = false;
    }

    fn has_open_dropdown(&self) -> bool {
        self.font_dropdown_open || self.shell_dropdown_open || self.session_backend_dropdown_open || self.project_dropdown_open
    }

    /// Switch to a different project (or "User" if None)
    fn select_project(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        self.selected_project_id = project_id.clone();
        self.project_dropdown_open = false;

        // When switching to project mode, ensure Hooks is selected
        if project_id.is_some() {
            let available = SettingsCategory::project_categories();
            if !available.contains(&self.active_category) {
                self.active_category = SettingsCategory::Hooks;
            }
        }

        // Reload project hook inputs for the new project
        self.reload_project_hook_inputs(cx);
        cx.notify();
    }

    /// Reload per-project hook inputs with values from the selected project
    fn reload_project_hook_inputs(&mut self, cx: &mut Context<Self>) {
        let global_hooks = settings_entity(cx).read(cx).settings.hooks.clone();
        let project_hooks = self.selected_project_id.as_ref().and_then(|pid| {
            self.workspace.read(cx).project(pid).map(|p| p.hooks.clone())
        });

        // Update placeholders and values
        self.project_hook_project_open.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_project_open.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_project_open.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_project_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_project_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_project_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_create.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_worktree_create.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_worktree_create.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_worktree_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_worktree_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
    }

    // ========================================================================
    // Header
    // ========================================================================

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                // Left: Project selector
                self.render_project_selector(cx),
            )
            .child(
                // Right: Edit in settings.json button
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("edit-settings-file-btn")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_secondary))
                            .child("Edit in settings.json")
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                open_settings_file();
                                this.close(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("settings-close-btn")
                            .cursor_pointer()
                            .w(px(24.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(14.0))
                            .text_color(rgb(t.text_muted))
                            .child("\u{2715}")
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.close(cx);
                            })),
                    ),
            )
    }

    fn render_project_selector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let label = match &self.selected_project_id {
            None => "User".to_string(),
            Some(pid) => {
                self.workspace.read(cx).project(pid)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Unknown".to_string())
            }
        };

        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_muted))
                    .child("\u{2014}"),
            )
            .child(
                dropdown_button("project-selector-btn", &label, self.project_dropdown_open, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.project_dropdown_open = !this.project_dropdown_open;
                        this.font_dropdown_open = false;
                        this.shell_dropdown_open = false;
                        this.session_backend_dropdown_open = false;
                        cx.notify();
                    })),
            )
    }

    fn render_project_dropdown_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let projects: Vec<(String, String)> = self.workspace.read(cx).projects()
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();

        let is_user_selected = self.selected_project_id.is_none();

        dropdown_overlay("project-selector-dropdown", 44.0, 32.0, &t)
            .left(px(16.0))
            .right_auto()
            .min_w(px(180.0))
            .max_h(px(250.0))
            .child(
                dropdown_option("project-opt-user", "User (Global)", is_user_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.select_project(None, cx);
                    }))
            )
            .children(projects.into_iter().map(|(id, name)| {
                let is_selected = self.selected_project_id.as_deref() == Some(&id);
                dropdown_option(format!("project-opt-{}", id), &name, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            this.select_project(Some(id.clone()), cx);
                        }
                    }))
            }))
    }

    // ========================================================================
    // Sidebar
    // ========================================================================

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let categories = if self.selected_project_id.is_some() {
            SettingsCategory::project_categories()
        } else {
            SettingsCategory::all()
        };

        div()
            .id("settings-sidebar")
            .w(px(120.0))
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(t.border))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(categories.iter().map(|cat| {
                let is_active = *cat == self.active_category;
                let category = *cat;

                div()
                    .id(ElementId::Name(format!("sidebar-{}", cat.label()).into()))
                    .cursor_pointer()
                    .mx(px(6.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .when(is_active, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_primary))
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!is_active, |d| {
                        d.text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .child(cat.label().to_string())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.active_category = category;
                        this.close_all_dropdowns();
                        cx.notify();
                    }))
            }))
    }

    // ========================================================================
    // Content area - dispatches to category renderers
    // ========================================================================

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-content")
            .flex_1()
            .overflow_y_scroll()
            .min_w_0()
            .child(match self.active_category {
                SettingsCategory::General => self.render_general(cx).into_any_element(),
                SettingsCategory::Font => self.render_font(cx).into_any_element(),
                SettingsCategory::Terminal => self.render_terminal(cx).into_any_element(),
                SettingsCategory::Hooks => self.render_hooks(cx).into_any_element(),
            })
    }

    // ========================================================================
    // General category
    // ========================================================================

    fn render_general(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div()
            .child(section_header("Appearance", &t))
            .child(
                section_container(&t)
                    .child(self.render_toggle(
                        "focus-border", "Show Focus Border", s.show_focused_border, true,
                        |state, val, cx| state.set_show_focused_border(val, cx), cx,
                    ))
                    .child(self.render_toggle(
                        "remote-server", "Remote Server", s.remote_server_enabled, true,
                        |state, val, cx| state.set_remote_server_enabled(val, cx), cx,
                    ))
                    .child(self.render_toggle(
                        "auto-update", "Auto Update", s.auto_update_enabled, false,
                        |state, val, cx| state.set_auto_update_enabled(val, cx), cx,
                    )),
            )
            .child(section_header("File Opener", &t))
            .child(
                section_container(&t)
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(rgb(t.text_primary))
                                            .child("Editor Command"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Command to open file paths (empty = system default)"),
                                    ),
                            )
                            .child(
                                div()
                                    .bg(rgb(t.bg_secondary))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .rounded(px(4.0))
                                    .child(SimpleInput::new(&self.file_opener_input).text_size(px(12.0))),
                            ),
                    ),
            )
    }

    // ========================================================================
    // Font category
    // ========================================================================

    fn render_font(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div()
            .child(section_header("Font", &t))
            .child(
                section_container(&t)
                    .child(self.render_number_stepper(
                        "font-size", "Font Size", s.font_size, "{}", 1.0, 50.0, true,
                        |state, val, cx| state.set_font_size(val, cx), cx,
                    ))
                    .child(self.render_font_dropdown_row(&s.font_family, cx))
                    .child(self.render_number_stepper(
                        "line-height", "Line Height", s.line_height, "{}", 0.1, 50.0, true,
                        |state, val, cx| state.set_line_height(val, cx), cx,
                    ))
                    .child(self.render_number_stepper(
                        "ui-font-size", "UI Font Size", s.ui_font_size, "{}", 1.0, 50.0, true,
                        |state, val, cx| state.set_ui_font_size(val, cx), cx,
                    ))
                    .child(self.render_number_stepper(
                        "file-font-size", "File Font Size", s.file_font_size, "{}", 1.0, 50.0, false,
                        |state, val, cx| state.set_file_font_size(val, cx), cx,
                    )),
            )
    }

    // ========================================================================
    // Terminal category
    // ========================================================================

    fn render_terminal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div()
            .child(section_header("Terminal", &t))
            .child(
                section_container(&t)
                    .child(self.render_shell_dropdown_row(&s.default_shell, cx))
                    .child(self.render_session_backend_dropdown_row(&s.session_backend, cx))
                    .child(self.render_toggle(
                        "show-shell-selector", "Show Shell Selector", s.show_shell_selector, true,
                        |state, val, cx| state.set_show_shell_selector(val, cx), cx,
                    ))
                    .child(self.render_toggle(
                        "cursor-blink", "Cursor Blink", s.cursor_blink, true,
                        |state, val, cx| state.set_cursor_blink(val, cx), cx,
                    ))
                    .child(self.render_integer_stepper(
                        "scrollback", "Scrollback Lines", s.scrollback_lines, 1000, 70.0, false,
                        |state, val, cx| state.set_scrollback_lines(val, cx), cx,
                    )),
            )
    }

    // ========================================================================
    // Hooks category
    // ========================================================================

    fn render_hooks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_project = self.selected_project_id.is_some();

        let (h1, h2, h3, h4) = if is_project {
            (
                self.project_hook_project_open.clone(),
                self.project_hook_project_close.clone(),
                self.project_hook_worktree_create.clone(),
                self.project_hook_worktree_close.clone(),
            )
        } else {
            (
                self.hook_project_open.clone(),
                self.hook_project_close.clone(),
                self.hook_worktree_create.clone(),
                self.hook_worktree_close.clone(),
            )
        };

        let scope_label = if is_project { "Project Hooks (override global)" } else { "Global Hooks" };
        let env_note = "Available env: $TERM_MANAGER_PROJECT_ID, $TERM_MANAGER_PROJECT_NAME, $TERM_MANAGER_PROJECT_PATH";

        div()
            .child(section_header(scope_label, &t))
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(8.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(env_note),
            )
            .child(
                section_container(&t)
                    .child(hook_input_row(
                        "hook-project-open", "On Project Open",
                        "Command to run when a project is opened",
                        &h1, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-project-close", "On Project Close",
                        "Command to run when a project is closed",
                        &h2, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-create", "On Worktree Create",
                        "Command to run after a git worktree is created",
                        &h3, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-close", "On Worktree Close",
                        "Command to run after a git worktree is removed",
                        &h4, "", &t, false,
                    )),
            )
    }

    // ========================================================================
    // Shared stepper/toggle/dropdown helpers
    // ========================================================================

    fn render_number_stepper(
        &self,
        id: &str,
        label: &str,
        value: f32,
        format: &str,
        step: f32,
        width: f32,
        has_border: bool,
        update_fn: impl Fn(&mut SettingsState, f32, &mut Context<SettingsState>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let dec_fn = update_fn.clone();
        let inc_fn = update_fn;

        settings_row(id.to_string(), label, &t, has_border).child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    stepper_button(format!("{}-dec", id), "-", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let dec_fn = dec_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                dec_fn(state, value - step, cx);
                            });
                        })),
                )
                .child(value_display(format.replace("{}", &format!("{:.1}", value)), width, &t))
                .child(
                    stepper_button(format!("{}-inc", id), "+", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let inc_fn = inc_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                inc_fn(state, value + step, cx);
                            });
                        })),
                ),
        )
    }

    fn render_integer_stepper(
        &self,
        id: &str,
        label: &str,
        value: u32,
        step: u32,
        width: f32,
        has_border: bool,
        update_fn: impl Fn(&mut SettingsState, u32, &mut Context<SettingsState>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let dec_fn = update_fn.clone();
        let inc_fn = update_fn;

        settings_row(id.to_string(), label, &t, has_border).child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    stepper_button(format!("{}-dec", id), "-", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let dec_fn = dec_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                dec_fn(state, value.saturating_sub(step), cx);
                            });
                        })),
                )
                .child(value_display(format!("{}", value), width, &t))
                .child(
                    stepper_button(format!("{}-inc", id), "+", &t)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            let inc_fn = inc_fn.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                inc_fn(state, value + step, cx);
                            });
                        })),
                ),
        )
    }

    fn render_toggle(
        &self,
        id: &str,
        label: &str,
        enabled: bool,
        has_border: bool,
        update_fn: impl Fn(&mut SettingsState, bool, &mut Context<SettingsState>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        settings_row(id.to_string(), label, &t, has_border).child(
            toggle_switch(format!("{}-toggle", id), enabled, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                    let update_fn = update_fn.clone();
                    settings_entity(cx).update(cx, |state, cx| {
                        update_fn(state, !enabled, cx);
                    });
                })),
        )
    }

    fn render_font_dropdown_row(&mut self, current_family: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        settings_row("font-family".to_string(), "Font Family", &t, true).child(
            dropdown_button("font-family-btn", current_family, self.font_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.font_dropdown_open = !this.font_dropdown_open;
                    this.shell_dropdown_open = false;
                    this.session_backend_dropdown_open = false;
                    this.project_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    fn render_font_dropdown_overlay(&self, current: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        dropdown_overlay("font-family-dropdown-list", 140.0, 32.0, &t)
            .children(FONT_FAMILIES.iter().map(|family| {
                let is_selected = *family == current;
                let family_str = family.to_string();

                dropdown_option(format!("font-opt-{}", family), family, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let family = family_str.clone();
                        move |this, _, _, cx| {
                            let family = family.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_font_family(family, cx);
                            });
                            this.font_dropdown_open = false;
                            cx.notify();
                        }
                    }))
            }))
    }

    fn render_shell_dropdown_row(&mut self, current_shell: &ShellType, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let display_name = current_shell.display_name();

        settings_row("default-shell".to_string(), "Default Shell", &t, true).child(
            dropdown_button("default-shell-btn", &display_name, self.shell_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.shell_dropdown_open = !this.shell_dropdown_open;
                    this.font_dropdown_open = false;
                    this.session_backend_dropdown_open = false;
                    this.project_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    fn render_shell_dropdown_overlay(&self, current_shell: &ShellType, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let available: Vec<_> = self.available_shells.iter()
            .filter(|s| s.available)
            .collect();

        dropdown_overlay("shell-dropdown-list", 290.0, 32.0, &t)
            .min_w(px(180.0))
            .max_h(px(250.0))
            .children(available.into_iter().map(|shell_info| {
                let is_selected = &shell_info.shell_type == current_shell;
                let shell_type = shell_info.shell_type.clone();
                let name = shell_info.name.clone();

                dropdown_option(format!("shell-opt-{}", name), &name, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let shell_type = shell_type.clone();
                        move |this, _, _, cx| {
                            let shell = shell_type.clone();
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_default_shell(shell, cx);
                            });
                            this.shell_dropdown_open = false;
                            cx.notify();
                        }
                    }))
            }))
    }

    fn render_session_backend_dropdown_row(&mut self, current_backend: &SessionBackend, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let display_name = current_backend.display_name();

        settings_row_with_desc("session-backend".to_string(), "Session Backend", "Requires restart", &t, true).child(
            dropdown_button("session-backend-btn", display_name, self.session_backend_dropdown_open, &t)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.session_backend_dropdown_open = !this.session_backend_dropdown_open;
                    this.font_dropdown_open = false;
                    this.shell_dropdown_open = false;
                    this.project_dropdown_open = false;
                    cx.notify();
                })),
        )
    }

    fn render_session_backend_dropdown_overlay(&self, current_backend: &SessionBackend, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        dropdown_overlay("session-backend-dropdown-list", 290.0, 70.0, &t)
            .min_w(px(180.0))
            .children(SessionBackend::all_variants().iter().map(|backend| {
                let is_selected = backend == current_backend;
                let backend_copy = *backend;
                let name = backend.display_name();

                dropdown_option(format!("backend-opt-{:?}", backend), name, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        settings_entity(cx).update(cx, |state, cx| {
                            state.set_session_backend(backend_copy, cx);
                        });
                        this.session_backend_dropdown_open = false;
                        cx.notify();
                    }))
            }))
    }

    // ========================================================================
    // Footer
    // ========================================================================

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let config_path = get_settings_path();

        div()
            .px(px(16.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_family("monospace")
                    .text_color(rgb(t.text_muted))
                    .child(format!("Config: {}", config_path.display())),
            )
    }
}

/// Convert empty string to None, non-empty to Some
fn opt_string(s: &str) -> Option<String> {
    if s.is_empty() { None } else { Some(s.to_string()) }
}

pub enum SettingsPanelEvent {
    Close,
}

impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        window.focus(&focus_handle, cx);

        modal_backdrop("settings-panel-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("SettingsPanel")
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    if this.has_open_dropdown() {
                        this.close_all_dropdowns();
                        cx.notify();
                    } else {
                        this.close(cx);
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.has_open_dropdown() {
                    this.close_all_dropdowns();
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .child(
                modal_content("settings-panel-modal", &t)
                    .relative()
                    .w(px(620.0))
                    .max_h(px(560.0))
                    // Header with project selector and edit button
                    .child(self.render_header(cx))
                    // Main body: sidebar + content
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_sidebar(cx))
                            .child(self.render_content(cx)),
                    )
                    // Footer
                    .child(self.render_footer(cx))
                    // Dropdown overlays (rendered last to be on top)
                    .when(self.project_dropdown_open, |modal| {
                        modal.child(self.render_project_dropdown_overlay(cx))
                    })
                    .when(self.font_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.font_family.clone();
                        modal.child(self.render_font_dropdown_overlay(&current, cx))
                    })
                    .when(self.shell_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.default_shell.clone();
                        modal.child(self.render_shell_dropdown_overlay(&current, cx))
                    })
                    .when(self.session_backend_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.session_backend;
                        modal.child(self.render_session_backend_dropdown_overlay(&current, cx))
                    }),
            )
    }
}

impl_focusable!(SettingsPanel);
