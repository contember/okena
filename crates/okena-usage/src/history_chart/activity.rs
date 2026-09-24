use crate::history::HistoryPoint;
use std::ops::RangeInclusive;

const IDLE_SECONDS: f64 = 3600.0;
const MAX_SECONDS: f64 = 24.0 * 3600.0;

/// Flat edges stay inside a run only when another rise follows within an hour.
pub(super) fn growth_runs(samples: &[HistoryPoint]) -> Vec<RangeInclusive<usize>> {
    let mut runs = Vec::new();
    let mut current: Option<RangeInclusive<usize>> = None;
    for (left, pair) in samples.windows(2).enumerate() {
        let right = left + 1;
        let gap = pair[1].recorded_at - pair[0].recorded_at;
        let change = pair[1].used_percent - pair[0].used_percent;
        if gap > IDLE_SECONDS || change < 0.0 {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            continue;
        }
        if change <= 0.0 {
            continue;
        }
        if let Some(run) = &current {
            let idle = pair[0].recorded_at - samples[*run.end()].recorded_at;
            let duration = pair[1].recorded_at - samples[*run.start()].recorded_at;
            if (idle >= IDLE_SECONDS || duration > MAX_SECONDS)
                && let Some(run) = current.take()
            {
                runs.push(run);
            }
        }
        let start = current.as_ref().map_or(left, |run| *run.start());
        current = Some(start..=right);
    }
    if let Some(run) = current {
        runs.push(run);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::growth_runs;
    use crate::history::HistoryPoint;

    fn samples(values: &[(f64, f64)]) -> Vec<HistoryPoint> {
        values
            .iter()
            .map(|&(minutes, used_percent)| HistoryPoint {
                recorded_at: minutes * 60.0,
                used_percent,
            })
            .collect()
    }

    #[test]
    fn trims_idle_edges_and_keeps_short_plateaus() {
        let points = samples(&[
            (0.0, 10.0),
            (5.0, 10.0),
            (10.0, 11.0),
            (30.0, 11.0),
            (35.0, 12.0),
            (40.0, 12.0),
            (90.0, 12.0),
        ]);
        assert_eq!(growth_runs(&points), vec![1..=4]);
    }

    #[test]
    fn one_hour_without_growth_separates_runs() {
        let points = samples(&[
            (0.0, 10.0),
            (5.0, 11.0),
            (35.0, 11.0),
            (65.0, 11.0),
            (70.0, 12.0),
            (75.0, 13.0),
        ]);
        assert_eq!(growth_runs(&points), vec![0..=1, 3..=5]);
    }

    #[test]
    fn missing_night_and_decreases_do_not_join_growth() {
        let points = samples(&[
            (0.0, 10.0),
            (5.0, 11.0),
            (720.0, 15.0),
            (725.0, 16.0),
            (730.0, 5.0),
            (735.0, 6.0),
        ]);
        assert_eq!(growth_runs(&points), vec![0..=1, 2..=3, 4..=5]);
    }

    #[test]
    fn long_growth_is_split_at_twenty_four_hours() {
        let points: Vec<_> = (0..=50)
            .map(|i| HistoryPoint {
                recorded_at: i as f64 * 1800.0,
                used_percent: i as f64,
            })
            .collect();
        assert_eq!(growth_runs(&points), vec![0..=48, 48..=50]);
    }

    #[test]
    fn flat_history_and_single_samples_have_no_growth() {
        assert!(growth_runs(&[]).is_empty());
        assert!(growth_runs(&samples(&[(0.0, 20.0)])).is_empty());
        assert!(growth_runs(&samples(&[(0.0, 20.0), (5.0, 20.0)])).is_empty());
    }
}
