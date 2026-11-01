//! What the macOS widget shows, written where the app and the widget share
//! files. The widget reads it on its own schedule: it never asks the app.

use std::path::PathBuf;
use std::time::Duration;

use relais_core::claude::desktop_switch;
use relais_core::fsutil;
use relais_core::service::Relais;
use relais_core::usage::Availability;
use relais_core::Layout;
use serde::Serialize;
use serde_json::{json, Value};

use crate::{platform, tray};

const GROUP: &str = "BT249938WK.ai.neoservice.relais";
const WRITE_EVERY: Duration = Duration::from_secs(60);

#[derive(Serialize)]
struct AccountLine {
    alias: String,
    color: String,
    used: Option<f64>,
    line: String,
    here: bool,
    paused: bool,
}

fn snapshot(relais: &Relais, now: i64, offset: chrono::FixedOffset) -> Value {
    let owner = desktop_switch::signed_in_account(&relais.layout, &relais.registry).ok();
    let french = platform::prefers_french();
    let mut lines: Vec<AccountLine> = relais
        .registry
        .accounts
        .iter()
        .map(|account| {
            let alias = account.alias.clone();
            let reading = relais.usage_of(&alias);
            let availability = reading.as_ref().map(|r| r.availability(now));
            let (used, back_at, paused) = match availability {
                Some(Availability::Available { used }) => (Some(used), None, false),
                Some(Availability::Limited { until, exact }) => {
                    (None, exact.then_some(until), true)
                }
                None => (None, None, false),
            };
            let title = tray::menu_title(
                &alias,
                reading.as_ref().and_then(|r| r.five_hour_now(now)),
                reading.as_ref().and_then(|r| r.seven_day_now(now)),
                back_at,
                now,
                offset,
                french,
            );
            AccountLine {
                line: title
                    .strip_prefix(&format!("{alias} · "))
                    .unwrap_or_default()
                    .to_string(),
                color: relais.registry.color_of(&alias).unwrap_or_default(),
                here: owner.as_deref() == Some(alias.as_str()),
                used,
                paused,
                alias,
            }
        })
        .collect();
    // Claude's account first, then the most room left, the paused ones last.
    lines.sort_by(|a, b| {
        let rank = |line: &AccountLine| (!line.here, line.paused);
        rank(a)
            .cmp(&rank(b))
            .then(a.used.unwrap_or(101.0).total_cmp(&b.used.unwrap_or(101.0)))
    });
    json!({ "accounts": lines })
}

fn file(layout: &Layout) -> PathBuf {
    layout
        .home
        .join("Library/Group Containers")
        .join(GROUP)
        .join("widget.json")
}

pub fn keep_fresh() {
    std::thread::spawn(|| {
        let mut last = Value::Null;
        loop {
            if let Ok(relais) = Layout::from_env().and_then(Relais::load) {
                let offset = *chrono::Local::now().offset();
                let next = snapshot(&relais, chrono::Utc::now().timestamp(), offset);
                let path = file(&relais.layout);
                if let Some(folder) = path.parent() {
                    let _ = std::fs::create_dir_all(folder);
                }
                // Fail open: without the widget, nothing depends on this file.
                if next != last && fsutil::write_json_atomic(&path, &next).is_ok() {
                    last = next;
                }
            }
            std::thread::sleep(WRITE_EVERY);
        }
    });
}
