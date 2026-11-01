//! What each account used over time, and the figures drawn from it.
//!
//! Samples come from two places: Relais keeps one line per status line reading
//! (`~/.relais/usage/<alias>.history.jsonl`, at most one every ten minutes
//! unless the usage moved), and the desktop app keeps its own history per
//! organization. Nothing is asked from Anthropic.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{FixedOffset, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::fsutil;
use crate::usage::UsageReading;

/// An unchanged usage is written again at most this often.
pub const RECORD_EVERY_SECS: i64 = 600;
/// How far back the history goes.
pub const KEEP_SECS: i64 = 35 * 86_400;
/// A drop of the weekly share larger than this is a reset; a smaller one is
/// two readings that disagree a little.
const RESET_DROP: f64 = 5.0;
const WEEK_SECS: i64 = 7 * 86_400;

/// One reading: the share used of the 5-hour window and of the week.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Unix seconds.
    pub t: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fh: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
}

impl Sample {
    pub fn from_reading(reading: &UsageReading) -> Self {
        Sample {
            t: reading.observed_at,
            fh: reading.five_hour.as_ref().map(|w| w.used_percentage),
            sd: reading.seven_day.as_ref().map(|w| w.used_percentage),
        }
    }

    fn moved_from(&self, other: &Sample) -> bool {
        let moved = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() >= 1.0,
            (a, b) => a.is_some() != b.is_some(),
        };
        moved(self.fh, other.fh) || moved(self.sd, other.sd)
    }
}

/// One history file per account, next to its latest reading.
pub struct HistoryStore {
    dir: PathBuf,
}

impl HistoryStore {
    pub fn new(dir: &Path) -> Self {
        HistoryStore {
            dir: dir.to_path_buf(),
        }
    }

    fn path(&self, alias: &str) -> PathBuf {
        self.dir.join(format!("{alias}.history.jsonl"))
    }

    /// Every sample that can be read, oldest first; damaged lines are skipped.
    pub fn load(&self, alias: &str) -> Vec<Sample> {
        let mut samples: Vec<Sample> = fs::read_to_string(self.path(alias))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        samples.sort_by_key(|s| s.t);
        samples
    }

    /// Adds a sample, unless the last one is recent and the usage has not moved.
    /// Samples older than [`KEEP_SECS`] go when the file is next rewritten.
    pub fn record(&self, alias: &str, sample: Sample) -> Result<()> {
        let samples = self.load(alias);
        if let Some(last) = samples.last() {
            if sample.t - last.t < RECORD_EVERY_SECS && !sample.moved_from(last) {
                return Ok(());
            }
        }
        let path = self.path(alias);
        let line = |s: &Sample| {
            serde_json::to_string(s).map_err(|e| Error::Json {
                path: path.clone(),
                source: e,
            })
        };
        let oldest_kept = sample.t - KEEP_SECS;
        if samples.first().is_some_and(|first| first.t < oldest_kept) {
            let mut text = String::new();
            for kept in samples.iter().filter(|s| s.t >= oldest_kept) {
                text.push_str(&line(kept)?);
                text.push('\n');
            }
            text.push_str(&line(&sample)?);
            text.push('\n');
            return fsutil::write_atomic(&path, text.as_bytes());
        }
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        let mut file = fs::File::options()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        writeln!(file, "{}", line(&sample)?).map_err(|e| Error::io(&path, e))
    }
}

/// The samples the desktop app keeps for `org_id`, oldest first.
pub fn from_desktop_history(history: &Value, org_id: &str) -> Vec<Sample> {
    let mut samples: Vec<Sample> = history
        .get("samples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.get("org").and_then(Value::as_str) == Some(org_id))
        .filter_map(|s| {
            let t = s.get("t")?.as_i64()?;
            let usage = s.get("u")?;
            Some(Sample {
                t: if t > 100_000_000_000 { t / 1000 } else { t },
                fh: usage.get("fh").and_then(Value::as_f64),
                sd: usage.get("sd").and_then(Value::as_f64),
            })
        })
        .collect();
    samples.sort_by_key(|s| s.t);
    samples
}

/// One local day of an account's week.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayUsage {
    /// Share of the weekly quota used that day; `None` when no reading fell on
    /// that day, which says nothing about the use.
    pub used: Option<f64>,
    /// A reading that day showed a full window, 5-hour or weekly.
    pub limited: bool,
}

