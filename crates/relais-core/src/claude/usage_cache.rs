//! What Claude Code keeps of the usage it asked Anthropic for, in each
//! profile's global config (`.claude.json`): the figures `/usage` shows
//! (`cachedUsageUtilization`) and, when it asked about them, the resets the
//! account holds. Reading them asks no server: Claude Code already did, for
//! its own needs. Nothing in them is a credential.

use std::path::Path;

use serde_json::Value;

use crate::fsutil;
use crate::resets::{self, iso_seconds, HeldResets};
use crate::usage::{ModelLimits, ModelWindow, UsageReading, UsageSource, Window};

/// A profile's global config, when it can be read.
pub fn read_config(path: &Path) -> Option<Value> {
    fsutil::read_json(path).ok().flatten()
}

/// The figures Claude Code last fetched for `account_uuid`, overall and per
/// model, dated when it fetched them. `None` when the profile keeps none, or
/// keeps another account's.
pub fn usage(config: &Value, account_uuid: &str) -> Option<(UsageReading, ModelLimits)> {
    let (usage, observed_at) = cached_usage(config, account_uuid)?;
    let window = |key: &str| {
        let window = usage.get(key)?;
        Some(Window {
            used_percentage: window.get("utilization")?.as_f64()?,
            resets_at: window
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(iso_seconds),
        })
    };
    let (five_hour, seven_day) = (window("five_hour"), window("seven_day"));
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    let windows = usage
        .get("limits")
        .and_then(Value::as_array)
        .map(|limits| {
            limits
                .iter()
                .filter(|limit| limit.get("kind").and_then(Value::as_str) == Some("weekly_scoped"))
                .filter_map(|limit| {
                    Some(ModelWindow {
                        model: limit
                            .pointer("/scope/model/display_name")?
                            .as_str()?
                            .to_string(),
                        used_percentage: limit.get("percent")?.as_f64()?,
                        resets_at: limit
                            .get("resets_at")
                            .and_then(Value::as_str)
                            .and_then(iso_seconds),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some((
        UsageReading {
            five_hour,
            seven_day,
            observed_at,
            session_id: None,
            source: UsageSource::UsageCommand,
        },
        ModelLimits {
            windows,
            observed_at,
        },
    ))
}

/// What Claude Code last learned of the resets `account_uuid` holds: the
/// grants it was given with a usage fetch (`cedar_ember`), or its note that
/// one is held (`unusedLimitResetStatusCache`), whichever is newer.
pub fn held(config: &Value, account_uuid: &str) -> Option<HeldResets> {
    let from_usage = cached_usage(config, account_uuid).and_then(|(usage, observed_at)| {
        resets::held_from_status(usage.get("cedar_ember")?, observed_at)
    });
    let from_note = config
        .get("unusedLimitResetStatusCache")
        .filter(|note| same_account(note, account_uuid))
        .and_then(|note| {
            Some(HeldResets {
                grants: Vec::new(),
                holds_any: note.get("holdsUnusedReset")?.as_bool()?,
                observed_at: note.get("checkedAt")?.as_i64()? / 1000,
            })
        });
    [from_usage, from_note]
        .into_iter()
        .flatten()
        .max_by_key(|held| held.observed_at)
}

/// The usage Claude Code cached for `account_uuid`, and when it fetched it.
fn cached_usage<'a>(config: &'a Value, account_uuid: &str) -> Option<(&'a Value, i64)> {
    let cache = config
        .get("cachedUsageUtilization")
        .filter(|cache| same_account(cache, account_uuid))?;
    let fetched_at = cache.get("fetchedAtMs")?.as_i64()? / 1000;
    Some((cache.get("utilization")?, fetched_at))
}

fn same_account(block: &Value, account_uuid: &str) -> bool {
    block
        .get("accountUuid")
        .and_then(Value::as_str)
        .is_some_and(|owner| owner.eq_ignore_ascii_case(account_uuid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resets::ResetKind;
    use crate::usage::UsageSource;
    use serde_json::json;

    const ACCOUNT: &str = "00000000-0000-4000-8000-000000000001";

    /// The shape Claude Code 2.1.322 writes, with made-up figures.
    fn config(account: &str, cedar_ember: Value) -> Value {
        json!({
            "oauthAccount": {"accountUuid": account, "emailAddress": "one@example.com"},
            "cachedUsageUtilization": {
                "fetchedAtMs": 1_794_604_500_519_i64,
                "accountUuid": account,
                "utilization": {
                    "five_hour": null,
                    "seven_day": {"utilization": 6, "resets_at": "2026-11-19T20:00:00.419345+00:00",
                                  "limit_dollars": null},
                    "seven_day_opus": null,
                    "cedar_ember": cedar_ember,
                    "limits": [
                        {"kind": "weekly_all", "group": "weekly", "percent": 6, "severity": "normal",
                         "resets_at": "2026-11-19T20:00:00.419345+00:00", "scope": null, "is_active": true},
                        {"kind": "weekly_scoped", "group": "weekly", "percent": 3, "severity": "normal",
                         "resets_at": "2026-11-19T20:00:00.419478+00:00",
                         "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
                         "is_active": false}
                    ]
                }
            }
        })
    }

    #[test]
    fn the_figures_claude_code_fetched_make_a_reading() {
        let (reading, models) = usage(&config(ACCOUNT, Value::Null), ACCOUNT).unwrap();

        assert_eq!(reading.observed_at, 1_794_604_500);
        assert_eq!(reading.source, UsageSource::UsageCommand);
        assert!(reading.five_hour.is_none());
        let week = reading.seven_day.unwrap();
        assert_eq!(week.used_percentage, 6.0);
        assert_eq!(week.resets_at, Some(1_795_118_400));
        assert_eq!(models.observed_at, 1_794_604_500);
        assert_eq!(models.windows.len(), 1);
        assert_eq!(models.windows[0].model, "Fable");
        assert_eq!(models.windows[0].used_percentage, 3.0);
    }

    #[test]
    fn another_accounts_figures_are_never_taken() {
        let other = "00000000-0000-4000-8000-000000000002";

        assert!(usage(&config(other, Value::Null), ACCOUNT).is_none());
        assert!(usage(&json!({}), ACCOUNT).is_none());
    }

    #[test]
    fn the_resets_claude_code_asked_about_are_kept() {
        let cedar_ember = json!({"eligible": true, "grants": [
            {"id": "fable-launch", "resets_total": 3, "resets_left": 3,
             "clears": ["seven_day_overage_included"], "gives_back_weekly": true,
             "ends_at": "2027-03-09T07:59:00+00:00"}
        ]});

        let held = held(&config(ACCOUNT, cedar_ember), ACCOUNT).unwrap();

        assert_eq!(held.observed_at, 1_794_604_500);
        assert_eq!(held.grants.len(), 1);
        assert_eq!(held.grants[0].kind, ResetKind::Fable);
        assert_eq!(held.grants[0].left, 3);
    }

    #[test]
    fn a_fetch_that_did_not_ask_about_resets_says_nothing_of_them() {
        assert!(held(&config(ACCOUNT, Value::Null), ACCOUNT).is_none());
    }

    #[test]
    fn claude_codes_note_of_an_unused_reset_is_enough_to_tell_one_is_held() {
        let mut config = config(ACCOUNT, Value::Null);
        config["unusedLimitResetStatusCache"] = json!({
            "accountUuid": ACCOUNT, "holdsUnusedReset": true,
            "checkedAt": 1_794_604_000_000_i64, "validUntil": 1_794_690_400_000_i64
        });

        let held = held(&config, ACCOUNT).unwrap();

        assert!(held.grants.is_empty());
        assert!(held.holds_any);
        assert_eq!(held.observed_at, 1_794_604_000);
    }

    #[test]
    fn the_newest_of_claude_codes_two_answers_wins() {
        let cedar_ember = json!({"eligible": true, "grants": [
            {"id": "w", "resets_left": 0, "clears": ["seven_day"]}
        ]});
        let mut config = config(ACCOUNT, cedar_ember);
        config["unusedLimitResetStatusCache"] = json!({
            "accountUuid": ACCOUNT, "holdsUnusedReset": true,
            "checkedAt": 1_794_000_000_000_i64, "validUntil": 1_794_090_000_000_i64
        });

        let held = held(&config, ACCOUNT).unwrap();

        assert!(!held.holds_any);
        assert_eq!(held.observed_at, 1_794_604_500);
    }
}
