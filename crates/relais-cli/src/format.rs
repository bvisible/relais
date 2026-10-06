//! Turns readings and timestamps into short translated text.

use chrono::{Datelike, Local, TimeZone};
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

/// `5h 64% (resets 16:00) · week 21% (resets Mon 13:00) · 3 min ago`.
pub fn usage(reading: Option<&UsageReading>, now: i64) -> String {
    let Some(reading) = reading else {
        return t("usage.never", &[]);
    };
    let freshness = age(reading.observed_at, now);
    if let Availability::Limited { until, exact } = reading.availability(now) {
        let limited = if exact {
            t("usage.limited_until", &[("time", &time(until, now))])
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
                t("usage.resets", &[("time", &time(at, now))])
            ));
        }
        parts.push(part);
    }
    parts.push(freshness);
    parts.join(" · ")
}

/// The percentage an account has used now, for prompts.
pub fn used_now(reading: Option<&UsageReading>, now: i64) -> Option<f64> {
    match reading?.availability(now) {
        Availability::Available { used } => Some(used),
        Availability::Limited { .. } => Some(100.0),
    }
}
