//! The panel under the menu bar item. On macOS it is an `NSPanel` that does
//! not activate Relais: it opens on the Space in front, over an app in full
//! screen, without pulling the screen back to the desktop.

use tauri::{AppHandle, Manager, WebviewWindow};

pub const LABEL: &str = "main";

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(RelaisPanel {
        config: {
            can_become_key_window: true,
            is_floating_panel: true
        }
    })
}

/// Turns the main window into the panel. As a plain window it stayed on the
/// desktop Space: with an app in full screen in front, a click on the menu
/// bar item either showed nothing or slid the screen back to the desktop.
pub fn prepare(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::{CollectionBehavior, PanelLevel, StyleMask, WebviewWindowExt};
        let panel = window.to_panel::<RelaisPanel>()?;
        panel.set_level(PanelLevel::Status.value());
        // A refused style change leaves a floating panel that activates Relais
        // when it opens: still usable, so it does not stop the app.
        let _ = panel.add_style_mask(StyleMask::empty().nonactivating_panel().value());
        panel.set_collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary()
                .value(),
        );
        panel.set_hides_on_deactivate(false);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    Ok(())
}

/// Shows the panel. `take_keyboard` makes it the key window, so that its
/// fields take typing; it never activates Relais.
pub fn show(app: &AppHandle, take_keyboard: bool) {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        if let Ok(panel) = app.get_webview_panel(LABEL) {
            if take_keyboard {
                panel.show_and_make_key();
            } else {
                panel.show();
            }
            return;
        }
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.show();
        if take_keyboard {
            let _ = window.set_focus();
        }
    }
}
