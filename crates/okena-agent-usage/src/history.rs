use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Samples older than this are dropped when the file is pruned. Longer than
/// the longest window we chart (7 days) plus slack.
const RETAIN_SECS: f64 = 8.0 * 86400.0;
/// Prune once the oldest sample is this much older than [`RETAIN_SECS`], so a
/// rewrite happens about once a day rather than on every append.
const PRUNE_SLACK_SECS: f64 = 86400.0;

// Claude reset timestamps jitter by fractions of a second between fetches.
const RESET_TIME_TOLERANCE_SECS: f64 = 1.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Grok,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LimitSample {
    pub name: String,
    pub used_percent: f64,
    pub window_seconds: f64,
    pub reset_at: Option<f64>,
}

/// Version 1 timestamps are Unix epoch seconds (UTC).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageSample {
    pub version: u32,
    pub recorded_at: f64,
    pub provider: Provider,
    /// Codex account ID or Claude credential directory, never an access token.
    pub source: String,
    pub plan: Option<String>,
    pub limits: Vec<LimitSample>,
}

#[derive(Clone, Default)]
pub struct History(Arc<[UsageSample]>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistoryPoint {
    pub recorded_at: f64,
    pub used_percent: f64,
}

impl History {
    pub fn points(&self, name: &str, reset_at: f64, period: f64, now: f64) -> Vec<HistoryPoint> {
        if !reset_at.is_finite() || !period.is_finite() || period <= 0.0 {
            return Vec::new();
        }
        let start = reset_at - period;
        let mut points: Vec<_> = self
            .0
            .iter()
            .filter(|sample| sample.recorded_at >= start && sample.recorded_at <= now.min(reset_at))
            .filter_map(|sample| {
                let limit = sample.limits.iter().find(|limit| {
                    limit.name == name
                        && limit.reset_at.is_some_and(|reset| {
                            (reset - reset_at).abs() < RESET_TIME_TOLERANCE_SECS
                        })
                        && (limit.window_seconds - period).abs() < 0.001
                })?;
                Some(HistoryPoint {
                    recorded_at: sample.recorded_at,
                    used_percent: limit.used_percent,
                })
            })
            .collect();
        points.sort_by(|a, b| a.recorded_at.total_cmp(&b.recorded_at));
        // Two processes can sample at the same millisecond; keep the later append.
        points.reverse();
        points.dedup_by(|a, b| a.recorded_at == b.recorded_at);
        points.reverse();
        points
    }
}

impl From<Vec<UsageSample>> for History {
    fn from(samples: Vec<UsageSample>) -> Self {
        History(samples.into())
    }
}

/// Collapse plateaus to their first and last sample, then cap the count by
/// keeping the newest sample per time bucket. Lossless for a line chart until
/// the cap kicks in; used to keep the wire payload small.
pub fn compact_points(points: &[HistoryPoint], max: usize) -> Vec<HistoryPoint> {
    let n = points.len();
    let mut kept: Vec<HistoryPoint> = points
        .iter()
        .enumerate()
        .filter(|&(i, point)| {
            i == 0
                || i + 1 == n
                || points[i - 1].used_percent != point.used_percent
                || points[i + 1].used_percent != point.used_percent
        })
        .map(|(_, point)| *point)
        .collect();
    if max == 0 {
        return Vec::new();
    }
    if kept.len() <= max {
        return kept;
    }
    let (Some(first), Some(last)) = (kept.first().copied(), kept.last().copied()) else {
        return kept;
    };
    let span = last.recorded_at - first.recorded_at;
    if span <= 0.0 || max < 2 {
        kept.truncate(max);
        return kept;
    }
    // Bucket 0 holds only the first point; the rest share max - 1 buckets.
    let buckets = (max - 1) as f64;
    let mut out: Vec<HistoryPoint> = vec![first];
    let mut last_bucket = 0usize;
    for point in kept.into_iter().skip(1) {
        let bucket = (((point.recorded_at - first.recorded_at) / span * buckets).ceil() as usize)
            .clamp(1, max - 1);
        if bucket == last_bucket {
            if let Some(slot) = out.last_mut() {
                *slot = point;
            }
        } else {
            out.push(point);
            last_bucket = bucket;
        }
    }
    out
}

/// Call from the fetch worker: persistence failures must not hide fresh usage.
pub fn record_and_load(
    provider: Provider,
    source: String,
    plan: Option<String>,
    limits: Vec<LimitSample>,
) -> History {
    if limits.is_empty() {
        return History::default();
    }
    let sample = UsageSample {
        version: 1,
        recorded_at: jiff::Timestamp::now().as_millisecond() as f64 / 1000.0,
        provider,
        source,
        plan,
        limits,
    };
    let directory = okena_core::profiles::config_root().join("usage-history");
    if let Err(error) = append(&directory, &sample) {
        log::warn!("[usage-history] could not save {provider:?} usage: {error}");
    }
    let since = sample
        .limits
        .iter()
        .filter_map(|limit| limit.reset_at.map(|reset| reset - limit.window_seconds))
        .filter(|start| start.is_finite())
        .min_by(f64::total_cmp)
        .unwrap_or(sample.recorded_at);
    match load(&directory, provider, &sample.source, since) {
        Ok(mut samples) => {
            samples.push(sample);
            History(samples.into())
        }
        Err(error) => {
            log::warn!("[usage-history] could not load {provider:?} usage: {error}");
            History(Arc::from([sample]))
        }
    }
}

/// Record one sample per window and attach each window's in-window history
/// (compacted for the wire). The window `id` is the history series name.
pub fn record_windows(
    provider: Provider,
    source: String,
    plan: Option<String>,
    windows: &mut [okena_core::api::ApiUsageWindow],
) {
    let limits = windows
        .iter()
        .map(|window| LimitSample {
            name: window.id.clone(),
            used_percent: window.used_percent,
            window_seconds: window.window_seconds,
            reset_at: window.reset_at,
        })
        .collect();
    let history = record_and_load(provider, source, plan, limits);
    attach_history(
        &history,
        windows,
        jiff::Timestamp::now().as_millisecond() as f64 / 1000.0,
    );
}

/// Fill `window.history` from `history` for every window with a reset time.
pub fn attach_history(
    history: &History,
    windows: &mut [okena_core::api::ApiUsageWindow],
    now: f64,
) {
    for window in windows {
        let Some(reset) = window.reset_at else {
            continue;
        };
        let points = history.points(&window.id, reset, window.window_seconds, now);
        window.history = compact_points(&points, okena_core::api::USAGE_HISTORY_MAX_POINTS)
            .into_iter()
            .map(|point| okena_core::api::ApiUsagePoint {
                recorded_at: point.recorded_at,
                used_percent: point.used_percent,
            })
            .collect();
    }
}

fn filename(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude.jsonl",
        Provider::Codex => "codex.jsonl",
        Provider::Grok => "grok.jsonl",
    }
}