/// The last `days` local days (in `offset`), oldest first. What the weekly
/// share gained between two readings counts for the day of the later one;
/// after a reset, the new share is what was used since.
pub fn daily_usage(
    samples: &[Sample],
    now: i64,
    days: usize,
    offset: FixedOffset,
) -> Vec<DayUsage> {
    let date = |t: i64| offset.timestamp_opt(t, 0).single().map(|d| d.date_naive());
    let mut usage = vec![
        DayUsage {
            used: None,
            limited: false,
        };
        days
    ];
    let Some(today) = date(now) else {
        return usage;
    };
    let slot = |t: i64| {
        let ago = (today - date(t)?).num_days();
        (0..days as i64)
            .contains(&ago)
            .then(|| days - 1 - ago as usize)
    };
    let mut sorted: Vec<&Sample> = samples.iter().collect();
    sorted.sort_by_key(|s| s.t);
    for sample in &sorted {
        if let Some(index) = slot(sample.t) {
            let day = &mut usage[index];
            day.used.get_or_insert(0.0);
            let full = |share: Option<f64>| share.is_some_and(|v| v >= 100.0);
            day.limited |= full(sample.fh) || full(sample.sd);
        }
    }
    let weekly: Vec<&&Sample> = sorted.iter().filter(|s| s.sd.is_some()).collect();
    for pair in weekly.windows(2) {
        let (Some(before), Some(after)) = (pair[0].sd, pair[1].sd) else {
            continue;
        };
        let gained = if after >= before {
            after - before
        } else if before - after > RESET_DROP {
            after
        } else {
            0.0
        };
        if let Some(index) = slot(pair[1].t) {
            *usage[index].used.get_or_insert(0.0) += gained;
        }
    }
    usage
}

/// The highest weekly share reached in each of the last `weeks` seven-day
/// spans ending now, oldest first; `None` for a span without readings.
pub fn weekly_peaks(samples: &[Sample], now: i64, weeks: usize) -> Vec<Option<f64>> {
    (0..weeks)
        .map(|index| {
            let end = now - (weeks - 1 - index) as i64 * WEEK_SECS;
            let start = end - WEEK_SECS;
            samples
                .iter()
                .filter(|s| s.t > start && s.t <= end)
                .filter_map(|s| s.sd)
                .fold(None, |peak: Option<f64>, sd| {
                    Some(peak.map_or(sd, |p| p.max(sd)))
                })
        })
        .collect()
}

/// How far back the pace of a window is read: long enough to smooth a burst,
/// short enough to be today's pace.
const WEEK_PACE_SPAN: i64 = 6 * 3600;
const FIVE_HOUR_PACE_SPAN: i64 = 90 * 60;
/// Less than this says nothing about a pace.
const PACE_MIN_SPAN: i64 = 20 * 60;
const PACE_MIN_GAIN: f64 = 1.0;

/// The last reading of a limit and its pace, in points a second, from what
/// followed its last reset. `share` picks the limit out of a sample.
fn pace(
    samples: &[Sample],
    share: fn(&Sample) -> Option<f64>,
    span: i64,
    now: i64,
) -> Option<((i64, f64), f64)> {
    let mut points: Vec<(i64, f64)> = samples
        .iter()
        .filter(|s| s.t > now - span && s.t <= now)
        .filter_map(|s| Some((s.t, share(s)?)))
        .collect();
    points.sort_by_key(|(t, _)| *t);
    // Only what follows the last reset belongs to this window.
    let start = points
        .windows(2)
        .rposition(|pair| pair[1].1 < pair[0].1)
        .map_or(0, |index| index + 1);
    let (first, last) = (points.get(start)?, points.last()?);
    let (elapsed, gained) = (last.0 - first.0, last.1 - first.1);
    if elapsed < PACE_MIN_SPAN || gained < PACE_MIN_GAIN {
        return None;
    }
    Some((*last, gained / elapsed as f64))
}

/// The points of the week an account has been using per second lately.
pub fn week_pace(samples: &[Sample], now: i64) -> Option<f64> {
    pace(samples, |s| s.sd, WEEK_PACE_SPAN, now).map(|(_, pace)| pace)
}

