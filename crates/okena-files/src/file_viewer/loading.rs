//! File loading and syntax highlighting for the file viewer.

use super::{
    font_format_for_path, image_format_for_path, DecodedImage, FileViewerTab, FontData,
    FontFormat, MAX_LINES,
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
                    w, h, MAX_SVG_PIXELS / 1024 / 1024
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
        match result {
            Ok(LoadedContent::Text(content)) => {
                self.content = content;
                self.do_highlight_content(&self.file_path.clone(), syntax_set, is_dark);
                if self.is_markdown {
                    self.markdown_doc = Some(MarkdownDocument::parse(&self.content));
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
                    self.line_count = 0;
                    self.line_num_width = 3;
                }
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
        self.line_count = self.highlighted_lines.len();
        self.line_num_width = self.line_count.to_string().len().max(3);
    }
}
