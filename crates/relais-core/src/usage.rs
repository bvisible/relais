//! Usage readings per account, and which account has the most room left.
//!
//! Readings come from what Claude Code hands to its status line
//! (`rate_limits.five_hour` and `rate_limits.seven_day`) and from the history
//! the desktop app keeps. Relais never asks Anthropic for them.

use std::cmp::Ordering;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone};
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

/// A weekly limit that applies to one model only, as `/usage` reports it:
/// `Current week (Fable): 0% used · resets Oct 13 at 7am`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelWindow {
    /// The model as Claude Code names it: `Fable`, `Sonnet only`.
    pub model: String,
    pub used_percentage: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

impl ModelWindow {
    /// Percent used now: a limit whose reset time has passed counts as empty.
    pub fn used_now(&self, now: i64) -> f64 {
        match self.resets_at {
            Some(at) if now >= at => 0.0,
            _ => self.used_percentage,
        }
    }
}

/// An account's per-model limits, from its last `/usage` report. Kept apart
/// from the reading: the status line, which updates the reading, never
/// carries them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimits {
    pub windows: Vec<ModelWindow>,
    /// Unix seconds when Claude Code reported them.
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    Statusline,
    DesktopHistory,
    /// Claude Code's `/usage` command, run for the account by Relais.
    UsageCommand,
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
    /// A window is full until `until` (Unix seconds). Without a reset time
    /// in the reading, `until` is the latest it can be and `exact` is false.
    Limited { until: i64, exact: bool },
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

/// Builds a reading from what Claude Code's `/usage` command prints: the share
/// of the session (5-hour) window and of the week, each with the time it resets
/// (`resets Oct 8 at 10pm (Europe/Zurich)`). Claude Code writes those times in
/// the machine's time zone, which is `now`'s.
pub fn parse_usage_report<Tz: TimeZone>(text: &str, now: DateTime<Tz>) -> Option<UsageReading> {
    let sections = report_sections(text);
    let window = |label: &str| {
        sections
            .iter()
            .find(|(name, _)| name == label)
            .and_then(|(_, body)| report_window(body, &now))
    };
    let five_hour = window("Current session");
    let seven_day = window(ALL_MODELS_WEEK);
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    Some(UsageReading {
        five_hour,
        seven_day,
        observed_at: now.timestamp(),
        session_id: None,
        source: UsageSource::UsageCommand,
    })
}

const ALL_MODELS_WEEK: &str = "Current week (all models)";

/// The weekly limits `/usage` reports for single models (`Current week
/// (Fable): 0% used · resets …`), in the order it lists them.
pub fn parse_model_limits<Tz: TimeZone>(text: &str, now: DateTime<Tz>) -> ModelLimits {
    let windows = report_sections(text)
        .iter()
        .filter(|(label, _)| label != ALL_MODELS_WEEK)
        .filter_map(|(label, body)| {
            let model = label.strip_prefix("Current week (")?.strip_suffix(')')?;
            let window = report_window(body, &now)?;
            Some(ModelWindow {
                model: model.trim().to_string(),
                used_percentage: window.used_percentage,
                resets_at: window.resets_at,
            })
        })
        .collect();
    ModelLimits {
        windows,
        observed_at: now.timestamp(),
    }
}

/// What ends a limit's section: the panel goes on with credits and sessions,
/// whose percentages are not that limit's.
const AFTER_LIMITS: [&str; 7] = [
    "Extra usage",
    "Usage credits",
    "Cloud credits",
    "Credits",
    "What's contributing",
    "Press ",
    "Esc to",
];

/// The report split by limit, as (label, what follows it). The label may
/// share its line with its figures or stand on its own above them.
fn report_sections(text: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut open = false;
    for line in text.lines().map(str::trim) {
        let label = if line.starts_with("Current session") {
            Some("Current session")
        } else if line.starts_with("Current week (") {
            line.find(')').map(|end| &line[..=end])
        } else {
            None
        };
        if let Some(label) = label {
            open = true;
            let rest = line[label.len()..].trim_start_matches(':').trim();
            sections.push((label.to_string(), rest.to_string()));
        } else if AFTER_LIMITS.iter().any(|end| line.starts_with(end)) {
            open = false;
        } else if open {
            if let Some((_, body)) = sections.last_mut() {
                body.push('\n');
                body.push_str(line);
            }
        }
    }
    sections
}

