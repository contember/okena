//! Git diff viewer overlay.
//!
//! Provides a read-only view of git diffs with working/staged toggle,
//! file tree sidebar, syntax highlighting, and selection support.

mod context_menu;
mod data;
mod line_render;
mod nav;
pub mod provider;
mod render;
mod scrollbar;
mod selection_ops;
mod side_by_side;
mod syntax;
mod types;

use okena_git::{CommitLogEntry, DiffMode, FileDiff};
use okena_core::selection::SelectionState;
use okena_core::types::DiffViewMode;
use okena_files::syntax::load_syntax_set;
use gpui::prelude::*;
use gpui::*;
use std::collections::HashSet;
use std::sync::Arc;
use syntect::parsing::SyntaxSet;

use types::{DiffDisplayFile, FileStats, FileTreeNode, HScrollbarDrag, ScrollbarDrag, SideBySideLine, SideBySideSide};

// Re-export for use in settings (and use locally)
pub use types::DiffViewMode as DiffViewModeReexport;

gpui::actions!(okena_git, [Cancel]);

/// Type alias for diff selection (line index, column).
type Selection = SelectionState<(usize, usize)>;

/// Width of file tree sidebar.
const SIDEBAR_WIDTH: f32 = 240.0;

use crate::settings::git_settings;

/// Git diff viewer overlay.
pub struct DiffViewer {
    pub(super) focus_handle: FocusHandle,
    pub(super) diff_mode: DiffMode,
    pub(super) view_mode: DiffViewMode,
    /// Ignore whitespace changes in diff.
    pub(super) ignore_whitespace: bool,
    /// Provider for fetching diff data (local or remote).
    pub(super) provider: Arc<dyn provider::GitProvider>,
    /// Whether diff data is currently being loaded.
    pub(super) loading: bool,
    /// Raw diff data for all files (not syntax highlighted).
    pub(super) raw_files: Vec<FileDiff>,
    /// Lightweight file stats for sidebar display.
    pub(super) file_stats: Vec<FileStats>,
    /// Currently processed file with syntax highlighting (lazy loaded).
    pub(super) current_file: Option<DiffDisplayFile>,
    pub(super) file_tree: FileTreeNode,
    pub(super) expanded_folders: HashSet<String>,
    pub(super) selected_file_index: usize,
    pub(super) selection: Selection,
    pub(super) scroll_handle: UniformListScrollHandle,
    pub(super) tree_scroll_handle: ScrollHandle,
    pub(super) error_message: Option<String>,
    pub(super) line_num_width: usize,
    pub(super) syntax_set: SyntaxSet,
    pub(super) scrollbar_drag: Option<ScrollbarDrag>,
    pub(super) file_font_size: f32,
    /// Cached side-by-side lines for current file.
    pub(super) side_by_side_lines: Vec<SideBySideLine>,
    /// Horizontal scroll offset in pixels.
    pub(super) scroll_x: f32,
    /// Maximum line length in characters (for horizontal scroll range).
    pub(super) max_line_chars: usize,
    /// Cached diff pane viewport width (updated from scroll handle geometry).
    pub(super) diff_pane_width: f32,
    /// Horizontal scrollbar drag state.
    pub(super) h_scrollbar_drag: Option<HScrollbarDrag>,
    /// Which side of the side-by-side view the current selection belongs to.
    pub(super) selection_side: Option<SideBySideSide>,
    /// Measured monospace character width (from font metrics).
    pub(super) measured_char_width: f32,
    /// Whether the current theme is dark (for syntax highlighting).
    pub(super) is_dark: bool,
    /// Cached old file content for re-highlighting on theme change.
    pub(super) current_file_old_content: Option<String>,
    /// Cached new file content for re-highlighting on theme change.
    pub(super) current_file_new_content: Option<String>,
    /// Commit message for display when viewing a commit diff.
    pub(super) commit_message: Option<String>,
    /// List of commits for prev/next navigation.
    pub(super) commits: Vec<CommitLogEntry>,
    /// Current index in the commits list.
    pub(super) commit_index: usize,
    /// Open file-tree right-click context menu (file or folder).
    pub(super) context_menu: Option<context_menu::DiffContextMenu>,
    /// Open "Delete file" confirmation modal.
    pub(super) delete_confirm: Option<context_menu::DeleteConfirmState>,
    /// Open "Discard changes" confirmation modal.
    pub(super) discard_confirm: Option<context_menu::DiscardConfirmState>,
    /// True when this viewer is hosted inside a detached window.
    /// Hides the "detach" button and is set by the detached host.
    pub(super) is_detached: bool,
    /// Open commit-hash right-click context menu.
    pub(super) commit_hash_menu: Option<context_menu::CommitHashContextMenu>,
    /// Right-click context menu over a non-empty text selection.
    pub(super) selection_context_menu: Option<Point<Pixels>>,
}

