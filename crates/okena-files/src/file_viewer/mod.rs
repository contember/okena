//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

mod blame_load;
mod blame_render;
mod context_menu;
mod loading;
mod render;
mod search;
mod selection;

use crate::blame::{BlameError, BlameLine, BlameProvider};
use crate::code_view::ScrollbarDrag;
use crate::list_directory::DirEntry;
use crate::selection::SelectionState;
use crate::syntax::{load_syntax_set, HighlightedLine};
use context_menu::{DeleteConfirmState, FileRenameState, FileTreeContextMenu, TabContextMenu};
use gpui::*;
use okena_markdown::{MarkdownDocument, MarkdownSelection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use syntect::parsing::SyntaxSet;

/// Maximum file size to load for text/markdown (5MB)
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum file size to load for image previews (20MB — image decoders
/// handle larger files comfortably than the text/syntect path).
const MAX_IMAGE_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// Maximum number of lines to display
const MAX_LINES: usize = 10000;

/// Maximum number of open tabs
const MAX_TABS: usize = 50;

/// Maximum navigation history stack size
const MAX_HISTORY: usize = 50;

/// Display mode for file viewer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum DisplayMode {
    #[default]
    Source,
    Preview,
}

/// Background fill behind an image / SVG preview. Single-colour SVGs (a
/// black icon) become invisible on a matching pane background, so the user
/// can flip between a checkerboard (default — shows both extremes) and
/// explicit Light / Dark fills.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewBackground {
    #[default]
    Checker,
    Light,
    Dark,
}

/// Per-tab zoom / pan / background state for the image preview.
///
/// `auto_fit` is the default: image renders with `ObjectFit::Contain` to
/// fill the pane, `zoom` and `pan` are ignored. Once the user wheel-zooms,
/// drags, or clicks 100%, `auto_fit` flips off and the view renders at
/// natural-size × `zoom` with `pan` applied.
#[derive(Clone)]
pub struct ImageViewState {
    /// Fit-to-pane mode (ObjectFit::Contain). Reset via Ctrl+0 or the Fit
    /// header button.
    pub auto_fit: bool,
    /// Scale factor: 1.0 = 100% natural pixel size. Ignored when `auto_fit`.
    pub zoom: f32,
    /// Pan offset (image translates by this). Ignored when `auto_fit`.
    pub pan: gpui::Point<gpui::Pixels>,
    /// True while a pan drag is in progress.
    pub is_panning: bool,
    /// Mouse position at drag start, plus the pan offset captured then, so
    /// we can compute pan = offset + (mouse - anchor) without drift.
    pub pan_anchor: Option<gpui::Point<gpui::Pixels>>,
    pub pan_anchor_offset: gpui::Point<gpui::Pixels>,
    pub background: PreviewBackground,
    /// True while a background SVG re-rasterization is in flight. Set on
    /// `maybe_rerender_svg` dispatch and cleared on apply, so concurrent
    /// zoom changes coalesce into a single follow-up raster instead of
    /// spamming the background executor.
    pub svg_rerender_in_flight: bool,
}

impl Default for ImageViewState {
    fn default() -> Self {
        Self {
            auto_fit: true,
            zoom: 1.0,
            pan: gpui::Point::default(),
            is_panning: false,
            pan_anchor: None,
            pan_anchor_offset: gpui::Point::default(),
            background: PreviewBackground::Checker,
            svg_rerender_in_flight: false,
        }
    }
}

impl ImageViewState {
    /// Clamp the zoom factor to a sane range. Below 0.1× the image
    /// disappears; above 10× the rendered surface dwarfs any reasonable
    /// display and pan becomes unusable.
    pub const MIN_ZOOM: f32 = 0.1;
    pub const MAX_ZOOM: f32 = 10.0;

    /// Set zoom and leave auto-fit mode. Pan stays as-is.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        self.auto_fit = false;
    }

    /// Multiply current zoom by `factor` and leave auto-fit mode.
    pub fn zoom_by(&mut self, factor: f32) {
        let current = if self.auto_fit { 1.0 } else { self.zoom };
        self.set_zoom(current * factor);
    }

    /// Reset to fit-to-pane.
    pub fn reset_to_fit(&mut self) {
        self.auto_fit = true;
        self.zoom = 1.0;
        self.pan = gpui::Point::default();
        self.is_panning = false;
        self.pan_anchor = None;
        self.pan_anchor_offset = gpui::Point::default();
        // Keep svg_rerender_in_flight as-is; the in-flight task will
        // resolve and we'll just discard its result via the apply check.
    }
}

/// Type alias for source view selection (line, column).
type Selection = SelectionState<(usize, usize)>;

/// Width of file tree sidebar.
const SIDEBAR_WIDTH: f32 = 240.0;

/// Per-file state for a single tab in the file viewer.
///
/// `relative_path` is the canonical identifier (project-relative, used for
/// `fs.read_file`, tab equality, history, and tree highlighting). `file_path`
/// is the derived absolute path used for filesystem-level context-menu
/// operations (rename/delete) — it's only meaningful for local projects.
pub(super) struct FileViewerTab {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub content: String,
    pub highlighted_lines: Vec<HighlightedLine>,
    pub line_count: usize,
    pub line_num_width: usize,
    pub error_message: Option<String>,
    pub selection: Selection,
    pub display_mode: DisplayMode,
    pub is_markdown: bool,
    pub markdown_doc: Option<MarkdownDocument>,
    pub markdown_selection: MarkdownSelection,
    /// Virtualized list state for the markdown preview. One list item per
    /// top-level block, so only visible blocks are built per frame. Lazily
    /// (re)created in render when the node count or font size changes.
    pub markdown_list_state: Option<ListState>,
    /// Node count `markdown_list_state` was built for (to detect doc reloads).
    pub markdown_list_nodes: usize,
    /// Font size the list heights were measured at (to trigger a remeasure).
    pub markdown_list_font: f32,
    pub source_scroll_handle: UniformListScrollHandle,
    pub scrollbar_drag: Option<ScrollbarDrag>,
    /// Last known modification time of the file (for detecting external changes).
    pub modified_at: Option<SystemTime>,
    /// Whether the tab content is still being loaded asynchronously.
    pub loading: bool,
    /// Per-line git blame for this file. Lazy-loaded when the user toggles
    /// the blame gutter on.
    pub blame: BlameLoadState,
    /// Monotonic counter bumped each time `spawn_tab_load` schedules a fresh
    /// async load for this tab. The bg task captures the generation it was
    /// scheduled at and `apply_loaded_content` is skipped if a newer load
    /// has been queued in the meantime, so a slow earlier load can't
    /// clobber a faster later one with stale content.
    pub load_generation: u64,
    /// True for files previewed as images (png/jpg/gif/webp/svg/...). When
    /// set, `image_data` carries the bytes and the source/markdown rendering
    /// paths are bypassed.
    pub is_image: bool,
    /// True for SVG files specifically. SVG is the one image format that
    /// also has a meaningful source view — the loader keeps the raw XML in
    /// `content`/`highlighted_lines` so the user can flip between Preview
    /// (rendered) and Source (highlighted XML) via the same toggle markdown
    /// uses.
    pub is_svg: bool,
    /// Decoded image data wrapped for GPUI's `img()` element. Populated by
    /// the async loader for image tabs.
    pub image_data: Option<DecodedImage>,
    /// Pan / zoom / background state for image and SVG-Preview tabs.
    /// Persists across freshness reloads so the user keeps their view as
    /// the file changes on disk.
    pub image_view: ImageViewState,
    /// True for font files (otf/ttf/woff/woff2). When set, `font_data`
    /// carries parsed metadata and the source/image rendering paths are
    /// bypassed in favour of the font-preview branch.
    pub is_font: bool,
    /// Parsed font metadata + family name. The TTF bytes themselves are
    /// registered with GPUI's text system on apply (kept inside the system,
    /// not on the tab) so the preview pane can render sample text in the
    /// font.
    pub font_data: Option<Arc<FontData>>,
}

