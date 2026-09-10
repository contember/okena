//! File loading and syntax highlighting for the file viewer.

use super::{FileViewerTab, MAX_LINES};
use crate::file_renderer::{FileRenderer, PreparedFile};
use crate::syntax::highlight_content;
use gpui::{App, AppContext, SvgRenderer};
use okena_markdown::MarkdownDocument;
use std::path::Path;
use syntect::parsing::SyntaxSet;

/// Content produced by the daemon-backed async loader.
pub(super) enum LoadedContent {
    Text {
        source: String,
        highlighted_lines: Vec<crate::syntax::HighlightedLine>,
        pretty_json: Option<(String, Vec<crate::syntax::HighlightedLine>)>,
    },
    Rendered(PreparedFile),
}

impl LoadedContent {
    pub(super) fn release(self, cx: &mut App) {
        if let Self::Rendered(prepared) = self {
            prepared.release(cx);
        }
    }
}

pub(super) fn build_text_content(
    path: &Path,
    source: String,
    syntax_set: &SyntaxSet,
    is_dark: bool,
) -> LoadedContent {
    let highlighted_lines = highlight_content(&source, path, syntax_set, MAX_LINES, is_dark);
    let pretty_json = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        .then(|| pretty_json_preserving_order(&source))
        .flatten()
        .filter(|pretty| pretty != &source)
        .map(|pretty| {
            let highlighted = highlight_content(&pretty, path, syntax_set, MAX_LINES, is_dark);
            (pretty, highlighted)
        });

    LoadedContent::Text {
        source,
        highlighted_lines,
        pretty_json,
    }
}

fn pretty_json_preserving_order(source: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(source).ok()?;

    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len() + source.len() / 8);
    let mut depth = 0usize;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            let character = source[index..].chars().next()?;
            output.push(character);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += character.len_utf8();
            continue;
        }

        match byte {
            b'"' => {
                in_string = true;
                output.push('"');
            }
            b'{' | b'[' => {
                output.push(byte as char);
                let next = bytes[index + 1..]
                    .iter()
                    .copied()
                    .find(|next| !next.is_ascii_whitespace());
                let closes_immediately =
                    matches!((byte, next), (b'{', Some(b'}')) | (b'[', Some(b']')));
                if !closes_immediately {
                    depth += 1;
                    output.push('\n');
                    output.push_str(&"  ".repeat(depth));
                }
            }
            b'}' | b']' => {
                let previous = bytes[..index]
                    .iter()
                    .rev()
                    .copied()
                    .find(|previous| !previous.is_ascii_whitespace());
                let was_empty = matches!((previous, byte), (Some(b'{'), b'}') | (Some(b'['), b']'));
                if !was_empty {
                    depth = depth.saturating_sub(1);
                    output.push('\n');
                    output.push_str(&"  ".repeat(depth));
                }
                output.push(byte as char);
            }
            b',' => {
                output.push(',');
                output.push('\n');
                output.push_str(&"  ".repeat(depth));
            }
            b':' => output.push_str(": "),
            byte if byte.is_ascii_whitespace() => {}
            _ => {
                let character = source[index..].chars().next()?;
                output.push(character);
                index += character.len_utf8() - 1;
            }
        }
        index += 1;
    }
    Some(output)
}

pub(super) fn build_rendered_content(
    path: &Path,
    bytes: Vec<u8>,
    svg_renderer: &SvgRenderer,
) -> Result<LoadedContent, String> {
    FileRenderer::prepare(path, bytes, svg_renderer).map(LoadedContent::Rendered)
}

