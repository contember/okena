//! Overlay scrollbar for `overflow_y_scroll` containers.
//!
//! Implemented as a raw `Element` rather than a styled `div` so it can read the
//! `ScrollHandle` during *its own* prepaint — a div would only ever see the
//! previous frame's geometry and stay invisible until something else redrew.

use crate::theme::{ThemeColors, with_alpha};
use gpui::*;
use std::cell::Cell;
use std::rc::Rc;

/// Width of the (invisible) hit area the thumb lives in.
const TRACK_WIDTH: Pixels = px(12.0);
const THUMB_WIDTH: Pixels = px(6.0);
const THUMB_HOVER_WIDTH: Pixels = px(8.0);
const THUMB_INSET: Pixels = px(3.0);
const MIN_THUMB: Pixels = px(28.0);

/// Thumb length and its top offset within the track.
///
/// `max_offset` is the container's `ScrollHandle::max_offset().y` (always >= 0)
/// and `offset_y` its current offset (always <= 0).
fn thumb_metrics(viewport: Pixels, max_offset: Pixels, offset_y: Pixels) -> (Pixels, Pixels) {
    let content = viewport + max_offset;
    let len = (viewport / content * viewport).max(MIN_THUMB).min(viewport);
    let progress = (-offset_y / max_offset).clamp(0.0, 1.0);
    (len, (viewport - len) * progress)
}

/// Scroll offset for a cursor `local_y` px down the track, grabbed `grab` px
/// into the thumb.
fn offset_for_cursor(
    local_y: Pixels,
    grab: Pixels,
    viewport: Pixels,
    thumb_len: Pixels,
    max_offset: Pixels,
) -> Pixels {
    let usable = viewport - thumb_len;
    if usable <= px(0.0) {
        return px(0.0);
    }
    -(max_offset * ((local_y - grab) / usable).clamp(0.0, 1.0))
}

#[derive(Clone, Copy, Default)]
struct State {
    /// Cursor offset inside the thumb while dragging, `None` when idle.
    grab: Option<Pixels>,
    hovered: bool,
}

#[derive(Clone, Default)]
struct SharedState(Rc<Cell<State>>);

/// Vertical overlay scrollbar for a `ScrollHandle`-tracked container.
///
/// Place it as the last child of a `relative()` parent that also holds the
/// scroll container; it pins itself to the parent's right edge and draws
/// nothing while the content fits.
pub fn vertical_scrollbar(
    id: impl Into<ElementId>,
    handle: &ScrollHandle,
    t: &ThemeColors,
) -> VerticalScrollbar {
    VerticalScrollbar {
        id: id.into(),
        handle: handle.clone(),
        thumb: with_alpha(t.scrollbar, 0.8),
        thumb_active: rgb(t.scrollbar_hover).into(),
    }
}

pub struct VerticalScrollbar {
    id: ElementId,
    handle: ScrollHandle,
    thumb: Hsla,
    thumb_active: Hsla,
}

#[doc(hidden)]
pub struct ScrollbarPrepaint {
    /// `None` while the content fits — nothing is painted or wired up.
    geometry: Option<Geometry>,
    state: SharedState,
}

struct Geometry {
    track: Bounds<Pixels>,
    /// Full-width grab area for the thumb.
    thumb_hit: Bounds<Pixels>,
    /// The visible (narrower) thumb.
    thumb_fill: Bounds<Pixels>,
    thumb_len: Pixels,
    max_offset: Pixels,
    color: Hsla,
    radius: Pixels,
}

impl IntoElement for VerticalScrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for VerticalScrollbar {
    type RequestLayoutState = ();
    type PrepaintState = ScrollbarPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: Position::Absolute,
            inset: Edges {
                top: px(0.0).into(),
                right: px(0.0).into(),
                bottom: px(0.0).into(),
                left: px(0.0).into(),
            },
            ..Default::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = window
            .use_state(cx, |_, _| SharedState::default())
            .read(cx)
            .clone();

        let viewport = bounds.size.height;
        let max_offset = self.handle.max_offset().y;
        if max_offset <= px(1.0) || viewport <= px(0.0) {
            return ScrollbarPrepaint {
                geometry: None,
                state,
            };
        }

        let (thumb_len, thumb_offset) = thumb_metrics(viewport, max_offset, self.handle.offset().y);
        let thumb_top = bounds.origin.y + thumb_offset;

        let track = Bounds {
            origin: point(
                bounds.origin.x + bounds.size.width - TRACK_WIDTH,
                bounds.origin.y,
            ),
            size: size(TRACK_WIDTH, viewport),
        };

        let active = state.0.get().hovered || state.0.get().grab.is_some();
        let width = if active {
            THUMB_HOVER_WIDTH
        } else {
            THUMB_WIDTH
        };
        let color = if active {
            self.thumb_active
        } else {
            self.thumb
        };

