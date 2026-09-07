//! File loading and syntax highlighting for the file viewer.

use super::{
    DecodedImage, FileViewerTab, FontData, FontFormat, MAX_LINES, font_format_for_path,
    image_format_for_path,
};
use crate::syntax::highlight_content;
use gpui::{Image, ImageFormat, SvgRenderer};
use okena_markdown::MarkdownDocument;
use std::path::Path;
use std::sync::Arc;
use syntect::parsing::SyntaxSet;

/// Max font file size (in source-on-disk bytes). Fonts are typically much
/// smaller than images; 20 MB is comfortably above any realistic OpenType
/// file and stops us hammering ttf-parser with multi-GB inputs.
pub(super) const MAX_FONT_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// Content produced by the daemon-backed async loader.
pub(super) enum LoadedContent {
    Text {
        source: String,
        highlighted_lines: Vec<crate::syntax::HighlightedLine>,
        pretty_json: Option<(String, Vec<crate::syntax::HighlightedLine>)>,
    },
    Image {
        decoded: DecodedImage,
        /// For SVG, the raw XML so the user can toggle into source view.
        /// `None` for raster formats.
        source: Option<String>,
    },
    Font {
        data: Arc<FontData>,
        /// OpenType bytes ready for `text_system.add_fonts`. After WOFF2
        /// decompression, this is the underlying TTF/OTF payload.
        ttf_bytes: Arc<Vec<u8>>,
    },
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

/// Map the `image` crate's content-sniffed format onto GPUI's `ImageFormat`.
/// `image::ImageFormat` covers more formats than GPUI knows; we return
/// `None` for anything GPUI can't render so the caller can fall back to
/// the extension-derived format.
fn image_format_from_image_crate(format: image::ImageFormat) -> Option<ImageFormat> {
    Some(match format {
        image::ImageFormat::Png => ImageFormat::Png,
        image::ImageFormat::Jpeg => ImageFormat::Jpeg,
        image::ImageFormat::Gif => ImageFormat::Gif,
        image::ImageFormat::WebP => ImageFormat::Webp,
        image::ImageFormat::Bmp => ImageFormat::Bmp,
        image::ImageFormat::Tiff => ImageFormat::Tiff,
        image::ImageFormat::Ico => ImageFormat::Ico,
        _ => return None,
    })
}

/// Decode raw image bytes into a `DecodedImage` based on file extension.
/// Used by both the initial async load and freshness reloads for image tabs.
///
/// Megapixel budget for a single rasterized SVG. tiny-skia's `Pixmap::new`
/// allocates `width * height * 4` bytes (RGBA), and `SMOOTH_SVG_SCALE_FACTOR`
/// inside GPUI doubles that. A hostile or accidentally-huge `viewBox` would
/// otherwise let one preview commit hundreds of MB / many GB before the
/// allocator complains. 64 MP ≈ 256 MB at 1× scale (~1 GB at 2×) — big
/// enough for any real-world icon or illustration, small enough to refuse
/// pathological inputs.
const MAX_SVG_PIXELS: u64 = 64 * 1024 * 1024;

/// Megapixel budget for a decoded raster image. The on-disk file is capped
/// at `MAX_IMAGE_FILE_SIZE` (20 MB), but that bounds the *compressed* size —
/// a small PNG/WebP can carry enormous pixel dimensions (a "decompression
/// bomb") that GPUI would expand into a multi-GB RGBA buffer when it decodes.
/// We probe the header dimensions and refuse anything past this ceiling, the
/// same 64 MP (~256 MB RGBA) limit the SVG path uses.
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

/// SVGs are pre-rasterized via the supplied `SvgRenderer` (with the BGRA
/// channel swap GPUI's built-in decoder skips for SVG) and the raw XML is
/// returned as `source` so the user can flip to a highlighted source view.
/// Raster formats are wrapped as `Image::from_bytes` and lean on GPUI's
/// asset cache to decode lazily on the UI thread.
pub(super) fn build_image_content(
    path: &Path,
    bytes: Vec<u8>,
    svg_renderer: &SvgRenderer,
) -> Result<LoadedContent, String> {
    let format = image_format_for_path(path).ok_or_else(|| {
        format!(
            "Unsupported image extension: {}",
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(none)")
        )
    })?;
    match format {
        ImageFormat::Svg => {
            // Pre-parse with usvg so we can refuse pathological dimensions
            // before SvgRenderer tries to allocate the pixmap. usvg::Tree
            // parsing is cheap relative to rasterization.
            let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
                .map_err(|e| format!("Cannot decode SVG: {}", e))?;
            let svg_size = tree.size();
            let w = svg_size.width().ceil() as u64;
            let h = svg_size.height().ceil() as u64;
            let pixels = w.saturating_mul(h);
            if pixels == 0 || pixels > MAX_SVG_PIXELS {
                return Err(format!(
                    "SVG dimensions out of range ({}×{}). Max {} megapixels.",
                    w,
                    h,
                    MAX_SVG_PIXELS / 1024 / 1024
                ));
            }
            let initial_scale: f32 = 1.0;
            let rendered = svg_renderer
                .render_single_frame(&bytes, initial_scale)
                .map_err(|e| format!("Cannot decode SVG: {}", e))?;
            // SVG is XML — UTF-8 unless someone hand-saved it weird. If
            // decoding fails we still surface the preview without source.
            let svg_bytes = Arc::new(bytes);
            let source = String::from_utf8(svg_bytes.as_ref().clone()).ok();
            Ok(LoadedContent::Image {
                decoded: DecodedImage::Rendered {
                    image: rendered,
                    width: w as u32,
                    height: h as u32,
                    svg_bytes,
                    rendered_scale: initial_scale,
                },
                source,
            })
        }
        _ => {
            // Probe intrinsic dimensions without decoding the full pixel
            // buffer; image::ImageReader reads only the header. Trust the
            // content-derived format over the extension-derived one so a
            // `.png` that's actually JPEG bytes (common after "Save As")
            // decodes through the right codec rather than failing silently
            // inside GPUI's lazy decoder with the user looking at a sized
            // but blank "Cannot decode image" box.
            let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|e| format!("Cannot read image header: {}", e))?;
            let guessed = reader.format();
            let (width, height) = reader
                .into_dimensions()
                .map_err(|e| format!("Cannot read image dimensions: {}", e))?;
            // Refuse decompression-bomb dimensions before handing the bytes
            // to GPUI's lazy decoder, which would otherwise allocate
            // width × height × 4 bytes of RGBA on the render thread.
            let pixels = (width as u64).saturating_mul(height as u64);
            if pixels == 0 || pixels > MAX_IMAGE_PIXELS {
                return Err(format!(
                    "Image dimensions out of range ({}×{}). Max {} megapixels.",
                    width,
                    height,
                    MAX_IMAGE_PIXELS / 1024 / 1024
                ));
            }
            let effective_format = guessed
                .and_then(image_format_from_image_crate)
                .unwrap_or(format);
            Ok(LoadedContent::Image {
                decoded: DecodedImage::Raster {
                    image: Arc::new(Image::from_bytes(effective_format, bytes)),
                    width,
                    height,
                },
                source: None,
            })
        }
    }
}

