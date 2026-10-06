//! Usage readings per account, and which account has the most room left.
//!
//! Readings come from what Claude Code hands to its status line
//! (`rate_limits.five_hour` and `rate_limits.seven_day`) and from the history
//! the desktop app keeps. Relais never asks Anthropic for them.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::fsutil;

pub const FIVE_HOURS_SECS: i64 = 5 * 3600;
pub const SEVEN_DAYS_SECS: i64 = 7 * 24 * 3600;

/// Percent used assumed for an account Relais has never seen a reading for.
pub const UNKNOWN_USAGE: f64 = 50.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub used_percentage: f64,
    /// Unix seconds when the window resets, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    Statusline,
    DesktopHistory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReading {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<Window>,
    /// Unix seconds when the reading was taken.
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub source: UsageSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Availability {
    /// The account can work; `used` is the highest window percentage right now.
    Available { used: f64 },
    /// A window is full until `until` (Unix seconds).
    Limited { until: i64 },
}

/// Builds a reading from the JSON Claude Code passes to the status line.
/// Returns `None` when the input carries no rate limits (API key sessions,
/// or before the first response of a session).
pub fn reading_from_statusline(input: &Value, now: i64) -> Option<UsageReading> {
    let limits = input.get("rate_limits")?;
    let five_hour = limits.get("five_hour").and_then(window_from_statusline);
    let seven_day = limits.get("seven_day").and_then(window_from_statusline);
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    Some(UsageReading {
        five_hour,
        seven_day,
        observed_at: now,
        session_id: input
            .get("session_id")
            .and_then(Value::as_str)
            .map(String::from),
        source: UsageSource::Statusline,
    })
}

fn window_from_statusline(value: &Value) -> Option<Window> {
    Some(Window {
        used_percentage: value.get("used_percentage")?.as_f64()?,
        resets_at: value.get("resets_at").and_then(Value::as_i64),
    })
}

/// Builds a reading for `org_id` from the desktop app's `plan-usage-history.json`.
pub fn reading_from_desktop_history(history: &Value, org_id: &str) -> Option<UsageReading> {
    let (timestamp, usage) = history
        .get("samples")?
        .as_array()?
        .iter()
        .filter(|sample| sample.get("org").and_then(Value::as_str) == Some(org_id))
        .filter_map(|sample| Some((sample.get("t")?.as_i64()?, sample.get("u")?)))
        .max_by_key(|(timestamp, _)| *timestamp)?;
    let window = |key: &str| {
        usage.get(key).and_then(Value::as_f64).map(|used| Window {
            used_percentage: used,
            resets_at: None,
        })
    };
    Some(UsageReading {
        five_hour: window("fh"),
        seven_day: window("sd"),
        observed_at: to_unix_secs(timestamp),
        session_id: None,
        source: UsageSource::DesktopHistory,
    })
}

/// The desktop app stores milliseconds; readings use seconds.
fn to_unix_secs(timestamp: i64) -> i64 {
    if timestamp > 100_000_000_000 {
        timestamp / 1000
    } else {
        timestamp
    }
}

impl UsageReading {
    pub fn five_hour_now(&self, now: i64) -> Option<f64> {
        self.window_now(self.five_hour.as_ref(), FIVE_HOURS_SECS, now)
    }

    pub fn seven_day_now(&self, now: i64) -> Option<f64> {
        self.window_now(self.seven_day.as_ref(), SEVEN_DAYS_SECS, now)
    }

    pub fn availability(&self, now: i64) -> Availability {
        let windows = [
            (self.five_hour.as_ref(), FIVE_HOURS_SECS),
            (self.seven_day.as_ref(), SEVEN_DAYS_SECS),
        ];
        let limited_until = windows
            .iter()
            .filter_map(|(window, length)| {
                let window = (*window)?;
                let until = window.resets_at.unwrap_or(self.observed_at + length);
                (window.used_percentage >= 100.0 && until > now).then_some(until)
            })
            .max();
        match limited_until {
            Some(until) => Availability::Limited { until },
            None => {
                let used = [self.five_hour_now(now), self.seven_day_now(now)]
                    .into_iter()
                    .flatten()
                    .fold(0.0, f64::max);
                Availability::Available { used }
            }
        }
    }

    /// Percent used now: a window whose reset time has passed counts as empty.
    /// Without a reset time, the window counts as empty once its length has
    /// passed since the reading.
    fn window_now(&self, window: Option<&Window>, length: i64, now: i64) -> Option<f64> {
        let window = window?;
        let reset_passed = match window.resets_at {
            Some(at) => now >= at,
            None => now - self.observed_at > length,
        };
        Some(if reset_passed {
            0.0
        } else {
            window.used_percentage
        })
    }
}

/// Picks the account with the most room left among `candidates`, skipping
/// limited ones. Ties go to the account whose weekly window resets first, so
/// quota that would be lost at the reset is used first.
pub fn best_account<'a>(
    candidates: &[(&'a str, Option<&UsageReading>)],
    now: i64,
) -> Option<&'a str> {
    candidates
        .iter()
        .filter_map(|(alias, reading)| match reading {
            None => Some((*alias, UNKNOWN_USAGE, i64::MAX)),
            Some(reading) => match reading.availability(now) {
                Availability::Limited { .. } => None,
                Availability::Available { used } => {
                    let weekly_reset = reading
                        .seven_day
                        .as_ref()
                        .and_then(|w| w.resets_at)
                        .filter(|at| *at > now)
                        .unwrap_or(i64::MAX);
                    Some((*alias, used, weekly_reset))
                }
            },
        })
        .min_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then(a.2.cmp(&b.2))
        })
        .map(|(alias, _, _)| alias)
}