fn load(
    directory: &Path,
    provider: Provider,
    source: &str,
    since: f64,
) -> io::Result<Vec<UsageSample>> {
    let file = std::fs::File::open(directory.join(filename(provider)))?;
    file.lock_shared()?;
    let mut samples = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        // A truncated line or an unknown schema must not hide the remaining history.
        let Ok(sample) = serde_json::from_str::<UsageSample>(&line) else {
            continue;
        };
        if sample.version != 1
            || sample.provider != provider
            || sample.source != source
            || !sample.recorded_at.is_finite()
            || sample.recorded_at < since
            || sample.limits.iter().any(|limit| {
                !limit.used_percent.is_finite()
                    || limit.used_percent < 0.0
                    || !limit.window_seconds.is_finite()
                    || limit.window_seconds < 0.0
                    || limit.reset_at.is_some_and(|reset| !reset.is_finite())
            })
        {
            continue;
        }
        samples.push(sample);
    }
    Ok(samples)
}

fn append(directory: &Path, sample: &UsageSample) -> io::Result<()> {
    let mut line = serde_json::to_vec(sample)?;
    line.push(b'\n');
    fs::create_dir_all(directory)?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true) // Windows file locking requires more than append-only access.
        .append(true)
        .open(directory.join(filename(sample.provider)))?;
    // Multiple Okena profiles/processes share this history directory.
    file.lock()?;
    if let Err(error) = prune_if_stale(&mut file, sample.recorded_at) {
        log::warn!("[usage-history] could not prune {:?} history: {error}", sample.provider);
    }
    file.write_all(&line)
}

/// Only the timestamp, so pruning keeps records of any schema version.
#[derive(Deserialize)]
struct RecordedAt {
    recorded_at: f64,
}

