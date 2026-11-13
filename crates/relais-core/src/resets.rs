//! Usage resets. One the user takes (on claude.ai, Settings > Usage, or in
//! Claude Code) shows as a limit that emptied before its time. Those an
//! account still holds are known only when Claude Code asked Anthropic about
//! them: Relais reads what Claude Code kept and never asks itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::fsutil;
use crate::usage::{ModelLimits, UsageReading, Window};

/// The least a limit has to fall to count as a reset: a point or two can be
/// rounding between two sources.
pub const RESET_DROP: f64 = 5.0;

/// How far the end of a window may move and still be the same window: the
/// panel gives it to the minute, the status line to the second.
const SAME_WINDOW: i64 = 3600;

/// Resets taken kept per account, the newest.
const TAKEN_KEPT: usize = 30;

/// The 5-hour limit, as [`ResetTaken::limit`] names it.
pub const SESSION: &str = "session";
/// The weekly limit on all models, as [`ResetTaken::limit`] names it.
pub const WEEK: &str = "week";

/// A limit that emptied between two readings while its window still ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetTaken {
    /// When the emptied limit was read. The reset came between `after` and then.
    pub seen_at: i64,
    pub after: i64,
    /// [`SESSION`], [`WEEK`], or the model of a weekly limit (`Fable`).
    pub limit: String,
    pub from: f64,
    pub to: f64,
}

/// The limits that emptied before their time between two readings of an account.
pub fn taken_between(older: &UsageReading, newer: &UsageReading) -> Vec<ResetTaken> {
    [
        (SESSION, &older.five_hour, &newer.five_hour),
        (WEEK, &older.seven_day, &newer.seven_day),
    ]
    .into_iter()
    .filter_map(|(limit, before, after)| {
        emptied(
            limit,
            before.as_ref()?,
            after.as_ref()?,
            older.observed_at,
            newer.observed_at,
        )
    })
    .collect()
}

/// The same for the weekly limits on single models.
pub fn models_taken_between(older: &ModelLimits, newer: &ModelLimits) -> Vec<ResetTaken> {
    newer
        .windows
        .iter()
        .filter_map(|after| {
            let before = older.windows.iter().find(|w| w.model == after.model)?;
            emptied(
                &after.model,
                &Window {
                    used_percentage: before.used_percentage,
                    resets_at: before.resets_at,
                },
                &Window {
                    used_percentage: after.used_percentage,
                    resets_at: after.resets_at,
                },
                older.observed_at,
                newer.observed_at,
            )
        })
        .collect()
}

/// A limit fell by [`RESET_DROP`] or more before the end its earlier reading
/// gave, and the later reading still describes that window.
fn emptied(
    limit: &str,
    before: &Window,
    after: &Window,
    then: i64,
    now: i64,
) -> Option<ResetTaken> {
    let end = before.resets_at?;
    let same_window =
        now > then && now < end && after.resets_at.is_none_or(|next| next <= end + SAME_WINDOW);
    (same_window && before.used_percentage - after.used_percentage >= RESET_DROP).then(|| {
        ResetTaken {
            seen_at: now,
            after: then,
            limit: limit.to_string(),
            from: before.used_percentage,
            to: after.used_percentage,
        }
    })
}

/// What a reset refills, as Claude Code tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResetKind {
    /// The 5-hour limit.
    Session,
    /// The weekly limit.
    Weekly,
    /// What Fable used of the weekly limit.
    Fable,
    /// Both the 5-hour and the weekly limits.
    Full,
}

/// Resets of one kind an account was granted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetGrant {
    pub kind: ResetKind,
    pub left: u32,
    #[serde(default)]
    pub total: u32,
    /// Unix seconds after which the resets left are lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(default)]
    pub paused: bool,
}

/// The resets an account holds, as Claude Code last learned them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldResets {
    /// By kind. Empty when Claude Code only learned that one is held.
    pub grants: Vec<ResetGrant>,
    /// At least one reset can be used.
    pub holds_any: bool,
    /// Unix seconds when Claude Code learned it.
    pub observed_at: i64,
}