/// Parse a font file and return the metadata + OpenType bytes ready for
/// GPUI's text-system registration. Only raw OpenType (TTF/OTF) is decoded;
/// WOFF/WOFF2 are rejected with a user-visible error (decompressing them
/// would require a dependency we deliberately don't pull in).
pub(super) fn build_font_content(path: &Path, bytes: Vec<u8>) -> Result<LoadedContent, String> {
    let format = font_format_for_path(path).ok_or_else(|| {
        format!(
            "Unsupported font extension: {}",
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(none)")
        )
    })?;
    let ttf_bytes: Vec<u8> = match format {
        FontFormat::OpenType => bytes,
        FontFormat::Woff => {
            return Err(
                "WOFF/WOFF2 preview is not supported yet — only OTF and TTF are.".to_string(),
            );
        }
    };
    let face =
        ttf_parser::Face::parse(&ttf_bytes, 0).map_err(|e| format!("Cannot parse font: {}", e))?;
    let read_name = |name_id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .find(|n| n.name_id == name_id && n.to_string().is_some())
            .and_then(|n| n.to_string())
    };
    let family_name = read_name(ttf_parser::name_id::FAMILY).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    });
    let full_name =
        read_name(ttf_parser::name_id::FULL_NAME).unwrap_or_else(|| family_name.clone());
    let style = read_name(ttf_parser::name_id::SUBFAMILY).unwrap_or_else(|| {
        if face.is_italic() {
            "Italic"
        } else {
            "Regular"
        }
        .to_string()
    });
    let version = read_name(ttf_parser::name_id::VERSION).unwrap_or_default();
    let data = Arc::new(FontData {
        family_name,
        full_name,
        style,
        version,
        num_glyphs: face.number_of_glyphs(),
        units_per_em: face.units_per_em(),
        weight_class: face.weight().to_number(),
        is_italic: face.is_italic(),
    });
    Ok(LoadedContent::Font {
        data,
        ttf_bytes: Arc::new(ttf_bytes),
    })
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
    ) {
        self.loading = false;
        self.modified_at = modified_at;
        self.markdown_table_scroll_handles.clear();
        match result {
            Ok(LoadedContent::Text {
                source,
                highlighted_lines,
                pretty_json,
            }) => {
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
            Ok(LoadedContent::Image { decoded, source }) => {
                self.image_data = Some(decoded);
                self.font_data = None;
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
            Ok(LoadedContent::Font { data, .. }) => {
                self.font_data = Some(data);
                self.image_data = None;
                // Font tabs have no source view; clear text fields so a
                // previously-loaded text/SVG doesn't leak through.
                self.content.clear();
                self.highlighted_lines.clear();
                self.source_rows.clear();
                self.line_count = 0;
                self.line_num_width = 3;
                self.longest_source_row = 0;
            }
            Err(e) => {
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
