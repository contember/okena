//! Overlay management utilities and OverlayManager Entity.
//!
//! Provides traits, helpers, and a centralized manager for modal overlay components
//! with consistent toggle and close behavior.

use gpui::*;

use std::path::PathBuf;

use crate::terminal::shell_config::ShellType;
use crate::views::command_palette::{CommandPalette, CommandPaletteEvent};
use crate::views::keybindings_help::{KeybindingsHelp, KeybindingsHelpEvent};
use crate::views::overlays::add_project_dialog::{AddProjectDialog, AddProjectDialogEvent};
use crate::views::overlays::context_menu::{ContextMenu, ContextMenuEvent};
use crate::views::overlays::folder_context_menu::{FolderContextMenu, FolderContextMenuEvent};
use crate::views::overlays::file_search::{FileSearchDialog, FileSearchDialogEvent};
use crate::views::overlays::diff_viewer::{DiffViewer, DiffViewerEvent};
use crate::views::overlays::file_viewer::{FileViewer, FileViewerEvent};
use crate::views::overlays::{ProjectSwitcher, ProjectSwitcherEvent, ShellSelectorOverlay, ShellSelectorOverlayEvent};
use crate::views::session_manager::{SessionManager, SessionManagerEvent};
use crate::views::settings_panel::{SettingsPanel, SettingsPanelEvent};
use crate::views::theme_selector::{ThemeSelector, ThemeSelectorEvent};
use crate::views::worktree_dialog::{WorktreeDialog, WorktreeDialogEvent};
use crate::workspace::state::{ContextMenuRequest, FolderContextMenuRequest, Workspace, WorkspaceData, SidebarRequest};

/// Trait for overlay events that support closing.
///
/// Implement this for your overlay's event enum to enable
/// automatic close handling.
pub trait CloseEvent {
    /// Returns true if this event represents a close action.
    fn is_close(&self) -> bool;
}

/// A slot that manages a single overlay entity with toggle behavior.
///
/// Provides:
/// - Toggle semantics (open if closed, close if open)
/// - Clean entity lifecycle management
pub struct OverlaySlot<T: 'static> {
    entity: Option<Entity<T>>,
}

impl<T: 'static> Default for OverlaySlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> OverlaySlot<T> {
    /// Create a new empty overlay slot.
    pub const fn new() -> Self {
        Self { entity: None }
    }

    /// Check if the overlay is currently open.
    pub fn is_open(&self) -> bool {
        self.entity.is_some()
    }

    /// Close the overlay.
    pub fn close(&mut self) {
        self.entity = None;
    }

    /// Set the entity directly.
    pub fn set(&mut self, entity: Entity<T>) {
        self.entity = Some(entity);
    }
}

impl<T: 'static + Render> OverlaySlot<T> {
    /// Render the overlay as an optional child element.
    ///
    /// Returns the entity clone if open, None otherwise.
    /// Use with `.when()` and `.child()` in your render method.
    pub fn render(&self) -> Option<Entity<T>> {
        self.entity.clone()
    }
}

/// Helper macro for toggling simple overlays.
///
/// Usage:
/// ```ignore
/// toggle_overlay!(self, cx, keybindings_help, KeybindingsHelpEvent, || KeybindingsHelp::new(cx));
/// ```
#[macro_export]
macro_rules! toggle_overlay {
    ($self:ident, $cx:ident, $field:ident, $event_type:ty, $factory:expr) => {
        if $self.$field.is_open() {
            $self.$field.close();
        } else {
            let entity = $cx.new($factory);
            $cx.subscribe(&entity, |this, _, event: &$event_type, cx| {
                if event.is_close() {
                    this.$field.close();
                    cx.notify();
                }
            })
            .detach();
            $self.$field.set(entity);
        }
        $cx.notify();
    };
}

// Implement CloseEvent for existing overlay events

impl CloseEvent for AddProjectDialogEvent {
    fn is_close(&self) -> bool {
        matches!(self, AddProjectDialogEvent::Close)
    }
}

impl CloseEvent for KeybindingsHelpEvent {
    fn is_close(&self) -> bool {
        matches!(self, KeybindingsHelpEvent::Close)
    }
}

impl CloseEvent for ThemeSelectorEvent {
    fn is_close(&self) -> bool {
        matches!(self, ThemeSelectorEvent::Close)
    }
}

