//! Overlay management utilities and OverlayManager Entity.
//!
//! Provides traits, helpers, and a centralized manager for modal overlay components
//! with consistent toggle and close behavior.

use gpui::*;

use std::path::PathBuf;

use crate::terminal::shell_config::ShellType;
use crate::views::overlays::command_palette::{CommandPalette, CommandPaletteEvent};
use crate::views::overlays::keybindings_help::{KeybindingsHelp, KeybindingsHelpEvent};
use crate::views::overlays::add_project_dialog::{AddProjectDialog, AddProjectDialogEvent};
use crate::views::overlays::context_menu::{ContextMenu, ContextMenuEvent};
use crate::views::overlays::folder_context_menu::{FolderContextMenu, FolderContextMenuEvent};
use crate::views::overlays::file_search::{FileSearchDialog, FileSearchDialogEvent};
use crate::views::overlays::diff_viewer::{DiffViewer, DiffViewerEvent};
use crate::views::overlays::file_viewer::{FileViewer, FileViewerEvent};
use crate::views::overlays::{ProjectSwitcher, ProjectSwitcherEvent, ShellSelectorOverlay, ShellSelectorOverlayEvent};
use crate::views::overlays::session_manager::{SessionManager, SessionManagerEvent};
use crate::views::overlays::settings_panel::{SettingsPanel, SettingsPanelEvent};
use crate::views::overlays::theme_selector::{ThemeSelector, ThemeSelectorEvent};
use crate::views::overlays::worktree_dialog::{WorktreeDialog, WorktreeDialogEvent};
use crate::workspace::requests::{ContextMenuRequest, FolderContextMenuRequest, SidebarRequest};
use crate::workspace::state::{Workspace, WorkspaceData};

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