/// Decoded image payload. Raster formats let GPUI's asset cache handle the
/// RGBA→BGRA swap; SVGs are pre-rasterized to BGRA ourselves because GPUI's
/// built-in `ImageDecoder` calls `render_single_frame(.., to_bgra=false)` for
/// SVG, leaving R/B swapped and the preview inverted. Each variant carries
/// the intrinsic pixel dimensions so the zoom UI can compute pan bounds
/// and a "100%" / "fit" mode without re-decoding.
#[derive(Clone)]
pub enum DecodedImage {
    Raster {
        image: Arc<Image>,
        width: u32,
        height: u32,
    },
    Rendered {
        image: Arc<RenderImage>,
        width: u32,
        height: u32,
        /// Raw SVG bytes kept for re-rasterization at higher resolutions
        /// when the user zooms in. Without this, `image` (a fixed bitmap)
        /// would visibly pixelate past the rasterized scale.
        svg_bytes: Arc<Vec<u8>>,
        /// The `scale_factor` argument that was passed to
        /// `SvgRenderer::render_single_frame` when `image` was produced.
        /// GPUI multiplies this by `SMOOTH_SVG_SCALE_FACTOR (= 2)`
        /// internally, so the actual pixmap is `intrinsic × scale_factor × 2`.
        /// Used to decide when a re-raster is needed (zoom > rendered_scale).
        rendered_scale: f32,
    },
}

impl DecodedImage {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            DecodedImage::Raster { width, height, .. } => (*width, *height),
            DecodedImage::Rendered { width, height, .. } => (*width, *height),
        }
    }
}

impl From<DecodedImage> for ImageSource {
    fn from(value: DecodedImage) -> Self {
        match value {
            DecodedImage::Raster { image, .. } => ImageSource::Image(image),
            DecodedImage::Rendered { image, .. } => ImageSource::Render(image),
        }
    }
}

/// Lifecycle of a tab's blame data.
#[derive(Clone, Debug, Default)]
pub enum BlameLoadState {
    #[default]
    NotLoaded,
    Loading,
    Loaded(std::sync::Arc<Vec<BlameLine>>),
    Error(BlameError),
}

/// Map an extension to a `gpui::ImageFormat` for files we can preview as
/// images. Returns `None` for non-image extensions.
pub(super) fn image_format_for_path(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        "svg" => ImageFormat::Svg,
        _ => return None,
    })
}

/// Font container format we know how to load.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FontFormat {
    /// OpenType / TrueType — fed straight to ttf-parser and registered as-is.
    OpenType,
    /// WOFF2 — Brotli-compressed OpenType, decompressed back to TTF before
    /// parsing and font-system registration.
    Woff2,
    /// WOFF (v1) — zlib-compressed per-table OpenType. We surface the file
    /// but reject decoding for now (rare in practice; adding decoder is a
    /// follow-up).
    Woff,
}

/// Detect whether `path`'s extension is one of the font formats we preview.
pub(super) fn font_format_for_path(path: &Path) -> Option<FontFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "ttf" | "otf" => FontFormat::OpenType,
        "woff2" => FontFormat::Woff2,
        "woff" => FontFormat::Woff,
        _ => return None,
    })
}

/// Parsed metadata for a font preview, plus the OpenType bytes that GPUI's
/// text system needs to actually render text in the font. Wrapped in `Arc`
/// on the tab so freshness reloads can swap it cheaply.
#[derive(Clone)]
pub struct FontData {
    /// Family name from the OpenType `name` table (e.g. "JetBrains Mono"),
    /// used as the `Font::family` for sample-text rendering.
    pub family_name: String,
    /// Full font name from the `name` table (e.g. "JetBrains Mono Bold Italic").
    pub full_name: String,
    /// Subfamily / style description ("Regular", "Bold", "Italic", ...).
    pub style: String,
    /// Font version string from the `name` table.
    pub version: String,
    /// Number of glyphs in the font.
    pub num_glyphs: u16,
    /// EM square units (typically 1000 for OTF, 2048 for TTF).
    pub units_per_em: u16,
    /// OS/2 `usWeightClass` — 100 (Thin) through 900 (Black).
    pub weight_class: u16,
    /// Whether the font advertises italic style.
    pub is_italic: bool,
}

impl FileViewerTab {
    /// Create a new tab for browsing (no file loaded).
    pub(super) fn new_empty() -> Self {
        Self {
            file_path: PathBuf::new(),
            relative_path: String::new(),
            content: String::new(),
            highlighted_lines: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            error_message: None,
            selection: Selection::default(),
            display_mode: DisplayMode::Source,
            is_markdown: false,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_list_state: None,
            markdown_list_nodes: 0,
            markdown_list_font: 0.0,
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            modified_at: None,
            loading: false,
            blame: BlameLoadState::NotLoaded,
            load_generation: 0,
            is_image: false,
            is_svg: false,
            image_data: None,
            image_view: ImageViewState::default(),
            is_font: false,
            font_data: None,
        }
    }

