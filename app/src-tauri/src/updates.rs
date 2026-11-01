//! New versions of Relais, from the project's GitHub releases. Each is checked
//! against the public key in `tauri.conf.json`, and installed on the user's
//! click only.

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    version: String,
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<AvailableUpdate>, String> {
    let update = app.updater().map_err(text)?.check().await.map_err(text)?;
    Ok(update.map(|update| AvailableUpdate {
        version: update.version,
    }))
}

/// Replaces the app and restarts it. On failure the app stays as it was.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let Some(update) = app.updater().map_err(text)?.check().await.map_err(text)? else {
        return Ok(());
    };
    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        eprintln!("Relais: the update failed: {error}");
        return Err(error.to_string());
    }
    app.restart();
}