/// A limit's share and reset, from what follows its label: `12% used · resets
/// Oct 7 at 2:20am (Europe/Zurich)`, or `2% 2% used` above `Resets 9pm`.
fn report_window<Tz: TimeZone>(body: &str, now: &DateTime<Tz>) -> Option<Window> {
    let used = body.lines().find_map(line_share)?;
    let resets_at = body.lines().find_map(|line| {
        let at = line.to_ascii_lowercase().find("resets ")?;
        parse_reset(line[at + "resets ".len()..].split(" · ").next()?, now)
    });
    Some(Window {
        used_percentage: used,
        resets_at,
    })
}

/// The share used a line states. Only a number with its role counts: a screen
/// reader gives the meter's value before its label (`2% 2% used`), and a share
/// left has to be turned over.
fn line_share(line: &str) -> Option<f64> {
    line.match_indices('%').find_map(|(index, _)| {
        let digits = line[..index]
            .rfind(|c: char| !c.is_ascii_digit() && c != '.')
            .map_or(0, |space| space + 1);
        let share = line[digits..index].parse::<f64>().ok()?;
        if !(0.0..=100.0).contains(&share) {
            return None;
        }
        let role = line[index + 1..].trim_start().to_ascii_lowercase();
        if ["used", "spent", "consumed"]
            .iter()
            .any(|word| role.starts_with(word))
        {
            Some(share)
        } else if ["left", "remaining", "available"]
            .iter()
            .any(|word| role.starts_with(word))
        {
            Some(100.0 - share)
        } else {
            None
        }
    })
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Oct 8 at 10pm (Europe/Zurich)`, `Oct 7 at 2:20am`, or a bare time, as the
/// next such moment in `now`'s time zone.
fn parse_reset<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Option<i64> {
    let text = text.split(" (").next()?.trim();
    let (date, clock) = match text.split_once(" at ") {
        Some((date, clock)) => (Some(date.trim()), clock.trim()),
        None => (None, text),
    };
    let clock = clock.to_ascii_lowercase().replace(' ', "");
    let (digits, meridiem) = if let Some(digits) = clock.strip_suffix("pm") {
        (digits, Some(true))
    } else if let Some(digits) = clock.strip_suffix("am") {
        (digits, Some(false))
    } else {
        (clock.as_str(), None)
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((hour, minute)) => (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?),
        None => (digits.parse::<u32>().ok()?, 0),
    };
    let hour = match meridiem {
        Some(pm) if (1..=12).contains(&hour) => (hour % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if hour < 24 => hour,
        None => return None,
    };
    let today = now.date_naive();
    let day = match date {
        Some(date) => {
            let (month_name, day) = date.split_once(' ')?;
            let month = MONTHS.iter().position(|m| month_name.starts_with(m))? as u32 + 1;
            let day: u32 = day.trim().parse().ok()?;
            let this_year = NaiveDate::from_ymd_opt(today.year(), month, day)?;
            if this_year + chrono::Days::new(1) < today {
                NaiveDate::from_ymd_opt(today.year() + 1, month, day)?
            } else {
                this_year
            }
        }
        None => today,
    };
    let at = now
        .timezone()
        .from_local_datetime(&day.and_hms_opt(hour, minute, 0)?)
        .earliest()?;
    let at = if date.is_none() && at.timestamp() < now.timestamp() {
        at + chrono::Duration::days(1)
    } else {
        at
    };
    Some(at.timestamp())
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
    /// Fills the reset times this reading lacks from an older one, when they
    /// were still ahead at this reading: the window it describes is the same
    /// one. The desktop app's history never carries reset times.
    pub fn with_resets_from(mut self, older: &UsageReading) -> UsageReading {
        let observed_at = self.observed_at;
        let carry = |window: &mut Option<Window>, older: &Option<Window>| {
            if let (Some(window), Some(older)) = (window.as_mut(), older) {
                if window.resets_at.is_none() {
                    window.resets_at = older.resets_at.filter(|at| *at > observed_at);
                }
            }
        };
        carry(&mut self.five_hour, &older.five_hour);
        carry(&mut self.seven_day, &older.seven_day);
        self
    }

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
                let exact = window.resets_at.is_some();
                let until = window.resets_at.unwrap_or(self.observed_at + length);
                (window.used_percentage >= 100.0 && until > now).then_some((until, exact))
            })
            .max_by_key(|(until, _)| *until);
        match limited_until {
            Some((until, exact)) => Availability::Limited { until, exact },
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

    /// Moves an account's reading to `target`, leaving the account without
    /// one. Returns `false` when it had none.
    pub fn move_to(&self, alias: &str, target: &Path) -> Result<bool> {
        let path = self.path(alias);
        if !path.is_file() {
            return Ok(false);
        }
        std::fs::rename(&path, target).map_err(|e| Error::io(&path, e))?;
        Ok(true)
    }

    pub fn load_models(&self, alias: &str) -> Result<Option<ModelLimits>> {
        let path = self.models_path(alias);
        match fsutil::read_json(&path)? {
            None => Ok(None),
            Some(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|e| Error::Json { path, source: e }),
        }
    }

    pub fn save_models(&self, alias: &str, limits: &ModelLimits) -> Result<()> {
        let path = self.models_path(alias);
        let value = serde_json::to_value(limits).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        fsutil::write_json_atomic(&path, &value)
    }

    fn path(&self, alias: &str) -> PathBuf {
        self.dir.join(format!("{alias}.json"))
    }

    fn models_path(&self, alias: &str) -> PathBuf {
        self.dir.join(format!("{alias}.models.json"))
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
            Availability::Limited {
                until: NOW + 7200,
                exact: true
            }
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
    fn a_model_limit_past_its_reset_counts_as_empty() {
        let window = ModelWindow {
            model: "Fable".into(),
            used_percentage: 80.0,
            resets_at: Some(NOW + 60),
        };
        assert_eq!(window.used_now(NOW), 80.0);
        assert_eq!(window.used_now(NOW + 60), 0.0);
    }

    #[test]
    fn model_limits_are_stored_beside_the_reading() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::new(dir.path());
        let limits = ModelLimits {
            windows: vec![ModelWindow {
                model: "Fable".into(),
                used_percentage: 12.0,
                resets_at: Some(NOW + 3600),
            }],
            observed_at: NOW,
        };
        assert_eq!(store.load_models("a").unwrap(), None);

        store.save_models("a", &limits).unwrap();

        assert_eq!(store.load_models("a").unwrap(), Some(limits));
        assert_eq!(store.load("a").unwrap(), None);
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

    #[test]
    fn a_full_window_without_reset_time_is_limited_with_an_estimate_only() {
        let mut r = reading(None, None);
        r.seven_day = Some(Window {
            used_percentage: 100.0,
            resets_at: None,
        });
        r.observed_at = NOW - 3600;

        assert_eq!(
            r.availability(NOW),
            Availability::Limited {
                until: NOW - 3600 + SEVEN_DAYS_SECS,
                exact: false
            }
        );
    }

    mod usage_report {
        use super::super::*;
        use chrono::{FixedOffset, TimeZone};

        const REPORT: &str =
            "You are currently using your subscription to power your Claude Code usage

Current session: 12% used · resets Oct 7 at 2:20am (Europe/Zurich)
Current week (all models): 90% used · resets Oct 8 at 10pm (Europe/Zurich)
Current week (Fable): 0% used · resets Oct 8 at 10pm (Europe/Zurich)

What's contributing to your limits usage?";

        fn zurich_now() -> chrono::DateTime<FixedOffset> {
            FixedOffset::east_opt(2 * 3600)
                .unwrap()
                .with_ymd_and_hms(2026, 10, 6, 22, 40, 0)
                .unwrap()
        }

        fn local(month: u32, day: u32, hour: u32, minute: u32) -> i64 {
            FixedOffset::east_opt(2 * 3600)
                .unwrap()
                .with_ymd_and_hms(2026, month, day, hour, minute, 0)
                .unwrap()
                .timestamp()
        }

        #[test]
        fn both_windows_and_their_resets_are_read() {
            let reading = parse_usage_report(REPORT, zurich_now()).unwrap();

            let five = reading.five_hour.unwrap();
            assert_eq!(five.used_percentage, 12.0);
            assert_eq!(five.resets_at, Some(local(10, 7, 2, 20)));
            let week = reading.seven_day.unwrap();
            assert_eq!(week.used_percentage, 90.0);
            assert_eq!(week.resets_at, Some(local(10, 8, 22, 0)));
            assert_eq!(reading.source, UsageSource::UsageCommand);
            assert_eq!(reading.observed_at, zurich_now().timestamp());
        }

        #[test]
        fn a_model_with_its_own_weekly_limit_is_read_apart() {
            let limits = parse_model_limits(REPORT, zurich_now());

            assert_eq!(
                limits,
                ModelLimits {
                    windows: vec![ModelWindow {
                        model: "Fable".into(),
                        used_percentage: 0.0,
                        resets_at: Some(local(10, 8, 22, 0)),
                    }],
                    observed_at: zurich_now().timestamp(),
                }
            );
        }

        #[test]
        fn every_model_line_is_kept_in_order() {
            let text = "Current week (all models): 40% used\nCurrent week (Sonnet only): 4% used · resets Oct 9 at 9am (Europe/Zurich)\nCurrent week (Fable): 75% used\n";

            let models: Vec<(String, f64)> = parse_model_limits(text, zurich_now())
                .windows
                .into_iter()
                .map(|w| (w.model, w.used_percentage))
                .collect();

            assert_eq!(
                models,
                vec![
                    ("Sonnet only".to_string(), 4.0),
                    ("Fable".to_string(), 75.0)
                ]
            );
        }

        #[test]
        fn a_window_without_a_reset_keeps_its_share() {
            let text = "Current session: 0% used\nCurrent week (all models): 100% used · resets Oct 11 at 7pm (Europe/Zurich)\n";

            let reading = parse_usage_report(text, zurich_now()).unwrap();

            assert_eq!(reading.five_hour.unwrap().resets_at, None);
            assert_eq!(
                reading.seven_day.unwrap().resets_at,
                Some(local(10, 11, 19, 0))
            );
        }

        #[test]
        fn a_reset_early_in_the_next_year_is_placed_in_it() {
            let december = FixedOffset::east_opt(3600)
                .unwrap()
                .with_ymd_and_hms(2026, 12, 30, 10, 0, 0)
                .unwrap();
            let text =
                "Current week (all models): 40% used · resets Jan 2 at 9am (Europe/Zurich)\n";

            let reading = parse_usage_report(text, december).unwrap();

            let expected = FixedOffset::east_opt(3600)
                .unwrap()
                .with_ymd_and_hms(2027, 1, 2, 9, 0, 0)
                .unwrap()
                .timestamp();
            assert_eq!(reading.seven_day.unwrap().resets_at, Some(expected));
        }

        /// The `/usage` panel as the interactive client draws it for a screen
        /// reader: each label on its line, and the meter's share read twice.
        const PANEL: &str = "Settings  Status   Config   Usage   Stats
Session
Total cost:            $0.0000
Current session
14% 14% used
Resets 11:59pm
Current week (all models)
2% 2% used
Resets Oct 8 at 10pm (Europe/Zurich)
Current week (Fable)
5% 5% used
Resets Oct 9 at 9am (Europe/Zurich)
What's contributing to your usage?
Approximate · sessions on this machine only
Usage credits
75% used";

        #[test]
        fn the_interactive_panel_keeps_each_share_and_reset_in_its_section() {
            let reading = parse_usage_report(PANEL, zurich_now()).unwrap();

            let five = reading.five_hour.unwrap();
            assert_eq!(five.used_percentage, 14.0);
            assert_eq!(five.resets_at, Some(local(10, 6, 23, 59)));
            let week = reading.seven_day.unwrap();
            assert_eq!(week.used_percentage, 2.0);
            assert_eq!(week.resets_at, Some(local(10, 8, 22, 0)));
        }

        #[test]
        fn the_interactive_panel_reads_a_model_limit_apart() {
            let limits = parse_model_limits(PANEL, zurich_now());

            assert_eq!(
                limits.windows,
                vec![ModelWindow {
                    model: "Fable".into(),
                    used_percentage: 5.0,
                    resets_at: Some(local(10, 9, 9, 0)),
                }]
            );
        }

        #[test]
        fn a_plan_without_the_session_limit_still_gives_its_week() {
            let text = "Current week (all models)\n2% 2% used\nResets Oct 8 at 10pm (Europe/Zurich)\nCurrent week (Fable)\n0% 0% used";

            let reading = parse_usage_report(text, zurich_now()).unwrap();

            assert_eq!(reading.five_hour, None);
            assert_eq!(reading.seven_day.unwrap().used_percentage, 2.0);
        }

        #[test]
        fn a_share_left_is_turned_into_a_share_used() {
            let text = "Current week (all models)\n36% remaining\n";

            let reading = parse_usage_report(text, zurich_now()).unwrap();

            assert_eq!(reading.seven_day.unwrap().used_percentage, 64.0);
        }

        #[test]
        fn a_window_not_started_never_borrows_a_percentage_from_what_follows() {
            let text = "Current session\nNot started\nCurrent week (all models)\nNot started\nUsage credits\n75% used\n";

            assert!(parse_usage_report(text, zurich_now()).is_none());
        }

        #[test]
        fn text_without_limits_gives_nothing() {
            assert!(parse_usage_report("You are using an API key.", zurich_now()).is_none());
        }
    }
}