    /// Create a tab in loading state (content will be filled asynchronously).
    fn new_loading(relative_path: String, file_path: PathBuf) -> Self {
        let image_format = image_format_for_path(&file_path);
        let is_image = image_format.is_some();
        let is_svg = image_format == Some(ImageFormat::Svg);
        let is_font = !is_image && font_format_for_path(&file_path).is_some();
        let is_markdown = !is_image && !is_font && Self::is_markdown_file(&file_path);
        Self {
            file_path,
            relative_path,
            content: String::new(),
            highlighted_lines: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            error_message: None,
            selection: Selection::default(),
            display_mode: if is_markdown || is_svg {
                DisplayMode::Preview
            } else {
                DisplayMode::Source
            },
            is_markdown,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_list_state: None,
            markdown_list_nodes: 0,
            markdown_list_font: 0.0,
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            modified_at: None,
            loading: true,
            blame: BlameLoadState::NotLoaded,
            load_generation: 0,
            is_image,
            is_svg,
            image_data: None,
            image_view: ImageViewState::default(),
            is_font,
            font_data: None,
        }
    }

    /// Get the filename for display in the tab bar.
    pub fn filename(&self) -> String {
        if let Some(name) = self.file_path.file_name() {
            name.to_string_lossy().to_string()
        } else if let Some(idx) = self.relative_path.rfind(['/', '\\']) {
            self.relative_path[idx + 1..].to_string()
        } else if !self.relative_path.is_empty() {
            self.relative_path.clone()
        } else {
            "Untitled".to_string()
        }
    }

    /// Check if this tab has no file loaded.
    pub fn is_empty(&self) -> bool {
        self.relative_path.is_empty()
    }
}

/// A single entry in the navigation history.
struct HistoryEntry {
    relative_path: String,
}

/// Back/forward navigation history.
pub(super) struct NavigationHistory {
    back_stack: Vec<HistoryEntry>,
    forward_stack: Vec<HistoryEntry>,
}

impl NavigationHistory {
    fn new() -> Self {
        Self {
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
        }
    }

    /// Record a navigation from `current` to a new file.
    fn push(&mut self, current: &str) {
        if current.is_empty() {
            return;
        }
        self.back_stack.push(HistoryEntry {
            relative_path: current.to_string(),
        });
        self.forward_stack.clear();
        if self.back_stack.len() > MAX_HISTORY {
            self.back_stack.remove(0);
        }
    }

    /// Go back. Returns the relative path to navigate to.
    fn go_back(&mut self, current: &str) -> Option<String> {
        let entry = self.back_stack.pop()?;
        if !current.is_empty() {
            self.forward_stack.push(HistoryEntry {
                relative_path: current.to_string(),
            });
        }
        Some(entry.relative_path)
    }

    /// Go forward. Returns the relative path to navigate to.
    fn go_forward(&mut self, current: &str) -> Option<String> {
        let entry = self.forward_stack.pop()?;
        if !current.is_empty() {
            self.back_stack.push(HistoryEntry {
                relative_path: current.to_string(),
            });
        }
        Some(entry.relative_path)
    }

    fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }
}

/// File viewer overlay for displaying file contents.
pub struct FileViewer {
    focus_handle: FocusHandle,
    project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
    /// Syntax set for highlighting (shared via `Arc`, large to clone)
    syntax_set: std::sync::Arc<SyntaxSet>,
    /// File font size from settings
    file_font_size: f32,
    /// Measured monospace character width (from font metrics)
    measured_char_width: f32,
    /// Whether the current theme is dark (for syntax highlighting)
    is_dark: bool,
    /// True until the project root directory listing arrives.
    loading: bool,
    /// Cache of directory listings keyed by project-relative folder path
    /// (`""` = project root). Populated lazily as folders are expanded.
    pub(super) loaded_dirs: HashMap<String, Vec<DirEntry>>,
    /// Folder paths whose listing is currently in flight.
    pub(super) loading_dirs: HashSet<String>,
    /// Which folder paths are currently expanded
    expanded_folders: HashSet<String>,
    /// Scroll handle for the file tree sidebar
    tree_scroll_handle: ScrollHandle,
    /// Whether the sidebar is visible
    sidebar_visible: bool,
    /// Open tabs
    pub(super) tabs: Vec<FileViewerTab>,
    /// Index of the active tab
    pub(super) active_tab: usize,
    /// Navigation history
    pub(super) history: NavigationHistory,
    /// Last time we checked files for external modifications
    last_change_check: std::time::Instant,
    /// True while a background freshness check (stat + possible reload) is in
    /// flight, so we don't spawn overlapping checks if a stat outlives the
    /// once-per-second throttle window (e.g. on a slow network mount).
    freshness_check_in_flight: bool,
    /// Whether to include gitignored files in the file tree
    pub(super) show_ignored: bool,
    /// Whether the filter popover is open
    pub(super) filter_popover_open: bool,
    /// Bounds of the filter button for popover positioning
    pub(super) filter_button_bounds: Option<Bounds<Pixels>>,
    /// Context menu state for file tree right-click
    pub(super) context_menu: Option<FileTreeContextMenu>,
    /// Context menu state for tab right-click
    pub(super) tab_context_menu: Option<TabContextMenu>,
    /// Inline rename state
    pub(super) rename_state: Option<FileRenameState>,
    /// Delete confirmation dialog state
    pub(super) delete_confirm: Option<DeleteConfirmState>,
    /// In-file search state (Ctrl+F)
    pub(super) search_state: Option<search::FileSearchState>,
    /// True when this viewer is hosted inside a detached window.
    /// Hides the "detach" button and is set by the detached host.
    pub(super) is_detached: bool,
    /// Optional provider for per-file git blame. `None` for projects that
    /// can't supply blame (no host wiring, non-git filesystems, etc).
    pub(super) blame_provider: Option<std::sync::Arc<dyn BlameProvider>>,
    /// Whether the blame gutter column is visible. Persisted in settings.
    pub(super) blame_visible: bool,
    /// Right-click context menu over a non-empty text selection.
    pub(super) selection_context_menu: Option<Point<Pixels>>,
    /// Monotonic counter used to stamp each `spawn_tab_load` invocation.
    /// Each background task captures the value it was scheduled at and only
    /// applies its result if the tab's recorded generation still matches,
    /// so a slow earlier load can't overwrite a faster later one.
    next_load_generation: u64,
}

impl FileViewer {
    /// Resolve a project-relative path to an absolute `PathBuf` for filesystem
    /// ops. For remote projects the absolute path doesn't exist locally;
    /// callers fall back to the relative path wrapped as a `PathBuf`.
    fn resolve_absolute(
        fs: &std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        relative_path: &str,
    ) -> PathBuf {
        match fs.project_root() {
            Some(root) => root.join(relative_path),
            None => PathBuf::from(relative_path),
        }
    }

