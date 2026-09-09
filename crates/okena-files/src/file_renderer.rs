//! Reusable renderer for complete image and font files.

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_sm;
use std::path::Path;
use std::sync::Arc;

pub const MAX_RENDERED_FILE_SIZE: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_SVG_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRendererKind {
    Image { is_svg: bool },
    Font,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum PreviewBackground {
    #[default]
    Checker,
    Light,
    Dark,
}

#[derive(Clone)]
struct ImageViewState {
    auto_fit: bool,
    zoom: f32,
    pan: Point<Pixels>,
    is_panning: bool,
    pan_anchor: Option<Point<Pixels>>,
    pan_anchor_offset: Point<Pixels>,
    background: PreviewBackground,
    svg_rerender_in_flight: bool,
}

impl Default for ImageViewState {
    fn default() -> Self {
        Self {
            auto_fit: true,
            zoom: 1.0,
            pan: Point::default(),
            is_panning: false,
            pan_anchor: None,
            pan_anchor_offset: Point::default(),
            background: PreviewBackground::Checker,
            svg_rerender_in_flight: false,
        }
    }
}

impl ImageViewState {
    const MIN_ZOOM: f32 = 0.1;
    const MAX_ZOOM: f32 = 10.0;

    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        self.auto_fit = false;
    }

    fn reset_to_fit(&mut self) {
        self.auto_fit = true;
        self.zoom = 1.0;
        self.pan = Point::default();
        self.is_panning = false;
        self.pan_anchor = None;
        self.pan_anchor_offset = Point::default();
    }
}

#[derive(Clone)]
enum DecodedImage {
    Raster {
        image: Arc<Image>,
        width: u32,
        height: u32,
    },
    Rendered {
        image: Arc<RenderImage>,
        width: u32,
        height: u32,
        svg_bytes: Arc<Vec<u8>>,
        rendered_scale: f32,
    },
}

impl DecodedImage {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Raster { width, height, .. } | Self::Rendered { width, height, .. } => {
                (*width, *height)
            }
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

#[derive(Clone)]
struct FontData {
    family_name: String,
    full_name: String,
    style: String,
    version: String,
    num_glyphs: u16,
    units_per_em: u16,
    weight_class: u16,
    is_italic: bool,
}

enum PreparedContent {
    Image(DecodedImage),
    Font {
        data: Arc<FontData>,
        bytes: Arc<Vec<u8>>,
    },
}

pub struct PreparedFile {
    content: PreparedContent,
    source: Option<String>,
}

impl PreparedFile {
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn release(self, cx: &mut App) {
        if let PreparedContent::Image(image) = self.content {
            release_image_assets(image, cx);
        }
    }
}

#[derive(Clone)]
enum RenderedContent {
    Image(DecodedImage),
    Font(Arc<FontData>),
}

pub struct FileRenderer {
    id: SharedString,
    content: Option<RenderedContent>,
    image_view: ImageViewState,
}

impl FileRenderer {
    pub fn kind_for_path(path: &Path) -> Option<FileRendererKind> {
        if let Some(format) = image_format_for_path(path) {
            Some(FileRendererKind::Image {
                is_svg: format == ImageFormat::Svg,
            })
        } else if font_format_for_path(path).is_some() {
            Some(FileRendererKind::Font)
        } else {
            None
        }
    }

    pub fn supports_path(path: &Path) -> bool {
        Self::kind_for_path(path).is_some()
    }

    pub fn prepare(
        path: &Path,
        bytes: Vec<u8>,
        svg_renderer: &SvgRenderer,
    ) -> Result<PreparedFile, String> {
        if bytes.len() as u64 > MAX_RENDERED_FILE_SIZE {
            return Err(format!(
                "File too large ({:.1} MB). Maximum size is 20 MB.",
                bytes.len() as f64 / 1024.0 / 1024.0
            ));
        }
        if image_format_for_path(path).is_some() {
            prepare_image(path, bytes, svg_renderer)
        } else if font_format_for_path(path).is_some() {
            prepare_font(path, bytes)
        } else {
            Err("No preview is available for this binary file".to_string())
        }
    }

