use crate::history::HistoryPoint;
use crate::{Segments, UsageRow, headline_color};
use gpui::prelude::*;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Sizable, h_flex, v_flex};
use okena_ui::theme::ThemeColors;
use okena_ui::tokens::ui_text_xs;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default)]
pub struct HistoryChartState {
    zoomed: HashMap<SharedString, f64>,
}

#[derive(Clone, Copy, Debug)]
struct ZoomRange {
    start: f64,
    end: f64,
    min_percent: f64,
    max_percent: f64,
}

impl ZoomRange {
    fn from_samples(samples: &[HistoryPoint]) -> Option<Self> {
        let first = samples.first()?;
        let last = samples.last()?;
        if last.recorded_at <= first.recorded_at {
            return None;
        }
        let min = samples
            .iter()
            .map(|p| p.used_percent)
            .fold(100.0, f64::min)
            .clamp(0.0, 100.0);
        let max = samples
            .iter()
            .map(|p| p.used_percent)
            .fold(0.0, f64::max)
            .clamp(0.0, 100.0);
        let span = (max - min + 5.0).clamp(10.0, 100.0);
        let min_percent = ((min + max - span) / 2.0).clamp(0.0, 100.0 - span).floor();
        let max_percent = (min_percent + span).ceil().min(100.0);
        Some(Self {
            start: first.recorded_at,
            end: last.recorded_at,
            min_percent,
            max_percent,
        })
    }

    fn project(&self, samples: &[HistoryPoint]) -> Vec<PlotPoint> {
        samples
            .iter()
            .filter(|sample| sample.recorded_at >= self.start && sample.recorded_at <= self.end)
            .map(|sample| PlotPoint {
                x: ((sample.recorded_at - self.start) / (self.end - self.start)) as f32,
                y: (sample.used_percent / 100.0).clamp(0.0, 1.0) as f32,
                recorded_at: sample.recorded_at,
            })
            .collect()
    }
}

#[derive(Clone)]
struct ActivityRegion {
    range: ZoomRange,
    left: f32,
    right: f32,
}

fn select_activity<'a>(
    regions: &'a [ActivityRegion],
    dividers: &[f32],
    x: f32,
) -> Option<&'a ActivityRegion> {
    if !(0.0..=1.0).contains(&x) {
        return None;
    }
    let start = dividers.iter().copied().rfind(|&d| d <= x).unwrap_or(0.0);
    let end = dividers.iter().copied().find(|&d| d > x).unwrap_or(1.0);
    regions
        .iter()
        .filter(|region| {
            if region.left == region.right {
                region.left >= start && (region.left < end || end == 1.0 && region.left == 1.0)
            } else {
                region.left < end && region.right > start
            }
        })
        .min_by(|a, b| {
            let distance =
                |region: &ActivityRegion| (region.left - x).max(x - region.right).max(0.0);
            distance(a).total_cmp(&distance(b))
        })
}

#[derive(Clone, Copy, Debug)]
struct PlotPoint {
    x: f32,
    y: f32,
    recorded_at: f64,
}

fn project(points: &[HistoryPoint], seg: &Segments, reset: f64, period: f64) -> Vec<PlotPoint> {
    points
        .iter()
        .map(|sample| PlotPoint {
            x: seg.time_fraction(sample.recorded_at, reset, period),
            y: (sample.used_percent / 100.0).clamp(0.0, 1.0) as f32,
            recorded_at: sample.recorded_at,
        })
        .collect()
}

struct InterpolatedPoint {
    y: f32,
    from: f64,
    to: f64,
}

fn interpolate(points: &[PlotPoint], x: f32) -> Option<InterpolatedPoint> {
    let first = points.first()?;
    let last = points.last()?;
    if x < first.x || x > last.x {
        return None;
    }
    let next = points.partition_point(|point| point.x <= x);
    let left = points[next.saturating_sub(1)];
    let Some(right) = points.get(next) else {
        return Some(InterpolatedPoint {
            y: left.y,
            from: left.recorded_at,
            to: left.recorded_at,
        });
    };
    let fraction = (x - left.x) / (right.x - left.x);
    Some(InterpolatedPoint {
        y: left.y + fraction * (right.y - left.y),
        from: left.recorded_at,
        to: right.recorded_at,
    })
}

