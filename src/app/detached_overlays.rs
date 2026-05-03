//! Generic helper for opening overlay entities (file viewer, diff viewer, …)
//! in a separate OS window. Mirrors the pattern used for detached terminals
//! but works for any modal overlay type that emits a `CloseEvent`.

use crate::settings::settings;
use crate::views::overlays::detached_overlay::DetachedOverlayView;
use gpui::*;
use okena_ui::overlay::CloseEvent;

#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;

/// Open the given overlay entity in a fresh OS window with a thin chrome bar.
///
/// The wrapper subscribes to the entity's events; when it emits `Close`, the
/// window is closed automatically. The entity continues to live as long as
/// any window holds a reference to it.
///
/// Window bounds (position + size) are restored from the last detached overlay
/// the user opened, so the window doesn't reset to a tiny default every time.
pub fn open_detached_overlay<T, E>(
    title: impl Into<SharedString>,
    content: Entity<T>,
    cx: &mut App,
) where
    T: Render + Focusable + EventEmitter<E> + 'static,
    E: CloseEvent + 'static,
{
    let title = title.into();
    let window_bounds = match settings(cx).detached_overlay_bounds {
        Some(b) => {
            let bounds = Bounds {
                origin: Point::new(px(b.origin_x), px(b.origin_y)),
                size: Size {
                    width: px(b.width),
                    height: px(b.height),
                },
            };
            use okena_workspace::settings::DetachedWindowState;
            match b.state {
                DetachedWindowState::Windowed => WindowBounds::Windowed(bounds),
                DetachedWindowState::Maximized => WindowBounds::Maximized(bounds),
                DetachedWindowState::Fullscreen => WindowBounds::Fullscreen(bounds),
            }
        }
        None => WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(1400.0), px(900.0)),
        }),
    };

    cx.open_window(
        WindowOptions {
            // Match the main app window: on Windows we draw the entire chrome
            // ourselves (titlebar: None), so the OS doesn't add a system caption
            // bar above our header. Other platforms keep the transparent titlebar
            // for native traffic-lights / server-side decorations.
            titlebar: if cfg!(target_os = "windows") {
                None
            } else {
                Some(TitlebarOptions {
                    title: Some(title.clone()),
                    appears_transparent: true,
                    ..Default::default()
                })
            },
            window_bounds: Some(window_bounds),
            is_resizable: true,
            window_decorations: Some(if cfg!(target_os = "windows") {
                WindowDecorations::Client
            } else {
                WindowDecorations::Server
            }),
            window_min_size: Some(Size {
                width: px(400.0),
                height: px(300.0),
            }),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| {
                DetachedOverlayView::new(content.clone(), title.clone(), window, cx)
            });
            cx.new(|cx| Root::new(view, window, cx))
        },
    )
    .ok();
}