impl FileViewerTab {
    /// Check if a file is a markdown file based on extension.
    pub(super) fn is_markdown_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let ext_lower = ext.to_lowercase();
                ext_lower == "md" || ext_lower == "markdown"
            })
            .unwrap_or(false)
    }
    /// Apply content that was loaded asynchronously in the background.
    pub(super) fn apply_loaded_content(
        &mut self,
        result: Result<LoadedContent, String>,
        modified_at: Option<u64>,
        syntax_set: &SyntaxSet,
        is_dark: bool,
        cx: &mut App,
    ) {
        self.loading = false;
        self.modified_at = modified_at;
        self.markdown_table_scroll_handles.clear();
        // Offsets are byte-based; the replacing content does not share them.
        self.selection.clear();
        self.markdown_selection.clear();
        match result {
            Ok(LoadedContent::Text {
                source,
                highlighted_lines,
                pretty_json,
            }) => {
                if let Some(renderer) = self.file_renderer.take() {
                    renderer.update(cx, |renderer, cx| renderer.release_assets(cx));
                }
                self.content = source;
                self.highlighted_lines = highlighted_lines;
                self.json_pretty = false;
                self.json_alternate =
                    pretty_json.map(|(content, highlighted_lines)| super::JsonAlternateView {
                        content,
                        highlighted_lines: Some(highlighted_lines),
                    });
                self.rebuild_source_rows();
                if self.is_markdown {
                    let mut doc = MarkdownDocument::parse(&self.content);
                    doc.highlight_code_blocks(is_dark);
                    self.markdown_doc = Some(doc);
                }
            }
            Ok(LoadedContent::Rendered(prepared)) => {
                let source = prepared.source().map(str::to_string);
                if let Some(renderer) = &self.file_renderer {
                    renderer.update(cx, |renderer, cx| renderer.replace(prepared, cx));
                } else {
                    let id = format!("file-renderer-{}", self.relative_path);
                    self.file_renderer = Some(cx.new(|cx| FileRenderer::new(id, prepared, cx)));
                }
                if let Some(content) = source {
                    self.content = content;
                    self.do_highlight_content(&self.file_path.clone(), syntax_set, is_dark);
                } else {
                    // Raster image or SVG with non-UTF-8 bytes — make sure
                    // we don't keep a stale source view alive from a
                    // previously-loaded text/SVG tab.
                    self.content.clear();
                    self.highlighted_lines.clear();
                    self.source_rows.clear();
                    self.line_count = 0;
                    self.line_num_width = 3;
                    self.longest_source_row = 0;
                }
            }
            Err(e) => {
                if let Some(renderer) = self.file_renderer.take() {
                    renderer.update(cx, |renderer, cx| renderer.release_assets(cx));
                }
                self.error_message = Some(e);
            }
        }
    }

    /// Apply syntax highlighting to the content using shared utilities.
    pub(super) fn do_highlight_content(
        &mut self,
        path: &Path,
        syntax_set: &SyntaxSet,
        is_dark: bool,
    ) {
        self.highlighted_lines =
            highlight_content(&self.content, path, syntax_set, MAX_LINES, is_dark);
        self.rebuild_source_rows();
    }

    pub(super) fn rebuild_source_rows(&mut self) {
        self.source_rows = build_source_rows(
            &self.highlighted_lines,
            self.wrap_lines.then_some(self.wrap_columns.max(1)),
        );
        self.line_count = self.source_rows.len();
        self.line_num_width = self.highlighted_lines.len().to_string().len().max(3);
        self.longest_source_row = self
            .source_rows
            .iter()
            .enumerate()
            .max_by_key(|(_, row)| row.columns)
            .map_or(0, |(index, _)| index);
    }
}

fn build_source_rows(
    lines: &[crate::syntax::HighlightedLine],
    wrap_columns: Option<usize>,
) -> Vec<super::SourceRow> {
    let mut rows = Vec::new();
    for (logical_line, line) in lines.iter().enumerate() {
        let text = line.plain_text.as_str();
        let Some(wrap_columns) = wrap_columns else {
            rows.push(super::SourceRow {
                logical_line,
                byte_range: 0..text.len(),
                columns: text.chars().count(),
            });
            continue;
        };

        if text.is_empty() {
            rows.push(super::SourceRow {
                logical_line,
                byte_range: 0..0,
                columns: 0,
            });
            continue;
        }

        let mut start = 0;
        let mut columns = 0;
        for (byte, _) in text.char_indices() {
            if columns == wrap_columns {
                rows.push(super::SourceRow {
                    logical_line,
                    byte_range: start..byte,
                    columns,
                });
                start = byte;
                columns = 0;
            }
            columns += 1;
        }
        rows.push(super::SourceRow {
            logical_line,
            byte_range: start..text.len(),
            columns,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{build_source_rows, pretty_json_preserving_order};
    use crate::syntax::HighlightedLine;

    fn line(text: &str) -> HighlightedLine {
        HighlightedLine {
            spans: Vec::new(),
            plain_text: text.to_string(),
        }
    }

    #[test]
    fn wraps_on_utf8_character_boundaries() {
        let lines = vec![line("aé🙂bc"), line("")];
        let rows = build_source_rows(&lines, Some(2));
        let slices = rows
            .iter()
            .map(|row| &lines[row.logical_line].plain_text[row.byte_range.clone()])
            .collect::<Vec<_>>();

        assert_eq!(slices, ["aé", "🙂b", "c", ""]);
        assert_eq!(
            rows.iter().map(|row| row.logical_line).collect::<Vec<_>>(),
            [0, 0, 0, 1]
        );
    }

    #[test]
    fn leaves_lines_unsplit_when_wrap_is_off() {
        let lines = vec![line("first"), line("second")];
        let rows = build_source_rows(&lines, None);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].byte_range, 0..5);
        assert_eq!(rows[1].byte_range, 0..6);
    }

    #[test]
    fn pretty_json_keeps_object_order_and_string_punctuation() {
        let pretty = pretty_json_preserving_order(
            r#"{"z":1,"a":{"text":"comma, brace } and quote \\\""},"empty":[]}"#,
        )
        .expect("valid JSON");

        assert_eq!(
            pretty,
            "{\n  \"z\": 1,\n  \"a\": {\n    \"text\": \"comma, brace } and quote \\\\\\\"\"\n  },\n  \"empty\": []\n}"
        );
        assert!(pretty.find("\"z\"").unwrap() < pretty.find("\"a\"").unwrap());
    }

    #[test]
    fn pretty_json_rejects_invalid_input() {
        assert!(pretty_json_preserving_order("{not json}").is_none());
    }
}