pub(super) fn render(
    t: &ThemeColors,
    cx: &App,
    row: &UsageRow,
    seg: &Segments,
    samples: Vec<HistoryPoint>,
    now: f64,
    chart_state: &Entity<HistoryChartState>,
) -> impl IntoElement {
    let reset = row.reset_epoch.unwrap_or(now);
    let now_x = seg.time_fraction(now, reset, row.period_secs);
    let regions: Vec<_> = activity::growth_runs(&samples)
        .into_iter()
        .filter_map(|run| {
            let range = ZoomRange::from_samples(&samples[run])?;
            Some(ActivityRegion {
                left: seg.time_fraction(range.start, reset, row.period_secs),
                right: seg.time_fraction(range.end, reset, row.period_secs),
                range,
            })
        })
        .collect();
    let zoom = chart_state
        .read(cx)
        .zoomed
        .get(&row.marker_id)
        .and_then(|start| regions.iter().find(|region| region.range.start == *start))
        .map(|region| region.range);
    let points = match zoom {
        Some(range) => range.project(&samples),
        None => project(&samples, seg, reset, row.period_secs),
    };
    let tooltip_points = points.clone();
    let sample_count = points.len();
    let color = rgb(headline_color(t, row.pct, Some(now_x as f64 * 100.0)));
    let mut grid = rgb(t.text_muted);
    grid.a = 0.18;
    let mut guide = rgb(t.text_primary);
    guide.a = 0.4;
    let dividers = if zoom.is_some() {
        vec![0.25, 0.5, 0.75]
    } else {
        seg.dividers.clone()
    };
    let bounds_cell = Rc::new(Cell::new(None::<Bounds<Pixels>>));
    let tooltip_bounds = bounds_cell.clone();
    let chart_id = SharedString::from(format!("{}-history", row.marker_id));
    let toggle_id = SharedString::from(format!("{}-zoom", row.marker_id));
    let marker_id = row.marker_id.clone();
    let chart_state = chart_state.clone();
    let tooltip_dividers = seg.dividers.clone();
    let mut hover_color = rgb(t.text_primary);
    hover_color.a = 0.06;
    let boundaries: Vec<_> = std::iter::once(0.0)
        .chain(seg.dividers.iter().copied())
        .chain(std::iter::once(1.0))
        .collect();
    let clickable_cells: Vec<_> = boundaries
        .windows(2)
        .enumerate()
        .filter(|(_, cell)| {
            select_activity(&regions, &seg.dividers, (cell[0] + cell[1]) / 2.0).is_some()
        })
        .map(|(index, cell)| {
            let click_bounds = bounds_cell.clone();
            let click_state = chart_state.clone();
            let click_marker = marker_id.clone();
            let click_regions = regions.clone();
            let click_dividers = seg.dividers.clone();
            let center = (cell[0] + cell[1]) / 2.0;
            div()
                .id(SharedString::from(format!("{}-day-{index}", row.marker_id)))
                .absolute()
                .top_0()
                .h_full()
                .left(relative(cell[0]))
                .w(relative(cell[1] - cell[0]))
                .cursor_pointer()
                .hover(move |style| style.bg(hover_color))
                .tab_stop(true)
                .on_click(move |event, _, cx| {
                    let Some(bounds) = click_bounds.get() else {
                        return;
                    };
                    let x = match event {
                        ClickEvent::Keyboard(_) => center,
                        _ => {
                            f32::from(event.position().x - bounds.left())
                                / f32::from(bounds.size.width)
                        }
                    };
                    let Some(region) = select_activity(&click_regions, &click_dividers, x) else {
                        return;
                    };
                    cx.stop_propagation();
                    click_state.update(cx, |state, cx| {
                        state
                            .zoomed
                            .insert(click_marker.clone(), region.range.start);
                        cx.notify();
                    });
                })
        })
        .collect();
    let scale_label = match zoom {
        Some(range) => format!("Zoom · {:.0}–{:.0}%", range.min_percent, range.max_percent),
        None if !regions.is_empty() => "0–100% · click to zoom".into(),
        None => "Usage · 0–100%".into(),
    };
    let chart = Chart {
        points,
        dividers,
        now_x: zoom.is_none().then_some(now_x),
        y_min: zoom.map_or(0.0, |range| (range.min_percent / 100.0) as f32),
        y_max: zoom.map_or(1.0, |range| (range.max_percent / 100.0) as f32),
        color,
        grid,
        guide,
    };
    let plot = canvas(
        move |bounds, _, _| {
            bounds_cell.set(Some(bounds));
        },
        move |bounds, _, window, _| chart.paint(bounds, window),
    )
    .size_full();

    v_flex()
        .gap(px(2.0))
        .child(
            div()
                .id(chart_id)
                .w_full()
                .h(px(64.0))
                .relative()
                .child(plot)
                .when(zoom.is_none(), |el| el.children(clickable_cells))
                .tooltip(move |window, cx| {
                    let x = tooltip_bounds.get().map(|bounds| {
                        f32::from(window.mouse_position().x - bounds.left())
                            / f32::from(bounds.size.width)
                    });
                    let value = x.and_then(|x| interpolate(&tooltip_points, x));
                    let mut text = match value {
                        Some(p) if p.from == p.to => format!(
                            "{:.1}% · {}",
                            p.y * 100.0,
                            crate::format_reset_time_epoch(p.from, true)
                        ),
                        Some(p) => format!(
                            "{:.1}% (interpolated)\n{} – {}",
                            p.y * 100.0,
                            crate::format_reset_time_epoch(p.from, true),
                            crate::format_reset_time_epoch(p.to, true)
                        ),
                        None if zoom.is_some() => "Zoomed to recorded measurements".into(),
                        None => concat!(
                            "No measurements here yet\n",
                            "X: current period · Y: usage 0–100%\n",
                            "Diagonal: even pace · dashed usage: gap over 15 min",
                        )
                        .into(),
                    };
                    if zoom.is_none()
                        && let Some(region) =
                            x.and_then(|x| select_activity(&regions, &tooltip_dividers, x))
                    {
                        text = format!(
                            "Growth: {}\nClick to zoom this activity",
                            chart_time_range(region.range.start, region.range.end)
                        );
                    }
                    Tooltip::new(text).build(window, cx)
                }),
        )
        .child(
            h_flex()
                .justify_between()
                .text_size(ui_text_xs(cx))
                .text_color(rgb(t.text_muted))
                .child(scale_label)
                .child(
                    h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .child(match sample_count {
                            0 => "Collecting history".to_string(),
                            1 => "First measurement".to_string(),
                            n => format!("{n} measurements"),
                        })
                        .when(zoom.is_some(), |el| {
                            el.child(
                                Button::new(toggle_id)
                                    .ghost()
                                    .xsmall()
                                    .label("Full period")
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        chart_state.update(cx, |state, cx| {
                                            state.zoomed.remove(&marker_id);
                                            cx.notify();
                                        });
                                    }),
                            )
                        }),
                ),
        )
        .when_some(zoom, |el, range| {
            let first = samples
                .iter()
                .find(|p| p.recorded_at == range.start)
                .map_or(0.0, |p| p.used_percent);
            let last = samples
                .iter()
                .find(|p| p.recorded_at == range.end)
                .map_or(0.0, |p| p.used_percent);
            el.child(
                h_flex()
                    .justify_between()
                    .text_size(ui_text_xs(cx))
                    .text_color(rgb(t.text_muted))
                    .child(chart_time_range(range.start, range.end))
                    .child(format!(
                        "{first:.0} → {last:.0}% · {:.0} min",
                        (range.end - range.start) / 60.0
                    )),
            )
        })
}

