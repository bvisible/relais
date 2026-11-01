//! Tells the user a limit is near, or an account is back, with a macOS
//! notification, panel closed. Its button opens the confirmation: Relais
//! suggests, it never switches.

use std::sync::Mutex;
use std::time::Duration;

use relais_core::alerts::Alert;
use relais_core::claude::desktop_switch;
use relais_core::service::{DueAlert, Relais};
use relais_core::Layout;
use tauri::AppHandle;

use crate::{notifications, platform};

const LOOK_EVERY: Duration = Duration::from_secs(30);

/// The account to switch to once the desktop app's sessions are done.
static SWITCH_WAITING: Mutex<Option<String>> = Mutex::new(None);

pub fn switch_waiting() -> Option<String> {
    SWITCH_WAITING.lock().ok()?.clone()
}

pub fn wait_to_switch(alias: Option<String>) {
    if let Ok(mut waiting) = SWITCH_WAITING.lock() {
        *waiting = alias;
    }
}

/// Set to show one notification at launch, for checks.
const TEST_ENV: &str = "RELAIS_TEST_NOTIFICATION";

pub fn keep_watching(app: AppHandle) {
    let aliases: Vec<String> = Layout::from_env()
        .and_then(Relais::load)
        .map(|relais| {
            let accounts = relais.registry.accounts.iter();
            accounts.map(|account| account.alias.clone()).collect()
        })
        .unwrap_or_default();
    notifications::prepare(app, &aliases);
    if std::env::var_os(TEST_ENV).is_some() {
        let alias = aliases.first().map_or("main", String::as_str);
        let title = platform::text("alert.near", &[("alias", alias), ("share", "85")]);
        let body = platform::text("alert.near_next", &[("next", alias)]);
        notifications::show(&title, &body, Some(alias));
    }
    std::thread::spawn(move || loop {
        if let Ok(relais) = Layout::from_env().and_then(Relais::load) {
            let owner = desktop_switch::signed_in_account(&relais.layout, &relais.registry).ok();
            let now = chrono::Utc::now().timestamp();
            for due in relais.due_alerts(owner.as_deref(), now) {
                notify_about(due);
            }
            if let Some(alias) = switch_waiting() {
                if relais.live_desktop_sessions(now).is_empty() {
                    wait_to_switch(None);
                    let title = platform::text("alert.sessions_done", &[]);
                    let body = platform::text("alert.sessions_done_detail", &[("alias", &alias)]);
                    notifications::show(&title, &body, Some(&alias));
                }
            }
        }
        std::thread::sleep(LOOK_EVERY);
    });
}

fn notify_about(due: DueAlert) {
    let alias = due.alias.as_str();
    let (title, body, target) = match due.alert {
        Alert::Near(share) => {
            let share = share.to_string();
            let title = platform::text("alert.near", &[("alias", alias), ("share", &share)]);
            match due.next.as_deref() {
                Some(next) => (
                    title,
                    platform::text("alert.near_next", &[("next", next)]),
                    Some(next.to_string()),
                ),
                None => (title, platform::text("alert.near_alone", &[]), None),
            }
        }
        Alert::Back => (
            platform::text("alert.back", &[("alias", alias)]),
            platform::text("alert.back_detail", &[]),
            Some(alias.to_string()),
        ),
    };
    notifications::show(&title, &body, target.as_deref());
}
