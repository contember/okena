use super::{
    FileRenderer, Pdf, PreparedContent, RenderRequest, RenderedContent, prepare, rasterize,
};
use gpui::{AppContext, TestAppContext, VisualTestContext, px, size};
use std::path::Path;

#[test]
#[ignore = "requires PDF_PROBE_PATH"]
fn probe_external_pdf() {
    let path = std::env::var_os("PDF_PROBE_PATH").expect("PDF_PROBE_PATH");
    let started = std::time::Instant::now();
    let bytes = std::fs::read(path).unwrap();
    eprintln!("read {} bytes: {:?}", bytes.len(), started.elapsed());
    let pdf = Pdf::new(bytes).unwrap();
    eprintln!(
        "parsed {} pages: {:?}",
        pdf.pages().len(),
        started.elapsed()
    );
    let dimensions: Vec<_> = pdf
        .pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect();
    eprintln!("page dimensions: {:?}", started.elapsed());
    let request = RenderRequest::new(0, dimensions[0], 1.0).unwrap();
    let raster = rasterize(&pdf, request).unwrap();
    eprintln!(
        "first page {}x{}: {:?}",
        raster.request.width,
        raster.request.height,
        started.elapsed()
    );
}

fn document_bytes() -> Vec<u8> {
    coloured_document_bytes("1 0 0")
}

fn coloured_document_bytes(colour: &str) -> Vec<u8> {
    let red = format!("{colour} rg 0 0 20 60 re f");
    let blue = "0 0 1 rg 0 0 80 40 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 60] /Resources << >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 80 40] /Rotate 90 /Resources << >> /Contents 6 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{red}\nendstream", red.len()),
        format!("<< /Length {} >>\nstream\n{blue}\nendstream", blue.len()),
    ];
    let mut pdf = "%PDF-1.7\n".to_string();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        offsets.len() + 1
    ));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    pdf.into_bytes()
}

#[test]
fn rasterizes_pages_with_rotation_white_paper_and_bgra_channels() {
    let prepared = prepare(document_bytes()).expect("valid PDF");
    let PreparedContent::Pdf(pdf) = prepared.content else {
        panic!("PDF preview expected");
    };
    assert_eq!(&*pdf.dimensions, &[(40.0, 60.0), (40.0, 80.0)]);
    let first = pdf.raster.unwrap();
    let pixels = first.image.as_bytes(0).unwrap();
    assert_eq!(&pixels[..4], &[0, 0, 255, 255]);
    assert_eq!(&pixels[30 * 4..31 * 4], &[255, 255, 255, 255]);
    let request = RenderRequest::new(1, pdf.dimensions[1], 2.0).unwrap();
    let second = rasterize(&pdf.document, request).unwrap();
    assert_eq!((request.width, request.height), (80, 160));
    assert!(
        second
            .image
            .as_bytes(0)
            .unwrap()
            .chunks_exact(4)
            .all(|p| p == [255, 0, 0, 255])
    );
}

#[test]
fn rejects_invalid_documents_and_page_requests() {
    assert!(prepare(b"not a PDF".to_vec()).is_err());
    for dimensions in [
        (0.0, 100.0),
        (f32::INFINITY, 100.0),
        (100.0, f32::NAN),
        (2_000_000.0, 10.0),
    ] {
        assert!(RenderRequest::new(0, dimensions, 1.0).is_err());
    }
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(RenderRequest::new(0, (100.0, 100.0), scale).is_err());
    }
    let pdf = Pdf::new(document_bytes()).unwrap();
    assert!(rasterize(&pdf, RenderRequest::new(2, (40.0, 60.0), 1.0).unwrap()).is_err());
}

#[test]
fn bounds_raster_memory_even_for_large_pages_and_zoom() {
    for dimensions in [
        (600.0, 800.0),
        (1_000_000.0, 1.0),
        (1_000_000.0, 1_000_000.0),
    ] {
        let request = RenderRequest::new(0, dimensions, 100.0).unwrap();
        assert!(u64::from(request.width) * u64::from(request.height) <= 8 * 1024 * 1024);
        assert!(request.width <= 8192 && request.height <= 8192);
        assert!(request.width > 0 && request.height > 0);
    }
}

