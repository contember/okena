//! File loading and syntax highlighting for the file viewer.

use super::{
    font_format_for_path, image_format_for_path, DecodedImage, FileViewerTab, FontData,
    FontFormat, MAX_FILE_SIZE, MAX_IMAGE_FILE_SIZE, MAX_LINES,
};
use crate::syntax::{highlight_content, HighlightedLine};
use gpui::{Image, ImageFormat, SvgRenderer};
use okena_markdown::MarkdownDocument;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use syntect::parsing::SyntaxSet;

/// Max font file size (in source-on-disk bytes). Fonts are typically much
/// smaller than images; 20 MB is comfortably above any realistic OpenType
/// file and stops us hammering ttf-parser with multi-GB inputs.
pub(super) const MAX_FONT_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// Content produced by the async loader. Text, image, and font tabs share
/// the same plumbing (apply_loaded_content / apply_freshness_reload) but
/// carry different payloads.
pub(super) enum LoadedContent {
    Text(String),
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

/// Result of a background freshness check. Carries enough info that the UI
/// thread can apply field assignments without doing any blocking I/O.
pub(super) enum FreshnessOutcome {
    /// File unchanged (or couldn't be stat'd) — nothing to do.
    Unchanged,
    /// File changed and was successfully re-read.
    Reloaded(FreshnessReload),
    /// File changed but re-read/decode failed. `new_mtime` is the file's
    /// current mtime so apply can pin it and avoid re-reading the bad file
    /// on every throttle tick.
    Failed {
        message: String,
        new_mtime: Option<SystemTime>,
    },
}

/// All the heavy work (stat, read, syntax highlighting, markdown parse) has
/// already happened on the background executor; applying this back on the
/// UI thread is just a set of field assignments.
pub(super) struct FreshnessReload {
    pub kind: FreshnessKind,
    pub modified_at: Option<SystemTime>,
}

pub(super) enum FreshnessKind {
    Text {
        content: String,
        highlighted_lines: Vec<HighlightedLine>,
        markdown_doc: Option<MarkdownDocument>,
    },
    Image {
        decoded: DecodedImage,
        /// For SVG, the raw XML plus its pre-highlighted lines so the user
        /// keeps their source view in sync across mtime reloads. `None`
        /// for raster, or for SVG whose bytes failed UTF-8 decode.
        source: Option<(String, Vec<HighlightedLine>)>,
    },
    Font {
        data: Arc<FontData>,
        ttf_bytes: Arc<Vec<u8>>,
    },
}

/// Stat `path` and, if its mtime differs from `old_mtime`, read and
/// re-highlight it. Returns `Ok(None)` when the file is unchanged (or can't be
/// stat'd), `Ok(Some(..))` with the recomputed content when it changed, and
/// `Err` when the file changed but could not be read.
///
/// Pure / blocking — meant to run on the background executor, so it captures no
/// GPUI handles and touches no entity state.
pub(super) fn compute_freshness_reload(
    path: &PathBuf,
    old_mtime: Option<SystemTime>,
    is_markdown: bool,
    syntax_set: &SyntaxSet,
    is_dark: bool,
    svg_renderer: &SvgRenderer,
) -> FreshnessOutcome {
    let Some(old_mtime) = old_mtime else {
        return FreshnessOutcome::Unchanged;
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return FreshnessOutcome::Unchanged;
    };
    let Ok(new_mtime) = metadata.modified() else {
        return FreshnessOutcome::Unchanged;
    };
    if new_mtime == old_mtime {
        return FreshnessOutcome::Unchanged;
    }
    // Inline helper so every Err branch carries new_mtime — the UI thread
    // pins this so the throttle loop doesn't re-read the bad file every
    // second.
    let failed = |message: String| FreshnessOutcome::Failed {
        message,
        new_mtime: Some(new_mtime),
    };
    if font_format_for_path(path).is_some() {
        if metadata.len() > MAX_FONT_FILE_SIZE {
            return failed(format!(
                "Font too large ({:.1} MB). Maximum size is 20 MB.",
                metadata.len() as f64 / 1024.0 / 1024.0
            ));
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return failed(format!("Cannot read file: {}", e)),
        };
        let (data, ttf_bytes) = match build_font_content(path, bytes) {
            Ok(LoadedContent::Font { data, ttf_bytes }) => (data, ttf_bytes),
            Ok(_) => return failed("Internal error decoding font".to_string()),
            Err(e) => return failed(e),
        };
        return FreshnessOutcome::Reloaded(FreshnessReload {
            kind: FreshnessKind::Font { data, ttf_bytes },
            modified_at: Some(new_mtime),
        });
    }
    if image_format_for_path(path).is_some() {
        if metadata.len() > MAX_IMAGE_FILE_SIZE {
            return failed(format!(
                "Image too large ({:.1} MB). Maximum size is 20 MB.",
                metadata.len() as f64 / 1024.0 / 1024.0
            ));
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return failed(format!("Cannot read file: {}", e)),
        };
        // Defense in depth: TOCTOU could grow the file between stat and
        // read. The build below has its own cost, so reject before paying it.
        if bytes.len() as u64 > MAX_IMAGE_FILE_SIZE {
            return failed(format!(
                "Image too large ({:.1} MB). Maximum size is 20 MB.",
                bytes.len() as f64 / 1024.0 / 1024.0
            ));
        }
        let (decoded, source) = match build_image_content(path, bytes, svg_renderer) {
            Ok(LoadedContent::Image { decoded, source }) => (decoded, source),
            Ok(_) => return failed("Internal error decoding image".to_string()),
            Err(e) => return failed(e),
        };
        let source = source.map(|content| {
            let highlighted =
                highlight_content(&content, path, syntax_set, MAX_LINES, is_dark);
            (content, highlighted)
        });
        return FreshnessOutcome::Reloaded(FreshnessReload {
            kind: FreshnessKind::Image { decoded, source },
            modified_at: Some(new_mtime),
        });
    }
    if metadata.len() > MAX_FILE_SIZE {
        return failed(format!(
            "File too large ({:.1} MB). Maximum size is 5 MB.",
            metadata.len() as f64 / 1024.0 / 1024.0
        ));
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            // Distinguish binary files from other read errors, matching load_file.
            let message = if let Ok(bytes) = std::fs::read(path)
                && bytes.iter().take(1024).any(|&b| b == 0)
            {
                "Cannot display binary file".to_string()
            } else {
                format!("Cannot read file: {}", e)
            };
            return failed(message);
        }
    };
    let highlighted_lines = highlight_content(&content, path, syntax_set, MAX_LINES, is_dark);
    let markdown_doc = if is_markdown {
        Some(MarkdownDocument::parse(&content))
    } else {
        None
    };
    FreshnessOutcome::Reloaded(FreshnessReload {
        kind: FreshnessKind::Text {
            content,
            highlighted_lines,
            markdown_doc,
        },
        modified_at: Some(new_mtime),
    })
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
                    w, h, MAX_SVG_PIXELS / 1024 / 1024
                ));
            }
            let initial_scale: f32 = 1.0;
            let rendered = svg_renderer
                .render_single_frame(&bytes, initial_scale, true)
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
pub(super) fn build_font_content(
    path: &Path,
    bytes: Vec<u8>,
) -> Result<LoadedContent, String> {
    let format = font_format_for_path(path).ok_or_else(|| {
        format!(
            "Unsupported font extension: {}",
            path.extension().and_then(|e| e.to_str()).unwrap_or("(none)")
        )
    })?;
    let ttf_bytes: Vec<u8> = match format {
        FontFormat::OpenType => bytes,
        FontFormat::Woff => {
            return Err(
                "WOFF/WOFF2 preview is not supported yet — only OTF and TTF are."
                    .to_string(),
            );
        }
    };
    let face = ttf_parser::Face::parse(&ttf_bytes, 0)
        .map_err(|e| format!("Cannot parse font: {}", e))?;
    let read_name = |name_id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .find(|n| n.name_id == name_id && n.to_string().is_some())
            .and_then(|n| n.to_string())
    };
    let family_name = read_name(ttf_parser::name_id::FAMILY)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string()
        });
    let full_name = read_name(ttf_parser::name_id::FULL_NAME)
        .unwrap_or_else(|| family_name.clone());
    let style = read_name(ttf_parser::name_id::SUBFAMILY)
        .unwrap_or_else(|| if face.is_italic() { "Italic" } else { "Regular" }.to_string());
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

    /// Load file content and apply syntax highlighting.
    pub(super) fn load_file(
        &mut self,
        path: &PathBuf,
        syntax_set: &SyntaxSet,
        is_dark: bool,
        svg_renderer: &SvgRenderer,
    ) {
        // Check file size first
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.error_message = Some(format!("Cannot read file: {}", e));
                return;
            }
        };

        if self.is_image {
            if metadata.len() > MAX_IMAGE_FILE_SIZE {
                self.error_message = Some(format!(
                    "Image too large ({:.1} MB). Maximum size is 20 MB.",
                    metadata.len() as f64 / 1024.0 / 1024.0
                ));
                self.image_data = None;
                return;
            }
            match std::fs::read(path) {
                Ok(bytes) => match build_image_content(path, bytes, svg_renderer) {
                    Ok(LoadedContent::Image { decoded, source }) => {
                        self.image_data = Some(decoded);
                        if let Some(content) = source {
                            self.content = content;
                            self.do_highlight_content(path, syntax_set, is_dark);
                        } else {
                            self.content.clear();
                            self.highlighted_lines.clear();
                            self.line_count = 0;
                            self.line_num_width = 3;
                        }
                        // Only commit the new mtime after a successful
                        // decode. If decode fails we leave modified_at at
                        // its previous value so a subsequent retry (e.g.
                        // file fixed in place without mtime change) still
                        // re-runs through reload_if_changed.
                        self.modified_at = metadata.modified().ok();
                    }
                    Ok(_) => {
                        // Unreachable: build_image_content only returns Image.
                        self.error_message = Some("Internal error decoding image".to_string());
                        self.image_data = None;
                    }
                    Err(e) => {
                        self.error_message = Some(e);
                        self.image_data = None;
                    }
                },
                Err(e) => {
                    self.error_message = Some(format!("Cannot read file: {}", e));
                    self.image_data = None;
                }
            }
            return;
        }

        if self.is_font {
            if metadata.len() > MAX_FONT_FILE_SIZE {
                self.error_message = Some(format!(
                    "Font too large ({:.1} MB). Maximum size is 20 MB.",
                    metadata.len() as f64 / 1024.0 / 1024.0
                ));
                self.font_data = None;
                return;
            }
            match std::fs::read(path) {
                Ok(bytes) => match build_font_content(path, bytes) {
                    Ok(LoadedContent::Font { data, .. }) => {
                        self.font_data = Some(data);
                        self.image_data = None;
                        self.content.clear();
                        self.highlighted_lines.clear();
                        self.line_count = 0;
                        self.line_num_width = 3;
                        // Note: bytes are not re-registered with the text
                        // system here. Sync reload via reload_if_changed
                        // updates the displayed metadata; the sample text
                        // keeps rendering with the originally-registered
                        // font (same family name resolves), which is
                        // acceptable until the user reopens the tab.
                        self.modified_at = metadata.modified().ok();
                    }
                    Ok(_) => {
                        self.error_message = Some("Internal error decoding font".to_string());
                        self.font_data = None;
                    }
                    Err(e) => {
                        self.error_message = Some(e);
                        self.font_data = None;
                    }
                },
                Err(e) => {
                    self.error_message = Some(format!("Cannot read file: {}", e));
                    self.font_data = None;
                }
            }
            return;
        }

        if metadata.len() > MAX_FILE_SIZE {
            self.error_message = Some(format!(
                "File too large ({:.1} MB). Maximum size is 5 MB.",
                metadata.len() as f64 / 1024.0 / 1024.0
            ));
            return;
        }
        self.modified_at = metadata.modified().ok();

        // Read file content
        match std::fs::read_to_string(path) {
            Ok(content) => {
                self.content = content;
                self.do_highlight_content(path, syntax_set, is_dark);
                // Parse markdown if this is a markdown file
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&self.content));
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

    /// Apply content that was loaded asynchronously in the background.
    pub(super) fn apply_loaded_content(
        &mut self,
        result: Result<LoadedContent, String>,
        syntax_set: &SyntaxSet,
        is_dark: bool,
    ) {
        self.loading = false;
        match result {
            Ok(LoadedContent::Text(content)) => {
                self.content = content;
                self.do_highlight_content(&self.file_path.clone(), syntax_set, is_dark);
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&self.content));
                }
                self.modified_at = self.local_mtime();
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
                    self.line_count = 0;
                    self.line_num_width = 3;
                }
                self.modified_at = self.local_mtime();
            }
            Ok(LoadedContent::Font { data, .. }) => {
                self.font_data = Some(data);
                self.image_data = None;
                // Font tabs have no source view; clear text fields so a
                // previously-loaded text/SVG doesn't leak through.
                self.content.clear();
                self.highlighted_lines.clear();
                self.line_count = 0;
                self.line_num_width = 3;
                self.modified_at = self.local_mtime();
            }
            Err(e) => {
                self.error_message = Some(e);
            }
        }
    }

    /// Read mtime for the active file, but only when `file_path` looks like
    /// an absolute on-disk path (local project). For remote projects the
    /// loader synthesises a relative path placeholder; stat'ing that on the
    /// UI host either fails or — worse — coincidentally matches an
    /// unrelated local file. Returning None there keeps the freshness loop
    /// dormant rather than poisoned.
    fn local_mtime(&self) -> Option<SystemTime> {
        if !self.file_path.is_absolute() {
            return None;
        }
        std::fs::metadata(&self.file_path)
            .ok()
            .and_then(|m| m.modified().ok())
    }

    /// Apply the result of a background freshness check computed by
    /// `compute_freshness_reload`. All heavy work (stat/read/highlight) already
    /// happened off-thread; this is just field assignment on the UI thread.
    pub(super) fn apply_freshness_reload(&mut self, outcome: FreshnessOutcome) {
        match outcome {
            FreshnessOutcome::Reloaded(reload) => {
                self.error_message = None;
                match reload.kind {
                    FreshnessKind::Text { content, highlighted_lines, markdown_doc } => {
                        self.content = content;
                        self.line_count = highlighted_lines.len();
                        self.line_num_width = self.line_count.to_string().len().max(3);
                        self.highlighted_lines = highlighted_lines;
                        self.markdown_doc = markdown_doc;
                        // A text reload replaces a previous image preview;
                        // drop the old bitmap so we don't keep it around.
                        self.image_data = None;
                    }
                    FreshnessKind::Image { decoded, source } => {
                        self.image_data = Some(decoded);
                        self.font_data = None;
                        // Always overwrite the source-view fields so a
                        // source=None reload doesn't leave stale XML from
                        // the previous revision visible in Source mode.
                        if let Some((content, highlighted)) = source {
                            self.content = content;
                            self.line_count = highlighted.len();
                            self.line_num_width = self.line_count.to_string().len().max(3);
                            self.highlighted_lines = highlighted;
                        } else {
                            self.content.clear();
                            self.highlighted_lines.clear();
                            self.line_count = 0;
                            self.line_num_width = 3;
                        }
                    }
                    FreshnessKind::Font { data, .. } => {
                        self.font_data = Some(data);
                        self.image_data = None;
                        self.content.clear();
                        self.highlighted_lines.clear();
                        self.line_count = 0;
                        self.line_num_width = 3;
                    }
                }
                self.modified_at = reload.modified_at;
            }
            FreshnessOutcome::Unchanged => {}
            FreshnessOutcome::Failed { message, new_mtime } => {
                self.error_message = Some(message);
                // Pin the mtime so we don't re-attempt the same expensive
                // read on every throttle tick. The next change to the file
                // bumps the mtime again and re-triggers a real reload.
                if let Some(mtime) = new_mtime {
                    self.modified_at = Some(mtime);
                }
            }
        }
    }

    /// Check if the file was modified externally and reload if so.
    /// Returns true if the file was reloaded.
    pub(super) fn reload_if_changed(
        &mut self,
        syntax_set: &SyntaxSet,
        is_dark: bool,
        svg_renderer: &SvgRenderer,
    ) -> bool {
        let Some(old_mtime) = self.modified_at else {
            return false;
        };
        let Ok(metadata) = std::fs::metadata(&self.file_path) else {
            return false;
        };
        let Ok(new_mtime) = metadata.modified() else {
            return false;
        };
        if new_mtime == old_mtime {
            return false;
        }
        let path = self.file_path.clone();
        self.error_message = None;
        self.load_file(&path, syntax_set, is_dark, svg_renderer);
        true
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
        self.line_count = self.highlighted_lines.len();
        self.line_num_width = self.line_count.to_string().len().max(3);
    }
}