/// Helper macro for toggling modal overlays via the single active_modal slot.
///
/// Usage:
/// ```ignore
/// toggle_overlay!(self, cx, KeybindingsHelp, KeybindingsHelpEvent, |cx| KeybindingsHelp::new(cx));
/// ```
#[macro_export]
macro_rules! toggle_overlay {
    ($self:ident, $cx:ident, $type:ty, $event_type:ty, $factory:expr) => {
        if $self.is_modal::<$type>() {
            $self.close_modal($cx);
        } else {
            let entity = $cx.new($factory);
            $cx.subscribe(&entity, |this, _, event: &$event_type, cx| {
                if event.is_close() {
                    this.close_modal(cx);
                }
            })
            .detach();
            $self.open_modal(entity, $cx);
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
/// Uses a single `active_modal` slot to enforce mutual exclusion -
/// only one modal can be open at a time. Context menus remain as
/// separate slots since they are positioned popups, not full-screen modals.
pub struct OverlayManager {
    workspace: Entity<Workspace>,

    /// The single active modal overlay (only one can be open at a time).
    active_modal: Option<AnyView>,

    /// TypeId of the active modal for toggle detection.
    modal_type_id: Option<std::any::TypeId>,

    // Context menus remain separate (positioned popups, not full-screen modals)
    context_menu: OverlaySlot<ContextMenu>,
    folder_context_menu: OverlaySlot<FolderContextMenu>,
}

impl OverlayManager {
    /// Create a new OverlayManager.
    pub fn new(workspace: Entity<Workspace>) -> Self {
        Self {
            workspace,
            active_modal: None,
            modal_type_id: None,
            context_menu: OverlaySlot::new(),
            folder_context_menu: OverlaySlot::new(),
        }
    }

    // ========================================================================
    // Modal management helpers
    // ========================================================================

    /// Close the active modal, restoring terminal focus if needed.
    fn close_modal(&mut self, cx: &mut Context<Self>) {
        if self.active_modal.is_some() {
            self.active_modal = None;
            self.modal_type_id = None;
            self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
            cx.notify();
        }
    }

    /// Check if the active modal is of a specific type.
    fn is_modal<T: 'static>(&self) -> bool {
        self.modal_type_id == Some(std::any::TypeId::of::<T>())
    }

    /// Open a modal, closing any existing one first.
    ///
    /// Automatically clears terminal focus so keyboard input goes to the modal.
    fn open_modal<T: Render + 'static>(&mut self, entity: Entity<T>, cx: &mut Context<Self>) {
        self.close_modal(cx);
        self.active_modal = Some(entity.into());
        self.modal_type_id = Some(std::any::TypeId::of::<T>());
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    /// Get the active modal for rendering.
    pub fn render_modal(&self) -> Option<AnyView> {
        self.active_modal.clone()
    }

    // ========================================================================
    // Context menu visibility checks (kept separate)
    // ========================================================================

    /// Check if context menu is open.
    pub fn has_context_menu(&self) -> bool {
        self.context_menu.is_open()
    }

    /// Check if folder context menu is open.
    pub fn has_folder_context_menu(&self) -> bool {
        self.folder_context_menu.is_open()
    }

    // ========================================================================
    // Simple toggle overlays
    // ========================================================================

    /// Toggle add project dialog overlay.
    pub fn toggle_add_project_dialog(&mut self, cx: &mut Context<Self>) {
        if self.is_modal::<AddProjectDialog>() {
            self.close_modal(cx);
        } else {
            let workspace = self.workspace.clone();
            let entity = cx.new(|cx| AddProjectDialog::new(workspace, cx));
            cx.subscribe(&entity, |this, _, event: &AddProjectDialogEvent, cx| {
                if event.is_close() {
                    this.close_modal(cx);
                }
            }).detach();
            self.open_modal(entity, cx);
        }
        cx.notify();
    }

    /// Toggle keybindings help overlay.
    pub fn toggle_keybindings_help(&mut self, cx: &mut Context<Self>) {
        toggle_overlay!(self, cx, KeybindingsHelp, KeybindingsHelpEvent, |cx| KeybindingsHelp::new(cx));
    }

    /// Toggle theme selector overlay.
    pub fn toggle_theme_selector(&mut self, cx: &mut Context<Self>) {
        toggle_overlay!(self, cx, ThemeSelector, ThemeSelectorEvent, |cx| ThemeSelector::new(cx));
    }

    /// Toggle command palette overlay.
    pub fn toggle_command_palette(&mut self, cx: &mut Context<Self>) {
        toggle_overlay!(self, cx, CommandPalette, CommandPaletteEvent, |cx| CommandPalette::new(cx));
    }

    /// Toggle settings panel overlay.
    pub fn toggle_settings_panel(&mut self, cx: &mut Context<Self>) {
        if self.is_modal::<SettingsPanel>() {
            self.close_modal(cx);
        } else {
            let workspace = self.workspace.clone();
            let entity = cx.new(|cx| SettingsPanel::new(workspace, cx));
            cx.subscribe(&entity, |this, _, event: &SettingsPanelEvent, cx| {
                if event.is_close() {
                    this.close_modal(cx);
                }
            }).detach();
            self.open_modal(entity, cx);
        }
        cx.notify();
    }

    /// Show settings panel opened to Hooks category for a specific project.
    pub fn show_settings_for_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let entity = cx.new(|cx| SettingsPanel::new_for_project(workspace, project_id, cx));
        cx.subscribe(&entity, |this, _, event: &SettingsPanelEvent, cx| {
            if event.is_close() {
                this.close_modal(cx);
            }
        }).detach();
        self.open_modal(entity, cx);
        cx.notify();
    }

    /// Toggle project switcher overlay.
    pub fn toggle_project_switcher(&mut self, cx: &mut Context<Self>) {
        if self.is_modal::<ProjectSwitcher>() {
            self.close_modal(cx);
        } else {
            let workspace = self.workspace.clone();
            let entity = cx.new(|cx| ProjectSwitcher::new(workspace, cx));
            cx.subscribe(&entity, |this, _, event: &ProjectSwitcherEvent, cx| {
                match event {
                    ProjectSwitcherEvent::Close => {
                        this.close_modal(cx);
                    }
                    ProjectSwitcherEvent::FocusProject(project_id) => {
                        cx.emit(OverlayManagerEvent::FocusProject(project_id.clone()));
                        this.close_modal(cx);
                    }
                    ProjectSwitcherEvent::ToggleVisibility(project_id) => {
                        cx.emit(OverlayManagerEvent::ToggleProjectVisibility(project_id.clone()));
                        cx.notify();
                    }
                }
            })
            .detach();
            self.open_modal(entity, cx);
        }
        cx.notify();
    }

    // ========================================================================
    // Session manager (complex - emits SwitchWorkspace event)
    // ========================================================================

    /// Toggle session manager overlay.
    pub fn toggle_session_manager(&mut self, cx: &mut Context<Self>) {
        if self.is_modal::<SessionManager>() {
            self.close_modal(cx);
        } else {
            let workspace = self.workspace.clone();
            let manager = cx.new(|cx| SessionManager::new(workspace, cx));
            cx.subscribe(&manager, |this, _, event: &SessionManagerEvent, cx| {
                match event {
                    SessionManagerEvent::Close => {
                        this.close_modal(cx);
                    }
                    SessionManagerEvent::SwitchWorkspace(data) => {
                        cx.emit(OverlayManagerEvent::SwitchWorkspace(data.clone()));
                        this.close_modal(cx);
                    }
                }
            })
            .detach();
            self.open_modal(manager, cx);
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
                    this.close_modal(cx);
                }
                ShellSelectorOverlayEvent::ShellSelected { shell_type, context } => {
                    if let Some((project_id, terminal_id)) = context {
                        cx.emit(OverlayManagerEvent::ShellSelected {
                            shell_type: shell_type.clone(),
                            project_id: project_id.clone(),
                            terminal_id: terminal_id.clone(),
                        });
                    }
                    this.close_modal(cx);
                }
            }
        }).detach();
        self.open_modal(entity, cx);
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
                    this.close_modal(cx);
                }
                WorktreeDialogEvent::Created(new_project_id) => {
                    cx.emit(OverlayManagerEvent::WorktreeCreated(new_project_id.clone()));
                    this.close_modal(cx);
                }
            }
        })
        .detach();
        self.open_modal(dialog, cx);
        cx.notify();
    }

    // ========================================================================
    // Context menu (parametric - remains as separate OverlaySlot)
    // ========================================================================

    /// Show context menu for a project.
    pub fn show_context_menu(&mut self, request: ContextMenuRequest, cx: &mut Context<Self>) {
        self.close_modal(cx);
        self.folder_context_menu.close();

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

        self.context_menu.set(menu);
        cx.notify();
    }

    /// Hide context menu.
    pub fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu.close();
        cx.notify();
    }

    /// Show folder context menu.
    pub fn show_folder_context_menu(&mut self, request: FolderContextMenuRequest, cx: &mut Context<Self>) {
        self.close_modal(cx);
        self.context_menu.close();

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

        self.folder_context_menu.set(menu);
        cx.notify();
    }

    /// Hide folder context menu.
    pub fn hide_folder_context_menu(&mut self, cx: &mut Context<Self>) {
        self.folder_context_menu.close();
        cx.notify();
    }

    // ========================================================================
    // File search (parametric)
    // ========================================================================

    /// Toggle file search dialog for a project.
    pub fn toggle_file_search(&mut self, project_path: PathBuf, cx: &mut Context<Self>) {
        if self.is_modal::<FileSearchDialog>() {
            self.close_modal(cx);
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
                    this.close_modal(cx);
                }
                FileSearchDialogEvent::FileSelected(path) => {
                    let path = path.clone();
                    this.close_modal(cx);
                    // Open the file viewer
                    this.show_file_viewer(path, cx);
                }
            }
        })
        .detach();

        self.open_modal(dialog, cx);
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
                    this.close_modal(cx);
                }
            }
        })
        .detach();

        self.open_modal(viewer, cx);
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
                    this.close_modal(cx);
                }
            }
        })
        .detach();

        self.open_modal(viewer, cx);
        cx.notify();
    }

    // ========================================================================
    // Render helpers (context menus only - modal uses render_modal())
    // ========================================================================

    /// Get context menu entity for rendering.
    pub fn render_context_menu(&self) -> Option<Entity<ContextMenu>> {
        self.context_menu.render()
    }

    /// Get folder context menu entity for rendering.
    pub fn render_folder_context_menu(&self) -> Option<Entity<FolderContextMenu>> {
        self.folder_context_menu.render()
    }
}

impl EventEmitter<OverlayManagerEvent> for OverlayManager {}