    pub fn new(
        id: impl Into<SharedString>,
        prepared: PreparedFile,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = install_prepared(prepared, cx);
        Self {
            id: id.into(),
            content: Some(content),
            image_view: ImageViewState::default(),
        }
    }

    pub fn replace(&mut self, prepared: PreparedFile, cx: &mut Context<Self>) {
        self.release_assets(cx);
        let is_image = matches!(prepared.content, PreparedContent::Image(_));
        self.content = Some(install_prepared(prepared, cx));
        if !is_image {
            self.image_view = ImageViewState::default();
        }
        cx.notify();
    }

    pub fn release_assets(&mut self, cx: &mut App) {
        if let Some(RenderedContent::Image(image)) = self.content.take() {
            release_image_assets(image, cx);
        }
    }

    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        if !matches!(self.content, Some(RenderedContent::Image(_))) {
            return;
        }
        let current = if self.image_view.auto_fit {
            1.0
        } else {
            self.image_view.zoom
        };
        self.image_view.set_zoom(current * factor);
        cx.notify();
        self.maybe_rerender_svg(cx);
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if !matches!(self.content, Some(RenderedContent::Image(_))) {
            return;
        }
        self.image_view.set_zoom(zoom);
        cx.notify();
        self.maybe_rerender_svg(cx);
    }

    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.image_view.reset_to_fit();
        cx.notify();
    }

    fn start_pan(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !matches!(self.content, Some(RenderedContent::Image(_))) {
            return;
        }
        self.image_view.is_panning = true;
        self.image_view.pan_anchor = Some(position);
        self.image_view.pan_anchor_offset = self.image_view.pan;
        cx.notify();
    }

    fn update_pan(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.image_view.is_panning {
            return;
        }
        let Some(anchor) = self.image_view.pan_anchor else {
            return;
        };
        let dx = position.x - anchor.x;
        let dy = position.y - anchor.y;
        if f32::from(dx) == 0.0 && f32::from(dy) == 0.0 {
            return;
        }
        if self.image_view.auto_fit {
            self.image_view.auto_fit = false;
            self.image_view.zoom = 1.0;
            self.image_view.pan_anchor_offset = Point::default();
        }
        self.image_view.pan = clamped_pan(self.image_view.pan_anchor_offset + point(dx, dy));
        cx.notify();
    }

    fn pan_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        self.image_view.pan = clamped_pan(self.image_view.pan + delta);
        cx.notify();
    }

    fn end_pan(&mut self, cx: &mut Context<Self>) {
        self.image_view.is_panning = false;
        self.image_view.pan_anchor = None;
        cx.notify();
    }

    fn set_background(&mut self, background: PreviewBackground, cx: &mut Context<Self>) {
        self.image_view.background = background;
        cx.notify();
    }

    fn maybe_rerender_svg(&mut self, cx: &mut Context<Self>) {
        let Some((bytes, target_scale)) = self.rerender_target() else {
            return;
        };
        let dispatched_bytes_id = Arc::as_ptr(&bytes) as usize;
        self.image_view.svg_rerender_in_flight = true;
        let svg_renderer = cx.svg_renderer();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    svg_renderer
                        .render_single_frame(&bytes, target_scale)
                        .map_err(|error| format!("Cannot re-rasterize SVG: {error}"))
                })
                .await;
            let Some(entity) = entity.upgrade() else {
                if let Ok(image) = result {
                    cx.update(|cx| cx.drop_image(image, None));
                }
                return;
            };
            entity.update(cx, |this, cx| {
                this.image_view.svg_rerender_in_flight = false;
                let mut rerender_again = false;
                match (result, this.content.as_mut()) {
                    (
                        Ok(new_image),
                        Some(RenderedContent::Image(DecodedImage::Rendered {
                            image,
                            rendered_scale,
                            svg_bytes,
                            ..
                        })),
                    ) if Arc::as_ptr(svg_bytes) as usize == dispatched_bytes_id => {
                        let old_image = std::mem::replace(image, new_image);
                        *rendered_scale = target_scale;
                        cx.drop_image(old_image, None);
                        rerender_again = true;
                        cx.notify();
                    }
                    (Ok(new_image), _) => cx.drop_image(new_image, None),
                    (
                        Err(_),
                        Some(RenderedContent::Image(DecodedImage::Rendered {
                            rendered_scale,
                            svg_bytes,
                            ..
                        })),
                    ) if Arc::as_ptr(svg_bytes) as usize == dispatched_bytes_id => {
                        *rendered_scale = target_scale;
                    }
                    (Err(_), _) => {}
                }
                if rerender_again {
                    this.maybe_rerender_svg(cx);
                }
            });
        })
        .detach();
    }

    fn rerender_target(&self) -> Option<(Arc<Vec<u8>>, f32)> {
        if self.image_view.auto_fit || self.image_view.svg_rerender_in_flight {
            return None;
        }
        let Some(RenderedContent::Image(DecodedImage::Rendered {
            svg_bytes,
            rendered_scale,
            width,
            height,
            ..
        })) = self.content.as_ref()
        else {
            return None;
        };
        let zoom = self.image_view.zoom;
        if zoom <= *rendered_scale * 1.1 {
            return None;
        }
        const MAX_RENDER_PIXELS: u64 = 256 * 1024 * 1024;
        let intrinsic = (*width as u64).saturating_mul(*height as u64).max(1);
        let max_by_pixels =
            ((MAX_RENDER_PIXELS as f64 / intrinsic as f64).sqrt() as f32 / 2.0).max(1.0);
        let target = (zoom * 1.25).clamp(1.0, 16.0).min(max_by_pixels);
        (target > *rendered_scale * 1.05).then(|| (svg_bytes.clone(), target))
    }

    fn render_image(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let Some(RenderedContent::Image(image)) = self.content.clone() else {
            return unavailable("Image not available", t, cx);
        };
        let (natural_width, natural_height) = image.dimensions();
        let view = self.image_view.clone();
        let (background, checker) = match view.background {
            PreviewBackground::Light => (0xFFFFFF, false),
            PreviewBackground::Dark => (0x111111, false),
            PreviewBackground::Checker => (0x808080, true),
        };
        let muted = t.text_muted;
        let image = img(image).with_fallback(move || {
            div()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.0))
                .text_color(rgb(muted))
                .child("Cannot decode image")
                .into_any_element()
        });
        let image = if view.auto_fit {
            image
                .object_fit(ObjectFit::Contain)
                .max_w_full()
                .max_h_full()
                .into_any_element()
        } else {
            image
                .object_fit(ObjectFit::Contain)
                .w(px(natural_width as f32 * view.zoom))
                .h(px(natural_height as f32 * view.zoom))
                .flex_shrink_0()
                .ml(view.pan.x)
                .mt(view.pan.y)
                .into_any_element()
        };
        let cursor = if view.auto_fit {
            CursorStyle::Arrow
        } else if view.is_panning {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        };
        let id = ElementId::Name(format!("{}-image", self.id).into());
        let mut container = div()
            .id(id)
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(rgb(background))
            .cursor(cursor);
        if checker {
            container = container.child(
                canvas(
                    |_, _, _| (),
                    |bounds, _, window, _| paint_checkerboard(bounds, window),
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        }
        container
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(17.0));
                let dx = f32::from(delta.x);
                let dy = f32::from(delta.y);
                if !dx.is_finite() || !dy.is_finite() {
                    return;
                }
                if event.modifiers.platform || event.modifiers.control {
                    if dy.abs() >= 0.5 {
                        this.zoom_by((dy / 250.0).exp(), cx);
                    }
                } else if !this.image_view.auto_fit {
                    let (pan_x, pan_y) = if event.modifiers.shift && dx.abs() < 0.5 {
                        (dy, 0.0)
                    } else {
                        (dx, dy)
                    };
                    if pan_x.abs() >= 0.5 || pan_y.abs() >= 0.5 {
                        this.pan_by(point(px(pan_x), px(pan_y)), cx);
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.fit(cx);
                    } else {
                        this.start_pan(event.position, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.update_pan(event.position, cx);
                } else if this.image_view.is_panning {
                    this.end_pan(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_pan(cx)),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(image),
            )
            .into_any_element()
    }

    fn render_toolbar(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.image_view.auto_fit {
            "Fit".to_string()
        } else {
            format!("{}%", (self.image_view.zoom * 100.0).round() as i32)
        };
        h_flex()
            .flex_shrink_0()
            .justify_end()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(4.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_header))
            .child(self.render_zoom_controls(label, t, cx))
            .child(self.render_background_toggle(t, cx))
    }

    fn render_zoom_controls(
        &self,
        label: String,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button =
            |suffix: &'static str, glyph: &'static str, factor: f32, cx: &mut Context<Self>| {
                div()
                    .id(ElementId::Name(format!("{}-{suffix}", self.id).into()))
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|style| style.bg(rgb(t.bg_hover)))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.zoom_by(factor, cx)))
                    .child(glyph)
            };
        h_flex()
            .gap(px(2.0))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .child(button("zoom-out", "−", 1.0 / 1.25, cx))
            .child(
                div()
                    .id(ElementId::Name(format!("{}-zoom-label", self.id).into()))
                    .cursor_pointer()
                    .min_w(px(48.0))
                    .text_align(TextAlign::Center)
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_muted))
                    .rounded(px(4.0))
                    .hover(|style| style.bg(rgb(t.bg_hover)))
                    .px(px(4.0))
                    .py(px(2.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.image_view.auto_fit {
                            this.set_zoom(1.0, cx);
                        } else {
                            this.fit(cx);
                        }
                    }))
                    .child(label),
            )
            .child(button("zoom-in", "+", 1.25, cx))
    }

    fn render_background_toggle(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let options = [
            (PreviewBackground::Checker, "Checker"),
            (PreviewBackground::Light, "Light"),
            (PreviewBackground::Dark, "Dark"),
        ];
        h_flex()
            .gap(px(2.0))
            .px(px(2.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .children(options.into_iter().map(|(background, label)| {
                let active = self.image_view.background == background;
                div()
                    .id(ElementId::Name(
                        format!("{}-background-{label}", self.id).into(),
                    ))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .when(active, |div| div.bg(rgb(t.bg_selection)))
                    .when(!active, |div| div.hover(|style| style.bg(rgb(t.bg_hover))))
                    .text_size(px(12.0))
                    .text_color(rgb(if active { t.text_primary } else { t.text_muted }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_background(background, cx);
                    }))
                    .child(label)
            }))
    }
}

