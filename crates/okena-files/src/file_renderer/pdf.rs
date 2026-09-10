use super::{
    FileRenderer, PreparedContent, PreparedFile, PreviewBackground, RenderedContent, unavailable,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, ElementId, ImageSource, RenderImage, WeakEntity, canvas, div, point,
    px, rgb, size,
};
use gpui_component::{h_flex, v_flex};
use hayro::hayro_syntax::Pdf;
use num_traits::ToPrimitive;
use okena_core::theme::ThemeColors;
use std::path::Path;
use std::sync::Arc;

const MAX_RASTER_PIXELS: f32 = 8.0 * 1024.0 * 1024.0;
const MAX_RASTER_SIDE: f32 = 8192.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderRequest {
    page: usize,
    scale: f32,
    width: u16,
    height: u16,
}

impl RenderRequest {
    fn new(page: usize, dimensions: (f32, f32), scale: f32) -> Result<Self, String> {
        let (width, height) = dimensions;
        if !width.is_finite()
            || !height.is_finite()
            || !(1.0..=1_000_000.0).contains(&width)
            || !(1.0..=1_000_000.0).contains(&height)
            || !scale.is_finite()
            || scale <= 0.0
        {
            return Err("PDF page dimensions are out of range".into());
        }
        let scale = scale
            .min((MAX_RASTER_PIXELS / (width * height)).sqrt())
            .min(MAX_RASTER_SIDE / width.max(height));
        let raster_dimension = |value: f32| {
            (value * scale)
                .floor()
                .max(1.0)
                .to_u16()
                .ok_or_else(|| "PDF raster dimensions are out of range".to_string())
        };
        Ok(Self {
            page,
            scale,
            width: raster_dimension(width)?,
            height: raster_dimension(height)?,
        })
    }
}

#[derive(Clone)]
struct PdfRaster {
    request: RenderRequest,
    image: Arc<RenderImage>,
}

#[derive(Clone)]
pub(super) struct PdfPreview {
    document: Arc<Pdf>,
    dimensions: Arc<Vec<(f32, f32)>>,
    page: usize,
    raster: Option<PdfRaster>,
    target: RenderRequest,
    failed: Option<RenderRequest>,
    error: Option<String>,
}

impl PdfPreview {
    pub(super) fn release(self, cx: &mut App) {
        if let Some(raster) = self.raster {
            cx.drop_image(raster.image, None);
        }
    }
}

pub(super) fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

pub(super) fn prepare(bytes: Vec<u8>) -> Result<PreparedFile, String> {
    let document =
        Arc::new(Pdf::new(bytes).map_err(|error| format!("Cannot open PDF: {error:?}"))?);
    let dimensions: Vec<_> = document
        .pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect();
    let first = dimensions
        .first()
        .copied()
        .ok_or_else(|| "PDF has no pages".to_string())?;
    let target = RenderRequest::new(0, first, 1.0)?;
    let raster = rasterize(&document, target)?;
    Ok(PreparedFile {
        content: PreparedContent::Pdf(PdfPreview {
            document,
            dimensions: Arc::new(dimensions),
            page: 0,
            raster: Some(raster),
            target,
            failed: None,
            error: None,
        }),
        source: None,
    })
}