/// When a limit fills up at the pace of the last readings, if that comes
/// before it starts over.
fn full_at(
    samples: &[Sample],
    share: fn(&Sample) -> Option<f64>,
    span: i64,
    now: i64,
    resets_at: Option<i64>,
) -> Option<i64> {
    let (last, pace) = pace(samples, share, span, now)?;
    if last.1 >= 100.0 {
        return None;
    }
    let at = last.0 + ((100.0 - last.1) / pace) as i64;
    (at > now && resets_at.is_none_or(|reset| at < reset)).then_some(at)
}

/// When the account reaches a limit at its current pace: the sooner of its two
/// windows, or `None` when it will not get there before they start over.
pub fn forecast(
    samples: &[Sample],
    now: i64,
    five_hour_resets: Option<i64>,
    week_resets: Option<i64>,
) -> Option<i64> {
    let five = full_at(
        samples,
        |s| s.fh,
        FIVE_HOUR_PACE_SPAN,
        now,
        five_hour_resets,
    );
    let week = full_at(samples, |s| s.sd, WEEK_PACE_SPAN, now, week_resets);
    five.into_iter().chain(week).min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;

    fn week(points: &[(i64, f64)]) -> Vec<Sample> {
        points
            .iter()
            .map(|&(ago, share)| Sample {
                t: NOW - ago,
                fh: None,
                sd: Some(share),
            })
            .collect()
    }

    #[test]
    fn the_pace_of_the_week_is_read_from_the_last_readings() {
        let samples = week(&[(3 * 3600, 70.0), (3600, 80.0)]);

        let pace = week_pace(&samples, NOW).unwrap();

        assert!((pace * 3600.0 - 5.0).abs() < 1e-9, "{pace}");
    }

    #[test]
    fn a_share_that_does_not_move_has_no_pace() {
        assert_eq!(
            week_pace(&week(&[(3 * 3600, 70.0), (3600, 70.0)]), NOW),
            None
        );
    }

    #[test]
    fn a_steady_pace_tells_when_the_week_fills_up() {
        let samples = week(&[(3 * 3600, 70.0), (3600, 80.0)]);

        // Five points an hour from 80 at NOW - 1 h: full four hours later.
        let at = forecast(&samples, NOW, None, Some(NOW + 86_400));

        assert_eq!(at, Some(NOW - 3600 + 4 * 3600));
    }

    #[test]
    fn a_limit_that_starts_over_first_is_no_threat() {
        let samples = week(&[(3 * 3600, 70.0), (3600, 80.0)]);

        assert_eq!(forecast(&samples, NOW, None, Some(NOW + 3600)), None);
    }

    #[test]
    fn a_share_that_does_not_move_forecasts_nothing() {
        let samples = week(&[(3 * 3600, 70.0), (3600, 70.0)]);

        assert_eq!(forecast(&samples, NOW, None, Some(NOW + 86_400)), None);
    }

    #[test]
    fn a_few_minutes_say_nothing_about_a_pace() {
        let samples = week(&[(600, 70.0), (60, 75.0)]);

        assert_eq!(forecast(&samples, NOW, None, Some(NOW + 86_400)), None);
    }

    #[test]
    fn only_what_follows_a_reset_counts() {
        let samples = week(&[(5 * 3600, 90.0), (3 * 3600, 2.0), (3600, 12.0)]);

        let at = forecast(&samples, NOW, None, None);

        // Ten points in two hours, 88 to go: 17.6 hours.
        assert_eq!(at, Some(NOW - 3600 + 63_360));
    }

    #[test]
    fn the_sooner_limit_is_the_one_told() {
        let mut samples = week(&[(3 * 3600, 10.0), (3600, 20.0)]);
        samples.push(Sample {
            t: NOW - 3600,
            fh: Some(50.0),
            sd: None,
        });
        samples.push(Sample {
            t: NOW - 600,
            fh: Some(75.0),
            sd: None,
        });

        assert_eq!(
            forecast(&samples, NOW, Some(NOW + 2 * 3600), Some(NOW + 86_400 * 5)),
            Some(NOW - 600 + 3000)
        );
    }

    const NOW: i64 = 1_791_270_000;
    const DAY: i64 = 86_400;

    fn sample(t: i64, fh: f64, sd: f64) -> Sample {
        Sample {
            t,
            fh: Some(fh),
            sd: Some(sd),
        }
    }

    #[test]
    fn a_sample_is_kept_at_most_every_ten_minutes_unless_usage_moved() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path());

        store.record("main", sample(NOW, 10.0, 20.0)).unwrap();
        store.record("main", sample(NOW + 60, 10.4, 20.0)).unwrap();
        store.record("main", sample(NOW + 120, 12.0, 20.0)).unwrap();
        store
            .record("main", sample(NOW + 120 + RECORD_EVERY_SECS, 12.0, 20.0))
            .unwrap();

        let times: Vec<i64> = store.load("main").iter().map(|s| s.t).collect();
        assert_eq!(times, vec![NOW, NOW + 120, NOW + 120 + RECORD_EVERY_SECS]);
    }

    #[test]
    fn samples_older_than_five_weeks_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path());

        store
            .record("main", sample(NOW - 40 * DAY, 5.0, 5.0))
            .unwrap();
        store.record("main", sample(NOW, 10.0, 20.0)).unwrap();

        let times: Vec<i64> = store.load("main").iter().map(|s| s.t).collect();
        assert_eq!(times, vec![NOW]);
    }

    #[test]
    fn a_missing_or_damaged_history_reads_as_what_can_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path());
        assert!(store.load("main").is_empty());

        std::fs::write(
            dir.path().join("main.history.jsonl"),
            format!("{{\"t\":{NOW},\"sd\":4}}\nnot json\n"),
        )
        .unwrap();
        assert_eq!(store.load("main").len(), 1);
    }

    #[test]
    fn the_desktop_history_gives_the_samples_of_one_organization() {
        let history = json!({"samples": [
            {"t": NOW * 1000, "org": "org-a", "u": {"fh": 12, "sd": 30}},
            {"t": (NOW - 60) * 1000, "org": "org-a", "u": {"fh": 10, "sd": 29}},
            {"t": NOW * 1000, "org": "org-b", "u": {"fh": 99, "sd": 99}}
        ]});

        let samples = from_desktop_history(&history, "org-a");

        assert_eq!(
            samples,
            vec![sample(NOW - 60, 10.0, 29.0), sample(NOW, 12.0, 30.0)]
        );
    }

    #[test]
    fn each_day_counts_what_the_week_gained_that_day() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let today = utc
            .timestamp_opt(NOW, 0)
            .unwrap()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let samples = [
            sample(today - 2 * DAY + 10 * 3600, 0.0, 10.0),
            sample(today - 2 * DAY + 18 * 3600, 0.0, 25.0),
            sample(today - DAY + 9 * 3600, 0.0, 30.0),
            // A reading a hair lower than the last one is noise, not a reset.
            sample(today - DAY + 12 * 3600, 0.0, 29.5),
            // The week reset overnight: what it shows was used since.
            sample(today + 3600, 0.0, 3.0),
            sample(today + 2 * 3600, 0.0, 10.0),
        ];

        let days = daily_usage(&samples, today + 3 * 3600, 3, utc);

        let used: Vec<Option<f64>> = days.iter().map(|d| d.used).collect();
        assert_eq!(used, vec![Some(15.0), Some(5.0), Some(10.0)]);
        assert!(days.iter().all(|d| !d.limited));
    }

    #[test]
    fn a_day_without_readings_is_unknown_and_a_day_at_the_limit_says_so() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let today = utc
            .timestamp_opt(NOW, 0)
            .unwrap()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let samples = [
            sample(today - 2 * DAY + 9 * 3600, 50.0, 40.0),
            sample(today - 2 * DAY + 15 * 3600, 100.0, 45.0),
            sample(today + 3600, 10.0, 50.0),
        ];

        let days = daily_usage(&samples, today + 2 * 3600, 3, utc);

        assert_eq!(
            days[0],
            DayUsage {
                used: Some(5.0),
                limited: true
            }
        );
        assert_eq!(
            days[1],
            DayUsage {
                used: None,
                limited: false
            }
        );
        assert_eq!(
            days[2],
            DayUsage {
                used: Some(5.0),
                limited: false
            }
        );
    }

    #[test]
    fn each_of_the_last_weeks_shows_its_highest_share() {
        let samples = [
            sample(NOW - 20 * DAY, 0.0, 100.0),
            sample(NOW - 19 * DAY, 0.0, 40.0),
            sample(NOW - 10 * DAY, 0.0, 87.0),
            sample(NOW - DAY, 0.0, 55.0),
        ];

        assert_eq!(
            weekly_peaks(&samples, NOW, 4),
            vec![None, Some(100.0), Some(87.0), Some(55.0)]
        );
    }
}