impl Render for FileRenderer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        match self.content.as_ref() {
            Some(RenderedContent::Image(_)) => v_flex()
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(self.render_toolbar(&t, cx))
                .child(self.render_image(&t, cx))
                .into_any_element(),
            Some(RenderedContent::Font(data)) => render_font(data.clone(), &t, cx),
            None => unavailable("File preview not available", &t, cx),
        }
    }
}

fn image_format_for_path(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
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

fn font_format_for_path(path: &Path) -> Option<bool> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "ttf" | "otf" => Some(true),
        "woff" | "woff2" => Some(false),
        _ => None,
    }
}

fn image_format_from_content(format: image::ImageFormat) -> Option<ImageFormat> {
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

fn prepare_image(
    path: &Path,
    bytes: Vec<u8>,
    svg_renderer: &SvgRenderer,
) -> Result<PreparedFile, String> {
    let format = image_format_for_path(path)
        .ok_or_else(|| "No preview is available for this image".to_string())?;
    if format == ImageFormat::Svg {
        let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
            .map_err(|error| format!("Cannot decode SVG: {error}"))?;
        let size = tree.size();
        let width = size.width().ceil() as u64;
        let height = size.height().ceil() as u64;
        let pixels = width.saturating_mul(height);
        if pixels == 0 || pixels > MAX_SVG_PIXELS {
            return Err(format!(
                "SVG dimensions out of range ({width}×{height}). Max {} megapixels.",
                MAX_SVG_PIXELS / 1024 / 1024
            ));
        }
        let image = svg_renderer
            .render_single_frame(&bytes, 1.0)
            .map_err(|error| format!("Cannot decode SVG: {error}"))?;
        let bytes = Arc::new(bytes);
        let source = String::from_utf8(bytes.as_ref().clone()).ok();
        return Ok(PreparedFile {
            content: PreparedContent::Image(DecodedImage::Rendered {
                image,
                width: width as u32,
                height: height as u32,
                svg_bytes: bytes,
                rendered_scale: 1.0,
            }),
            source,
        });
    }

    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|error| format!("Cannot read image header: {error}"))?;
    let guessed = reader.format();
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("Cannot read image dimensions: {error}"))?;
    let pixels = (width as u64).saturating_mul(height as u64);
    if pixels == 0 || pixels > MAX_IMAGE_PIXELS {
        return Err(format!(
            "Image dimensions out of range ({width}×{height}). Max {} megapixels.",
            MAX_IMAGE_PIXELS / 1024 / 1024
        ));
    }
    let format = guessed
        .and_then(image_format_from_content)
        .unwrap_or(format);
    Ok(PreparedFile {
        content: PreparedContent::Image(DecodedImage::Raster {
            image: Arc::new(Image::from_bytes(format, bytes)),
            width,
            height,
        }),
        source: None,
    })
}

