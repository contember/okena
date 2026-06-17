//! uniffi `Record` / `Enum` types mirroring the data the FFI returns.
//!
//! uniffi derive macros are kept off the internal `crate::api` data structs
//! (which `TerminalHolder` and the state accessors produce), so we define the
//! uniffi-facing mirrors here and convert from the internal ones via `From`.
//! The shapes match 1:1 so the conversions are mechanical.

use std::collections::HashMap;

use crate::api::{
    connection::ConnectionStatus as NativeConnectionStatus,
    state::{
        FolderInfo as NativeFolderInfo, FullscreenInfo as NativeFullscreenInfo,
        ProjectInfo as NativeProjectInfo, ServiceInfo as NativeServiceInfo,
    },
    terminal::{
        CellData as NativeCellData, CursorShape as NativeCursorShape,
        CursorState as NativeCursorState, ScrollInfo as NativeScrollInfo,
        SelectionBounds as NativeSelectionBounds,
    },
};

/// Connection status surfaced to the RN layer.
///
/// Mirrors `crate::api::connection::ConnectionStatus` (which itself collapses
/// core's `Reconnecting { attempt }` into `Connecting`).
#[derive(Debug, Clone, uniffi::Enum)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Pairing,
    Error { message: String },
}

impl From<NativeConnectionStatus> for ConnectionStatus {
    fn from(s: NativeConnectionStatus) -> Self {
        match s {
            NativeConnectionStatus::Disconnected => ConnectionStatus::Disconnected,
            NativeConnectionStatus::Connecting => ConnectionStatus::Connecting,
            NativeConnectionStatus::Connected => ConnectionStatus::Connected,
            NativeConnectionStatus::Pairing => ConnectionStatus::Pairing,
            NativeConnectionStatus::Error { message } => ConnectionStatus::Error { message },
        }
    }
}

/// A single terminal grid cell (flat, FFI-friendly).
#[derive(Debug, Clone, uniffi::Record)]
pub struct CellData {
    /// The character in this cell (empty string for wide-char spacers).
    pub character: String,
    /// Foreground color as ARGB packed u32.
    pub fg: u32,
    /// Background color as ARGB packed u32.
    pub bg: u32,
    /// Flags: bold(1) | italic(2) | underline(4) | strikethrough(8) | inverse(16) | dim(32).
    pub flags: u8,
}

impl From<NativeCellData> for CellData {
    fn from(c: NativeCellData) -> Self {
        CellData {
            character: c.character,
            fg: c.fg,
            bg: c.bg,
            flags: c.flags,
        }
    }
}

/// Cursor shape variants.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
}

impl From<NativeCursorShape> for CursorShape {
    fn from(s: NativeCursorShape) -> Self {
        match s {
            NativeCursorShape::Block => CursorShape::Block,
            NativeCursorShape::Underline => CursorShape::Underline,
            NativeCursorShape::Beam => CursorShape::Beam,
        }
    }
}

/// Cursor state for rendering.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CursorState {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

impl From<NativeCursorState> for CursorState {
    fn from(c: NativeCursorState) -> Self {
        CursorState {
            col: c.col,
            row: c.row,
            shape: c.shape.into(),
            visible: c.visible,
        }
    }
}

/// Scroll info: total/visible line counts and the current display offset.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScrollInfo {
    pub total_lines: u32,
    pub visible_lines: u32,
    pub display_offset: u32,
}

impl From<NativeScrollInfo> for ScrollInfo {
    fn from(s: NativeScrollInfo) -> Self {
        ScrollInfo {
            total_lines: s.total_lines,
            visible_lines: s.visible_lines,
            display_offset: s.display_offset,
        }
    }
}

/// Selection bounds (rows are buffer-relative, adjusted for display offset).
#[derive(Debug, Clone, uniffi::Record)]
pub struct SelectionBounds {
    pub start_col: u16,
    pub start_row: i32,
    pub end_col: u16,
    pub end_row: i32,
}

impl From<NativeSelectionBounds> for SelectionBounds {
    fn from(s: NativeSelectionBounds) -> Self {
        SelectionBounds {
            start_col: s.start_col,
            start_row: s.start_row,
            end_col: s.end_col,
            end_row: s.end_row,
        }
    }
}

/// Service entry inside a project.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ServiceInfo {
    pub name: String,
    pub status: String,
    pub terminal_id: Option<String>,
    pub ports: Vec<u16>,
    pub exit_code: Option<u32>,
    pub kind: String,
    pub is_extra: bool,
}

impl From<NativeServiceInfo> for ServiceInfo {
    fn from(s: NativeServiceInfo) -> Self {
        ServiceInfo {
            name: s.name,
            status: s.status,
            terminal_id: s.terminal_id,
            ports: s.ports,
            exit_code: s.exit_code,
            kind: s.kind,
            is_extra: s.is_extra,
        }
    }
}

/// Flat, FFI-friendly project info.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub show_in_overview: bool,
    pub terminal_ids: Vec<String>,
    pub terminal_names: HashMap<String, String>,
    pub git_branch: Option<String>,
    pub git_lines_added: u32,
    pub git_lines_removed: u32,
    pub services: Vec<ServiceInfo>,
    pub folder_color: String,
}

impl From<NativeProjectInfo> for ProjectInfo {
    fn from(p: NativeProjectInfo) -> Self {
        ProjectInfo {
            id: p.id,
            name: p.name,
            path: p.path,
            show_in_overview: p.show_in_overview,
            terminal_ids: p.terminal_ids,
            terminal_names: p.terminal_names,
            git_branch: p.git_branch,
            git_lines_added: p.git_lines_added,
            git_lines_removed: p.git_lines_removed,
            services: p.services.into_iter().map(Into::into).collect(),
            folder_color: p.folder_color,
        }
    }
}

/// Folder grouping projects.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FolderInfo {
    pub id: String,
    pub name: String,
    pub project_ids: Vec<String>,
    pub folder_color: String,
}

impl From<NativeFolderInfo> for FolderInfo {
    fn from(f: NativeFolderInfo) -> Self {
        FolderInfo {
            id: f.id,
            name: f.name,
            project_ids: f.project_ids,
            folder_color: f.folder_color,
        }
    }
}

/// The currently fullscreened terminal, if any.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FullscreenInfo {
    pub project_id: String,
    pub terminal_id: String,
}

impl From<NativeFullscreenInfo> for FullscreenInfo {
    fn from(f: NativeFullscreenInfo) -> Self {
        FullscreenInfo {
            project_id: f.project_id,
            terminal_id: f.terminal_id,
        }
    }
}