    /// Create a new file viewer with `relative_path` (project-relative) opened
    /// in the first tab.
    pub fn new(
        relative_path: String,
        project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        blame_provider: Option<std::sync::Arc<dyn BlameProvider>>,
        blame_visible: bool,
        font_size: f32,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let expanded_folders = Self::compute_expanded_for_relative(&relative_path);
        let syntax_set = load_syntax_set();

        let file_path = Self::resolve_absolute(&project_fs, &relative_path);
        let tab = FileViewerTab::new_loading(relative_path.clone(), file_path.clone());

        let mut viewer = Self {
            focus_handle,
            project_fs,
            syntax_set,
            file_font_size: font_size,
            measured_char_width: font_size * 0.6,
            is_dark,
            loading: true,
            loaded_dirs: HashMap::new(),
            loading_dirs: HashSet::new(),
            expanded_folders,
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: true,
            tabs: vec![tab],
            active_tab: 0,
            history: NavigationHistory::new(),
            last_change_check: std::time::Instant::now(),
            freshness_check_in_flight: false,
            show_ignored: false,
            filter_popover_open: false,
            filter_button_bounds: None,
            context_menu: None,
            tab_context_menu: None,
            rename_state: None,
            delete_confirm: None,
            search_state: None,
            is_detached: false,
            blame_provider,
            blame_visible,
            selection_context_menu: None,
            next_load_generation: 0,
        };

        // Kick off the root directory listing and any expanded ancestors so
        // the tree fills in around the opened file.
        viewer.fetch_initial_dirs(cx);
        viewer.spawn_tab_load(relative_path, cx);
        if viewer.blame_visible {
            viewer.spawn_blame_load_for_active(cx);
        }
        viewer
    }

    /// Create a file viewer for browsing a project without a pre-selected file.
    ///
    /// Opens the sidebar file tree with no file loaded.
    pub fn new_browse(
        project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        blame_provider: Option<std::sync::Arc<dyn BlameProvider>>,
        blame_visible: bool,
        font_size: f32,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let mut viewer = Self {
            focus_handle,
            project_fs,
            syntax_set: load_syntax_set(),
            file_font_size: font_size,
            measured_char_width: font_size * 0.6,
            is_dark,
            loading: true,
            loaded_dirs: HashMap::new(),
            loading_dirs: HashSet::new(),
            expanded_folders: HashSet::new(),
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: true,
            tabs: vec![FileViewerTab::new_empty()],
            active_tab: 0,
            history: NavigationHistory::new(),
            last_change_check: std::time::Instant::now(),
            freshness_check_in_flight: false,
            show_ignored: false,
            filter_popover_open: false,
            filter_button_bounds: None,
            context_menu: None,
            tab_context_menu: None,
            rename_state: None,
            delete_confirm: None,
            search_state: None,
            is_detached: false,
            blame_provider,
            blame_visible,
            selection_context_menu: None,
            next_load_generation: 0,
        };
        viewer.fetch_initial_dirs(cx);
        viewer
    }