fn prepare_font(path: &Path, bytes: Vec<u8>) -> Result<PreparedFile, String> {
    if !font_format_for_path(path).unwrap_or(false) {
        return Err("WOFF/WOFF2 preview is not supported yet — only OTF and TTF are.".to_string());
    }
    let face = ttf_parser::Face::parse(&bytes, 0)
        .map_err(|error| format!("Cannot parse font: {error}"))?;
    let read_name = |name_id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .find(|name| name.name_id == name_id && name.to_string().is_some())
            .and_then(|name| name.to_string())
    };
    let family_name = read_name(ttf_parser::name_id::FAMILY).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
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
    let data = Arc::new(FontData {
        family_name,
        full_name,
        style,
        version: read_name(ttf_parser::name_id::VERSION).unwrap_or_default(),
        num_glyphs: face.number_of_glyphs(),
        units_per_em: face.units_per_em(),
        weight_class: face.weight().to_number(),
        is_italic: face.is_italic(),
    });
    Ok(PreparedFile {
        content: PreparedContent::Font {
            data,
            bytes: Arc::new(bytes),
        },
        source: None,
    })
}

fn install_prepared(prepared: PreparedFile, cx: &mut App) -> RenderedContent {
    match prepared.content {
        PreparedContent::Image(image) => RenderedContent::Image(image),
        PreparedContent::Font { data, bytes } => {
            register_font_bytes(cx, &bytes);
            RenderedContent::Font(data)
        }
    }
}