fn recorded_at(line: &str) -> Option<f64> {
    serde_json::from_str::<RecordedAt>(line)
        .ok()
        .map(|record| record.recorded_at)
        .filter(|at| at.is_finite())
}

/// Drop samples older than [`RETAIN_SECS`] when the file's first (oldest)
/// record is past the retention plus a day of slack. Normally reads just the
/// first line. Call with the exclusive lock held; rewrites in place (not via
/// rename) so other processes blocked on the lock keep a valid handle.
fn prune_if_stale(file: &mut fs::File, now: f64) -> io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut first = String::new();
    BufReader::new(&*file).read_line(&mut first)?;
    if first.is_empty() {
        return Ok(());
    }
    let stale = match recorded_at(first.trim_end()) {
        Some(at) => at < now - RETAIN_SECS - PRUNE_SLACK_SECS,
        // A truncated or garbage first line: rewrite once to drop it.
        None => true,
    };
    if !stale {
        return Ok(());
    }

    file.seek(SeekFrom::Start(0))?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let cutoff = now - RETAIN_SECS;
    let mut kept = String::with_capacity(content.len());
    for line in content.lines() {
        if recorded_at(line).is_some_and(|at| at >= cutoff) {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(kept.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_at(recorded_at: f64, reset_at: f64, used_percent: f64) -> UsageSample {
        UsageSample {
            version: 1,
            recorded_at,
            provider: Provider::Claude,
            source: "account-a".into(),
            plan: None,
            limits: vec![LimitSample {
                name: "five_hour".into(),
                used_percent,
                window_seconds: 18000.0,
                reset_at: Some(reset_at),
            }],
        }
    }

    fn pt(recorded_at: f64, used_percent: f64) -> HistoryPoint {
        HistoryPoint {
            recorded_at,
            used_percent,
        }
    }

    #[test]
    fn compact_collapses_plateaus() {
        let points = vec![
            pt(0.0, 1.0),
            pt(1.0, 1.0),
            pt(2.0, 1.0),
            pt(3.0, 2.0),
            pt(4.0, 2.0),
            pt(5.0, 2.0),
        ];
        assert_eq!(
            compact_points(&points, 100),
            vec![pt(0.0, 1.0), pt(2.0, 1.0), pt(3.0, 2.0), pt(5.0, 2.0)]
        );
    }

    #[test]
    fn compact_caps_count_and_keeps_ends() {
        let points: Vec<_> = (0..2000)
            .map(|i| pt(i as f64 * 300.0, i as f64 / 20.0))
            .collect();
        let out = compact_points(&points, 400);
        assert!(out.len() <= 400, "got {}", out.len());
        assert!(out.len() > 300, "too aggressive: {}", out.len());
        assert_eq!(out.first(), points.first());
        assert_eq!(out.last(), points.last());
        assert!(out.windows(2).all(|w| w[0].recorded_at < w[1].recorded_at));
    }

    #[test]
    fn reset_timestamp_jitter_keeps_measurements_in_the_same_window() {
        let reset = 1_790_272_800.0;
        let start = reset - 18000.0;
        let history = History(
            vec![
                sample_at(start + 1000.0, reset + 0.147, 16.0),
                sample_at(start + 1300.0, reset + 0.447, 16.0),
                sample_at(start + 1600.0, reset - 0.315, 17.0),
                sample_at(start + 1900.0, reset - 0.281, 21.0),
                sample_at(start + 2000.0, reset + 60.0, 1.0),
            ]
            .into(),
        );
        for current_reset in [reset + 0.447, reset - 0.315] {
            let points = history.points("five_hour", current_reset, 18000.0, start + 2200.0);
            assert_eq!(points.len(), 4);
            assert_eq!(points.first().unwrap().used_percent, 16.0);
            assert_eq!(points.last().unwrap().used_percent, 21.0);
        }
    }

    #[test]
    fn selects_active_window_and_orders_measurements() {
        let history = History(
            vec![
                sample_at(1500.0, 19000.0, 25.0),
                sample_at(900.0, 19000.0, 2.0),
                sample_at(1200.0, 18000.0, 90.0),
                sample_at(1000.0, 19000.0, 10.0),
                sample_at(1500.0, 19000.0, 30.0),
                sample_at(1700.0, 19000.0, 40.0),
            ]
            .into(),
        );
        assert_eq!(
            history.points("five_hour", 19000.0, 18000.0, 1600.0),
            vec![
                HistoryPoint {
                    recorded_at: 1000.0,
                    used_percent: 10.0
                },
                HistoryPoint {
                    recorded_at: 1500.0,
                    used_percent: 30.0
                },
            ]
        );
        assert!(
            history
                .points("seven_day", 19000.0, 18000.0, 1600.0)
                .is_empty()
        );
        assert!(
            history
                .points("five_hour", 19000.0, 3600.0, 1600.0)
                .is_empty()
        );
    }

    #[test]
    fn loading_isolates_sources_and_skips_corrupt_or_unknown_records() {
        let temp = tempfile::tempdir().unwrap();
        let sample = sample_at(1500.0, 19000.0, 25.0);
        append(temp.path(), &sample_at(500.0, 19000.0, 1.0)).unwrap();
        let mut other = sample.clone();
        other.source = "account-b".into();
        append(temp.path(), &other).unwrap();
        other.source = sample.source.clone();
        other.version = 2;
        append(temp.path(), &other).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(temp.path().join("claude.jsonl"))
            .unwrap();
        file.write_all(b"{broken JSON}\n").unwrap();
        append(temp.path(), &sample).unwrap();
        file.write_all(b"{truncated").unwrap();
        assert_eq!(
            load(temp.path(), Provider::Claude, "account-a", 1000.0).unwrap(),
            vec![sample]
        );
    }

    #[test]
    fn appends_samples_and_keeps_providers_separate() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("history");
        let mut sample = UsageSample {
            version: 1,
            recorded_at: 1000.0,
            provider: Provider::Claude,
            source: "test-source".into(),
            plan: Some("pro".into()),
            limits: vec![LimitSample {
                name: "five_hour".into(),
                used_percent: 42.5,
                window_seconds: 18000.0,
                reset_at: Some(19000.0),
            }],
        };
        append(&directory, &sample).unwrap();
        sample.recorded_at = 1300.0;
        sample.limits[0].used_percent = 0.0;
        sample.limits[0].reset_at = None;
        append(&directory, &sample).unwrap();
        let content = fs::read_to_string(directory.join("claude.jsonl")).unwrap();
        let samples: Vec<UsageSample> = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].limits[0].used_percent, 42.5);
        assert_eq!(samples[1], sample);
        sample.provider = Provider::Codex;
        append(&directory, &sample).unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("claude.jsonl")).unwrap(),
            content
        );
        let codex: UsageSample =
            serde_json::from_str(&fs::read_to_string(directory.join("codex.jsonl")).unwrap())
                .unwrap();
        assert_eq!(codex, sample);
    }

    #[test]
    fn reports_unwritable_history_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, "existing data").unwrap();
        let sample = UsageSample {
            version: 1,
            recorded_at: 1000.0,
            provider: Provider::Claude,
            source: "test-source".into(),
            plan: None,
            limits: vec![],
        };
        assert!(append(&path, &sample).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "existing data");
    }

    #[test]
    fn append_prunes_samples_older_than_retention_about_daily() {
        let temp = tempfile::tempdir().unwrap();
        let day = 86400.0;
        let now = 100.0 * day;
        let path = temp.path().join("claude.jsonl");

        // Oldest record within retention + slack: nothing is rewritten, even
        // an out-of-order old record further down survives.
        append(temp.path(), &sample_at(now - 8.5 * day, now, 1.0)).unwrap();
        append(temp.path(), &sample_at(now - 30.0 * day, now, 2.0)).unwrap();
        append(temp.path(), &sample_at(now, now + day, 3.0)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);

        // Once the first record is past retention + slack, old samples go,
        // newer ones (and records of unknown schema versions) stay.
        let mut future = sample_at(now - day, now, 4.0);
        future.version = 2;
        append(temp.path(), &future).unwrap();
        append(temp.path(), &sample_at(now + 1.0 * day, now + 2.0 * day, 5.0)).unwrap();
        let later = now + 1.0 * day;
        append(temp.path(), &sample_at(later, later + day, 6.0)).unwrap();
        let kept: Vec<f64> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| recorded_at(line).unwrap())
            .collect();
        assert_eq!(kept.len(), 4);
        assert!(kept.iter().all(|&at| at >= later - RETAIN_SECS));

        // Garbage at the head is dropped by one rewrite.
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{{trunc\n{content}")).unwrap();
        append(temp.path(), &sample_at(later, later + day, 7.0)).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 5);
        assert!(content.lines().all(|line| recorded_at(line).is_some()));
    }
}