        let thumb_hit = Bounds {
            origin: point(track.origin.x, thumb_top),
            size: size(TRACK_WIDTH, thumb_len),
        };
        let thumb_fill = Bounds {
            origin: point(
                track.origin.x + TRACK_WIDTH - THUMB_INSET - width,
                thumb_top,
            ),
            size: size(width, thumb_len),
        };

        window.insert_hitbox(track, HitboxBehavior::Normal);

        ScrollbarPrepaint {
            geometry: Some(Geometry {
                track,
                thumb_hit,
                thumb_fill,
                thumb_len,
                max_offset,
                color,
                radius: width / 2.0,
            }),
            state,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(geo) = prepaint.geometry.take() else {
            return;
        };
        let state = prepaint.state.clone();
        let view_id = window.current_view();

        window.paint_quad(fill(geo.thumb_fill, geo.color).corner_radii(geo.radius));

        let Geometry {
            track,
            thumb_hit,
            thumb_len,
            max_offset,
            ..
        } = geo;

        // Maps a cursor position to a scroll offset, given where inside the
        // thumb the drag started.
        let handle = self.handle.clone();
        let scroll_to = move |y: Pixels, grab: Pixels| {
            let next = offset_for_cursor(
                y - track.origin.y,
                grab,
                track.size.height,
                thumb_len,
                max_offset,
            );
            handle.set_offset(point(handle.offset().x, next));
        };

        window.on_mouse_event({
            let state = state.clone();
            let scroll_to = scroll_to.clone();
            move |event: &MouseDownEvent, phase, _, cx| {
                if !phase.bubble() || !track.contains(&event.position) {
                    return;
                }
                cx.stop_propagation();
                let grab = if thumb_hit.contains(&event.position) {
                    event.position.y - thumb_hit.origin.y
                } else {
                    // Clicked the empty track: centre the thumb on the cursor.
                    thumb_len / 2.0
                };
                scroll_to(event.position.y, grab);
                let mut s = state.0.get();
                s.grab = Some(grab);
                state.0.set(s);
                cx.notify(view_id);
            }
        });

        window.on_mouse_event({
            let state = state.clone();
            move |event: &MouseMoveEvent, _, _, cx| {
                let mut s = state.0.get();
                if let Some(grab) = s.grab {
                    scroll_to(event.position.y, grab);
                    cx.notify(view_id);
                    return;
                }
                let hovered = track.contains(&event.position);
                if hovered != s.hovered {
                    s.hovered = hovered;
                    state.0.set(s);
                    cx.notify(view_id);
                }
            }
        });

        window.on_mouse_event({
            move |_: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() {
                    return;
                }
                let mut s = state.0.get();
                if s.grab.take().is_some() {
                    state.0.set(s);
                    cx.notify(view_id);
                }
            }
        });

        let _ = cx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::rust_2024::test;

    #[test]
    fn thumb_shrinks_with_content_and_tracks_the_offset() {
        let viewport = px(100.0);
        // Content is 4x the viewport -> quarter-height thumb, but never below MIN_THUMB.
        let (len, top) = thumb_metrics(viewport, px(300.0), px(0.0));
        assert_eq!(len, MIN_THUMB);
        assert_eq!(top, px(0.0));

        // Scrolled to the bottom the thumb sits flush against the track end.
        let (_, top) = thumb_metrics(viewport, px(300.0), px(-300.0));
        assert_eq!(top, viewport - MIN_THUMB);

        // Halfway.
        let (_, top) = thumb_metrics(viewport, px(300.0), px(-150.0));
        assert_eq!(top, (viewport - MIN_THUMB) / 2.0);
    }

    #[test]
    fn thumb_never_exceeds_the_track() {
        // A viewport shorter than MIN_THUMB must not produce an oversized thumb.
        let (len, top) = thumb_metrics(px(20.0), px(200.0), px(-200.0));
        assert_eq!(len, px(20.0));
        assert_eq!(top, px(0.0));
    }

    #[test]
    fn dragging_maps_cursor_to_offset_and_clamps() {
        let (viewport, thumb, max) = (px(100.0), px(28.0), px(300.0));
        // Grabbed at the thumb's top edge, dragged to the track's top.
        assert_eq!(
            offset_for_cursor(px(0.0), px(0.0), viewport, thumb, max),
            px(0.0)
        );
        // Dragged past the bottom -> clamped to the maximum scroll.
        assert_eq!(
            offset_for_cursor(px(500.0), px(0.0), viewport, thumb, max),
            -max
        );
        // Grab offset is subtracted, so the thumb keeps its position under the cursor.
        assert_eq!(
            offset_for_cursor(px(46.0), px(10.0), viewport, thumb, max),
            px(-150.0)
        );
    }

    #[test]
    fn no_scroll_range_yields_no_offset() {
        assert_eq!(
            offset_for_cursor(px(50.0), px(0.0), px(28.0), px(28.0), px(0.0)),
            px(0.0)
        );
    }
}