fn chart_time_range(start: f64, end: f64) -> String {
    let (Some(start), Some(end)) = (crate::epoch_to_local(start), crate::epoch_to_local(end))
    else {
        return String::new();
    };
    let format = if start.date() == end.date() {
        "%H:%M"
    } else {
        "%b %-d %H:%M"
    };
    format!("{} – {}", start.strftime(format), end.strftime(format))
}

struct Chart {
    points: Vec<PlotPoint>,
    dividers: Vec<f32>,
    now_x: Option<f32>,
    y_min: f32,
    y_max: f32,
    color: Rgba,
    grid: Rgba,
    guide: Rgba,
}

impl Chart {
    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let xy = |x: f32, y: f32| {
            let y = (y - self.y_min) / (self.y_max - self.y_min);
            point(
                bounds.left() + bounds.size.width * x,
                bounds.top() + px(3.0) + (bounds.size.height - px(6.0)) * (1.0 - y),
            )
        };
        for y in [self.y_min, (self.y_min + self.y_max) / 2.0, self.y_max] {
            line(window, xy(0.0, y), xy(1.0, y), self.grid, 1.0);
        }
        for x in &self.dividers {
            line(
                window,
                xy(*x, self.y_min),
                xy(*x, self.y_max),
                self.grid,
                1.0,
            );
        }
        if let Some(now_x) = self.now_x {
            dashed_line(window, xy(0.0, 0.0), xy(1.0, 1.0), self.grid);
            line(window, xy(now_x, 0.0), xy(now_x, 1.0), self.guide, 1.0);
        }

