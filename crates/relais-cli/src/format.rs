//! Turns readings and timestamps into short translated text.

use chrono::{Datelike, Local, TimeZone};
use relais_core::resets::{AccountResets, ResetKind, SESSION, WEEK};
use relais_core::usage::{Availability, UsageReading};

use crate::i18n::t;

/// `16:00` today, `Mon 16:00` within a week, `13.10. 16:00` beyond.
pub fn time(ts: i64, now: i64) -> String {
    let (Some(at), Some(today)) = (
        Local.timestamp_opt(ts, 0).single(),
        Local.timestamp_opt(now, 0).single(),
    ) else {
        return String::new();
    };
    let hm = at.format("%H:%M").to_string();
    let days_apart = (at.date_naive() - today.date_naive()).num_days();
    if days_apart == 0 {
        hm
    } else if (1..7).contains(&days_apart) {
        let names = t("weekdays", &[]);
        let day = names
            .split(',')
            .nth(at.weekday().num_days_from_monday() as usize)
            .unwrap_or("")
            .to_string();
        format!("{day} {hm}")
    } else {
        format!("{} {hm}", at.format("%d.%m."))
    }
}

/// How long until `ts`: `45 min`, `1h 05m`, `4d 6h` (`1 h 05`, `4 j 6 h` in French).
/// Rounded to the nearest minute, so a reading taken a second ago does not
/// lose a minute.
pub fn remaining(ts: i64, now: i64) -> String {
    let minutes = ((ts - now).max(0) + 30) / 60;
    if minutes < 1 {
        return t("in.under_minute", &[]);
    }
    if minutes < 60 {
        return t("in.minutes", &[("m", &minutes.to_string())]);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return t(
            "in.hours",
            &[
                ("h", &hours.to_string()),
                ("mm", &format!("{:02}", minutes % 60)),
            ],
        );
    }
    let (days, hours) = (hours / 24, hours % 24);
    if hours == 0 {
        t("in.days_only", &[("d", &days.to_string())])
    } else {
        t(
            "in.days",
            &[("d", &days.to_string()), ("h", &hours.to_string())],
        )
    }
}

pub fn age(observed_at: i64, now: i64) -> String {
    let minutes = (now - observed_at).max(0) / 60;
    if minutes < 1 {
        t("age.now", &[])
    } else if minutes < 60 {
        t("age.minutes", &[("n", &minutes.to_string())])
    } else if minutes < 48 * 60 {
        t("age.hours", &[("n", &(minutes / 60).to_string())])
    } else {
        t("age.days", &[("n", &(minutes / (24 * 60)).to_string())])
    }
}

pub fn percent(value: f64) -> String {
    t("usage.percent", &[("n", &format!("{:.0}", value))])
}

/// `5h 64% (resets in 1h 30m) · week 21% (resets in 1d 6h) · 3 min ago`.
pub fn usage(reading: Option<&UsageReading>, now: i64) -> String {
    let Some(reading) = reading else {
        return t("usage.never", &[]);
    };
    let freshness = age(reading.observed_at, now);
    if let Availability::Limited { until, exact } = reading.availability(now) {
        let limited = if exact {
            t(
                "usage.limited_until",
                &[("in", &remaining(until, now)), ("time", &time(until, now))],
            )
        } else {
            t("usage.limited_unknown", &[])
        };
        return format!("{limited} · {freshness}");
    }
    let mut parts = Vec::new();
    let windows = [
        (
            "usage.five_hour",
            reading.five_hour_now(now),
            reading.five_hour.as_ref().and_then(|w| w.resets_at),
        ),
        (
            "usage.week",
            reading.seven_day_now(now),
            reading.seven_day.as_ref().and_then(|w| w.resets_at),
        ),
    ];
    for (key, used, resets_at) in windows {
        let Some(used) = used else { continue };
        let mut part = t(key, &[("pct", &percent(used))]);
        if let Some(at) = resets_at.filter(|at| *at > now) {
            part.push_str(&format!(
                " ({})",
                t("usage.resets", &[("in", &remaining(at, now))])
            ));
        }
        parts.push(part);
    }
    parts.push(freshness);
    parts.join(" · ")
}

/// How long resets taken are told: the length of a week.
const RESETS_TAKEN_SHOWN_FOR: i64 = 7 * 86_400;

/// What is known of an account's resets, a line each: the last ones taken
/// this week (`↺ reset 2 h ago: week 100% → 5%`), and those left to use
/// (`↺ to use: 2 Fable (until 21.02.)`), as Claude Code last learned them.
pub fn resets(resets: &AccountResets, now: i64) -> Vec<String> {
    let mut lines: Vec<String> = resets
        .taken_since(now - RESETS_TAKEN_SHOWN_FOR)
        .into_iter()
        .take(2)
        .map(|reset| {
            let limit = match reset.limit.as_str() {
                SESSION => t("resets.limit.session", &[]),
                WEEK => t("resets.limit.week", &[]),
                model => model.to_string(),
            };
            t(
                "resets.taken",
                &[
                    ("age", &age(reset.seen_at, now)),
                    ("limit", &limit),
                    ("from", &percent(reset.from)),
                    ("to", &percent(reset.to)),
                ],
            )
        })
        .collect();
    let Some(held) = resets.held.as_ref().filter(|held| held.any_usable(now)) else {
        return lines;
    };
    let usable = held.usable(now);
    let mut line = if usable.is_empty() {
        t("resets.held_unknown", &[])
    } else {
        let list: Vec<String> = usable
            .iter()
            .map(|grant| {
                let mut item = format!("{} {}", grant.left, reset_kind(grant.kind));
                if let Some(end) = grant.ends_at {
                    item.push_str(&format!(
                        " ({})",
                        t("resets.until", &[("date", &date(end))])
                    ));
                }
                item
            })
            .collect();
        t("resets.held", &[("list", &list.join(", "))])
    };
    if now - held.observed_at >= 86_400 {
        line.push_str(&format!(
            " · {}",
            t("resets.seen", &[("age", &age(held.observed_at, now))])
        ));
    }
    lines.push(line);
    lines
}

fn reset_kind(kind: ResetKind) -> String {
    t(
        match kind {
            ResetKind::Session => "resets.kind.session",
            ResetKind::Weekly => "resets.kind.weekly",
            ResetKind::Fable => "resets.kind.fable",
            ResetKind::Full => "resets.kind.full",
        },
        &[],
    )
}

/// `21.02.`, in the machine's time zone.
pub fn date(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|at| at.format("%d.%m.").to_string())
        .unwrap_or_default()
}

/// The percentage an account has used now, for prompts.
pub fn used_now(reading: Option<&UsageReading>, now: i64) -> Option<f64> {
    match reading?.availability(now) {
        Availability::Available { used } => Some(used),
        Availability::Limited { .. } => Some(100.0),
    }
}
