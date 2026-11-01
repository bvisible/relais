//! Relais: the menu bar app.

mod alerts;
mod commands;
mod measure;
mod notifications;
mod panel;
mod panel_geometry;
mod platform;
mod tray;
mod updates;
mod widget;

use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Set to keep the panel open and centered, for screenshots and checks. The
/// panel then does not take the keyboard from the app in front.
const SHOW_WINDOW_ENV: &str = "RELAIS_SHOW_WINDOW";

/// Control-Option-R opens the panel: Command-Option-R reloads a browser's page.
fn panel_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyR)
}

/// Takes or gives back the shortcut. Fail open: another app may hold it.
pub fn set_panel_shortcut(app: &AppHandle, on: bool) {
    let shortcuts = app.global_shortcut();
    let _ = if on {
        shortcuts.register(panel_shortcut())
    } else {
        shortcuts.unregister(panel_shortcut())
    };
}

fn main() {
    let pinned = std::env::var_os(SHOW_WINDOW_ENV).is_some();
    let builder = tauri::Builder::default();
    // A menu bar app starting at login or after an update must not take the
    // front from the app in use (macOS 13 still honors the launch activation).
    #[cfg(target_os = "macos")]
    let builder = builder
        .activate_ignoring_other_apps(false)
        .plugin(tauri_nspanel::init());
    builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        tray::toggle_from_keyboard(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::switch_account,
            commands::set_color,
            commands::account_stats,
            commands::account_sign_ins,
            commands::sign_in_again,
            commands::allow_measure,
            commands::remove_account,
            commands::switch_desktop,
            commands::desktop_plan,
            commands::refresh_usage,
            commands::conversations,
            commands::resume,
            commands::add_account,
            commands::suggest_alias,
            commands::set_statusline,
            commands::set_shell,
            commands::set_alerts,
            commands::set_shortcut,
            commands::wait_to_switch,
            commands::bring_back,
            commands::project_usage,
            commands::remove_folder,
            commands::hide,
            commands::quit,
            updates::check_update,
            updates::install_update,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            tray::create(app.handle())?;
            tray::keep_title_fresh(app.handle().clone());
            measure::keep_measuring();
            widget::keep_fresh();
            let shortcut = relais_core::Layout::from_env()
                .and_then(relais_core::service::Relais::load)
                .map(|relais| relais.shortcut_on())
                .unwrap_or(true);
            set_panel_shortcut(app.handle(), shortcut);
            alerts::keep_watching(app.handle().clone());
            if let Some(window) = app.get_webview_window(panel::LABEL) {
                // Fail open: as a plain window the panel still works on the desktop.
                if let Err(error) = panel::prepare(&window) {
                    eprintln!("Relais: the panel stays a plain window ({error})");
                }
                if pinned {
                    let _ = window.center();
                    panel::show(app.handle(), false);
                }
            }
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::Focused(false) = event {
                if !pinned {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Relais could not start");
}
