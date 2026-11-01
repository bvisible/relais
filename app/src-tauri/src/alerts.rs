//! Tells the user a limit is near, or an account is back, with a macOS
//! notification, panel closed. Its button opens the confirmation: Relais
//! suggests, it never switches.

use std::time::Duration;

use mac_notification_sys::{MainButton, Notification, NotificationResponse};
use relais_core::alerts::Alert;
use relais_core::claude::desktop_switch;
use relais_core::service::{DueAlert, Relais};
use relais_core::Layout;
use tauri::{AppHandle, Emitter};

use crate::{platform, tray};

const BUNDLE_ID: &str = "ai.neoservice.relais";
const LOOK_EVERY: Duration = Duration::from_secs(30);

/// What the panel listens to: the account to confirm a switch to.
pub const SWITCH_TO_EVENT: &str = "relais-switch-to";

/// Set to show one notification at launch, for checks.
const TEST_ENV: &str = "RELAIS_TEST_NOTIFICATION";

pub fn keep_watching(app: AppHandle) {
    let _ = mac_notification_sys::set_application(BUNDLE_ID);
    if std::env::var_os(TEST_ENV).is_some() {
        let title = platform::text("alert.near", &[("alias", "main"), ("share", "85")]);
        let body = platform::text("alert.near_next", &[("next", "spare")]);
        notify(app.clone(), title, body, Some("spare".to_string()));
    }
    std::thread::spawn(move || loop {
        if let Ok(relais) = Layout::from_env().and_then(Relais::load) {
            let owner = desktop_switch::signed_in_account(&relais.layout, &relais.registry).ok();
            let now = chrono::Utc::now().timestamp();
            for due in relais.due_alerts(owner.as_deref(), now) {
                notify_about(&app, due);
            }
        }
        std::thread::sleep(LOOK_EVERY);
    });
}

fn notify_about(app: &AppHandle, due: DueAlert) {
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
    notify(app.clone(), title, body, target);
}

/// Shows a notification; its button, or a click on it, opens the panel on the
/// confirmation for `target`. Waits for the answer on a thread of its own.
pub fn notify(app: AppHandle, title: String, body: String, target: Option<String>) {
    std::thread::spawn(move || {
        let action = target
            .as_deref()
            .map(|alias| platform::text("alert.switch", &[("alias", alias)]));
        let mut notification = Notification::new();
        notification
            .title(&title)
            .message(&body)
            .wait_for_click(true);
        if let Some(action) = action.as_deref() {
            notification.main_button(MainButton::SingleAction(action));
        }
        // Fail open: a notification that cannot show leaves the panel as it is.
        let Ok(response) = notification.send() else {
            return;
        };
        if matches!(
            response,
            NotificationResponse::ActionButton(_) | NotificationResponse::Click
        ) {
            tray::open_panel(&app);
            if let Some(alias) = target {
                let _ = app.emit(SWITCH_TO_EVENT, alias);
            }
        }
    });
}