/// One JSON file per account under `~/.relais/usage`.
#[derive(Debug, Clone)]
pub struct UsageStore {
    dir: PathBuf,
}

impl UsageStore {
    pub fn new(dir: &Path) -> Self {
        UsageStore {
            dir: dir.to_path_buf(),
        }
    }

    pub fn load(&self, alias: &str) -> Result<Option<UsageReading>> {
        let path = self.path(alias);
        match fsutil::read_json(&path)? {
            None => Ok(None),
            Some(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|e| Error::Json { path, source: e }),
        }
    }

    /// Stores `reading` unless a newer one is already stored.
    pub fn save(&self, alias: &str, reading: &UsageReading) -> Result<()> {
        if let Ok(Some(stored)) = self.load(alias) {
            if stored.observed_at > reading.observed_at {
                return Ok(());
            }
        }
        let path = self.path(alias);
        let value = serde_json::to_value(reading).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        fsutil::write_json_atomic(&path, &value)
    }

    fn path(&self, alias: &str) -> PathBuf {
        self.dir.join(format!("{alias}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_791_270_000;

    fn reading(five: Option<(f64, i64)>, seven: Option<(f64, i64)>) -> UsageReading {
        UsageReading {
            five_hour: five.map(|(used, at)| Window {
                used_percentage: used,
                resets_at: Some(at),
            }),
            seven_day: seven.map(|(used, at)| Window {
                used_percentage: used,
                resets_at: Some(at),
            }),
            observed_at: NOW,
            session_id: None,
            source: UsageSource::Statusline,
        }
    }

    #[test]
    fn statusline_input_with_rate_limits_gives_a_reading() {
        let input = json!({
            "session_id": "s-1",
            "rate_limits": {
                "five_hour": {"used_percentage": 65, "resets_at": NOW + 3600},
                "seven_day": {"used_percentage": 21.5, "resets_at": NOW + 86400}
            }
        });

        let reading = reading_from_statusline(&input, NOW).unwrap();

        assert_eq!(
            reading.five_hour,
            Some(Window {
                used_percentage: 65.0,
                resets_at: Some(NOW + 3600)
            })
        );
        assert_eq!(
            reading.seven_day,
            Some(Window {
                used_percentage: 21.5,
                resets_at: Some(NOW + 86400)
            })
        );
        assert_eq!(reading.observed_at, NOW);
        assert_eq!(reading.session_id.as_deref(), Some("s-1"));
        assert_eq!(reading.source, UsageSource::Statusline);
    }

    #[test]
    fn statusline_input_without_rate_limits_gives_nothing() {
        assert!(reading_from_statusline(&json!({"session_id": "s-1"}), NOW).is_none());
    }

    #[test]
    fn a_window_past_its_reset_counts_as_empty() {
        let r = reading(Some((80.0, NOW - 1)), Some((40.0, NOW + 10)));
        assert_eq!(r.five_hour_now(NOW), Some(0.0));
        assert_eq!(r.seven_day_now(NOW), Some(40.0));
    }

    #[test]
    fn a_full_window_limits_the_account_until_its_reset() {
        let r = reading(Some((100.0, NOW + 3600)), Some((100.0, NOW + 7200)));
        assert_eq!(
            r.availability(NOW),
            Availability::Limited { until: NOW + 7200 }
        );
    }

    #[test]
    fn the_account_is_available_again_after_the_reset() {
        let r = reading(Some((100.0, NOW - 1)), Some((30.0, NOW + 7200)));
        assert_eq!(r.availability(NOW), Availability::Available { used: 30.0 });
    }

    #[test]
    fn best_account_has_the_most_room_and_is_not_limited() {
        let full = reading(Some((100.0, NOW + 60)), None);
        let busy = reading(Some((70.0, NOW + 60)), Some((10.0, NOW + 900)));
        let quiet = reading(Some((5.0, NOW + 60)), Some((20.0, NOW + 900)));

        let best = best_account(
            &[
                ("full", Some(&full)),
                ("busy", Some(&busy)),
                ("quiet", Some(&quiet)),
            ],
            NOW,
        );

        assert_eq!(best, Some("quiet"));
    }

    #[test]
    fn ties_go_to_the_weekly_window_that_resets_first() {
        let late = reading(Some((10.0, NOW + 60)), Some((10.0, NOW + 9000)));
        let soon = reading(Some((10.0, NOW + 60)), Some((10.0, NOW + 100)));

        assert_eq!(
            best_account(&[("late", Some(&late)), ("soon", Some(&soon))], NOW),
            Some("soon")
        );
    }

    #[test]
    fn an_unknown_account_ranks_behind_one_with_plenty_of_room() {
        let roomy = reading(Some((10.0, NOW + 60)), Some((20.0, NOW + 900)));
        assert_eq!(
            best_account(&[("unknown", None), ("roomy", Some(&roomy))], NOW),
            Some("roomy")
        );
    }

    #[test]
    fn no_candidate_is_returned_when_all_are_limited() {
        let full = reading(None, Some((100.0, NOW + 60)));
        assert_eq!(best_account(&[("full", Some(&full))], NOW), None);
    }

    #[test]
    fn store_keeps_the_newest_reading() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::new(dir.path());
        let mut newer = reading(Some((50.0, NOW + 60)), None);
        newer.observed_at = NOW + 10;
        let older = reading(Some((20.0, NOW + 60)), None);

        store.save("a", &newer).unwrap();
        store.save("a", &older).unwrap();

        assert_eq!(store.load("a").unwrap(), Some(newer));
        assert_eq!(store.load("b").unwrap(), None);
    }

    #[test]
    fn desktop_history_gives_the_latest_sample_of_the_organization() {
        let history = json!({"version": 1, "samples": [
            {"t": (NOW - 600) * 1000, "org": "org-a", "u": {"fh": 10, "sd": 5}},
            {"t": (NOW - 60) * 1000, "org": "org-a", "u": {"fh": 30, "sd": 6, "xu": 0}},
            {"t": (NOW - 30) * 1000, "org": "org-b", "u": {"fh": 90, "sd": 99}}
        ]});

        let reading = reading_from_desktop_history(&history, "org-a").unwrap();

        assert_eq!(reading.observed_at, NOW - 60);
        assert_eq!(
            reading.five_hour,
            Some(Window {
                used_percentage: 30.0,
                resets_at: None
            })
        );
        assert_eq!(
            reading.seven_day,
            Some(Window {
                used_percentage: 6.0,
                resets_at: None
            })
        );
        assert_eq!(reading.source, UsageSource::DesktopHistory);
        assert!(reading_from_desktop_history(&history, "org-z").is_none());
    }

    #[test]
    fn a_window_without_reset_time_counts_as_empty_once_its_length_has_passed() {
        let mut r = reading(None, None);
        r.five_hour = Some(Window {
            used_percentage: 90.0,
            resets_at: None,
        });
        r.observed_at = NOW - FIVE_HOURS_SECS - 1;
        assert_eq!(r.five_hour_now(NOW), Some(0.0));
        r.observed_at = NOW - 60;
        assert_eq!(r.five_hour_now(NOW), Some(90.0));
    }

    #[test]
    fn invalid_store_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.json"), "{not json").unwrap();
        assert!(matches!(
            UsageStore::new(dir.path()).load("a"),
            Err(Error::Json { .. })
        ));
    }
}
