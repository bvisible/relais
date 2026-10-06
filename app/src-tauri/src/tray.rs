//! The menu bar item: the active account's usage at a glance, and a click to
//! open the panel right under it.

use std::time::Duration;

use relais_core::service::Relais;
use relais_core::usage::Availability;
use relais_core::Layout;
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Rect};

use crate::platform;

const TRAY_ID: &str = "relais";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let icon = Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Relais")
        .title(title_text())
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                toggle_panel(tray.app_handle(), rect);
            }
        })
        .build(app)?;
    Ok(())
}

/// Shows the panel centered under the menu bar item, or hides it.
fn toggle_panel(app: &AppHandle, rect: Rect) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let position = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    let width = window
        .outer_size()
        .map(|s| f64::from(s.width))
        .unwrap_or(380.0 * scale);
    let x = position.x + size.width / 2.0 - width / 2.0;
    let y = position.y + size.height + 4.0 * scale;
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.show();
    let _ = window.set_focus();
}

/// `20 %` for the active account's 5-hour window, or the time it comes back
/// when it is limited. Empty without accounts or readings.
pub fn title_text() -> String {
    let Ok(relais) = Layout::from_env().and_then(Relais::load) else {
        return String::new();
    };
    let Some(active) = relais.registry.active() else {
        return String::new();
    };
    let Some(reading) = relais.usage_of(&active.alias) else {
        return String::new();
    };
    let now = chrono::Utc::now().timestamp();
    match reading.availability(now) {
        Availability::Limited { until, exact: true } => chrono::DateTime::from_timestamp(until, 0)
            .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
            .unwrap_or_default(),
        Availability::Limited { exact: false, .. } => "100 %".to_string(),
        Availability::Available { .. } => match reading.five_hour_now(now) {
            Some(used) if platform::prefers_french() => format!("{used:.0} %"),
            Some(used) => format!("{used:.0}%"),
            None => String::new(),
        },
    }
}

pub fn refresh_title(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_title(Some(title_text()));
    }
}

/// Usage readings arrive whenever a Claude Code session renders its status
/// line; the title follows them every few seconds.
pub fn keep_title_fresh(app: AppHandle) {
    std::thread::spawn(move || loop {
        refresh_title(&app);
        std::thread::sleep(Duration::from_secs(15));
    });
}