impl CloseEvent for CommandPaletteEvent {
    fn is_close(&self) -> bool {
        matches!(self, CommandPaletteEvent::Close)
    }
}

impl CloseEvent for SettingsPanelEvent {
    fn is_close(&self) -> bool {
        matches!(self, SettingsPanelEvent::Close)
    }
}

impl CloseEvent for ShellSelectorOverlayEvent {
    fn is_close(&self) -> bool {
        matches!(self, ShellSelectorOverlayEvent::Close)
    }
}

impl CloseEvent for FileSearchDialogEvent {
    fn is_close(&self) -> bool {
        matches!(self, FileSearchDialogEvent::Close)
    }
}

impl CloseEvent for FileViewerEvent {
    fn is_close(&self) -> bool {
        matches!(self, FileViewerEvent::Close)
    }
}

impl CloseEvent for DiffViewerEvent {
    fn is_close(&self) -> bool {
        matches!(self, DiffViewerEvent::Close)
    }
}

// ============================================================================
// OverlayManager Entity
// ============================================================================

/// Events emitted by OverlayManager that require handling by RootView.
///
/// These events are forwarded from individual overlays when they require
/// actions that need access to RootView's state (terminals, PTY manager, etc.)
#[derive(Clone)]
pub enum OverlayManagerEvent {
    /// Session manager requested workspace switch
    SwitchWorkspace(WorkspaceData),

    /// Worktree dialog created a new project
    WorktreeCreated(String),

    /// Shell selector selected a shell for a terminal
    ShellSelected {
        shell_type: ShellType,
        project_id: String,
        terminal_id: String,
    },

    /// Context menu: Add terminal to project
    AddTerminal { project_id: String },

    /// Context menu: Create worktree from project
    CreateWorktree { project_id: String, project_path: String },

    /// Context menu: Rename project
    RenameProject { project_id: String, project_name: String },

    /// Context menu: Close worktree project
    CloseWorktree { project_id: String },

    /// Context menu: Delete project
    DeleteProject { project_id: String },

    /// Context menu: Configure hooks for a project
    ConfigureHooks { project_id: String },

    /// Project switcher: Focus a specific project
    FocusProject(String),

    /// Project switcher: Toggle project visibility
    ToggleProjectVisibility(String),
}

/// Centralized overlay manager that handles all modal overlays.
///
/// This entity owns all overlay state and provides methods for showing/hiding
/// overlays. Complex events that require RootView interaction are forwarded
/// via OverlayManagerEvent.
pub struct OverlayManager {
    workspace: Entity<Workspace>,

    // Simple toggle overlays
    add_project_dialog: OverlaySlot<AddProjectDialog>,
    keybindings_help: OverlaySlot<KeybindingsHelp>,
    theme_selector: OverlaySlot<ThemeSelector>,
    command_palette: OverlaySlot<CommandPalette>,
    settings_panel: OverlaySlot<SettingsPanel>,
    project_switcher: OverlaySlot<ProjectSwitcher>,

    // Parametric overlays
    shell_selector: OverlaySlot<ShellSelectorOverlay>,
    worktree_dialog: Option<Entity<WorktreeDialog>>,
    context_menu: Option<Entity<ContextMenu>>,
    folder_context_menu: Option<Entity<FolderContextMenu>>,
    session_manager: Option<Entity<SessionManager>>,

    // File search and viewer
    file_search: Option<Entity<FileSearchDialog>>,
    file_viewer: Option<Entity<FileViewer>>,
    diff_viewer: Option<Entity<DiffViewer>>,
}

impl OverlayManager {
    /// Create a new OverlayManager.
    pub fn new(workspace: Entity<Workspace>) -> Self {
        Self {
            workspace,
            add_project_dialog: OverlaySlot::new(),
            keybindings_help: OverlaySlot::new(),
            theme_selector: OverlaySlot::new(),
            command_palette: OverlaySlot::new(),
            settings_panel: OverlaySlot::new(),
            project_switcher: OverlaySlot::new(),
            shell_selector: OverlaySlot::new(),
            worktree_dialog: None,
            context_menu: None,
            folder_context_menu: None,
            session_manager: None,
            file_search: None,
            file_viewer: None,
            diff_viewer: None,
        }
    }

    // ========================================================================
    // Visibility checks
    // ========================================================================

    /// Check if add project dialog is open.
    pub fn has_add_project_dialog(&self) -> bool {
        self.add_project_dialog.is_open()
    }