        let (Some(first), Some(last)) = (self.points.first(), self.points.last()) else {
            return;
        };
        let mut fill = PathBuilder::fill();
        fill.move_to(xy(first.x, self.y_min));
        for p in &self.points {
            fill.line_to(xy(p.x, p.y));
        }
        fill.line_to(xy(last.x, self.y_min));
        fill.close();
        if let Ok(path) = fill.build() {
            let mut tint = self.color;
            tint.a = 0.1;
            window.paint_path(path, tint);
        }
        let mut stroke = PathBuilder::stroke(px(1.75));
        stroke.move_to(xy(first.x, first.y));
        for pair in self.points.windows(2) {
            let [left, right] = [pair[0], pair[1]];
            if right.recorded_at - left.recorded_at > 900.0 {
                dashed_line(window, xy(left.x, left.y), xy(right.x, right.y), self.color);
                stroke.move_to(xy(right.x, right.y));
            } else {
                stroke.line_to(xy(right.x, right.y));
            }
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, self.color);
        }
        window.paint_quad(
            gpui::fill(
                Bounds::new(
                    xy(last.x, last.y) - point(px(2.5), px(2.5)),
                    size(px(5.0), px(5.0)),
                ),
                self.color,
            )
            .corner_radii(px(2.5)),
        );
    }
}

