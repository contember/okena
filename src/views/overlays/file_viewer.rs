//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

use crate::theme::theme;
use crate::views::components::{modal_backdrop, modal_content, segmented_toggle};
use super::markdown_renderer::{MarkdownDocument, MarkdownSelection, RenderedNode};
use gpui::*;
use gpui::prelude::*;
use std::path::PathBuf;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Maximum file size to load (5MB)
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum number of lines to display
const MAX_LINES: usize = 10000;

/// Display mode for file viewer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum DisplayMode {
    #[default]
    Source,
    Preview,
}

/// A pre-processed span with color and text ready for display.
#[derive(Clone)]
struct DisplaySpan {
    color: Rgba,
    text: String,
}

/// A highlighted line with pre-processed spans.
#[derive(Clone)]
struct HighlightedLine {
    spans: Vec<DisplaySpan>,
    /// Plain text content of the line (for selection/copy)
    plain_text: String,
}

/// Selection state for the file viewer.
#[derive(Clone, Default)]
struct Selection {
    /// Start position (line, column)
    start: Option<(usize, usize)>,
    /// End position (line, column)
    end: Option<(usize, usize)>,
    /// Whether we're currently dragging
    is_selecting: bool,
}

impl Selection {
    /// Get normalized selection range (start <= end)
    fn normalized(&self) -> Option<((usize, usize), (usize, usize))> {
        match (self.start, self.end) {
            (Some(start), Some(end)) => {
                if start.0 < end.0 || (start.0 == end.0 && start.1 <= end.1) {
                    Some((start, end))
                } else {
                    Some((end, start))
                }
            }
            _ => None,
        }
    }

    /// Check if line is fully or partially selected
    fn line_has_selection(&self, line: usize) -> bool {
        if let Some(((start_line, _), (end_line, _))) = self.normalized() {
            line >= start_line && line <= end_line
        } else {
            false
        }
    }
}

/// File viewer overlay for displaying file contents.
pub struct FileViewer {
    focus_handle: FocusHandle,
    file_path: PathBuf,
    content: String,
    highlighted_lines: Vec<HighlightedLine>,
    line_count: usize,
    line_num_width: usize,
    error_message: Option<String>,
    selection: Selection,
    /// Current display mode (source or preview)
    display_mode: DisplayMode,
    /// Whether the file is a markdown file
    is_markdown: bool,
    /// Parsed markdown document for preview mode
    markdown_doc: Option<MarkdownDocument>,
    /// Selection state for markdown preview mode
    markdown_selection: MarkdownSelection,
    /// Scroll handle for markdown preview (to track scroll offset)
    markdown_scroll_handle: ScrollHandle,
}

impl FileViewer {
    /// Create a new file viewer for the given file path.
    pub fn new(file_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let is_markdown = Self::is_markdown_file(&file_path);

        let mut viewer = Self {
            focus_handle,
            file_path: file_path.clone(),
            content: String::new(),
            highlighted_lines: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            error_message: None,
            selection: Selection::default(),
            display_mode: if is_markdown { DisplayMode::Preview } else { DisplayMode::Source },
            is_markdown,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_scroll_handle: ScrollHandle::new(),
        };

        // Load and highlight the file
        viewer.load_file(&file_path);

        viewer
    }

