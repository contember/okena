//! Generic wrapper for hosting a modal overlay in a separate OS window.
//!
//! `DetachedOverlayView<T, E>` wraps any overlay entity that fills its parent
//! and emits a `CloseEvent`-compatible event (e.g. `FileViewer`, `DiffViewer`).
//! The wrapper itself adds no chrome — the wrapped overlay's own header is
//! responsible for rendering window-move drag area and minimize/maximize
//! buttons (using helpers in `okena_ui::modal`). When the wrapped entity
//! emits `Close`, the wrapper closes its window.

use gpui::*;
use okena_ui::overlay::CloseEvent;
use okena_ui::theme::theme;
use std::marker::PhantomData;

pub struct DetachedOverlayView<T: Render + 'static, E: 'static> {
    content: Entity<T>,
    #[allow(dead_code)]
    title: SharedString,
    focus_handle: FocusHandle,
    pending_focus: bool,
    should_close: bool,
    _phantom: PhantomData<E>,
}

impl<T, E> DetachedOverlayView<T, E>
where
    T: Render + Focusable + EventEmitter<E> + 'static,
    E: CloseEvent + 'static,
{
    pub fn new(
        content: Entity<T>,
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Close the window when the wrapped content emits Close.
        cx.subscribe(&content, |this, _, event: &E, cx| {
            if event.is_close() {
                this.should_close = true;
                cx.notify();
            }
        })
        .detach();

        // Repaint when the wrapped content notifies (since we render it inside
        // our hierarchy, GPUI doesn't otherwise know to re-run our render fn).
        cx.observe(&content, |_, _, cx| {
            cx.notify();
        })
        .detach();

        // Persist window bounds + state (windowed/maximized/fullscreen) whenever
        // they change so the next detached window opens the same way.
        cx.observe_window_bounds(window, |_this, window, cx| {
            use okena_workspace::settings::{DetachedWindowBounds, DetachedWindowState};
            let wb = window.window_bounds();
            let bounds = wb.get_bounds();
            let state = match wb {
                WindowBounds::Windowed(_) => DetachedWindowState::Windowed,
                WindowBounds::Maximized(_) => DetachedWindowState::Maximized,
                WindowBounds::Fullscreen(_) => DetachedWindowState::Fullscreen,
            };
            let snapshot = DetachedWindowBounds {
                origin_x: f32::from(bounds.origin.x),
                origin_y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                state,
            };
            if let Some(global) = cx.try_global::<crate::settings::GlobalSettings>() {
                global.0.clone().update(cx, |state, cx| {
                    state.set_detached_overlay_bounds(snapshot, cx);
                });
            }
        })
        .detach();

        Self {
            content,
            title: title.into(),
            focus_handle,
            pending_focus: true,
            should_close: false,
            _phantom: PhantomData,
        }
    }
}

impl<T, E> Render for DetachedOverlayView<T, E>
where
    T: Render + Focusable + EventEmitter<E> + 'static,
    E: CloseEvent + 'static,
{
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.should_close {
            window.remove_window();
            return div().into_any_element();
        }

        // Hand initial focus to the wrapped content so its key_context activates.
        if self.pending_focus {
            let inner = self.content.read(cx).focus_handle(cx);
            window.focus(&inner, cx);
            self.pending_focus = false;
        }

        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        div()
            .id("detached-overlay-root")
            .track_focus(&focus_handle)
            .key_context("DetachedOverlay")
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(
                AnyView::from(self.content.clone())
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }
}

impl<T, E> Focusable for DetachedOverlayView<T, E>
where
    T: Render + Focusable + EventEmitter<E> + 'static,
    E: CloseEvent + 'static,
{
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