impl DiffViewer {
    /// Create a new diff viewer with the given provider, optionally selecting a specific file, mode, commit message, and commit navigation list.
    pub fn new(
        provider: Arc<dyn provider::GitProvider>,
        select_file: Option<String>,
        mode: Option<DiffMode>,
        commit_message: Option<String>,
        commits: Option<Vec<CommitLogEntry>>,
        commit_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let gs = git_settings(cx);
        let font_size = gs.file_font_size;
        let view_mode = gs.diff_view_mode;
        let ignore_whitespace = gs.diff_ignore_whitespace;
        let is_dark = gs.is_dark;

        let mut viewer = Self {
            focus_handle,
            diff_mode: DiffMode::WorkingTree,
            view_mode,
            ignore_whitespace,
            provider: provider.clone(),
            loading: false,
            raw_files: Vec::new(),
            file_stats: Vec::new(),
            current_file: None,
            file_tree: FileTreeNode::default(),
            expanded_folders: HashSet::new(),
            selected_file_index: 0,
            selection: Selection::default(),
            scroll_handle: UniformListScrollHandle::new(),
            tree_scroll_handle: ScrollHandle::new(),
            error_message: None,
            line_num_width: 4,
            syntax_set: load_syntax_set(),
            scrollbar_drag: None,
            file_font_size: font_size,
            side_by_side_lines: Vec::new(),
            scroll_x: 0.0,
            max_line_chars: 0,
            diff_pane_width: 0.0,
            h_scrollbar_drag: None,
            selection_side: None,
            measured_char_width: font_size * 0.6,
            is_dark,
            current_file_old_content: None,
            current_file_new_content: None,
            commit_message,
            commits: commits.unwrap_or_default(),
            commit_index: commit_index.unwrap_or(0),
            context_menu: None,
            delete_confirm: None,
            discard_confirm: None,
            is_detached: false,
            commit_hash_menu: None,
            selection_context_menu: None,
        };

        if !provider.is_git_repo() {
            viewer.error_message = Some("Not a git repository".to_string());
            return viewer;
        }

        viewer.load_diff_async(mode.unwrap_or(DiffMode::WorkingTree), select_file, cx);
        viewer
    }

    /// Current diff view mode (for persisting on close).
    pub fn view_mode(&self) -> DiffViewMode { self.view_mode }

    /// Current ignore-whitespace setting (for persisting on close).
    pub fn ignore_whitespace(&self) -> bool { self.ignore_whitespace }

    /// Update configuration (font size, theme) from outside.
    pub fn update_config(&mut self, font_size: f32, is_dark: bool) {
        self.file_font_size = font_size;
        if is_dark != self.is_dark {
            self.is_dark = is_dark;
            self.rehighlight_current_file();
            self.update_side_by_side_cache();
        }
    }
}

/// Events emitted by the diff viewer.
#[derive(Clone, Debug)]
pub enum DiffViewerEvent {
    Close,
    /// User requested to detach the viewer into a separate OS window.
    Detach,
    /// User clicked "Send to terminal" on a selection. Carries the structured
    /// payload; the host formats it (relative to terminal CWD) before pasting.
    SendToTerminal(okena_core::send_payload::SendPayload),
}

impl EventEmitter<DiffViewerEvent> for DiffViewer {}

impl okena_ui::overlay::CloseEvent for DiffViewerEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}
