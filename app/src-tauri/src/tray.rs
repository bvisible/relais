//! The menu bar item: the active account's usage at a glance, and a click to
//! open the panel right under it.

use std::time::Duration;

use relais_core::claude::desktop_switch;
use relais_core::service::Relais;
use relais_core::usage::Availability;
use relais_core::Layout;
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Monitor, Rect, WebviewWindow};

use crate::panel;
use crate::panel_geometry::{self, Bounds};
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

/// Shows the panel centered under the menu bar item, or hides it, on the
/// screen that was clicked and whole within it.
fn toggle_panel(app: &AppHandle, rect: Rect) {
    let Some(window) = app.get_webview_window(panel::LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let monitor = clicked_monitor(&window);
    let scale = monitor
        .as_ref()
        .map(Monitor::scale_factor)
        .unwrap_or_else(|| window.scale_factor().unwrap_or(1.0));
    let position = rect.position.to_logical::<f64>(scale);
    let size = rect.size.to_logical::<f64>(scale);
    let preferred = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == panel::LABEL)
        .map(|config| (config.width, config.height))
        .unwrap_or((400.0, 720.0));
    if let Some(monitor) = monitor {
        let work = monitor.work_area();
        let origin = work.position.to_logical::<f64>(scale);
        let extent = work.size.to_logical::<f64>(scale);
        let placement = panel_geometry::below_tray(
            Bounds {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            },
            preferred,
            Bounds {
                x: origin.x,
                y: origin.y,
                width: extent.width,
                height: extent.height,
            },
        );
        // The content already scrolls within the viewport. Fit the window too,
        // so a short screen cannot put the tab bar below the Dock.
        let _ = window.set_size(LogicalSize::new(placement.width, placement.height));
        let _ = window.set_position(LogicalPosition::new(placement.x, placement.y));
    } else {
        // Missing monitor metadata must never stop the panel from opening.
        let _ = window.set_position(LogicalPosition::new(
            position.x + size.width / 2.0 - preferred.0 / 2.0,
            position.y + size.height + 4.0,
        ));
    }
    panel::show(app, true);
}

fn clicked_monitor(window: &WebviewWindow) -> Option<Monitor> {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::{
            objc2::MainThreadMarker,
            objc2_app_kit::{NSEvent, NSScreen},
        };
        // AppKit mouse coordinates use logical points with a bottom-left
        // origin. Tauri's monitor lookup uses logical, top-left coordinates
        // on macOS, unlike the physical coordinates in the tray event.
        if let Some(main_thread) = MainThreadMarker::new() {
            let screens = NSScreen::screens(main_thread);
            if let Some(primary) = screens.firstObject() {
                let cursor = NSEvent::mouseLocation();
                if let Ok(Some(monitor)) =
                    window.monitor_from_point(cursor.x, primary.frame().size.height - cursor.y)
                {
                    return Some(monitor);
                }
            }
        }
    }
    window.current_monitor().ok().flatten()
}

/// The account Claude Desktop uses (the terminal's active one when that is
/// unknown) and its usage, for the menu bar. Empty without accounts.
pub fn title_text() -> String {
    let Ok(relais) = Layout::from_env().and_then(Relais::load) else {
        return String::new();
    };
    let alias = desktop_switch::signed_in_account(&relais.layout, &relais.registry)
        .ok()
        .or_else(|| relais.registry.active().map(|a| a.alias.clone()));
    let Some(alias) = alias else {
        return String::new();
    };
    let now = chrono::Utc::now().timestamp();
    let offset = *chrono::Local::now().offset();
    let french = platform::prefers_french();
    let Some(reading) = relais.usage_of(&alias) else {
        return menu_title(&alias, None, None, None, now, offset, french);
    };
    let back_at = match reading.availability(now) {
        Availability::Limited { until, exact: true } => Some(until),
        _ => None,
    };
    menu_title(
        &alias,
        reading.five_hour_now(now),
        reading.seven_day_now(now),
        back_at,
        now,
        offset,
        french,
    )
}