fn rasterize(document: &Pdf, request: RenderRequest) -> Result<PdfRaster, String> {
    let page = document
        .pages()
        .get(request.page)
        .ok_or_else(|| "PDF page not found".to_string())?;
    // Hayro's borrowing, non-Send cache stays inside this synchronous worker invocation.
    let cache = hayro::RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &Default::default(),
        &hayro::RenderSettings {
            x_scale: request.scale,
            y_scale: request.scale,
            width: Some(request.width),
            height: Some(request.height),
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    let mut pixels = pixmap.data_as_u8_slice().to_vec();
    // GPUI consumes BGRA; the white paper makes every pixel opaque.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer =
        image::RgbaImage::from_raw(u32::from(request.width), u32::from(request.height), pixels)
            .ok_or_else(|| "Invalid PDF raster buffer".to_string())?;
    Ok(PdfRaster {
        request,
        image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
    })
}

impl FileRenderer {
    fn pdf_display_scale(&self, pdf: &PdfPreview) -> f32 {
        if !self.image_view.auto_fit {
            return self.image_view.zoom;
        }
        let (width, height) = pdf.dimensions[pdf.page];
        self.pdf_viewport.map_or(1.0, |(viewport, _)| {
            (f32::from(viewport.width) / width).min(f32::from(viewport.height) / height)
        })
    }

    pub(super) fn zoom_pdf_by(&mut self, factor: f32, cx: &mut Context<Self>) -> bool {
        let Some(RenderedContent::Pdf(pdf)) = &self.content else {
            return false;
        };
        let zoom = self.pdf_display_scale(pdf) * factor;
        if zoom.is_finite() && zoom > 0.0 {
            self.set_zoom(zoom, cx);
        }
        true
    }

    pub fn change_pdf_page(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(RenderedContent::Pdf(pdf)) = &mut self.content else {
            return;
        };
        let page = pdf
            .page
            .saturating_add_signed(delta)
            .min(pdf.dimensions.len() - 1);
        if page == pdf.page {
            return;
        }
        pdf.page = page;
        pdf.error = None;
        pdf.failed = None;
        if let Some(raster) = pdf.raster.take() {
            cx.drop_image(raster.image, None);
        }
        self.image_view.pan = point(px(0.0), px(0.0));
        self.image_view.is_panning = false;
        self.image_view.pan_anchor = None;
        self.maybe_render_pdf(cx);
        cx.notify();
    }

    pub(super) fn maybe_render_pdf(&mut self, cx: &mut Context<Self>) {
        let Some(RenderedContent::Pdf(pdf)) = &self.content else {
            return;
        };
        let scale = self.pdf_display_scale(pdf) * self.pdf_viewport.map_or(1.0, |(_, scale)| scale);
        let target = RenderRequest::new(pdf.page, pdf.dimensions[pdf.page], scale);
        let Some(RenderedContent::Pdf(pdf)) = &mut self.content else {
            return;
        };
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                pdf.error = Some(error);
                return;
            }
        };
        pdf.target = target;
        if self.pdf_render_in_flight
            || pdf
                .raster
                .as_ref()
                .is_some_and(|raster| raster.request == target)
            || pdf.failed == Some(target)
        {
            return;
        }
        pdf.error = None;
        self.pdf_render_in_flight = true;
        let document = pdf.document.clone();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let render_document = document.clone();
            let result = cx
                .background_executor()
                .spawn(async move { rasterize(&render_document, target) })
                .await;
            let Some(entity) = entity.upgrade() else {
                if let Ok(raster) = result {
                    cx.update(|cx| cx.drop_image(raster.image, None));
                }
                return;
            };
            entity.update(cx, |this, cx| {
                this.pdf_render_in_flight = false;
                match (this.content.as_mut(), result) {
                    (Some(RenderedContent::Pdf(pdf)), result)
                        if Arc::ptr_eq(&pdf.document, &document)
                            && pdf.target == target
                            && pdf.page == target.page =>
                    {
                        match result {
                            Ok(raster) => {
                                if let Some(old) = pdf.raster.replace(raster) {
                                    cx.drop_image(old.image, None);
                                }
                                pdf.error = None;
                                pdf.failed = None;
                            }
                            Err(error) => {
                                pdf.error = Some(error);
                                pdf.failed = Some(target);
                            }
                        }
                    }
                    (_, Ok(raster)) => cx.drop_image(raster.image, None),
                    (_, Err(_)) => {}
                }
                this.maybe_render_pdf(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_pdf(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let Some(RenderedContent::Pdf(pdf)) = &self.content else {
            return unavailable("PDF not available", t, cx);
        };
        let label = if self.image_view.auto_fit {
            "Fit".to_string()
        } else {
            format!("{:.0}%", self.image_view.zoom * 100.0)
        };
        let page_button = |suffix: &'static str,
                           label: &str,
                           delta: isize,
                           enabled: bool,
                           cx: &mut Context<Self>| {
            div()
                .id(ElementId::Name(format!("{}-{suffix}", self.id).into()))
                .debug_selector(move || suffix.to_string())
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .text_color(rgb(t.text_muted))
                .when(enabled, |d| {
                    d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
                })
                .when(!enabled, |d| d.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if enabled {
                        this.change_pdf_page(delta, cx);
                    }
                }))
                .child(label.to_string())
        };
        let toolbar = h_flex()
            .flex_shrink_0()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(4.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_header))
            .text_size(px(12.0))
            .child(page_button(
                "pdf-previous",
                "Previous",
                -1,
                pdf.page > 0,
                cx,
            ))
            .child(div().text_color(rgb(t.text_primary)).child(format!(
                "{} / {}",
                pdf.page + 1,
                pdf.dimensions.len()
            )))
            .child(page_button(
                "pdf-next",
                "Next",
                1,
                pdf.page + 1 < pdf.dimensions.len(),
                cx,
            ))
            .child(div().flex_1())
            .when(self.pdf_render_in_flight, |d| {
                d.child(div().text_color(rgb(t.text_muted)).child("Rendering…"))
            })
            .child(self.render_zoom_controls(label, t, cx));
        let body = if let Some(error) = &pdf.error {
            unavailable(error, t, cx)
        } else if let Some(raster) = &pdf.raster {
            let (width, height) = pdf.dimensions[pdf.page];
            let scale = self.pdf_display_scale(pdf);
            self.render_bitmap(
                ImageSource::Render(raster.image.clone()),
                Some(size(px(width * scale), px(height * scale))),
                PreviewBackground::Dark,
                t,
                cx,
            )
        } else {
            unavailable("Rendering PDF page…", t, cx)
        };
        let entity = cx.entity().downgrade();
        let previous_viewport = self.pdf_viewport;
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(toolbar)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .debug_selector(|| "pdf-viewport".to_string())
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                let viewport = (bounds.size, window.scale_factor());
                                if previous_viewport != Some(viewport)
                                    && bounds.size.width > px(0.0)
                                    && bounds.size.height > px(0.0)
                                {
                                    cx.defer(move |cx| {
                                        let _ = entity.update(cx, |this, cx| {
                                            if this.pdf_viewport != Some(viewport) {
                                                this.pdf_viewport = Some(viewport);
                                                this.maybe_render_pdf(cx);
                                                cx.notify();
                                            }
                                        });
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