    /// Check if a file is a markdown file based on extension.
    fn is_markdown_file(path: &PathBuf) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let ext_lower = ext.to_lowercase();
                ext_lower == "md" || ext_lower == "markdown"
            })
            .unwrap_or(false)
    }

    /// Toggle between source and preview display modes.
    fn toggle_display_mode(&mut self, cx: &mut Context<Self>) {
        if !self.is_markdown {
            return;
        }
        self.display_mode = match self.display_mode {
            DisplayMode::Source => DisplayMode::Preview,
            DisplayMode::Preview => DisplayMode::Source,
        };
        cx.notify();
    }

    /// Load file content and apply syntax highlighting.
    fn load_file(&mut self, path: &PathBuf) {
        // Check file size first
        match std::fs::metadata(path) {
            Ok(metadata) => {
                if metadata.len() > MAX_FILE_SIZE {
                    self.error_message = Some(format!(
                        "File too large ({:.1} MB). Maximum size is 5 MB.",
                        metadata.len() as f64 / 1024.0 / 1024.0
                    ));
                    return;
                }
            }
            Err(e) => {
                self.error_message = Some(format!("Cannot read file: {}", e));
                return;
            }
        }

        // Read file content
        match std::fs::read_to_string(path) {
            Ok(content) => {
                self.content = content.clone();
                self.highlight_content(path);
                // Parse markdown if this is a markdown file
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&content));
                }
            }
            Err(e) => {
                // Try reading as binary and check if it's a binary file
                match std::fs::read(path) {
                    Ok(bytes) => {
                        if bytes.iter().take(1024).any(|&b| b == 0) {
                            self.error_message = Some("Cannot display binary file".to_string());
                        } else {
                            self.error_message = Some(format!("Cannot read file: {}", e));
                        }
                    }
                    Err(_) => {
                        self.error_message = Some(format!("Cannot read file: {}", e));
                    }
                }
            }
        }
    }

    /// Map file extension to syntax name for better coverage.
    fn map_extension_to_syntax(ext: &str) -> Option<&'static str> {
        match ext.to_lowercase().as_str() {
            // TypeScript/JavaScript variants - use JavaScript syntax
            "ts" | "tsx" | "mts" | "cts" => Some("js"),
            "jsx" | "mjs" | "cjs" => Some("js"),
            // Vue/Svelte - use HTML
            "vue" | "svelte" => Some("html"),
            // Config files
            "yml" | "yaml" => Some("yaml"),
            "json" | "jsonc" | "json5" => Some("json"),
            "toml" => Some("toml"),
            "ini" | "cfg" | "conf" => Some("ini"),
            // Shell scripts
            "sh" | "bash" | "zsh" | "fish" => Some("sh"),
            "ps1" | "psm1" | "psd1" => Some("ps1"),
            // Web
            "html" | "htm" | "xhtml" => Some("html"),
            "css" | "scss" | "sass" | "less" => Some("css"),
            "xml" | "svg" | "xsl" | "xslt" => Some("xml"),
            // Common languages
            "py" | "pyw" | "pyi" => Some("py"),
            "rb" | "erb" | "rake" => Some("rb"),
            "rs" => Some("rs"),
            "go" => Some("go"),
            "c" | "h" => Some("c"),
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some("cpp"),
            "java" => Some("java"),
            "kt" | "kts" => Some("kt"),
            "swift" => Some("swift"),
            "cs" => Some("cs"),
            "php" => Some("php"),
            "pl" | "pm" => Some("pl"),
            "lua" => Some("lua"),
            "sql" => Some("sql"),
            "md" | "markdown" => Some("md"),
            "tex" | "latex" => Some("tex"),
            "diff" | "patch" => Some("diff"),
            "dockerfile" => Some("dockerfile"),
            _ => None,
        }
    }

    /// Apply syntax highlighting to the content.
    fn highlight_content(&mut self, path: &PathBuf) {
        let ss = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();

        // Try to find syntax by extension with our custom mapping
        let ext = path.extension().and_then(|e| e.to_str());
        let syntax = ext
            .and_then(|e| Self::map_extension_to_syntax(e))
            .and_then(|mapped| ss.find_syntax_by_extension(mapped))
            .or_else(|| {
                // Try direct extension match
                ext.and_then(|e| ss.find_syntax_by_extension(e))
            })
            .or_else(|| {
                // Try by filename for special files
                path.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|name| {
                        let name_lower = name.to_lowercase();
                        match name_lower.as_str() {
                            "makefile" | "gnumakefile" => ss.find_syntax_by_extension("makefile"),
                            "dockerfile" => ss.find_syntax_by_extension("dockerfile"),
                            "cargo.toml" | "cargo.lock" | "pyproject.toml" => ss.find_syntax_by_extension("toml"),
                            "package.json" | "tsconfig.json" | "jsconfig.json" => ss.find_syntax_by_extension("json"),
                            ".gitignore" | ".dockerignore" | ".npmignore" => ss.find_syntax_by_name("Git Ignore"),
                            ".bashrc" | ".zshrc" | ".bash_profile" | ".profile" => ss.find_syntax_by_extension("sh"),
                            ".env" | ".env.local" | ".env.development" | ".env.production" => ss.find_syntax_by_extension("sh"),
                            _ => None,
                        }
                    })
            })
            .unwrap_or_else(|| ss.find_syntax_plain_text());

        // Use a dark theme that works well with our terminal themes
        let theme = &ts.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        // Default text color for fallback
        let default_color = Rgba {
            r: 0.8,
            g: 0.8,
            b: 0.8,
            a: 1.0,
        };

        let mut lines = Vec::new();
        let mut line_count = 0;

        for line in LinesWithEndings::from(&self.content) {
            if line_count >= MAX_LINES {
                break;
            }

            let (display_spans, plain_text) = match highlighter.highlight_line(line, &ss) {
                Ok(spans) => {
                    // Merge consecutive spans with the same color and pre-process text
                    let mut merged: Vec<DisplaySpan> = Vec::new();
                    let mut plain = String::new();

                    for (style, text) in spans {
                        let color = Rgba {
                            r: style.foreground.r as f32 / 255.0,
                            g: style.foreground.g as f32 / 255.0,
                            b: style.foreground.b as f32 / 255.0,
                            a: style.foreground.a as f32 / 255.0,
                        };

                        // Pre-process text: remove newlines, expand tabs
                        let processed = text
                            .trim_end_matches(&['\n', '\r'][..])
                            .replace('\t', "    ");

                        if processed.is_empty() {
                            continue;
                        }

                        plain.push_str(&processed);

                        // Try to merge with previous span if same color
                        if let Some(last) = merged.last_mut() {
                            if (last.color.r - color.r).abs() < 0.01
                                && (last.color.g - color.g).abs() < 0.01
                                && (last.color.b - color.b).abs() < 0.01
                            {
                                last.text.push_str(&processed);
                                continue;
                            }
                        }

                        merged.push(DisplaySpan { color, text: processed });
                    }

                    (merged, plain)
                }
                Err(_) => {
                    // Fallback: no highlighting
                    let text = line
                        .trim_end_matches(&['\n', '\r'][..])
                        .replace('\t', "    ");
                    (vec![DisplaySpan { color: default_color, text: text.clone() }], text)
                }
            };

            lines.push(HighlightedLine { spans: display_spans, plain_text });
            line_count += 1;
        }

        self.highlighted_lines = lines;
        self.line_count = line_count;
        self.line_num_width = line_count.to_string().len().max(3);
    }

    /// Close the viewer.
    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Close);
    }

    /// Get selected text.
    fn get_selected_text(&self) -> Option<String> {
        let ((start_line, start_col), (end_line, end_col)) = self.selection.normalized()?;

        let mut result = String::new();

        for line_idx in start_line..=end_line {
            if line_idx >= self.highlighted_lines.len() {
                break;
            }

            let line = &self.highlighted_lines[line_idx];
            let text = &line.plain_text;

            if start_line == end_line {
                // Single line selection
                let start = start_col.min(text.len());
                let end = end_col.min(text.len());
                result.push_str(&text[start..end]);
            } else if line_idx == start_line {
                // First line of multi-line selection
                let start = start_col.min(text.len());
                result.push_str(&text[start..]);
                result.push('\n');
            } else if line_idx == end_line {
                // Last line of multi-line selection
                let end = end_col.min(text.len());
                result.push_str(&text[..end]);
            } else {
                // Middle line - take entire line
                result.push_str(text);
                result.push('\n');
            }
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Copy selected text to clipboard.
    fn copy_selection(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.get_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Select all text.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.highlighted_lines.is_empty() {
            return;
        }
        let last_line = self.highlighted_lines.len() - 1;
        let last_col = self.highlighted_lines[last_line].plain_text.len();
        self.selection.start = Some((0, 0));
        self.selection.end = Some((last_line, last_col));
        cx.notify();
    }


    /// Get selected text from markdown preview (using character indices).
    fn get_selected_markdown_text(&self) -> Option<String> {
        let doc = self.markdown_doc.as_ref()?;
        let (start, end) = self.markdown_selection.normalized()?;

        let chars: Vec<char> = doc.plain_text.chars().collect();
        let char_count = chars.len();
        let start = start.min(char_count);
        let end = end.min(char_count);

        if start == end {
            None
        } else {
            Some(chars[start..end].iter().collect())
        }
    }

    /// Copy selected markdown text to clipboard.
    fn copy_markdown_selection(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.get_selected_markdown_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Select all markdown text (using character count).
    fn select_all_markdown(&mut self, cx: &mut Context<Self>) {
        if let Some(doc) = &self.markdown_doc {
            self.markdown_selection.start = Some(0);
            self.markdown_selection.end = Some(doc.plain_text.chars().count());
            cx.notify();
        }
    }

    /// Render a single highlighted line with selection support.
    fn render_line(&self, line_number: usize, line: &HighlightedLine, t: &crate::theme::ThemeColors, cx: &mut Context<Self>) -> Stateful<Div> {
        // Format line number with right padding
        let line_num_str = format!("{:>width$}", line_number + 1, width = self.line_num_width);
        let has_selection = self.selection.line_has_selection(line_number);
        let line_num_width = self.line_num_width;

        // Selection highlight color
        let selection_bg = Rgba {
            r: 0.2,
            g: 0.4,
            b: 0.7,
            a: 0.4,
        };

        div()
            .id(ElementId::Name(format!("line-{}", line_number).into()))
            .flex()
            .h(px(18.0))
            .text_size(px(12.0))
            .font_family("monospace")
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                let col = this.x_to_column(f32::from(event.position.x), line_num_width);
                this.selection.start = Some((line_number, col));
                this.selection.end = Some((line_number, col));
                this.selection.is_selecting = true;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                if this.selection.is_selecting {
                    let col = this.x_to_column(f32::from(event.position.x), line_num_width);
                    this.selection.end = Some((line_number, col));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.selection.is_selecting = false;
                cx.notify();
            }))
            .child(
                // Line number gutter
                div()
                    .w(px((self.line_num_width * 8 + 16) as f32))
                    .pr(px(8.0))
                    .text_color(rgb(t.text_muted))
                    .flex()
                    .justify_end()
                    .flex_shrink_0()
                    .child(line_num_str),
            )
            .child(
                // Line content with syntax highlighting and selection
                if has_selection {
                    // Render with selection highlighting
                    self.render_line_with_selection(line_number, line, t, selection_bg)
                } else {
                    // Simple render without selection
                    div()
                        .flex_1()
                        .flex()
                        .overflow_hidden()
                        .children(
                            line.spans.iter().map(|span| {
                                div()
                                    .text_color(span.color)
                                    .child(span.text.clone())
                            }),
                        )
                },
            )
    }

    /// Render a line with selection highlighting.
    fn render_line_with_selection(
        &self,
        line_number: usize,
        line: &HighlightedLine,
        _t: &crate::theme::ThemeColors,
        selection_bg: Rgba,
    ) -> Div {
        let ((start_line, start_col), (end_line, end_col)) = match self.selection.normalized() {
            Some(range) => range,
            None => {
                return div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .children(
                        line.spans.iter().map(|span| {
                            div()
                                .text_color(span.color)
                                .child(span.text.clone())
                        }),
                    );
            }
        };

        // Determine selection bounds for this line
        let line_len = line.plain_text.len();
        let sel_start = if line_number == start_line { start_col.min(line_len) } else { 0 };
        let sel_end = if line_number == end_line { end_col.min(line_len) } else { line_len };

        // Build character-level rendering with selection
        let mut elements: Vec<Div> = Vec::new();
        let mut current_col = 0;

        for span in &line.spans {
            let span_len = span.text.len();
            let span_end = current_col + span_len;

            // Check if this span intersects with selection
            let span_sel_start = sel_start.max(current_col);
            let span_sel_end = sel_end.min(span_end);

            if span_sel_start < span_sel_end && span_sel_start < span_end && span_sel_end > current_col {
                // Span has some selection - split into parts
                let rel_sel_start = span_sel_start - current_col;
                let rel_sel_end = span_sel_end - current_col;

                // Before selection
                if rel_sel_start > 0 {
                    elements.push(
                        div()
                            .text_color(span.color)
                            .child(span.text[..rel_sel_start].to_string())
                    );
                }

                // Selected part
                elements.push(
                    div()
                        .bg(selection_bg)
                        .text_color(span.color)
                        .child(span.text[rel_sel_start..rel_sel_end].to_string())
                );

                // After selection
                if rel_sel_end < span_len {
                    elements.push(
                        div()
                            .text_color(span.color)
                            .child(span.text[rel_sel_end..].to_string())
                    );
                }
            } else {
                // No selection in this span
                elements.push(
                    div()
                        .text_color(span.color)
                        .child(span.text.clone())
                );
            }

            current_col = span_end;
        }

        div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .children(elements)
    }

    /// Calculate column position from x coordinate.
    fn x_to_column(&self, x: f32, line_num_width: usize) -> usize {
        // Approximate: assume 7.2px per character in monospace font at 12px
        let char_width = 7.2;
        let gutter_width = (line_num_width * 8 + 16) as f32;
        let text_x = (x - gutter_width).max(0.0);
        (text_x / char_width) as usize
    }
}