/// The menu bar title: the account Claude Desktop uses, then each limit its
/// plan has (some plans no longer have the 5-hour one), or when it is back.
pub fn menu_title(
    alias: &str,
    five_hour: Option<f64>,
    week: Option<f64>,
    back_at: Option<i64>,
    now: i64,
    offset: chrono::FixedOffset,
    french: bool,
) -> String {
    if let Some(back) = back_at.filter(|back| *back > now) {
        let when = if back - now < 24 * 3600 {
            chrono::DateTime::from_timestamp(back, 0)
                .map(|at| at.with_timezone(&offset).format("%H:%M").to_string())
                .unwrap_or_default()
        } else if french {
            format!("dans {}", remaining_text(back - now, true))
        } else {
            format!("in {}", remaining_text(back - now, false))
        };
        let back_word = if french { "revient" } else { "back" };
        return format!("{alias} · {back_word} {when}");
    }
    let percent = |used: f64| {
        if french {
            format!("{used:.0} %")
        } else {
            format!("{used:.0}%")
        }
    };
    let week_label = if french { "7j" } else { "7d" };
    let mut parts = vec![alias.to_string()];
    if let Some(used) = five_hour {
        parts.push(format!("5h {}", percent(used)));
    }
    if let Some(used) = week {
        parts.push(format!("{week_label} {}", percent(used)));
    }
    parts.join(" · ")
}

/// A countdown short enough for the menu bar: `45 min`, `1 h 12`, `4 j`
/// (`45m`, `1h12`, `4d` in English), rounded up to the minute.
fn remaining_text(secs: i64, french: bool) -> String {
    let minutes = ((secs.max(0) + 59) / 60).max(1);
    let (hours, days) = (minutes / 60, minutes / (24 * 60));
    match (french, days, hours) {
        (true, 0, 0) => format!("{minutes} min"),
        (true, 0, _) => format!("{hours} h {:02}", minutes % 60),
        (true, _, _) => format!("{days} j"),
        (false, 0, 0) => format!("{minutes}m"),
        (false, 0, _) => format!("{hours}h{:02}", minutes % 60),
        (false, _, _) => format!("{days}d"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-01 10:00 UTC.
    const NOW: i64 = 1_793_527_200;

    fn utc() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn the_title_names_the_account_and_the_limits_its_plan_has() {
        assert_eq!(
            menu_title("main", None, Some(96.6), None, NOW, utc(), true),
            "main · 7j 97 %"
        );
        assert_eq!(
            menu_title("work", Some(12.0), Some(40.0), None, NOW, utc(), true),
            "work · 5h 12 % · 7j 40 %"
        );
        assert_eq!(
            menu_title("work", Some(12.0), Some(40.0), None, NOW, utc(), false),
            "work · 5h 12% · 7d 40%"
        );
        assert_eq!(menu_title("new", None, None, None, NOW, utc(), true), "new");
    }

    #[test]
    fn a_paused_account_says_when_it_is_back() {
        assert_eq!(
            menu_title(
                "hello",
                Some(100.0),
                Some(50.0),
                Some(NOW + 8 * 3600),
                NOW,
                utc(),
                true
            ),
            "hello · revient 18:00"
        );
        assert_eq!(
            menu_title(
                "hello",
                Some(100.0),
                Some(50.0),
                Some(NOW + 8 * 3600),
                NOW,
                utc(),
                false
            ),
            "hello · back 18:00"
        );
        assert_eq!(
            menu_title(
                "spare",
                None,
                Some(100.0),
                Some(NOW + 2 * 86400),
                NOW,
                utc(),
                true
            ),
            "spare · revient dans 2 j"
        );
    }

    #[test]
    fn a_limit_shows_how_long_until_the_account_is_back() {
        assert_eq!(remaining_text(40, true), "1 min");
        assert_eq!(remaining_text(45 * 60, true), "45 min");
        assert_eq!(remaining_text(72 * 60, true), "1 h 12");
        assert_eq!(remaining_text(4 * 86400 + 6 * 3600, true), "4 j");
        assert_eq!(remaining_text(45 * 60, false), "45m");
        assert_eq!(remaining_text(72 * 60, false), "1h12");
        assert_eq!(remaining_text(4 * 86400 + 6 * 3600, false), "4d");
    }
}
