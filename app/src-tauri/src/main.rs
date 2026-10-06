//! Relais: the menu bar app.

mod commands;
mod platform;
mod tray;

use tauri::{Manager, WindowEvent};

/// Set to keep the panel open and centered, for screenshots and checks. The
/// panel then does not take the keyboard from the app in front.
const SHOW_WINDOW_ENV: &str = "RELAIS_SHOW_WINDOW";

fn main() {
    let pinned = std::env::var_os(SHOW_WINDOW_ENV).is_some();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::switch_account,
            commands::set_color,
            commands::conversations,
            commands::resume,
            commands::add_account,
            commands::suggest_alias,
            commands::set_statusline,
            commands::set_shell,
            commands::hide,
            commands::quit,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            tray::create(app.handle())?;
            tray::keep_title_fresh(app.handle().clone());
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                platform::open_on_every_space(&window);
                if pinned {
                    let _ = window.center();
                    let _ = window.show();
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