/// Events emitted by the file viewer.
#[derive(Clone, Debug)]
pub enum FileViewerEvent {
    /// Viewer was closed.
    Close,
}

impl EventEmitter<FileViewerEvent> for FileViewer {}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let has_error = self.error_message.is_some();
        let error_message = self.error_message.clone();
        let has_selection = self.selection.normalized().is_some();
        let is_markdown = self.is_markdown;
        let display_mode = self.display_mode;
        let is_preview_mode = display_mode == DisplayMode::Preview;

        let filename = self.file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "File".to_string());

        let relative_path = self.file_path.to_string_lossy().to_string();

        // Pre-render source lines for better performance
        let rendered_lines: Vec<Stateful<Div>> = if !has_error && !is_preview_mode {
            self.highlighted_lines
                .iter()
                .enumerate()
                .map(|(i, line)| self.render_line(i, line, &t, cx))
                .collect()
        } else {
            Vec::new()
        };

        // Pre-render markdown preview with selection - using per-node handlers
        let preview_nodes: Vec<RenderedNode> = if !has_error && is_preview_mode && is_markdown {
            self.markdown_doc.as_ref().map(|doc| {
                let selection = self.markdown_selection.normalized();
                doc.render_nodes_with_offsets(&t, selection)
            }).unwrap_or_default()
        } else {
            Vec::new()
        };
        let has_markdown_selection = self.markdown_selection.normalized().is_some();

        // Focus on first render
        window.focus(&focus_handle, cx);

        modal_backdrop("file-viewer-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("FileViewer")
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                let is_preview = this.display_mode == DisplayMode::Preview;

                match key {
                    "escape" => {
                        // Clear selection first, then close if no selection
                        if is_preview && this.markdown_selection.normalized().is_some() {
                            this.markdown_selection = MarkdownSelection::default();
                            cx.notify();
                        } else if this.selection.normalized().is_some() {
                            this.selection = Selection::default();
                            cx.notify();
                        } else {
                            this.close(cx);
                        }
                    }
                    "tab" if this.is_markdown => {
                        this.toggle_display_mode(cx);
                    }
                    "c" if modifiers.platform || modifiers.control => {
                        if is_preview {
                            this.copy_markdown_selection(cx);
                        } else {
                            this.copy_selection(cx);
                        }
                    }
                    "a" if modifiers.platform || modifiers.control => {
                        if is_preview {
                            this.select_all_markdown(cx);
                        } else {
                            this.select_all(cx);
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("file-viewer-modal", &t)
                    // Larger modal - 90% width, 85% height with max bounds
                    .w(relative(0.9))
                    .max_w(px(1200.0))
                    .h(relative(0.85))
                    .max_h(px(900.0))
                    .when(!is_preview_mode, |d| d.cursor(CursorStyle::IBeam))
                    // Custom header with toggle for markdown files
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                // Left side: filename and path
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(t.text_primary))
                                            .child(filename),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(t.text_muted))
                                            .child(relative_path),
                                    ),
                            )
                            .child(
                                // Right side: toggle (for markdown) and close button
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .when(is_markdown, |d| {
                                        d.child(
                                            div()
                                                .id("display-mode-toggle")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.toggle_display_mode(cx);
                                                }))
                                                .child(segmented_toggle(
                                                    &[
                                                        ("Preview", is_preview_mode),
                                                        ("Source", !is_preview_mode),
                                                    ],
                                                    &t,
                                                ))
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("close-button")
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .hover(|s| s.bg(rgb(t.bg_secondary)))
                                            .on_click(cx.listener(|this, _, _window, cx| this.close(cx)))
                                            .child(
                                                div()
                                                    .text_size(px(18.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("×"),
                                            ),
                                    ),
                            ),
                    )
                    .when(has_error, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(error_message.unwrap_or_default()),
                                ),
                        )
                    })
                    // Source view (syntax highlighted)
                    .when(!has_error && !is_preview_mode, |d| {
                        d.child(
                            div()
                                .id("file-content")
                                .flex_1()
                                .overflow_y_scroll()
                                .overflow_x_scroll()
                                .bg(rgb(t.bg_secondary))
                                .py(px(8.0))
                                .children(rendered_lines),
                        )
                    })
                    // Preview view (rendered markdown) - with per-node selection handlers
                    .when(!has_error && is_preview_mode, |d| {
                        // Build content with per-node/line handlers
                        let mut content_children: Vec<AnyElement> = Vec::new();
                        let mut node_idx = 0usize;

                        for rendered_node in preview_nodes {
                            match rendered_node {
                                RenderedNode::Simple { div: node_div, start_offset, end_offset } => {
                                    // Block-level selection for simple nodes
                                    let node_end = end_offset.saturating_sub(1);
                                    let idx = node_idx;
                                    content_children.push(
                                        div()
                                            .id(ElementId::Name(format!("md-node-{}", idx).into()))
                                            .w_full()
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                if event.click_count == 2 {
                                                    // Double-click: select entire block
                                                    this.markdown_selection.start = Some(start_offset);
                                                    this.markdown_selection.end = Some(node_end);
                                                    this.markdown_selection.is_selecting = false;
                                                } else {
                                                    this.markdown_selection.start = Some(start_offset);
                                                    this.markdown_selection.end = Some(start_offset);
                                                    this.markdown_selection.is_selecting = true;
                                                }
                                                cx.notify();
                                            }))
                                            .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                if this.markdown_selection.is_selecting {
                                                    if let Some(sel_start) = this.markdown_selection.start {
                                                        if start_offset >= sel_start {
                                                            this.markdown_selection.end = Some(node_end);
                                                        } else {
                                                            this.markdown_selection.end = Some(start_offset);
                                                        }
                                                        cx.notify();
                                                    }
                                                }
                                            }))
                                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                this.markdown_selection.is_selecting = false;
                                                cx.notify();
                                            }))
                                            .child(node_div)
                                            .into_any_element()
                                    );
                                    node_idx += 1;
                                }
                                RenderedNode::CodeBlock { language, lines, .. } => {
                                    // Code block with per-line selection
                                    let lang_label = language.as_deref().unwrap_or("");
                                    let idx = node_idx;

                                    // Build lines with handlers
                                    let line_children: Vec<AnyElement> = lines.into_iter().enumerate().map(|(line_idx, (line_div, start_offset, end_offset))| {
                                        let line_end = end_offset.saturating_sub(1); // Exclude newline
                                        div()
                                            .id(ElementId::Name(format!("md-code-{}-line-{}", idx, line_idx).into()))
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                if event.click_count == 2 {
                                                    // Double-click: select entire line
                                                    this.markdown_selection.start = Some(start_offset);
                                                    this.markdown_selection.end = Some(line_end);
                                                    this.markdown_selection.is_selecting = false;
                                                } else {
                                                    this.markdown_selection.start = Some(start_offset);
                                                    this.markdown_selection.end = Some(start_offset);
                                                    this.markdown_selection.is_selecting = true;
                                                }
                                                cx.notify();
                                            }))
                                            .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                if this.markdown_selection.is_selecting {
                                                    if let Some(sel_start) = this.markdown_selection.start {
                                                        if start_offset >= sel_start {
                                                            this.markdown_selection.end = Some(line_end);
                                                        } else {
                                                            this.markdown_selection.end = Some(start_offset);
                                                        }
                                                        cx.notify();
                                                    }
                                                }
                                            }))
                                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                this.markdown_selection.is_selecting = false;
                                                cx.notify();
                                            }))
                                            .child(line_div)
                                            .into_any_element()
                                    }).collect();

                                    // Build code block container
                                    let code_block = div()
                                        .id(ElementId::Name(format!("md-codeblock-{}", idx).into()))
                                        .flex()
                                        .flex_col()
                                        .rounded(px(6.0))
                                        .bg(rgb(t.bg_primary))
                                        .border_1()
                                        .border_color(rgb(t.border))
                                        .overflow_hidden()
                                        .when(!lang_label.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .px(px(12.0))
                                                    .py(px(4.0))
                                                    .bg(rgb(t.bg_header))
                                                    .border_b_1()
                                                    .border_color(rgb(t.border))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(lang_label.to_string())
                                            )
                                        })
                                        .child(
                                            div()
                                                .p(px(12.0))
                                                .font_family("monospace")
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_secondary))
                                                .flex()
                                                .flex_col()
                                                .children(line_children)
                                        );

                                    content_children.push(code_block.into_any_element());
                                    node_idx += 1;
                                }
                                RenderedNode::Table { header, rows } => {
                                    // Table with per-row selection
                                    let idx = node_idx;

                                    let mut table_rows: Vec<AnyElement> = Vec::new();

                                    // Header row with handler
                                    if let Some((header_div, start_offset, end_offset)) = header {
                                        let row_end = end_offset.saturating_sub(1);
                                        table_rows.push(
                                            div()
                                                .id(ElementId::Name(format!("md-table-{}-header", idx).into()))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                    if event.click_count == 2 {
                                                        // Double-click: select entire row
                                                        this.markdown_selection.start = Some(start_offset);
                                                        this.markdown_selection.end = Some(row_end);
                                                        this.markdown_selection.is_selecting = false;
                                                    } else {
                                                        this.markdown_selection.start = Some(start_offset);
                                                        this.markdown_selection.end = Some(start_offset);
                                                        this.markdown_selection.is_selecting = true;
                                                    }
                                                    cx.notify();
                                                }))
                                                .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                    if this.markdown_selection.is_selecting {
                                                        if let Some(sel_start) = this.markdown_selection.start {
                                                            if start_offset >= sel_start {
                                                                this.markdown_selection.end = Some(row_end);
                                                            } else {
                                                                this.markdown_selection.end = Some(start_offset);
                                                            }
                                                            cx.notify();
                                                        }
                                                    }
                                                }))
                                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                    this.markdown_selection.is_selecting = false;
                                                    cx.notify();
                                                }))
                                                .child(header_div)
                                                .into_any_element()
                                        );
                                    }

                                    // Data rows with handlers
                                    for (row_idx, (row_div, start_offset, end_offset)) in rows.into_iter().enumerate() {
                                        let row_end = end_offset.saturating_sub(1);
                                        table_rows.push(
                                            div()
                                                .id(ElementId::Name(format!("md-table-{}-row-{}", idx, row_idx).into()))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                    if event.click_count == 2 {
                                                        // Double-click: select entire row
                                                        this.markdown_selection.start = Some(start_offset);
                                                        this.markdown_selection.end = Some(row_end);
                                                        this.markdown_selection.is_selecting = false;
                                                    } else {
                                                        this.markdown_selection.start = Some(start_offset);
                                                        this.markdown_selection.end = Some(start_offset);
                                                        this.markdown_selection.is_selecting = true;
                                                    }
                                                    cx.notify();
                                                }))
                                                .on_mouse_move(cx.listener(move |this, _event: &MouseMoveEvent, _window, cx| {
                                                    if this.markdown_selection.is_selecting {
                                                        if let Some(sel_start) = this.markdown_selection.start {
                                                            if start_offset >= sel_start {
                                                                this.markdown_selection.end = Some(row_end);
                                                            } else {
                                                                this.markdown_selection.end = Some(start_offset);
                                                            }
                                                            cx.notify();
                                                        }
                                                    }
                                                }))
                                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                                    this.markdown_selection.is_selecting = false;
                                                    cx.notify();
                                                }))
                                                .child(row_div)
                                                .into_any_element()
                                        );
                                    }

                                    // Build table container
                                    let table = div()
                                        .id(ElementId::Name(format!("md-table-{}", idx).into()))
                                        .flex()
                                        .flex_col()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(rgb(t.border))
                                        .overflow_hidden()
                                        .children(table_rows);

                                    content_children.push(table.into_any_element());
                                    node_idx += 1;
                                }
                            }
                        }

                        let content_div = div()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .p(px(16.0))
                            .max_w(px(900.0))
                            .children(content_children);

                        d.child(
                            div()
                                .id("markdown-preview")
                                .flex_1()
                                .overflow_y_scroll()
                                .overflow_x_scroll()
                                .track_scroll(&self.markdown_scroll_handle)
                                .bg(rgb(t.bg_secondary))
                                .cursor(CursorStyle::IBeam)
                                // Global mouse up to handle case where mouse up happens outside a node
                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                    this.markdown_selection.is_selecting = false;
                                    cx.notify();
                                }))
                                .child(content_div)
                        )
                    })
                    .child(
                        // Footer with hints
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(16.0))
                                    // Tab toggle (only for markdown)
                                    .when(is_markdown, |d| {
                                        d.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .child(
                                                    div()
                                                        .px(px(4.0))
                                                        .py(px(1.0))
                                                        .rounded(px(3.0))
                                                        .bg(rgb(t.bg_secondary))
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("Tab"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("toggle preview"),
                                                ),
                                        )
                                    })
                                    // Copy
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(if cfg!(target_os = "macos") { "Cmd+C" } else { "Ctrl+C" }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("copy"),
                                            ),
                                    )
                                    // Select all
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(if cfg!(target_os = "macos") { "Cmd+A" } else { "Ctrl+A" }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("select all"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("Esc"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("close"),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .when(has_selection && !is_preview_mode, |d| {
                                        d.child("Selection active")
                                    })
                                    .when(!has_selection && !is_preview_mode, |d| {
                                        d.child(format!("{} lines", self.line_count))
                                    })
                                    .when(is_preview_mode && has_markdown_selection, |d| {
                                        d.child("Selection active")
                                    })
                                    .when(is_preview_mode && !has_markdown_selection, |d| {
                                        d.child("Preview mode")
                                    }),
                            ),
                    ),
            )
    }
}

impl_focusable!(FileViewer);