    /// Check if keybindings help is open.
    pub fn has_keybindings_help(&self) -> bool {
        self.keybindings_help.is_open()
    }

    /// Check if session manager is open.
    pub fn has_session_manager(&self) -> bool {
        self.session_manager.is_some()
    }

    /// Check if theme selector is open.
    pub fn has_theme_selector(&self) -> bool {
        self.theme_selector.is_open()
    }

    /// Check if command palette is open.
    pub fn has_command_palette(&self) -> bool {
        self.command_palette.is_open()
    }

    /// Check if settings panel is open.
    pub fn has_settings_panel(&self) -> bool {
        self.settings_panel.is_open()
    }

    /// Check if project switcher is open.
    pub fn has_project_switcher(&self) -> bool {
        self.project_switcher.is_open()
    }

    /// Check if shell selector is open.
    pub fn has_shell_selector(&self) -> bool {
        self.shell_selector.is_open()
    }

    /// Check if worktree dialog is open.
    pub fn has_worktree_dialog(&self) -> bool {
        self.worktree_dialog.is_some()
    }

    /// Check if context menu is open.
    pub fn has_context_menu(&self) -> bool {
        self.context_menu.is_some()
    }

    /// Check if folder context menu is open.
    pub fn has_folder_context_menu(&self) -> bool {
        self.folder_context_menu.is_some()
    }

    /// Check if file search is open.
    pub fn has_file_search(&self) -> bool {
        self.file_search.is_some()
    }

    /// Check if file viewer is open.
    pub fn has_file_viewer(&self) -> bool {
        self.file_viewer.is_some()
    }

    /// Check if diff viewer is open.
    pub fn has_diff_viewer(&self) -> bool {
        self.diff_viewer.is_some()
    }

    // ========================================================================
    // Simple toggle overlays
    // ========================================================================

    /// Toggle add project dialog overlay.
    pub fn toggle_add_project_dialog(&mut self, cx: &mut Context<Self>) {
        if self.add_project_dialog.is_open() {
            self.add_project_dialog.close();
            self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        } else {
            let workspace = self.workspace.clone();
            let entity = cx.new(|cx| AddProjectDialog::new(workspace, cx));
            cx.subscribe(&entity, |this, _, event: &AddProjectDialogEvent, cx| {
                if event.is_close() {
                    this.add_project_dialog.close();
                    this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                    cx.notify();
                }
            }).detach();
            self.add_project_dialog.set(entity);
            self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        }
        cx.notify();
    }

    /// Toggle keybindings help overlay.
    pub fn toggle_keybindings_help(&mut self, cx: &mut Context<Self>) {
        toggle_overlay!(self, cx, keybindings_help, KeybindingsHelpEvent, |cx| KeybindingsHelp::new(cx));
    }

    /// Toggle theme selector overlay.
    pub fn toggle_theme_selector(&mut self, cx: &mut Context<Self>) {
        toggle_overlay!(self, cx, theme_selector, ThemeSelectorEvent, |cx| ThemeSelector::new(cx));
    }

    /// Toggle command palette overlay.
    pub fn toggle_command_palette(&mut self, cx: &mut Context<Self>) {
        toggle_overlay!(self, cx, command_palette, CommandPaletteEvent, |cx| CommandPalette::new(cx));
    }

    /// Toggle settings panel overlay.
    pub fn toggle_settings_panel(&mut self, cx: &mut Context<Self>) {
        if self.settings_panel.is_open() {
            self.settings_panel.close();
        } else {
            let workspace = self.workspace.clone();
            let entity = cx.new(|cx| SettingsPanel::new(workspace, cx));
            cx.subscribe(&entity, |this, _, event: &SettingsPanelEvent, cx| {
                if event.is_close() {
                    this.settings_panel.close();
                    cx.notify();
                }
            }).detach();
            self.settings_panel.set(entity);
        }
        cx.notify();
    }

    /// Show settings panel opened to Hooks category for a specific project.
    pub fn show_settings_for_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        // Close existing settings panel if open
        self.settings_panel.close();