    /// Mark this viewer as hosted in a detached window so the detach button
    /// is hidden and the viewer renders for that context.
    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.is_detached != detached {
            self.is_detached = detached;
            cx.notify();
        }
    }

    /// Whether this viewer is hosted in a detached window.
    pub fn is_detached(&self) -> bool {
        self.is_detached
    }

    /// Request to detach the viewer into a separate OS window.
    pub(super) fn request_detach(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Detach);
    }

    /// Update configuration (font size and dark mode) from the host app.
    /// Also refreshes the file tree and all tabs that were modified externally.
    pub fn update_config(&mut self, font_size: f32, is_dark: bool, cx: &mut Context<Self>) {
        let rehighlight = is_dark != self.is_dark;
        self.file_font_size = font_size;
        self.is_dark = is_dark;

        // Re-fetch directory listings so the sidebar reflects added/removed files
        self.refresh_file_tree_async(cx);

        let svg_renderer = cx.svg_renderer();
        for tab in &mut self.tabs {
            if tab.is_empty() {
                continue;
            }
            // Reload externally modified files (also re-highlights)
            if tab.reload_if_changed(&self.syntax_set, self.is_dark, &svg_renderer) {
                tab.blame = BlameLoadState::NotLoaded;
                continue;
            }
            // Theme changed — re-highlight without reloading. Raster image
            // and font tabs have no highlighted content; SVG tabs do (the
            // source-view XML), so they need the rehighlight too.
            if rehighlight && !tab.is_font && (!tab.is_image || tab.is_svg) {
                tab.do_highlight_content(
                    &tab.file_path.clone(),
                    &self.syntax_set,
                    self.is_dark,
                );
            }
        }

        // Kick off blame load for the active tab if the gutter is visible and
        // its blame got invalidated by the reload above.
        if self.blame_visible {
            self.spawn_blame_load_for_active(cx);
        }
    }

    /// Invalidate the cached directory listings and re-fetch the ones that are
    /// currently expanded. Called when settings change or after file ops that
    /// might affect multiple folders (e.g. rename across hierarchies).
    pub(super) fn refresh_file_tree_async(&mut self, cx: &mut Context<Self>) {
        let to_refetch: Vec<String> = std::iter::once(String::new())
            .chain(self.expanded_folders.iter().cloned())
            .collect();
        self.loaded_dirs.clear();
        self.loading_dirs.clear();
        for path in to_refetch {
            self.fetch_directory(path, cx);
        }
    }

    /// Re-fetch listings for a single directory and any of its loaded
    /// descendants. Use after a targeted file op (create/delete/rename of one
    /// entry) where a global rescan would be wasteful.
    pub(super) fn invalidate_directory(&mut self, relative_path: &str, cx: &mut Context<Self>) {
        let prefix = if relative_path.is_empty() {
            String::new()
        } else {
            format!("{}/", relative_path)
        };
        let to_refetch: Vec<String> = self
            .loaded_dirs
            .keys()
            .filter(|k| k.as_str() == relative_path || k.starts_with(&prefix))
            .cloned()
            .collect();
        for path in &to_refetch {
            self.loaded_dirs.remove(path);
            self.loading_dirs.remove(path);
        }
        // Always re-fetch the target dir even if it wasn't loaded before — the
        // caller asked us to refresh it.
        self.fetch_directory(relative_path.to_string(), cx);
        for path in to_refetch {
            if path != relative_path {
                self.fetch_directory(path, cx);
            }
        }
    }

    /// Fetch the initial directory listings: the project root plus any
    /// ancestor folders that are expanded (so a viewer opened for
    /// `a/b/c.rs` shows the path expanded out to that file).
    fn fetch_initial_dirs(&mut self, cx: &mut Context<Self>) {
        self.fetch_directory(String::new(), cx);
        let dirs: Vec<String> = self.expanded_folders.iter().cloned().collect();
        for dir in dirs {
            self.fetch_directory(dir, cx);
        }
    }

    /// Spawn a background task to load `relative_path`'s immediate children
    /// and stash them in `loaded_dirs`. No-op if already loaded or in flight.
    pub(super) fn fetch_directory(&mut self, relative_path: String, cx: &mut Context<Self>) {
        if self.loaded_dirs.contains_key(&relative_path)
            || !self.loading_dirs.insert(relative_path.clone())
        {
            return;
        }
        let fs = self.project_fs.clone();
        let show_ignored = self.show_ignored;
        let path_for_task = relative_path.clone();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result: Result<Vec<DirEntry>, String> = cx
                .background_executor()
                .spawn(async move { fs.list_directory(&path_for_task, show_ignored) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.loading_dirs.remove(&relative_path);
                match result {
                    Ok(entries) => {
                        this.loaded_dirs.insert(relative_path.clone(), entries);
                    }
                    Err(e) => {
                        log::warn!("list_directory({}) failed: {}", relative_path, e);
                        // Cache an empty vec so we don't retry on every render.
                        this.loaded_dirs.insert(relative_path.clone(), Vec::new());
                    }
                }
                if relative_path.is_empty() {
                    this.loading = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Check if the active tab's file was modified externally and reload it if
    /// so. Throttled to at most once per second. The actual stat + (on change)
    /// read + re-highlight runs on the background executor so it never blocks
    /// the render/UI thread; results are swapped in via `entity.update`.
    ///
    /// Called cheaply from `render`: the render thread only checks the throttle
    /// and (at most once/sec) schedules a background task — no filesystem I/O.
    pub(super) fn check_active_tab_freshness(&mut self, cx: &mut Context<Self>) {
        if self.freshness_check_in_flight
            || self.last_change_check.elapsed() < std::time::Duration::from_secs(1)
        {
            return;
        }
        self.last_change_check = std::time::Instant::now();

        let tab = &self.tabs[self.active_tab];
        if tab.is_empty() {
            return;
        }

        // Capture only the plain data the background work needs — no entity or
        // tab borrows held across the await.
        let relative_path = tab.relative_path.clone();
        let path = tab.file_path.clone();
        let old_mtime = tab.modified_at;
        let is_markdown = tab.is_markdown;
        let syntax_set = self.syntax_set.clone();
        let is_dark = self.is_dark;
        let svg_renderer = cx.svg_renderer();

        self.freshness_check_in_flight = true;
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    loading::compute_freshness_reload(
                        &path,
                        old_mtime,
                        is_markdown,
                        &syntax_set,
                        is_dark,
                        &svg_renderer,
                    )
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.freshness_check_in_flight = false;
                let reloaded = matches!(result, loading::FreshnessOutcome::Reloaded(_));
                // Register fresh font bytes with the platform text system
                // before apply, mirroring spawn_tab_load. add_fonts is
                // idempotent so a no-change font reload is a cheap no-op.
                if let loading::FreshnessOutcome::Reloaded(reload) = &result
                    && let loading::FreshnessKind::Font { ttf_bytes, .. } = &reload.kind
                {
                    register_font_bytes(cx, ttf_bytes);
                }
                // Re-find the tab by relative_path; concurrent reorders/closes
                // mean the index may have shifted since we scheduled the check.
                if let Some(tab) =
                    this.tabs.iter_mut().find(|t| t.relative_path == relative_path)
                {
                    tab.apply_freshness_reload(result);
                    if reloaded {
                        tab.blame = BlameLoadState::NotLoaded;
                    }
                }
                if reloaded {
                    if this.blame_visible {
                        this.spawn_blame_load_for_active(cx);
                    }
                    // The freshness reload of an SVG always rebuilds the
                    // bitmap at rendered_scale=1.0, but image_view.zoom
                    // persists across the reload. Without this kick the
                    // user's previously-tuned high-zoom view stays
                    // pixelated until they touch the wheel.
                    if let Some(tab) = this
                        .tabs
                        .iter()
                        .find(|t| t.relative_path == relative_path)
                        && tab.is_svg
                    {
                        this.maybe_rerender_svg_for(relative_path.clone(), cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// If the active tab is an SVG and the current zoom exceeds the
    /// rasterized bitmap's resolution, kick off a background re-raster at
    /// a higher scale so the preview stays crisp.
    ///
    /// Coalescing: at most one re-raster is in flight per tab. While one
    /// is running, further zoom changes are silently absorbed; when the
    /// result arrives we re-check the current zoom and schedule another
    /// raster if the user has zoomed further in the meantime.
    pub(super) fn maybe_rerender_svg(&mut self, cx: &mut Context<Self>) {
        let relative_path = self.active_tab().relative_path.clone();
        self.maybe_rerender_svg_for(relative_path, cx);
    }

    /// Re-raster the SVG on the tab identified by `relative_path` (not
    /// necessarily the active tab). Used both as the entry point from
    /// zoom actions (which pass the active tab) and from the apply
    /// callback's chase-loop recursion, where the user may have switched
    /// tabs mid-raster and `self.active_tab()` would target the wrong
    /// path.
    pub(super) fn maybe_rerender_svg_for(
        &mut self,
        relative_path: String,
        cx: &mut Context<Self>,
    ) {
        let svg_renderer = cx.svg_renderer();
        let Some((bytes, target_scale)) = self.compute_rerender_target(&relative_path) else {
            return;
        };
        // Capture the source bytes' Arc identity so we can detect — at
        // apply time — that image_data was swapped to a different SVG
        // (freshness reload, tab-replace) while our raster was in flight.
        // Without this guard the stale bitmap would overwrite the fresh
        // image, showing pre-edit pixels for the post-edit file.
        let dispatched_bytes_id = Arc::as_ptr(&bytes) as usize;

        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.relative_path == relative_path)
        {
            tab.image_view.svg_rerender_in_flight = true;
        }

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    svg_renderer
                        .render_single_frame(&bytes, target_scale, true)
                        .map_err(|e| format!("Cannot re-rasterize SVG: {}", e))
                })
                .await;
            let path_for_callback = relative_path.clone();
            let _ = entity.update(cx, |this, cx| {
                let mut should_chase = false;
                if let Some(tab) = this
                    .tabs
                    .iter_mut()
                    .find(|t| t.relative_path == relative_path)
                {
                    tab.image_view.svg_rerender_in_flight = false;
                    match (result, tab.image_data.as_mut()) {
                        (Ok(new_image), Some(DecodedImage::Rendered {
                            image,
                            rendered_scale,
                            svg_bytes,
                            ..
                        })) => {
                            // Discard the result if image_data was
                            // replaced (different SVG bytes) while we
                            // were rasterizing — otherwise the stale
                            // raster would overwrite the new bitmap.
                            if Arc::as_ptr(svg_bytes) as usize == dispatched_bytes_id {
                                *image = new_image;
                                *rendered_scale = target_scale;
                                cx.notify();
                                should_chase = true;
                            }
                            // Bytes mismatch — the freshness reload's own
                            // 1× bitmap is what's now displayed; let any
                            // subsequent zoom action kick a fresh raster.
                        }
                        (Err(_), Some(DecodedImage::Rendered {
                            rendered_scale,
                            svg_bytes,
                            ..
                        })) => {
                            // Pin rendered_scale to the failed target so
                            // compute_rerender_target stops requesting
                            // the same raster on every chase tick. The
                            // bitmap stays at its previous resolution
                            // (the user keeps seeing whatever crisp /
                            // soft state they had before). Only apply
                            // this pin to the SVG we dispatched against.
                            if Arc::as_ptr(svg_bytes) as usize == dispatched_bytes_id {
                                *rendered_scale = target_scale;
                            }
                        }
                        _ => {}
                    }
                }
                // Recurse against the captured relative_path, not the
                // active tab — the user may have switched tabs while the
                // raster ran.
                if should_chase {
                    this.maybe_rerender_svg_for(path_for_callback, cx);
                }
            });
        })
        .detach();
    }

    /// Return `(svg_bytes, target_scale)` if a re-raster of the named tab
    /// would meaningfully sharpen the preview. Returns `None` when the tab
    /// isn't an SVG, isn't zoomed in past its rendered scale, or already
    /// has a re-raster in flight.
    fn compute_rerender_target(
        &self,
        relative_path: &str,
    ) -> Option<(Arc<Vec<u8>>, f32)> {
        let tab = self.tabs.iter().find(|t| t.relative_path == relative_path)?;
        if !tab.is_svg || tab.image_view.svg_rerender_in_flight {
            return None;
        }
        // Fit mode renders via ObjectFit::Contain so the existing 2× pixmap
        // is plenty — no need to re-raster.
        if tab.image_view.auto_fit {
            return None;
        }
        let DecodedImage::Rendered {
            svg_bytes,
            rendered_scale,
            ..
        } = tab.image_data.as_ref()?
        else {
            return None;
        };
        let zoom = tab.image_view.zoom;
        // Small threshold avoids re-rastering on micro-zoom adjustments
        // and on the way back down from a previously-rendered high scale.
        if zoom <= *rendered_scale * 1.1 {
            return None;
        }
        // Cap to keep memory bounded. At 16× a 100×100 SVG produces a
        // 3200×3200 RGBA bitmap (~40 MB) which is the absolute ceiling
        // before the user notices the allocator.
        let target = (zoom * 1.25).clamp(1.0, 16.0);
        Some((svg_bytes.clone(), target))
    }

    /// Get the active tab.
    pub(super) fn active_tab(&self) -> &FileViewerTab {
        &self.tabs[self.active_tab]
    }

    /// Get the active tab mutably.
    pub(super) fn active_tab_mut(&mut self) -> &mut FileViewerTab {
        &mut self.tabs[self.active_tab]
    }

    /// Open a file in a tab (VS Code style).
    /// - If already open in a tab, switches to it.
    /// - If current tab is empty, replaces it.
    /// - Otherwise creates a new tab after the active one.
    pub fn open_file_in_tab(&mut self, relative_path: String, cx: &mut Context<Self>) {
        // Already open? Switch to it.
        if let Some(idx) = self.tabs.iter().position(|t| t.relative_path == relative_path) {
            if idx != self.active_tab {
                let current = self.active_tab().relative_path.clone();
                self.history.push(&current);
                self.active_tab = idx;
            }
            self.expand_ancestors_and_fetch(&relative_path, cx);
            cx.notify();
            return;
        }

        self.expand_ancestors_and_fetch(&relative_path, cx);

        let file_path = Self::resolve_absolute(&self.project_fs, &relative_path);
        let new_tab = FileViewerTab::new_loading(relative_path.clone(), file_path);

        // If current tab is empty (no file loaded), replace it
        if self.active_tab().is_empty() {
            self.tabs[self.active_tab] = new_tab;
            self.spawn_tab_load(relative_path, cx);
            cx.notify();
            return;
        }

        // Push history for the current file
        let current = self.active_tab().relative_path.clone();
        self.history.push(&current);

        self.active_tab = Self::insert_tab_after_active(&mut self.tabs, self.active_tab, new_tab);

        self.spawn_tab_load(relative_path, cx);
        cx.notify();
    }

    /// Insert `new_tab` directly after the active tab and return the new
    /// active index (the inserted tab). When already at `MAX_TABS`, the oldest
    /// tab is evicted first to make room — never the active tab, so the file
    /// the user is looking at is preserved. Evicting a tab before the active
    /// one shifts the active index left by one.
    fn insert_tab_after_active(
        tabs: &mut Vec<FileViewerTab>,
        active: usize,
        new_tab: FileViewerTab,
    ) -> usize {
        let mut active = active;
        if tabs.len() >= MAX_TABS {
            // Oldest tab is index 0; skip it only if it's the active tab.
            let evict = if active == 0 { 1 } else { 0 };
            tabs.remove(evict);
            if evict < active {
                active -= 1;
            }
        }
        let insert_at = active + 1;
        tabs.insert(insert_at, new_tab);
        insert_at
    }

    /// Mark all ancestor folders of `relative_path` as expanded and ensure
    /// their listings are loaded so the tree reveals down to the file.
    fn expand_ancestors_and_fetch(&mut self, relative_path: &str, cx: &mut Context<Self>) {
        let expanded = Self::compute_expanded_for_relative(relative_path);
        for path in &expanded {
            self.fetch_directory(path.clone(), cx);
        }
        self.expanded_folders.extend(expanded);
    }

    /// Close a tab by index.
    pub(super) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            cx.emit(FileViewerEvent::Close);
            return;
        }

        self.tabs.remove(index);

        if index == self.active_tab {
            // Closed the active tab: prefer the tab to the right (same index),
            // or the last tab if we were at the end
            self.active_tab = index.min(self.tabs.len() - 1);
        } else if self.active_tab > index {
            // Closed a tab before the active one: shift index left
            self.active_tab -= 1;
        }
        // If closed tab was after active tab, active_tab stays the same

        cx.notify();
    }

    /// Close all tabs except the one at `index`.
    pub(super) fn close_other_tabs(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            let kept = self.tabs.remove(index);
            self.tabs.clear();
            self.tabs.push(kept);
            self.active_tab = 0;
            cx.notify();
        }
    }

    /// Close all tabs, leaving an empty viewer state.
    pub(super) fn close_all_tabs(&mut self, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.tabs.push(FileViewerTab::new_empty());
        self.active_tab = 0;
        cx.notify();
    }

    /// Switch to a tab by index.
    pub(super) fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && index != self.active_tab {
            let current = self.active_tab().relative_path.clone();
            self.history.push(&current);
            self.active_tab = index;
            if self.blame_visible {
                self.spawn_blame_load_for_active(cx);
            }
            // Update expanded folders to reveal active tab's file
            let tab_rel = self.tabs[self.active_tab].relative_path.clone();
            self.expand_ancestors_and_fetch(&tab_rel, cx);
            // Re-run search for the new tab's content
            if self.search_state.is_some() {
                self.perform_file_search(cx);
            }
            cx.notify();
        }
    }

    /// Navigate back in history.
    pub(super) fn go_back(&mut self, cx: &mut Context<Self>) {
        let current = self.active_tab().relative_path.clone();
        if let Some(target) = self.history.go_back(&current) {
            self.navigate_to_file_no_history(target, cx);
        }
    }

    /// Navigate forward in history.
    pub(super) fn go_forward(&mut self, cx: &mut Context<Self>) {
        let current = self.active_tab().relative_path.clone();
        if let Some(target) = self.history.go_forward(&current) {
            self.navigate_to_file_no_history(target, cx);
        }
    }

    /// Navigate to a file without pushing history (used by back/forward).
    fn navigate_to_file_no_history(&mut self, relative_path: String, cx: &mut Context<Self>) {
        // If file is open in a tab, switch to it
        if let Some(idx) = self.tabs.iter().position(|t| t.relative_path == relative_path) {
            self.active_tab = idx;
            cx.notify();
            return;
        }

        self.expand_ancestors_and_fetch(&relative_path, cx);

        let file_path = Self::resolve_absolute(&self.project_fs, &relative_path);
        let new_tab = FileViewerTab::new_loading(relative_path.clone(), file_path);
        self.tabs[self.active_tab] = new_tab;
        self.spawn_tab_load(relative_path, cx);
        cx.notify();
    }

    /// Spawn a background task to load file content for a tab. The tab is
    /// identified by `relative_path` so concurrent reorders don't bind us to
    /// a stale index, AND by a per-load generation token so a slow earlier
    /// load can't overwrite a faster later one for the same path.
    fn spawn_tab_load(&mut self, relative_path: String, cx: &mut Context<Self>) {
        self.next_load_generation = self.next_load_generation.wrapping_add(1);
        let generation = self.next_load_generation;
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.relative_path == relative_path)
        {
            tab.load_generation = generation;
        }
        let fs = self.project_fs.clone();
        let rel = relative_path.clone();
        // Image / font detection is driven purely by extension, so we can
        // decide the load strategy off-thread without holding the tab borrow.
        let asset_path = PathBuf::from(&relative_path);
        let is_image = image_format_for_path(&asset_path).is_some();
        let is_font = !is_image && font_format_for_path(&asset_path).is_some();
        let svg_renderer = cx.svg_renderer();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result: Result<loading::LoadedContent, String> = cx
                .background_executor()
                .spawn(async move {
                    if is_image {
                        // Don't do a separate file_size round-trip — the
                        // server enforces MAX_IMAGE_FILE_SIZE inside
                        // read_file_bytes (TOCTOU close), and for local
                        // projects build_image_content rejects oversize
                        // bytes too.
                        let bytes = fs.read_file_bytes(&rel)?;
                        if bytes.len() as u64 > MAX_IMAGE_FILE_SIZE {
                            return Err(format!(
                                "Image too large ({:.1} MB). Maximum size is 20 MB.",
                                bytes.len() as f64 / 1024.0 / 1024.0
                            ));
                        }
                        loading::build_image_content(&asset_path, bytes, &svg_renderer)
                    } else if is_font {
                        let bytes = fs.read_file_bytes(&rel)?;
                        if bytes.len() as u64 > loading::MAX_FONT_FILE_SIZE {
                            return Err(format!(
                                "Font too large ({:.1} MB). Maximum size is 20 MB.",
                                bytes.len() as f64 / 1024.0 / 1024.0
                            ));
                        }
                        loading::build_font_content(&asset_path, bytes)
                    } else {
                        let size = fs.file_size(&rel)?;
                        if size > MAX_FILE_SIZE {
                            return Err(format!(
                                "File too large ({:.1} MB). Maximum size is 5 MB.",
                                size as f64 / 1024.0 / 1024.0
                            ));
                        }
                        fs.read_file(&rel).map(loading::LoadedContent::Text)
                    }
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if let Some(tab) =
                    this.tabs.iter_mut().find(|t| t.relative_path == relative_path)
                {
                    // Drop stale results: a newer spawn_tab_load has been
                    // queued for this tab (closed-and-reopened, navigated
                    // away and back, etc.) and its result is what the user
                    // is waiting for.
                    if tab.load_generation != generation {
                        return;
                    }
                    // Register font bytes with the platform text system
                    // BEFORE applying, so the family name is resolvable by
                    // the time render runs. add_fonts is idempotent on
                    // re-registration (same internal name resolves), so
                    // closing and reopening the same font is cheap.
                    if let Ok(loading::LoadedContent::Font { ttf_bytes, .. }) = &result {
                        register_font_bytes(cx, ttf_bytes);
                    }
                    tab.apply_loaded_content(result, &this.syntax_set, this.is_dark);
                    tab.blame = BlameLoadState::NotLoaded;
                    cx.notify();
                }
                if this.blame_visible {
                    this.spawn_blame_load_for_active(cx);
                }
            });
        })
        .detach();
    }

    /// Compute which folder paths should be expanded to reveal a file.
    fn compute_expanded_for_relative(relative_path: &str) -> HashSet<String> {
        let mut expanded = HashSet::new();
        let parts: Vec<&str> = relative_path.split(['/', '\\']).collect();
        // Expand all ancestor directories (not the file itself)
        let mut path_so_far = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !path_so_far.is_empty() {
                path_so_far.push('/');
            }
            path_so_far.push_str(part);
            expanded.insert(path_so_far.clone());
        }
        expanded
    }
}

