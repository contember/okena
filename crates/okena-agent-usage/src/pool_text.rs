//! The pool's values in the status bar's words, so the desktop and the CLI
//! read alike.

use okena_core::api::{ApiPoolAmount, ApiPoolNonstop};

const MINUTE_MS: i64 = 60 * 1000;

fn seconds_to_ms(seconds: f64) -> i64 {
    (seconds * 1000.0) as i64
}

/// "45m", "3h10m", "26h", "3d".
pub fn short_duration(ms: i64) -> String {
    let minutes = (ms / MINUTE_MS).max(1);
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 10 * 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else if minutes < 48 * 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (24 * 60))
    }
}

/// Today's soft limit from `ApiPoolToday::left`: "3h00m left" or "45m over".
pub fn today(left: ApiPoolAmount) -> String {
    let left = seconds_to_ms(left.work_seconds);
    if left >= 0 {
        format!("{} left", short_duration(left))
    } else {
        format!("{} over", short_duration(-left))
    }
}

/// The hard limit: "3h10m", "7d+", "out, 2h" or "out". `None` for a state
/// added by a newer daemon.
pub fn nonstop(nonstop: &ApiPoolNonstop, now_ms: i64) -> Option<String> {
    let text = match *nonstop {
        ApiPoolNonstop::Lasts { work_seconds } => short_duration(seconds_to_ms(work_seconds)),
        ApiPoolNonstop::BeyondHorizon { horizon_seconds } => {
            format!("{}+", short_duration(seconds_to_ms(horizon_seconds)))
        }
        ApiPoolNonstop::Blocked { until: Some(until) } => {
            format!("out, {}", short_duration(seconds_to_ms(until) - now_ms))
        }
        ApiPoolNonstop::Blocked { until: None } => "out".into(),
        ApiPoolNonstop::Unknown => return None,
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_duration_keeps_the_status_bar_narrow() {
        assert_eq!(short_duration(0), "1m");
        assert_eq!(short_duration(45 * 60 * 1000), "45m");
        assert_eq!(short_duration((3 * 60 + 10) * 60 * 1000), "3h10m");
        assert_eq!(short_duration(26 * 60 * 60 * 1000), "26h");
        assert_eq!(short_duration(7 * 24 * 60 * 60 * 1000), "7d");
    }

    #[test]
    fn today_reads_left_or_over() {
        let left = |hours: f64| ApiPoolAmount {
            work_seconds: hours * 3600.0,
            account_week_percent: 0.0,
        };
        assert_eq!(today(left(3.0)), "3h00m left");
        assert_eq!(today(left(-0.75)), "45m over");
    }

    #[test]
    fn nonstop_reads_each_state_and_skips_unknown_ones() {
        let now_ms = 1_000_000;
        let cases = [
            (
                ApiPoolNonstop::Lasts {
                    work_seconds: 3000.0,
                },
                "50m",
            ),
            (
                ApiPoolNonstop::BeyondHorizon {
                    horizon_seconds: 7.0 * 86400.0,
                },
                "7d+",
            ),
            (
                ApiPoolNonstop::Blocked {
                    until: Some(1000.0 + 2.0 * 3600.0),
                },
                "out, 2h00m",
            ),
            (ApiPoolNonstop::Blocked { until: None }, "out"),
        ];
        for (state, text) in cases {
            assert_eq!(nonstop(&state, now_ms).as_deref(), Some(text));
        }
        assert_eq!(nonstop(&ApiPoolNonstop::Unknown, now_ms), None);
    }
}
