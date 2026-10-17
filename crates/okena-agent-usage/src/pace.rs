//! Pure pace / severity / working-day math shared by the desktop usage
//! widgets and (as reference) by other clients of the usage API.
//!
//! Everything here is gpui-free: the GPUI widgets in `okena-usage` re-export
//! it and add only theme colors and rendering.

// ============================================================================
// Severity
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

/// Linear time-elapsed percentage (0–100) of a window that resets at
/// `reset_epoch` and lasts `period_secs`, at `now`.
pub fn linear_time_pct(reset_epoch: f64, period_secs: f64, now: f64) -> Option<f64> {
    if period_secs <= 0.0 || !reset_epoch.is_finite() {
        return None;
    }
    let remaining = (reset_epoch - now).max(0.0);
    let elapsed = (period_secs - remaining).max(0.0);
    Some((elapsed / period_secs * 100.0).clamp(0.0, 100.0))
}

/// Severity from absolute utilization — how close to the hard cap.
pub fn abs_severity(pct: f64) -> Severity {
    if pct > 80.0 {
        Severity::Critical
    } else if pct > 60.0 {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// Severity from pace — how far ahead usage is of where it "should" be at this
/// point in the period. `Critical` means the user is burning budget fast enough
/// to run out before the period resets unless they slow down.
pub fn pace_severity(usage_pct: f64, time_pct: Option<f64>) -> Severity {
    match time_pct {
        Some(tp) if usage_pct > tp + 15.0 => Severity::Critical,
        Some(tp) if usage_pct > tp + 5.0 => Severity::Warning,
        _ => Severity::Normal,
    }
}

/// The worse of nearness-to-cap and burn pace — the headline severity the
/// popover percentage and the status-bar trigger both color from.
pub fn headline_severity(pct: f64, time_pct: Option<f64>) -> Severity {
    abs_severity(pct).max(pace_severity(pct, time_pct))
}

// ============================================================================
// Working days (shared setting)
// ============================================================================

/// Settings namespace for the shared usage preferences blob. Stored in the
/// generic `extension_settings` map (not tied to one extension) so both the
/// Claude and Codex widgets read the same value.
pub const USAGE_SETTINGS_KEY: &str = "usage";

/// The days of the week the user works, indexed `0 = Monday .. 6 = Sunday`.
///
/// Default (and the "I work every day" selection) is all seven days, which
/// reproduces the original behavior — the bar is not reshaped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorkingDays {
    pub days: [bool; 7],
}

impl WorkingDays {
    pub fn all() -> Self {
        Self { days: [true; 7] }
    }

    pub fn count(&self) -> usize {
        self.days.iter().filter(|&&d| d).count()
    }

    pub fn is_all(&self) -> bool {
        self.days.iter().all(|&d| d)
    }

    /// Parse from the persisted `{"working_days": [..]}` blob. An absent or
    /// empty list means "every day" (no tailoring).
    pub fn from_value(v: Option<&serde_json::Value>) -> Self {
        match v
            .and_then(|v| v.get("working_days"))
            .and_then(|a| a.as_array())
        {
            Some(arr) if !arr.is_empty() => {
                let mut days = [false; 7];
                for x in arr {
                    if let Some(i) = x.as_u64()
                        && (i as usize) < 7
                    {
                        days[i as usize] = true;
                    }
                }
                // A list that parsed to nothing valid falls back to all days.
                if days.iter().any(|&d| d) {
                    Self { days }
                } else {
                    Self::all()
                }
            }
            _ => Self::all(),
        }
    }

    /// Selected days as `0 = Monday .. 6 = Sunday` indices (the wire form).
    pub fn to_indices(&self) -> Vec<u8> {
        (0..7u8).filter(|&i| self.days[i as usize]).collect()
    }

    /// Inverse of [`Self::to_indices`]; out-of-range entries are ignored and an
    /// empty selection means every day.
    pub fn from_indices(indices: &[u8]) -> Self {
        let mut days = [false; 7];
        for &i in indices {
            if (i as usize) < 7 {
                days[i as usize] = true;
            }
        }
        if days.iter().any(|&d| d) {
            Self { days }
        } else {
            Self::all()
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        let idx: Vec<u64> = (0..7).filter(|&i| self.days[i]).map(|i| i as u64).collect();
        serde_json::json!({ "working_days": idx })
    }
}

// ============================================================================
// Reset-anchored grid + working-day reshaping
// ============================================================================

// ============================================================================
// Reset-anchored grid + working-day reshaping
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentUnit {
    Hour,
    Day,
}

/// Grid geometry for a usage bar.
#[derive(Clone, Default)]
pub struct Segments {
    /// Divider fractions in `(0, 1)`, excluding the ends.
    pub dividers: Vec<f32>,
    /// `(start, end)` fractions of the block that currently contains "now".
    pub current: Option<(f32, f32)>,
    /// When working-day reshaping is active, the time-elapsed fraction (0–100)
    /// recomputed on *working* time. `None` → the caller's own linear value is
    /// used instead.
    pub time_pct: Option<f64>,
    working_dates: Vec<jiff::civil::Date>,
}

impl Segments {
    /// Fraction (0–1) of the bar at `epoch`, on working time when reshaped.
    pub fn time_fraction(&self, epoch: f64, reset: f64, period: f64) -> f32 {
        if !self.working_dates.is_empty()
            && let Some(position) = working_day_position(&self.working_dates, epoch)
        {
            return (position.time_pct / 100.0) as f32;
        }
        ((epoch - (reset - period)) / period).clamp(0.0, 1.0) as f32
    }
}

pub fn epoch_of(z: &jiff::Zoned) -> f64 {
    z.timestamp().as_millisecond() as f64 / 1_000.0
}

/// Unix epoch seconds → local zoned datetime.
pub fn epoch_to_local(epoch: f64) -> Option<jiff::Zoned> {
    let ts = jiff::Timestamp::from_millisecond((epoch * 1_000.0) as i64).ok()?;
    Some(ts.to_zoned(jiff::tz::TimeZone::system()))
}

/// Current Unix time in seconds.
pub fn now_secs() -> Option<f64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

/// Compute reset-anchored grid boundaries for a usage bar, plus the block that
/// currently contains "now".
///
/// Boundaries land on the user's *actual* reset time-of-day — e.g. a Sunday
/// 12:00 weekly reset yields noon-to-noon day blocks — computed with calendar
/// arithmetic so they stay correct across DST and for any reset time.
///
/// When `working` is a strict subset of the week and the window is roughly a
/// week long, the weekly grid is **reshaped to one equal block per working
/// day** (N blocks instead of 7): non-working days are dropped from the axis,
/// and the returned [`Segments::time_pct`] advances only across working time so
/// the pace marker reflects the user's real schedule.
pub fn reset_aligned_segments(
    reset_epoch: f64,
    period_secs: f64,
    unit: SegmentUnit,
    working: WorkingDays,
) -> Segments {
    if period_secs <= 0.0 || reset_epoch <= 0.0 {
        return Segments::default();
    }
    let reset = match epoch_to_local(reset_epoch) {
        Some(z) => z,
        None => return Segments::default(),
    };
    let start_epoch = reset_epoch - period_secs;
    let now_epoch = now_secs().unwrap_or(start_epoch);

    // Reshape to working-day blocks only for ~weekly windows with a real subset
    // of days selected. Hour windows and longer/shorter day windows keep the
    // plain calendar grid.
    let day_units = (period_secs / 86_400.0).round() as i64;
    let remap = matches!(unit, SegmentUnit::Day)
        && working.count() >= 1
        && !working.is_all()
        && (2..=8).contains(&day_units);
    // If no working day lands in the window, `working_day_reshape` returns
    // `None` and we fall through to the plain grid below.
    if remap && let Some(seg) = working_day_reshape(start_epoch, reset_epoch, working, now_epoch) {
        return seg;
    }

    // Keep the grid legible on a thin bar: if a window spans more units than
    // this, step by a whole multiple (e.g. every 2 days), still anchored to the
    // reset time-of-day.
    let (unit_secs, max_blocks) = match unit {
        SegmentUnit::Hour => (3_600.0, 12.0),
        SegmentUnit::Day => (86_400.0, 14.0),
    };
    let units = (period_secs / unit_secs).round().max(1.0);
    let multiplier = (units / max_blocks).ceil().max(1.0) as i64;
    let step = match unit {
        SegmentUnit::Hour => jiff::Span::new().hours(multiplier),
        SegmentUnit::Day => jiff::Span::new().days(multiplier),
    };

    // Walk back from the reset, one block at a time, until the window start is
    // covered. `bounds` ends up holding boundary datetimes in ascending order.
    let mut bounds = vec![reset.clone()];
    let mut cursor = reset;
    for _ in 0..64 {
        cursor = match cursor.checked_sub(step) {
            Ok(z) => z,
            Err(_) => break,
        };
        let epoch = epoch_of(&cursor);
        bounds.push(cursor.clone());
        if epoch <= start_epoch {
            break;
        }
    }
    bounds.reverse();

    let frac = |epoch: f64| ((epoch - start_epoch) / period_secs) as f32;
    let dividers = bounds
        .iter()
        .map(|z| frac(epoch_of(z)))
        .filter(|&f| f > 0.001 && f < 0.999)
        .collect();
    let epochs: Vec<f64> = bounds.iter().map(epoch_of).collect();
    let current = epochs.windows(2).find_map(|w| {
        (now_epoch >= w[0] && now_epoch < w[1]).then(|| (frac(w[0]).max(0.0), frac(w[1]).min(1.0)))
    });
    Segments {
        dividers,
        current,
        time_pct: None,
        working_dates: Vec::new(),
    }
}

/// Reshape a ~weekly window to one equal block per **working calendar day**.
///
/// Blocks are keyed by calendar date (local midnight to midnight), not by the
/// reset time-of-day, so "today" always maps to today's block. With a reset at,
/// say, 13:00, a reset-anchored grid would put Thursday morning in the block
/// that *started* Wednesday — one block too early. Keying by date fixes that.
///
/// The window's two partial edge days together cover one weekday; we keep
/// whichever of them has more coverage, leaving exactly one date per weekday.
/// Working days are then laid out as equal segments in chronological order, and
/// the pace marker advances across working days only (frozen on off-days).
/// Returns `None` if no working day falls in the window (caller uses the plain
/// grid instead).
pub fn working_day_reshape(
    start_epoch: f64,
    reset_epoch: f64,
    working: WorkingDays,
    now_epoch: f64,
) -> Option<Segments> {
    let tz = jiff::tz::TimeZone::system();
    let start_date = epoch_to_local(start_epoch)?.date();
    let end_date = epoch_to_local(reset_epoch)?.date();

    // For each weekday keep the in-window calendar date with the most coverage,
    // so the two partial edge days collapse to a single weekday.
    let mut best: [Option<(jiff::civil::Date, f64)>; 7] = [None; 7];
    let mut d = start_date;
    for _ in 0..10 {
        let day_start = epoch_of(&d.to_zoned(tz.clone()).ok()?);
        let next = d.tomorrow().ok()?;
        let day_end = epoch_of(&next.to_zoned(tz.clone()).ok()?);
        let overlap = day_end.min(reset_epoch) - day_start.max(start_epoch);
        if overlap > 0.0 {
            let wd = (d.weekday().to_monday_zero_offset() as usize) % 7;
            if best[wd].is_none_or(|(_, ov)| overlap > ov) {
                best[wd] = Some((d, overlap));
            }
        }
        if d == end_date {
            break;
        }
        d = next;
    }

    // The working calendar dates, chronological.
    let mut dates: Vec<jiff::civil::Date> = (0..7)
        .filter(|&wd| working.days[wd])
        .filter_map(|wd| best[wd].map(|(date, _)| date))
        .collect();
    dates.sort();
    let n = dates.len();
    if n == 0 {
        return None;
    }

    let seg_w = 1.0_f32 / n as f32;
    let dividers: Vec<f32> = (1..n).map(|k| k as f32 * seg_w).collect();

    let position = working_day_position(&dates, now_epoch)?;
    Some(Segments {
        dividers,
        current: position.current,
        time_pct: Some(position.time_pct),
        working_dates: dates,
    })
}

struct WorkingDayPosition {
    current: Option<(f32, f32)>,
    time_pct: f64,
}

fn working_day_position(dates: &[jiff::civil::Date], now_epoch: f64) -> Option<WorkingDayPosition> {
    let tz = jiff::tz::TimeZone::system();
    let n = dates.len();
    let seg_w = 1.0_f32 / n as f32;
    let today = epoch_to_local(now_epoch)?.date();
    let (current, time_pct) = if let Some(k) = dates.iter().position(|&dt| dt == today) {
        let midnight = today
            .to_zoned(tz.clone())
            .ok()
            .map(|z| epoch_of(&z))
            .unwrap_or(now_epoch);
        let frac = (((now_epoch - midnight) / 86_400.0).clamp(0.0, 1.0)) as f32;
        let seg_start = k as f32 * seg_w;
        let tp = ((k as f32 + frac) / n as f32 * 100.0).clamp(0.0, 100.0) as f64;
        (Some((seg_start, seg_start + seg_w)), tp)
    } else {
        // Off-day (or outside the window): sit on the boundary after the last
        // elapsed working day; no current-block highlight.
        let elapsed = dates.iter().filter(|&&dt| dt < today).count();
        let tp = (elapsed as f64 / n as f64 * 100.0).clamp(0.0, 100.0);
        (None, tp)
    };

    Some(WorkingDayPosition { current, time_pct })
}

/// The time-elapsed fraction a row will actually use: the working-day-reshaped
/// value when reshaping applies, otherwise the caller's linear value. Lets the
/// status-bar trigger compute the same pace color the popover row shows.
pub fn effective_time_pct(
    reset_epoch: Option<f64>,
    period_secs: f64,
    unit: Option<SegmentUnit>,
    working: WorkingDays,
    linear: Option<f64>,
) -> Option<f64> {
    match (unit, reset_epoch) {
        (Some(u), Some(r)) => reset_aligned_segments(r, period_secs, u, working)
            .time_pct
            .or(linear),
        _ => linear,
    }
}

// ============================================================================
// Reset-time formatting (absolute, with smart labels)
// ============================================================================

/// Format a reset time (Unix epoch seconds) to a human-readable short form in
/// the local timezone, e.g. `today, 14:00 PST` / `Tue, 09:00 PST` /
/// `Jun 21, 09:00 PST`. Returns an empty string for an unset/invalid time.
pub fn format_reset_time_epoch(reset_epoch: f64, include_date: bool) -> String {
    if reset_epoch <= 0.0 {
        return String::new();
    }
    let Some(zoned) = epoch_to_local(reset_epoch) else {
        return String::new();
    };

    if include_date {
        let today = jiff::Zoned::now().date();
        let reset_date = zoned.date();

        let diff_days = today
            .until(reset_date)
            .ok()
            .map(|span| span.get_days())
            .unwrap_or(i32::MAX);

        let date_label = match diff_days {
            0 => Some("today"),
            1 => Some("tomorrow"),
            _ => None,
        };

        return match date_label {
            Some(label) => format!("{}, {}", label, zoned.strftime("%H:%M %Z")),
            None if (2..=6).contains(&diff_days) => zoned.strftime("%a, %H:%M %Z").to_string(),
            None => zoned.strftime("%b %-d, %H:%M %Z").to_string(),
        };
    }

    zoned.strftime("%H:%M %Z").to_string()
}

/// Pick the grid granularity for a window: per-hour up to a day, per-day for
/// longer windows. Sub-hour windows get no grid.
pub fn segment_unit_for_window(window_seconds: f64) -> Option<SegmentUnit> {
    if window_seconds <= 3600.0 {
        None
    } else if window_seconds <= 86400.0 {
        Some(SegmentUnit::Hour)
    } else {
        Some(SegmentUnit::Day)
    }
}

/// Short period badge for a window length (`5h`, `7d`, …).
pub fn format_window_label(window_seconds: f64) -> &'static str {
    let secs = window_seconds.max(0.0) as u64;
    match secs {
        0..=3600 => "1h",
        3601..=18000 => "5h",
        18001..=86400 => "1d",
        86401..=604800 => "7d",
        _ => "30d",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_days_default_is_all() {
        assert!(WorkingDays::from_value(None).is_all());
        let empty = serde_json::json!({ "working_days": [] });
        assert!(WorkingDays::from_value(Some(&empty)).is_all());
    }

    #[test]
    fn working_days_round_trip() {
        let wd = WorkingDays {
            days: [true, true, true, true, true, false, false], // Mon-Fri
        };
        assert_eq!(wd.count(), 5);
        assert!(!wd.is_all());
        let parsed = WorkingDays::from_value(Some(&wd.to_value()));
        assert_eq!(parsed, wd);
        assert_eq!(wd.to_indices(), vec![0, 1, 2, 3, 4]);
        assert_eq!(WorkingDays::from_indices(&wd.to_indices()), wd);
        assert!(WorkingDays::from_indices(&[]).is_all());
    }

    #[test]
    fn working_days_ignores_out_of_range() {
        let v = serde_json::json!({ "working_days": [0, 9, 3] });
        let wd = WorkingDays::from_value(Some(&v));
        assert!(wd.days[0] && wd.days[3]);
        assert_eq!(wd.count(), 2);
    }

    #[test]
    fn all_days_keeps_calendar_grid() {
        // All days selected → no reshaping → linear grid, no working-time override.
        let period = 7.0 * 86_400.0;
        let seg = reset_aligned_segments(
            1_700_000_000.0,
            period,
            SegmentUnit::Day,
            WorkingDays::all(),
        );
        assert!(seg.time_pct.is_none(), "all-days must not override pace");
        assert!(
            (5..=7).contains(&seg.dividers.len()),
            "expected ~6 dividers, got {}",
            seg.dividers.len()
        );
        assert!(seg.dividers.iter().all(|&f| f > 0.0 && f < 1.0));
        assert!(seg.dividers.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn five_working_days_reshape_to_five_blocks() {
        // Mon-Fri over a 7-day window → 5 equal blocks → 4 internal dividers,
        // and a working-time pace override.
        let period = 7.0 * 86_400.0;
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = reset_aligned_segments(1_700_000_000.0, period, SegmentUnit::Day, mon_fri);
        assert_eq!(
            seg.dividers.len(),
            4,
            "5 blocks → 4 dividers, got {:?}",
            seg.dividers
        );
        // Equal blocks at 1/5, 2/5, 3/5, 4/5.
        for (i, &d) in seg.dividers.iter().enumerate() {
            let expected = (i + 1) as f32 / 5.0;
            assert!(
                (d - expected).abs() < 1e-4,
                "divider {i} = {d}, expected {expected}"
            );
        }
        assert!(
            seg.time_pct.is_some(),
            "reshaping must provide a working-time pace"
        );
        let tp = seg.time_pct.unwrap();
        assert!((0.0..=100.0).contains(&tp));
    }

    #[test]
    fn reshape_marker_tracks_calendar_day_not_reset_time() {
        // Weekly window resetting Sat 2026-06-20 11:00Z, Mon–Fri working.
        // "Now" is Thursday 2026-06-18 09:00Z. Thursday is the 4th of 5 working
        // days, so the marker must land in the 4th block (second-to-last),
        // *not* the 3rd — the reset is at 11:00/13:00 local, but blocks key off
        // the calendar date, not the reset time-of-day.
        let reset = 1_781_953_200.0_f64; // 2026-06-20T11:00:00Z (Sat)
        let now = 1_781_773_200.0_f64; // 2026-06-18T09:00:00Z (Thu)
        let start = reset - 7.0 * 86_400.0;
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = working_day_reshape(start, reset, mon_fri, now).expect("reshape");
        assert_eq!(seg.dividers, vec![0.2, 0.4, 0.6, 0.8], "5 equal blocks");
        let (cs, ce) = seg
            .current
            .expect("Thursday is a working day → highlighted");
        assert!(
            (cs - 0.6).abs() < 1e-4 && (ce - 0.8).abs() < 1e-4,
            "Thursday must be the 4th block, got ({cs}, {ce})"
        );
        let tp = seg.time_pct.expect("working pace");
        assert!((60.0..80.0).contains(&tp), "marker mid-Thursday, got {tp}");
    }

    #[test]
    fn reshape_off_day_pegs_to_boundary() {
        // Same window, but "now" is Saturday 2026-06-20 09:00Z (an off-day,
        // after the whole Mon–Fri week): the marker pegs to 100% with no
        // current-block highlight.
        let reset = 1_781_953_200.0_f64;
        let now = 1_781_946_000.0_f64; // 2026-06-20T09:00:00Z (Sat)
        let start = reset - 7.0 * 86_400.0;
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = working_day_reshape(start, reset, mon_fri, now).expect("reshape");
        assert!(seg.current.is_none(), "off-day → no highlight");
        assert_eq!(seg.time_pct, Some(100.0), "all 5 working days elapsed");
    }

    #[test]
    fn hour_window_never_reshapes() {
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg =
            reset_aligned_segments(1_700_000_000.0, 5.0 * 3_600.0, SegmentUnit::Hour, mon_fri);
        assert!(seg.time_pct.is_none(), "hour windows keep linear pace");
    }

    #[test]
    fn guards_reject_bad_input() {
        let seg = reset_aligned_segments(0.0, 7.0 * 86_400.0, SegmentUnit::Day, WorkingDays::all());
        assert!(seg.dividers.is_empty() && seg.current.is_none());
        let seg =
            reset_aligned_segments(1_700_000_000.0, 0.0, SegmentUnit::Day, WorkingDays::all());
        assert!(seg.dividers.is_empty() && seg.current.is_none());
    }

    #[test]
    fn headline_severity_takes_worse_of_abs_and_pace() {
        // 65% absolute is only Warning, but far ahead of a 30% pace → Critical.
        // The popover row and the status-bar trigger both color from this same
        // max(abs, pace), so they can never show different colors for one metric.
        assert_eq!(
            headline_severity(65.0, Some(30.0)),
            Severity::Critical,
            "over-pace 65% must be critical"
        );
        assert_eq!(
            headline_severity(65.0, Some(64.0)),
            Severity::Warning,
            "on-pace 65% stays warning"
        );
    }

    #[test]
    fn linear_time_pct_midway() {
        assert_eq!(linear_time_pct(150.0, 100.0, 100.0), Some(50.0));
        assert_eq!(linear_time_pct(150.0, 100.0, 10.0), Some(0.0));
        assert_eq!(linear_time_pct(150.0, 100.0, 500.0), Some(100.0));
        assert_eq!(linear_time_pct(150.0, 0.0, 100.0), None);
    }

    #[test]
    fn window_units_and_labels() {
        assert!(segment_unit_for_window(0.0).is_none());
        assert!(segment_unit_for_window(3600.0).is_none());
        assert_eq!(
            segment_unit_for_window(5.0 * 3600.0),
            Some(SegmentUnit::Hour)
        );
        assert_eq!(segment_unit_for_window(86400.0), Some(SegmentUnit::Hour));
        assert_eq!(
            segment_unit_for_window(7.0 * 86400.0),
            Some(SegmentUnit::Day)
        );
        assert_eq!(format_window_label(18000.0), "5h");
        assert_eq!(format_window_label(604800.0), "7d");
    }

    #[test]
    fn format_reset_time_empty_for_unset() {
        assert_eq!(format_reset_time_epoch(0.0, true), "");
        assert_eq!(format_reset_time_epoch(-5.0, false), "");
    }

    #[test]
    fn format_reset_time_has_clock_and_tz() {
        let result = format_reset_time_epoch(1_700_000_000.0, false);
        assert!(result.contains(':'), "expected HH:MM, got {result}");
        assert!(!result.is_empty());
    }
}