#[gpui::test]
fn coalesces_page_and_zoom_changes_to_the_latest_request(cx: &mut TestAppContext) {
    let svg = cx.update(|cx| cx.svg_renderer());
    let prepared = FileRenderer::prepare(Path::new("sample.PDF"), document_bytes(), &svg).unwrap();
    assert!(prepared.source().is_none());
    let renderer = cx.new(|cx| FileRenderer::new("pdf-test", prepared, cx));
    renderer.update(cx, |renderer, cx| {
        renderer.pdf_viewport = Some((size(px(80.0), px(120.0)), 2.0));
        renderer.zoom_by(1.25, cx);
        assert_eq!(renderer.image_view.zoom, 2.5);
        renderer.change_pdf_page(1, cx);
        renderer.change_pdf_page(1, cx);
        renderer.set_zoom(3.0, cx);
        assert!(renderer.pdf_render_in_flight);
    });
    cx.run_until_parked();
    renderer.update(cx, |renderer, cx| {
        let Some(RenderedContent::Pdf(pdf)) = &renderer.content else {
            panic!("PDF preview expected");
        };
        let raster = pdf.raster.as_ref().unwrap();
        assert_eq!(raster.request.page, 1);
        assert_eq!((raster.request.width, raster.request.height), (240, 480));
        assert!(!renderer.pdf_render_in_flight);
        renderer.release_assets(cx);
    });
}

#[gpui::test]
fn discards_render_results_after_reload_or_release(cx: &mut TestAppContext) {
    let renderer =
        cx.new(|cx| FileRenderer::new("pdf-test", prepare(document_bytes()).unwrap(), cx));
    renderer.update(cx, |renderer, cx| {
        renderer.set_zoom(2.0, cx);
        renderer.replace(prepare(coloured_document_bytes("0 1 0")).unwrap(), cx);
        renderer.set_zoom(2.0, cx);
    });
    cx.run_until_parked();
    renderer.update(cx, |renderer, cx| {
        let Some(RenderedContent::Pdf(pdf)) = &renderer.content else {
            panic!("PDF preview expected");
        };
        assert_eq!(pdf.raster.as_ref().unwrap().request.page, 0);
        assert_eq!(
            &pdf.raster.as_ref().unwrap().image.as_bytes(0).unwrap()[..4],
            &[0, 255, 0, 255]
        );
        assert!(!renderer.pdf_render_in_flight);
        renderer.change_pdf_page(1, cx);
        renderer.release_assets(cx);
    });
    cx.run_until_parked();
    renderer.update(cx, |renderer, _| {
        assert!(renderer.content.is_none());
        assert!(!renderer.pdf_render_in_flight);
    });
}

#[gpui::test]
fn fits_the_viewport_and_changes_pages_through_the_toolbar(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    cx.update(|cx| {
        cx.set_global(okena_ui::theme::GlobalThemeProvider(|_| {
            okena_core::theme::DARK_THEME
        }));
    });
    let prepared = prepare(document_bytes()).unwrap();
    let (renderer, vcx) =
        cx.add_window_view(move |_, cx| FileRenderer::new("pdf-test", prepared, cx));
    let vcx: &mut VisualTestContext = vcx;
    vcx.run_until_parked();
    vcx.update(|window, cx| _ = window.draw(cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| _ = window.draw(cx));
    let bounds = vcx
        .debug_bounds("pdf-viewport")
        .expect("visible PDF viewport");
    renderer.update(vcx, |renderer, _| {
        let Some(RenderedContent::Pdf(pdf)) = &renderer.content else {
            panic!("PDF preview expected");
        };
        assert!(bounds.size.width > px(0.0) && bounds.size.height > px(0.0));
        assert_eq!(renderer.pdf_viewport.unwrap().0, bounds.size);
        let scale = renderer.pdf_display_scale(pdf);
        assert!(40.0 * scale <= f32::from(bounds.size.width) + 0.01);
        assert!(60.0 * scale <= f32::from(bounds.size.height) + 0.01);
        assert_eq!(pdf.raster.as_ref().unwrap().request, pdf.target);
    });
    let next = vcx.debug_bounds("pdf-next").expect("next page button");
    vcx.simulate_click(next.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    renderer.update(vcx, |renderer, cx| {
        let Some(RenderedContent::Pdf(pdf)) = &renderer.content else {
            panic!("PDF preview expected");
        };
        assert_eq!(pdf.page, 1);
        assert_eq!(pdf.raster.as_ref().unwrap().request.page, 1);
        renderer.release_assets(cx);
    });
}