fn register_font_bytes(cx: &mut App, bytes: &Arc<Vec<u8>>) {
    use std::collections::HashSet;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};
    static REGISTERED: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();

    let registered = REGISTERED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.as_ref().hash(&mut hasher);
    let hash = hasher.finish();
    let mut registered = match registered.lock() {
        Ok(registered) => registered,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !registered.insert(hash) {
        return;
    }
    if let Err(error) = cx
        .text_system()
        .add_fonts(vec![std::borrow::Cow::Owned(bytes.as_ref().clone())])
    {
        log::warn!("Failed to register font with text system: {error}");
        registered.remove(&hash);
    }
}

fn release_image_assets(image: DecodedImage, cx: &mut App) {
    match image {
        DecodedImage::Raster { image, .. } => image.remove_asset(cx),
        DecodedImage::Rendered { image, .. } => cx.drop_image(image, None),
    }
}

fn clamped_pan(pan: Point<Pixels>) -> Point<Pixels> {
    const LIMIT: f32 = 10_000.0;
    point(
        px(f32::from(pan.x).clamp(-LIMIT, LIMIT)),
        px(f32::from(pan.y).clamp(-LIMIT, LIMIT)),
    )
}

fn paint_checkerboard(bounds: Bounds<Pixels>, window: &mut Window) {
    const TILE: f32 = 12.0;
    let light = Rgba {
        r: 0.62,
        g: 0.62,
        b: 0.62,
        a: 1.0,
    };
    let dark = Rgba {
        r: 0.42,
        g: 0.42,
        b: 0.42,
        a: 1.0,
    };
    window.paint_quad(fill(bounds, light));
    let columns = (f32::from(bounds.size.width) / TILE).ceil() as usize + 1;
    let rows = (f32::from(bounds.size.height) / TILE).ceil() as usize + 1;
    for row in 0..rows {
        for column in 0..columns {
            if (row + column) % 2 == 0 {
                continue;
            }
            window.paint_quad(fill(
                Bounds {
                    origin: point(
                        px(f32::from(bounds.origin.x) + column as f32 * TILE),
                        px(f32::from(bounds.origin.y) + row as f32 * TILE),
                    ),
                    size: size(px(TILE), px(TILE)),
                },
                dark,
            ));
        }
    }
}