/// Events emitted by the file viewer.
#[derive(Clone, Debug)]
pub enum FileViewerEvent {
    /// Viewer was closed.
    Close,
    /// User requested to detach the viewer into a separate OS window.
    Detach,
    /// User clicked a blame entry — open the named commit in the diff viewer.
    OpenCommit(String),
    /// User toggled the blame gutter — host persists the preference.
    BlamePreferenceChanged(bool),
    /// User clicked "Send to terminal" on a selection. Carries the structured
    /// payload; the host formats it (relative to terminal CWD) before pasting.
    SendToTerminal(okena_core::send_payload::SendPayload),
}

/// Register a font's OpenType bytes with the active text system so any
/// subsequent render of `Font { family: family_name, .. }` resolves to it.
/// GPUI's `add_fonts` is idempotent on a re-registration (same internal
/// name resolves), so this is safe to call on every freshness reload.
fn register_font_bytes(cx: &mut App, ttf_bytes: &Arc<Vec<u8>>) {
    let bytes: Vec<u8> = ttf_bytes.as_ref().clone();
    if let Err(e) = cx.text_system().add_fonts(vec![std::borrow::Cow::Owned(bytes)]) {
        log::warn!("Failed to register font with text system: {}", e);
    }
}

impl EventEmitter<FileViewerEvent> for FileViewer {}