        let workspace = self.workspace.clone();
        let entity = cx.new(|cx| SettingsPanel::new_for_project(workspace, project_id, cx));
        cx.subscribe(&entity, |this, _, event: &SettingsPanelEvent, cx| {
            if event.is_close() {
                this.settings_panel.close();
                cx.notify();
            }
        }).detach();
        self.settings_panel.set(entity);
        cx.notify();
    }

    /// Toggle project switcher overlay.
    pub fn toggle_project_switcher(&mut self, cx: &mut Context<Self>) {
        if self.project_switcher.is_open() {
            self.project_switcher.close();
            // Restore focus after closing
            self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        } else {
            let workspace = self.workspace.clone();
            let entity = cx.new(|cx| ProjectSwitcher::new(workspace, cx));
            cx.subscribe(&entity, |this, _, event: &ProjectSwitcherEvent, cx| {
                match event {
                    ProjectSwitcherEvent::Close => {
                        this.project_switcher.close();
                        this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                        cx.notify();
                    }
                    ProjectSwitcherEvent::FocusProject(project_id) => {
                        this.project_switcher.close();
                        cx.emit(OverlayManagerEvent::FocusProject(project_id.clone()));
                        this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                        cx.notify();
                    }
                    ProjectSwitcherEvent::ToggleVisibility(project_id) => {
                        cx.emit(OverlayManagerEvent::ToggleProjectVisibility(project_id.clone()));
                        cx.notify();
                    }
                }
            })
            .detach();
            self.project_switcher.set(entity);
            // Clear focus during modal
            self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        }
        cx.notify();
    }

    // ========================================================================
    // Session manager (complex - emits SwitchWorkspace event)
    // ========================================================================

    /// Toggle session manager overlay.
    pub fn toggle_session_manager(&mut self, cx: &mut Context<Self>) {
        if self.session_manager.is_some() {
            self.session_manager = None;
        } else {
            let workspace = self.workspace.clone();
            let manager = cx.new(|cx| SessionManager::new(workspace, cx));
            cx.subscribe(&manager, |this, _, event: &SessionManagerEvent, cx| {
                match event {
                    SessionManagerEvent::Close => {
                        this.session_manager = None;
                        cx.notify();
                    }
                    SessionManagerEvent::SwitchWorkspace(data) => {
                        this.session_manager = None;
                        cx.emit(OverlayManagerEvent::SwitchWorkspace(data.clone()));
                        cx.notify();
                    }
                }
            })
            .detach();
            self.session_manager = Some(manager);
        }
        cx.notify();
    }

    // ========================================================================
    // Shell selector (parametric)
    // ========================================================================

    /// Show shell selector overlay for a terminal.
    pub fn show_shell_selector(
        &mut self,
        current_shell: ShellType,
        project_id: String,
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        let context = Some((project_id.clone(), terminal_id.clone()));
        let entity = cx.new(|cx| ShellSelectorOverlay::new(current_shell, context, cx));
        cx.subscribe(&entity, move |this, _, event: &ShellSelectorOverlayEvent, cx| {
            match event {
                ShellSelectorOverlayEvent::Close => {
                    this.shell_selector.close();
                    this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                    cx.notify();
                }
                ShellSelectorOverlayEvent::ShellSelected { shell_type, context } => {
                    this.shell_selector.close();
                    if let Some((project_id, terminal_id)) = context {
                        cx.emit(OverlayManagerEvent::ShellSelected {
                            shell_type: shell_type.clone(),
                            project_id: project_id.clone(),
                            terminal_id: terminal_id.clone(),
                        });
                    }
                    this.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
                    cx.notify();
                }
            }
        }).detach();
        self.shell_selector.set(entity);
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    // ========================================================================
    // Worktree dialog (parametric)
    // ========================================================================

    /// Show worktree dialog for a project.
    pub fn show_worktree_dialog(
        &mut self,
        project_id: String,
        project_path: String,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let dialog = cx.new(|cx| {
            WorktreeDialog::new(workspace, project_id, project_path, cx)
        });
        cx.subscribe(&dialog, |this, _, event: &WorktreeDialogEvent, cx| {
            match event {
                WorktreeDialogEvent::Close => {
                    this.hide_worktree_dialog(cx);
                }
                WorktreeDialogEvent::Created(new_project_id) => {
                    cx.emit(OverlayManagerEvent::WorktreeCreated(new_project_id.clone()));
                    this.hide_worktree_dialog(cx);
                }
            }
        })
        .detach();
        self.worktree_dialog = Some(dialog);
        // Clear focused terminal during modal
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    /// Close worktree dialog.
    pub fn hide_worktree_dialog(&mut self, cx: &mut Context<Self>) {
        self.worktree_dialog = None;
        // Restore focus after modal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    // ========================================================================
    // Context menu (parametric)
    // ========================================================================

    /// Show context menu for a project.
    pub fn show_context_menu(&mut self, request: ContextMenuRequest, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let menu = cx.new(|cx| ContextMenu::new(workspace.clone(), request, cx));

        cx.subscribe(&menu, |this, _, event: &ContextMenuEvent, cx| {
            match event {
                ContextMenuEvent::Close => {
                    this.hide_context_menu(cx);
                }
                ContextMenuEvent::AddTerminal { project_id } => {
                    this.hide_context_menu(cx);
                    cx.emit(OverlayManagerEvent::AddTerminal {
                        project_id: project_id.clone(),
                    });
                }
                ContextMenuEvent::CreateWorktree { project_id, project_path } => {
                    this.hide_context_menu(cx);
                    cx.emit(OverlayManagerEvent::CreateWorktree {
                        project_id: project_id.clone(),
                        project_path: project_path.clone(),
                    });
                }
                ContextMenuEvent::RenameProject { project_id, project_name } => {
                    this.hide_context_menu(cx);
                    cx.emit(OverlayManagerEvent::RenameProject {
                        project_id: project_id.clone(),
                        project_name: project_name.clone(),
                    });
                }
                ContextMenuEvent::CloseWorktree { project_id } => {
                    this.hide_context_menu(cx);
                    cx.emit(OverlayManagerEvent::CloseWorktree {
                        project_id: project_id.clone(),
                    });
                }
                ContextMenuEvent::DeleteProject { project_id } => {
                    this.hide_context_menu(cx);
                    cx.emit(OverlayManagerEvent::DeleteProject {
                        project_id: project_id.clone(),
                    });
                }
                ContextMenuEvent::ConfigureHooks { project_id } => {
                    this.hide_context_menu(cx);
                    cx.emit(OverlayManagerEvent::ConfigureHooks {
                        project_id: project_id.clone(),
                    });
                }
            }
        })
        .detach();

        self.context_menu = Some(menu);
        cx.notify();
    }

    /// Hide context menu.
    pub fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
    }

    /// Show folder context menu.
    pub fn show_folder_context_menu(&mut self, request: FolderContextMenuRequest, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let menu = cx.new(|cx| FolderContextMenu::new(workspace.clone(), request, cx));

        cx.subscribe(&menu, |this, _, event: &FolderContextMenuEvent, cx| {
            match event {
                FolderContextMenuEvent::Close => {
                    this.hide_folder_context_menu(cx);
                }
                FolderContextMenuEvent::RenameFolder { folder_id, folder_name } => {
                    this.hide_folder_context_menu(cx);
                    this.workspace.update(cx, |ws, cx| {
                        ws.push_sidebar_request(SidebarRequest::RenameFolder {
                            folder_id: folder_id.clone(),
                            folder_name: folder_name.clone(),
                        }, cx);
                    });
                }
                FolderContextMenuEvent::DeleteFolder { folder_id } => {
                    this.hide_folder_context_menu(cx);
                    this.workspace.update(cx, |ws, cx| {
                        ws.delete_folder(folder_id, cx);
                    });
                }
            }
        })
        .detach();

        self.folder_context_menu = Some(menu);
        cx.notify();
    }

    /// Hide folder context menu.
    pub fn hide_folder_context_menu(&mut self, cx: &mut Context<Self>) {
        self.folder_context_menu = None;
        cx.notify();
    }

    // ========================================================================
    // File search (parametric)
    // ========================================================================

    /// Toggle file search dialog for a project.
    pub fn toggle_file_search(&mut self, project_path: PathBuf, cx: &mut Context<Self>) {
        if self.file_search.is_some() {
            self.hide_file_search(cx);
        } else {
            self.show_file_search(project_path, cx);
        }
    }

    /// Show file search dialog for a project.
    pub fn show_file_search(&mut self, project_path: PathBuf, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| FileSearchDialog::new(project_path, cx));

        cx.subscribe(&dialog, |this, _, event: &FileSearchDialogEvent, cx| {
            match event {
                FileSearchDialogEvent::Close => {
                    this.hide_file_search(cx);
                }
                FileSearchDialogEvent::FileSelected(path) => {
                    let path = path.clone();
                    this.hide_file_search(cx);
                    // Open the file viewer
                    this.show_file_viewer(path, cx);
                }
            }
        })
        .detach();

        self.file_search = Some(dialog);
        // Clear focused terminal during modal
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    /// Hide file search dialog.
    pub fn hide_file_search(&mut self, cx: &mut Context<Self>) {
        self.file_search = None;
        // Restore focus after modal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    // ========================================================================
    // File viewer (parametric)
    // ========================================================================

    /// Show file viewer for a file.
    pub fn show_file_viewer(&mut self, file_path: PathBuf, cx: &mut Context<Self>) {
        let viewer = cx.new(|cx| FileViewer::new(file_path, cx));

        cx.subscribe(&viewer, |this, _, event: &FileViewerEvent, cx| {
            match event {
                FileViewerEvent::Close => {
                    this.hide_file_viewer(cx);
                }
            }
        })
        .detach();

        self.file_viewer = Some(viewer);
        // Clear focused terminal during modal
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    /// Hide file viewer.
    pub fn hide_file_viewer(&mut self, cx: &mut Context<Self>) {
        self.file_viewer = None;
        // Restore focus after modal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    // ========================================================================
    // Diff viewer (parametric)
    // ========================================================================

    /// Show diff viewer for a project, optionally selecting a specific file.
    pub fn show_diff_viewer(&mut self, project_path: String, select_file: Option<String>, cx: &mut Context<Self>) {
        let viewer = cx.new(|cx| DiffViewer::new(project_path, select_file, cx));

        cx.subscribe(&viewer, |this, _, event: &DiffViewerEvent, cx| {
            match event {
                DiffViewerEvent::Close => {
                    this.hide_diff_viewer(cx);
                }
            }
        })
        .detach();

        self.diff_viewer = Some(viewer);
        // Clear focused terminal during modal
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    /// Hide diff viewer.
    pub fn hide_diff_viewer(&mut self, cx: &mut Context<Self>) {
        self.diff_viewer = None;
        // Restore focus after modal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    // ========================================================================
    // Render helpers
    // ========================================================================

    /// Get add project dialog entity for rendering.
    pub fn render_add_project_dialog(&self) -> Option<Entity<AddProjectDialog>> {
        self.add_project_dialog.render()
    }

    /// Get keybindings help entity for rendering.
    pub fn render_keybindings_help(&self) -> Option<Entity<KeybindingsHelp>> {
        self.keybindings_help.render()
    }

    /// Get session manager entity for rendering.
    pub fn render_session_manager(&self) -> Option<Entity<SessionManager>> {
        self.session_manager.clone()
    }

    /// Get theme selector entity for rendering.
    pub fn render_theme_selector(&self) -> Option<Entity<ThemeSelector>> {
        self.theme_selector.render()
    }

    /// Get command palette entity for rendering.
    pub fn render_command_palette(&self) -> Option<Entity<CommandPalette>> {
        self.command_palette.render()
    }

    /// Get settings panel entity for rendering.
    pub fn render_settings_panel(&self) -> Option<Entity<SettingsPanel>> {
        self.settings_panel.render()
    }

    /// Get project switcher entity for rendering.
    pub fn render_project_switcher(&self) -> Option<Entity<ProjectSwitcher>> {
        self.project_switcher.render()
    }

    /// Get shell selector entity for rendering.
    pub fn render_shell_selector(&self) -> Option<Entity<ShellSelectorOverlay>> {
        self.shell_selector.render()
    }

    /// Get worktree dialog entity for rendering.
    pub fn render_worktree_dialog(&self) -> Option<Entity<WorktreeDialog>> {
        self.worktree_dialog.clone()
    }

    /// Get context menu entity for rendering.
    pub fn render_context_menu(&self) -> Option<Entity<ContextMenu>> {
        self.context_menu.clone()
    }

    /// Get folder context menu entity for rendering.
    pub fn render_folder_context_menu(&self) -> Option<Entity<FolderContextMenu>> {
        self.folder_context_menu.clone()
    }

    /// Get file search dialog entity for rendering.
    pub fn render_file_search(&self) -> Option<Entity<FileSearchDialog>> {
        self.file_search.clone()
    }

    /// Get file viewer entity for rendering.
    pub fn render_file_viewer(&self) -> Option<Entity<FileViewer>> {
        self.file_viewer.clone()
    }

    /// Get diff viewer entity for rendering.
    pub fn render_diff_viewer(&self) -> Option<Entity<DiffViewer>> {
        self.diff_viewer.clone()
    }
}

impl EventEmitter<OverlayManagerEvent> for OverlayManager {}