fn unavailable<T: 'static>(message: &str, t: &ThemeColors, cx: &mut Context<T>) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(t.bg_secondary))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(message.to_string())
        .into_any_element()
}

fn render_font<T: 'static>(
    data: Arc<FontData>,
    t: &ThemeColors,
    cx: &mut Context<T>,
) -> AnyElement {
    let family: SharedString = data.family_name.clone().into();
    let sample = "The quick brown fox jumps over the lazy dog";
    let metadata = [
        ("Family", data.family_name.clone()),
        ("Full name", data.full_name.clone()),
        ("Style", data.style.clone()),
        ("Weight class", data.weight_class.to_string()),
        (
            "Italic",
            if data.is_italic { "yes" } else { "no" }.to_string(),
        ),
        ("Glyphs", data.num_glyphs.to_string()),
        ("Units per em", data.units_per_em.to_string()),
        (
            "Version",
            if data.version.is_empty() {
                "—".to_string()
            } else {
                data.version.clone()
            },
        ),
    ];
    div()
        .id("file-renderer-font-preview")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .bg(rgb(t.bg_secondary))
        .child(
            v_flex()
                .gap(px(32.0))
                .p(px(32.0))
                .child(v_flex().gap(px(20.0)).children(
                    [48.0, 32.0, 24.0, 18.0, 14.0].into_iter().map(|font_size| {
                        div()
                            .text_size(px(font_size))
                            .text_color(rgb(t.text_primary))
                            .font_family(family.clone())
                            .child(sample)
                    }),
                ))
                .child(div().h(px(1.0)).bg(rgb(t.border)).w_full())
                .child(
                    v_flex()
                        .gap(px(6.0))
                        .children(metadata.map(|(label, value)| {
                            h_flex()
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .w(px(120.0))
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_primary))
                                        .child(value),
                                )
                        })),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::FileRenderer;
    use std::path::Path;

    #[test]
    fn recognizes_supported_image_and_font_paths() {
        assert!(FileRenderer::supports_path(Path::new("photo.PNG")));
        assert!(FileRenderer::supports_path(Path::new("icon.svg")));
        assert!(FileRenderer::supports_path(Path::new("typeface.otf")));
        assert!(!FileRenderer::supports_path(Path::new("archive.zip")));
    }
}