impl okena_ui::overlay::CloseEvent for FileViewerEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

impl Focusable for FileViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileViewer, FileViewerTab, NavigationHistory, MAX_TABS};

    fn tab(name: &str) -> FileViewerTab {
        FileViewerTab::new_loading(name.to_string(), name.into())
    }

    fn paths(tabs: &[FileViewerTab]) -> Vec<&str> {
        tabs.iter().map(|t| t.relative_path.as_str()).collect()
    }

    #[::core::prelude::v1::test]
    fn insert_tab_below_limit_inserts_after_active() {
        let mut tabs = vec![tab("a"), tab("b"), tab("c")];
        let active = FileViewer::insert_tab_after_active(&mut tabs, 0, tab("new"));
        assert_eq!(active, 1);
        assert_eq!(paths(&tabs), ["a", "new", "b", "c"]);
    }

    #[::core::prelude::v1::test]
    fn insert_tab_at_limit_evicts_oldest_and_keeps_active() {
        let mut tabs: Vec<FileViewerTab> =
            (0..MAX_TABS).map(|i| tab(&format!("f{i}"))).collect();
        // Active is somewhere in the middle.
        let active = FileViewer::insert_tab_after_active(&mut tabs, 10, tab("new"));
        assert_eq!(tabs.len(), MAX_TABS);
        // Oldest (index 0) was evicted; everything shifted left by one, so the
        // active file f10 stays active and the new tab lands right after it.
        assert_eq!(tabs[0].relative_path, "f1");
        assert_eq!(tabs[active - 1].relative_path, "f10");
        assert_eq!(tabs[active].relative_path, "new");
    }

    #[::core::prelude::v1::test]
    fn insert_tab_at_limit_skips_active_when_active_is_oldest() {
        let mut tabs: Vec<FileViewerTab> =
            (0..MAX_TABS).map(|i| tab(&format!("f{i}"))).collect();
        // Active IS the oldest tab — must not evict it.
        let active = FileViewer::insert_tab_after_active(&mut tabs, 0, tab("new"));
        assert_eq!(tabs.len(), MAX_TABS);
        assert_eq!(active, 1);
        // f0 (active) preserved at index 0; f1 (next oldest) evicted.
        assert_eq!(tabs[0].relative_path, "f0");
        assert_eq!(tabs[1].relative_path, "new");
        assert_eq!(tabs[2].relative_path, "f2");
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_root_file() {
        let expanded = FileViewer::compute_expanded_for_relative("README.md");
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_nested_file() {
        let expanded = FileViewer::compute_expanded_for_relative("src/views/mod.rs");
        assert_eq!(expanded.len(), 2);
        assert!(expanded.contains("src"));
        assert!(expanded.contains("src/views"));
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_empty_string() {
        let expanded = FileViewer::compute_expanded_for_relative("");
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_no_slash() {
        let expanded = FileViewer::compute_expanded_for_relative("Cargo.toml");
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_history_back_forward() {
        let mut history = NavigationHistory::new();

        // Navigate a -> b -> c
        history.push("a.rs");
        history.push("b.rs");

        assert!(history.can_go_back());
        assert!(!history.can_go_forward());

        // Go back from c
        let target = history.go_back("c.rs").unwrap();
        assert_eq!(target, "b.rs");
        assert!(history.can_go_forward());

        // Go back again
        let target = history.go_back("b.rs").unwrap();
        assert_eq!(target, "a.rs");

        // Go forward
        let target = history.go_forward("a.rs").unwrap();
        assert_eq!(target, "b.rs");

        let target = history.go_forward("b.rs").unwrap();
        assert_eq!(target, "c.rs");

        assert!(!history.can_go_forward());
    }

    #[::core::prelude::v1::test]
    fn test_history_new_navigation_clears_forward() {
        let mut history = NavigationHistory::new();

        history.push("a.rs");
        history.push("b.rs");

        // Go back from c to b
        history.go_back("c.rs");

        // New navigation from b
        history.push("b.rs");

        // Forward should be empty
        assert!(!history.can_go_forward());

        // Back should give b then a
        let target = history.go_back("d.rs").unwrap();
        assert_eq!(target, "b.rs");
        let target = history.go_back("b.rs").unwrap();
        assert_eq!(target, "a.rs");
    }

    #[::core::prelude::v1::test]
    fn test_history_limit() {
        let mut history = NavigationHistory::new();

        for i in 0..60 {
            history.push(&format!("file_{}.rs", i));
        }

        assert_eq!(history.back_stack.len(), 50);

        // First entry should be file_59 (0-9 were trimmed)
        let mut target = history.go_back("current.rs").unwrap();
        assert_eq!(target, "file_59.rs");

        // Drain remaining
        let mut count = 1;
        while let Some(t) = history.go_back(&target) {
            target = t;
            count += 1;
        }
        assert_eq!(count, 50);
    }
}