fn line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Rgba, width: f32) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn dashed_line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Rgba) {
    let dx = f32::from(to.x - from.x);
    let dy = f32::from(to.y - from.y);
    let length = dx.hypot(dy);
    if length == 0.0 {
        return;
    }
    let at = |distance: f32| {
        point(
            from.x + px(dx * distance / length),
            from.y + px(dy * distance / length),
        )
    };
    let mut path = PathBuilder::stroke(px(1.25));
    let mut offset = 0.0;
    while offset < length {
        path.move_to(at(offset));
        path.line_to(at((offset + 3.0).min(length)));
        offset += 6.0;
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivityRegion, PlotPoint, ZoomRange, interpolate, project, select_activity};
    use crate::history::HistoryPoint;
    use crate::{Segments, WorkingDays, working_day_reshape};

    fn region(left: f32, right: f32, start: f64) -> ActivityRegion {
        ActivityRegion {
            left,
            right,
            range: ZoomRange {
                start,
                end: start + 3600.0,
                min_percent: 20.0,
                max_percent: 30.0,
            },
        }
    }

    #[test]
    fn clicking_a_day_selects_its_nearest_growth_not_another_day() {
        let regions = [
            region(0.22, 0.23, 1000.0),
            region(0.35, 0.37, 2000.0),
            region(0.62, 0.65, 3000.0),
        ];
        let dividers = [0.2, 0.4, 0.6, 0.8];
        assert_eq!(
            select_activity(&regions, &dividers, 0.25)
                .unwrap()
                .range
                .start,
            1000.0
        );
        assert_eq!(
            select_activity(&regions, &dividers, 0.39)
                .unwrap()
                .range
                .start,
            2000.0
        );
        assert_eq!(
            select_activity(&regions, &dividers, 0.79)
                .unwrap()
                .range
                .start,
            3000.0
        );
        assert!(select_activity(&regions, &dividers, 0.5).is_none());
        assert!(select_activity(&regions, &dividers, 0.9).is_none());
    }

    #[test]
    fn growth_spanning_a_divider_is_selectable_on_either_side() {
        let regions = [region(0.35, 0.45, 1000.0)];
        for x in [0.3, 0.4, 0.5] {
            assert!(select_activity(&regions, &[0.2, 0.4, 0.6, 0.8], x).is_some());
        }
    }

    #[test]
    fn boundaries_do_not_select_growth_from_the_previous_day() {
        assert!(select_activity(&[region(0.3, 0.4, 1000.0)], &[0.2, 0.4, 0.6, 0.8], 0.5).is_none());
        let collapsed = [region(1.0, 1.0, 2000.0)];
        assert_eq!(
            select_activity(&collapsed, &[0.2, 0.4, 0.6, 0.8], 0.99)
                .unwrap()
                .range
                .start,
            2000.0
        );
    }

    #[test]
    fn zoom_only_projects_the_selected_fragment() {
        let samples = [
            HistoryPoint {
                recorded_at: 1000.0,
                used_percent: 20.0,
            },
            HistoryPoint {
                recorded_at: 1300.0,
                used_percent: 21.0,
            },
            HistoryPoint {
                recorded_at: 2000.0,
                used_percent: 22.0,
            },
            HistoryPoint {
                recorded_at: 2300.0,
                used_percent: 23.0,
            },
        ];
        let zoom = ZoomRange::from_samples(&samples[2..]).unwrap();
        let points = zoom.project(&samples);
        assert_eq!(points.len(), 2);
        assert_eq!((points[0].x, points[1].x), (0.0, 1.0));
        assert_eq!(
            (points[0].recorded_at, points[1].recorded_at),
            (2000.0, 2300.0)
        );
    }

    #[test]
    fn zoom_expands_short_history_without_changing_tooltip_percentages() {
        let samples = [
            HistoryPoint {
                recorded_at: 1000.0,
                used_percent: 27.0,
            },
            HistoryPoint {
                recorded_at: 3220.0,
                used_percent: 28.0,
            },
            HistoryPoint {
                recorded_at: 5440.0,
                used_percent: 30.0,
            },
        ];
        let range = ZoomRange::from_samples(&samples).unwrap();
        let points = range.project(&samples);
        assert_eq!((points[0].x, points[1].x, points[2].x), (0.0, 0.5, 1.0));
        assert_eq!((range.start, range.end), (1000.0, 5440.0));
        assert!(range.min_percent < 27.0 && range.max_percent > 30.0);
        assert_eq!(range.max_percent - range.min_percent, 10.0);
        assert!((interpolate(&points, 0.5).unwrap().y * 100.0 - 28.0).abs() < 0.001);
    }

    #[test]
    fn zoom_handles_flat_usage_at_both_scale_edges() {
        for percent in [0.0, 50.0, 100.0] {
            let samples = [
                HistoryPoint {
                    recorded_at: 1000.0,
                    used_percent: percent,
                },
                HistoryPoint {
                    recorded_at: 1300.0,
                    used_percent: percent,
                },
            ];
            let range = ZoomRange::from_samples(&samples).unwrap();
            assert!(range.min_percent >= 0.0 && range.max_percent <= 100.0);
            assert!(range.min_percent <= percent && range.max_percent >= percent);
            assert_eq!(range.max_percent - range.min_percent, 10.0);
        }
    }

    #[test]
    fn zoom_requires_a_nonzero_time_span() {
        assert!(ZoomRange::from_samples(&[]).is_none());
        let point = HistoryPoint {
            recorded_at: 1000.0,
            used_percent: 27.0,
        };
        assert!(ZoomRange::from_samples(&[point]).is_none());
        assert!(ZoomRange::from_samples(&[point, point]).is_none());
    }

    #[test]
    fn session_uses_entire_window_and_fixed_percent_scale() {
        let points = project(
            &[
                HistoryPoint {
                    recorded_at: 4600.0,
                    used_percent: 10.0,
                },
                HistoryPoint {
                    recorded_at: 10000.0,
                    used_percent: 120.0,
                },
            ],
            &Segments::default(),
            19000.0,
            18000.0,
        );
        assert!((points[0].x - 0.2).abs() < 0.0001);
        assert!((points[0].y - 0.1).abs() < 0.0001);
        assert!((points[1].x - 0.5).abs() < 0.0001);
        assert_eq!(points[1].y, 1.0);
    }

    #[test]
    fn weekly_chart_matches_working_day_marker() {
        let reset = 1_781_953_200.0;
        let now = 1_781_773_200.0;
        let period = 7.0 * 86400.0;
        let working = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = working_day_reshape(reset - period, reset, working, now).unwrap();
        let points = project(
            &[HistoryPoint {
                recorded_at: now,
                used_percent: 35.0,
            }],
            &seg,
            reset,
            period,
        );
        assert!((points[0].x - seg.time_pct.unwrap() as f32 / 100.0).abs() < 0.0001);
        assert!((0.6..0.8).contains(&points[0].x));
    }

    #[test]
    fn interpolates_gaps_without_extrapolating() {
        let points = [
            PlotPoint {
                x: 0.2,
                y: 0.1,
                recorded_at: 100.0,
            },
            PlotPoint {
                x: 0.6,
                y: 0.5,
                recorded_at: 200.0,
            },
        ];
        let middle = interpolate(&points, 0.4).unwrap();
        assert!((middle.y - 0.3).abs() < 0.0001);
        assert_eq!((middle.from, middle.to), (100.0, 200.0));
        assert!(interpolate(&points, 0.1).is_none());
        assert!(interpolate(&points, 0.7).is_none());
        assert!(interpolate(&[], 0.5).is_none());
    }

    #[test]
    fn collapsed_off_days_keep_latest_value_at_boundary() {
        let points = [
            PlotPoint {
                x: 0.4,
                y: 0.1,
                recorded_at: 100.0,
            },
            PlotPoint {
                x: 0.4,
                y: 0.3,
                recorded_at: 200.0,
            },
            PlotPoint {
                x: 0.6,
                y: 0.5,
                recorded_at: 300.0,
            },
        ];
        assert_eq!(interpolate(&points, 0.4).unwrap().y, 0.3);
        assert_eq!(interpolate(&points[..1], 0.4).unwrap().y, 0.1);
    }
}
mod activity;