impl HeldResets {
    /// The grants with resets left that `now` can still use.
    pub fn usable(&self, now: i64) -> Vec<&ResetGrant> {
        self.grants
            .iter()
            .filter(|grant| !grant.paused && grant.left > 0)
            .filter(|grant| grant.ends_at.is_none_or(|end| end > now))
            .collect()
    }

    /// Whether a reset can still be used at `now`.
    pub fn any_usable(&self, now: i64) -> bool {
        if self.grants.is_empty() {
            self.holds_any
        } else {
            !self.usable(now).is_empty()
        }
    }
}

/// The resets in the status block Claude Code receives about them
/// (`cedar_ember`), read the way Claude Code reads them. `None` when the block
/// is missing or says nothing.
pub fn held_from_status(status: &Value, observed_at: i64) -> Option<HeldResets> {
    let eligible = status.get("eligible")?.as_bool()?;
    let grants: Vec<ResetGrant> = if eligible {
        status
            .get("grants")
            .and_then(Value::as_array)
            .map(|grants| grants.iter().filter_map(grant_from_status).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let holds_any = grants.iter().any(|grant| !grant.paused && grant.left > 0);
    Some(HeldResets {
        grants,
        holds_any,
        observed_at,
    })
}

fn grant_from_status(grant: &Value) -> Option<ResetGrant> {
    let left = u32::try_from(grant.get("resets_left")?.as_u64()?).ok()?;
    let total = grant
        .get("resets_total")
        .and_then(Value::as_u64)
        .and_then(|total| u32::try_from(total).ok())
        .unwrap_or(left);
    let clears: Vec<&str> = grant
        .get("clears")
        .and_then(Value::as_array)
        .map(|clears| clears.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let flag = |key: &str| grant.get(key).and_then(Value::as_bool).unwrap_or(false);
    let kind = if flag("gives_back_weekly") {
        ResetKind::Fable
    } else {
        match grant.get("type").and_then(Value::as_str) {
            Some("session") => ResetKind::Session,
            Some("weekly") => ResetKind::Weekly,
            Some("full") => ResetKind::Full,
            _ => {
                let five_hour = clears.contains(&"five_hour");
                let others = clears.iter().any(|limit| *limit != "five_hour");
                match (five_hour, others) {
                    (true, false) => ResetKind::Session,
                    (false, true) => ResetKind::Weekly,
                    _ => ResetKind::Full,
                }
            }
        }
    };
    Some(ResetGrant {
        kind,
        left,
        total,
        ends_at: grant
            .get("ends_at")
            .and_then(Value::as_str)
            .and_then(iso_seconds),
        paused: flag("paused"),
    })
}

/// `2026-11-19T20:00:00.419345+00:00` as Unix seconds.
pub(crate) fn iso_seconds(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp())
}

/// An account's resets, taken and held.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountResets {
    /// Oldest first.
    #[serde(default)]
    pub taken: Vec<ResetTaken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<HeldResets>,
}

impl AccountResets {
    /// The resets taken since `since`, newest first.
    pub fn taken_since(&self, since: i64) -> Vec<&ResetTaken> {
        let mut taken: Vec<&ResetTaken> = self
            .taken
            .iter()
            .filter(|reset| reset.seen_at >= since)
            .collect();
        taken.reverse();
        taken
    }
}

/// One file per account under `~/.relais/usage`: `<alias>.resets.json`.
#[derive(Debug, Clone)]
pub struct ResetStore {
    dir: PathBuf,
}

impl ResetStore {
    pub fn new(dir: &Path) -> Self {
        ResetStore {
            dir: dir.to_path_buf(),
        }
    }

    /// What is known of an account's resets; nothing when the file is
    /// missing or unreadable.
    pub fn load(&self, alias: &str) -> AccountResets {
        fsutil::read_json(&self.path(alias))
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// Adds resets taken, each once, keeping the newest.
    pub fn note_taken(&self, alias: &str, taken: &[ResetTaken]) -> Result<()> {
        let mut resets = self.load(alias);
        let mut changed = false;
        for reset in taken {
            let known = resets
                .taken
                .iter()
                .any(|seen| seen.limit == reset.limit && seen.after == reset.after);
            if !known {
                resets.taken.push(reset.clone());
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        resets.taken.sort_by_key(|reset| reset.seen_at);
        let extra = resets.taken.len().saturating_sub(TAKEN_KEPT);
        resets.taken.drain(..extra);
        self.save(alias, &resets)
    }

    /// Keeps what Claude Code learned of the resets held, unless something
    /// newer is kept already.
    pub fn note_held(&self, alias: &str, held: &HeldResets) -> Result<()> {
        let mut resets = self.load(alias);
        if resets
            .held
            .as_ref()
            .is_some_and(|known| known.observed_at >= held.observed_at)
        {
            return Ok(());
        }
        resets.held = Some(held.clone());
        self.save(alias, &resets)
    }

    fn save(&self, alias: &str, resets: &AccountResets) -> Result<()> {
        let path = self.path(alias);
        let value = serde_json::to_value(resets).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        fsutil::write_json_atomic(&path, &value)
    }

    fn path(&self, alias: &str) -> PathBuf {
        self.dir.join(format!("{alias}.resets.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ModelWindow, UsageSource};
    use serde_json::json;

    const NOW: i64 = 1_794_600_000;
    const WEEK_END: i64 = NOW + 5 * 86_400;

    fn reading(at: i64, five: Option<(f64, i64)>, week: Option<(f64, i64)>) -> UsageReading {
        let window = |(used, end): (f64, i64)| Window {
            used_percentage: used,
            resets_at: Some(end),
        };
        UsageReading {
            five_hour: five.map(window),
            seven_day: week.map(window),
            observed_at: at,
            session_id: None,
            source: UsageSource::UsageCommand,
        }
    }

    #[test]
    fn a_week_that_empties_before_its_end_was_reset() {
        let before = reading(NOW - 3600, None, Some((100.0, WEEK_END)));
        let after = reading(NOW, None, Some((5.0, WEEK_END)));

        let taken = taken_between(&before, &after);

        assert_eq!(
            taken,
            vec![ResetTaken {
                seen_at: NOW,
                after: NOW - 3600,
                limit: WEEK.into(),
                from: 100.0,
                to: 5.0,
            }]
        );
    }

    #[test]
    fn a_week_that_starts_over_at_its_end_was_not_reset() {
        let before = reading(NOW - 3600, None, Some((100.0, NOW - 60)));
        let after = reading(NOW, None, Some((0.0, NOW - 60 + 7 * 86_400)));

        assert!(taken_between(&before, &after).is_empty());
    }

    #[test]
    fn a_new_window_is_not_a_reset_even_when_read_early() {
        // The later reading ends a week later: it describes the next window.
        let before = reading(NOW - 3600, None, Some((80.0, WEEK_END)));
        let after = reading(NOW, None, Some((2.0, WEEK_END + 7 * 86_400)));

        assert!(taken_between(&before, &after).is_empty());
    }

    #[test]
    fn a_point_or_two_of_rounding_is_not_a_reset() {
        let before = reading(NOW - 3600, None, Some((41.0, WEEK_END)));
        let after = reading(NOW, None, Some((39.0, WEEK_END)));

        assert!(taken_between(&before, &after).is_empty());
    }

    #[test]
    fn without_the_end_of_the_window_nothing_can_be_told() {
        let mut before = reading(NOW - 3600, None, Some((100.0, WEEK_END)));
        before.seven_day.as_mut().unwrap().resets_at = None;
        let after = reading(NOW, None, Some((5.0, WEEK_END)));

        assert!(taken_between(&before, &after).is_empty());
    }

    #[test]
    fn the_five_hour_limit_is_told_apart_from_the_week() {
        let before = reading(NOW - 600, Some((100.0, NOW + 3600)), Some((60.0, WEEK_END)));
        let after = reading(NOW, Some((0.0, NOW + 3600)), Some((60.0, WEEK_END)));

        let taken = taken_between(&before, &after);

        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].limit, SESSION);
    }

    #[test]
    fn a_fable_reset_shows_on_the_fable_limit() {
        let limits = |at: i64, used: f64| ModelLimits {
            windows: vec![ModelWindow {
                model: "Fable".into(),
                used_percentage: used,
                resets_at: Some(WEEK_END),
            }],
            observed_at: at,
        };

        let taken = models_taken_between(&limits(NOW - 3600, 80.0), &limits(NOW, 0.0));

        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].limit, "Fable");
        assert_eq!((taken[0].from, taken[0].to), (80.0, 0.0));
    }

    fn status(grants: Value) -> Value {
        json!({"eligible": true, "grants": grants, "next_grant_id": null})
    }

    #[test]
    fn held_resets_are_read_the_way_claude_code_reads_them() {
        let held = held_from_status(
            &status(json!([
                {"id": "fable-launch", "label": "Fable reset", "resets_total": 3,
                 "resets_left": 2, "ends_at": "2027-03-09T07:59:00+00:00",
                 "clears": ["seven_day_overage_included"], "gives_back_weekly": true},
                {"id": "weekly-1", "label": "", "resets_total": 1, "resets_left": 1,
                 "clears": ["seven_day"], "type": "weekly"},
                {"id": "session-1", "label": "", "resets_left": 1, "clears": ["five_hour"]},
                {"id": "both", "label": "", "resets_left": 0, "clears": ["five_hour", "seven_day"]}
            ])),
            NOW,
        )
        .unwrap();

        let kinds: Vec<(ResetKind, u32)> = held.grants.iter().map(|g| (g.kind, g.left)).collect();
        assert_eq!(
            kinds,
            vec![
                (ResetKind::Fable, 2),
                (ResetKind::Weekly, 1),
                (ResetKind::Session, 1),
                (ResetKind::Full, 0),
            ]
        );
        assert_eq!(held.grants[0].total, 3);
        assert_eq!(held.grants[0].ends_at, Some(1_804_579_140));
        assert!(held.holds_any);
        assert_eq!(held.usable(NOW).len(), 3);
    }

    #[test]
    fn spent_paused_or_expired_resets_cannot_be_used() {
        let held = held_from_status(
            &status(json!([
                {"id": "a", "resets_left": 0, "clears": ["seven_day"]},
                {"id": "b", "resets_left": 1, "clears": ["seven_day"], "paused": true},
                {"id": "c", "resets_left": 1, "clears": ["seven_day"],
                 "ends_at": "2026-11-01T00:00:00+00:00"}
            ])),
            NOW,
        )
        .unwrap();

        assert!(held.usable(NOW).is_empty());
        assert!(!held.any_usable(NOW));
    }

    #[test]
    fn an_account_not_eligible_holds_none() {
        let held = held_from_status(
            &json!({"eligible": false, "ineligible_reason": "tier"}),
            NOW,
        )
        .unwrap();

        assert!(held.grants.is_empty());
        assert!(!held.any_usable(NOW));
        assert!(held_from_status(&Value::Null, NOW).is_none());
    }

    #[test]
    fn each_reset_is_kept_once_and_only_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let store = ResetStore::new(dir.path());
        let reset = |at: i64| ResetTaken {
            seen_at: at,
            after: at - 60,
            limit: WEEK.into(),
            from: 100.0,
            to: 0.0,
        };

        store.note_taken("main", &[reset(NOW)]).unwrap();
        store.note_taken("main", &[reset(NOW)]).unwrap();
        assert_eq!(store.load("main").taken.len(), 1);

        let many: Vec<ResetTaken> = (0..40).map(|i| reset(NOW + i * 3600)).collect();
        store.note_taken("main", &many).unwrap();
        let kept = store.load("main");
        assert_eq!(kept.taken.len(), TAKEN_KEPT);
        assert_eq!(kept.taken.last().unwrap().seen_at, NOW + 39 * 3600);
        assert_eq!(kept.taken_since(NOW + 38 * 3600).len(), 2);
        assert_eq!(
            kept.taken_since(NOW + 38 * 3600)[0].seen_at,
            NOW + 39 * 3600
        );
    }

    #[test]
    fn an_older_answer_about_held_resets_never_replaces_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = ResetStore::new(dir.path());
        let held = |at: i64, holds: bool| HeldResets {
            grants: Vec::new(),
            holds_any: holds,
            observed_at: at,
        };

        store.note_held("main", &held(NOW, true)).unwrap();
        store.note_held("main", &held(NOW - 60, false)).unwrap();

        assert_eq!(store.load("main").held, Some(held(NOW, true)));
        assert_eq!(
            ResetStore::new(dir.path()).load("other"),
            AccountResets::default()
        );
    }
}
