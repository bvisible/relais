//! The calls the interface makes, all built on `relais-core`.

use std::path::Path;

use relais_core::claude::transcripts;
use relais_core::integration;
use relais_core::registry;
use relais_core::service::Relais;
use relais_core::usage::{Availability, UsageReading, Window};
use relais_core::Layout;
use serde::Serialize;
use tauri::{AppHandle, WebviewWindow};

use crate::platform;
use crate::tray;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindowView {
    used: f64,
    resets_at: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    alias: String,
    email: Option<String>,
    active: bool,
    five_hour: Option<UsageWindowView>,
    seven_day: Option<UsageWindowView>,
    observed_at: Option<i64>,
    limited: bool,
    limited_until: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    accounts: Vec<AccountView>,
    best: Option<String>,
    statusline: bool,
    shell: bool,
    cli_found: bool,
    now: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    id: String,
    title: Option<String>,
    cwd: Option<String>,
    last_activity: i64,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn layout() -> Result<Layout, String> {
    Layout::from_env().map_err(text)
}

fn load() -> Result<Relais, String> {
    Relais::load(layout()?).map_err(text)
}

fn window_view(window: Option<&Window>, used_now: Option<f64>) -> Option<UsageWindowView> {
    window.map(|w| UsageWindowView {
        used: used_now.unwrap_or(w.used_percentage),
        resets_at: w.resets_at,
    })
}

pub fn build_overview(relais: &Relais) -> Overview {
    let now = now();
    let accounts = relais
        .registry
        .accounts
        .iter()
        .map(|account| {
            let reading: Option<UsageReading> = relais.usage_of(&account.alias);
            let limit = reading.as_ref().and_then(|r| match r.availability(now) {
                Availability::Limited { until, exact } => Some(exact.then_some(until)),
                Availability::Available { .. } => None,
            });
            AccountView {
                alias: account.alias.clone(),
                email: account.email.clone(),
                active: relais.registry.active.as_deref() == Some(account.alias.as_str()),
                five_hour: reading
                    .as_ref()
                    .and_then(|r| window_view(r.five_hour.as_ref(), r.five_hour_now(now))),
                seven_day: reading
                    .as_ref()
                    .and_then(|r| window_view(r.seven_day.as_ref(), r.seven_day_now(now))),
                observed_at: reading.as_ref().map(|r| r.observed_at),
                limited: limit.is_some(),
                limited_until: limit.flatten(),
            }
        })
        .collect();
    Overview {
        accounts,
        best: relais.best_other_account(now),
        statusline: integration::statusline_installed(&relais.layout),
        shell: integration::shell_installed(&relais.layout),
        cli_found: platform::relais_cli().is_some(),
        now,
    }
}

#[tauri::command]
pub fn overview() -> Result<Overview, String> {
    Ok(build_overview(&load()?))
}

#[tauri::command]
pub fn switch_account(app: AppHandle, alias: String) -> Result<Overview, String> {
    let mut relais = load()?;
    relais.registry.set_active(&alias).map_err(text)?;
    relais.save().map_err(text)?;
    if let Some(account) = relais.registry.get(&alias).cloned() {
        let _ = relais.prepare_profile(&account);
        let _ = relais.sync_shared_config(now());
    }
    tray::refresh_title(&app);
    Ok(build_overview(&relais))
}

#[tauri::command]
pub fn conversations(limit: usize) -> Result<Vec<ConversationView>, String> {
    let layout = layout()?;
    let home = layout.home.display().to_string();
    let list = transcripts::list_conversations(&layout.claude_dir.join("projects"), limit)
        .map_err(text)?;
    Ok(list
        .into_iter()
        .map(|c| ConversationView {
            id: c.session_id,
            title: c.title,
            cwd: c.cwd.map(|cwd| cwd.replacen(&home, "~", 1)),
            last_activity: c.last_activity,
        })
        .collect())
}

#[tauri::command]
pub fn resume(window: WebviewWindow, session_id: String, target: String) -> Result<(), String> {
    let layout = layout()?;
    let conversation =
        transcripts::find_conversation(&layout.claude_dir.join("projects"), &session_id)
            .map_err(text)?
            .ok_or("conversation not found")?;
    let cwd = conversation
        .cwd
        .clone()
        .filter(|dir| Path::new(dir).is_dir())
        .unwrap_or_else(|| layout.home.display().to_string());
    match target.as_str() {
        "desktop" => platform::resume_in_desktop(&layout, &session_id, &cwd)?,
        _ => platform::open_terminal_with(&layout, &cwd, &["continue", &session_id])?,
    }
    let _ = window.hide();
    Ok(())
}

/// The name Relais will give the account of `email`.
#[tauri::command]
pub fn suggest_alias(email: String) -> Result<String, String> {
    let relais = load()?;
    let taken: Vec<&str> = relais
        .registry
        .accounts
        .iter()
        .map(|a| a.alias.as_str())
        .collect();
    Ok(registry::alias_from_email(&email, &taken))
}

/// Opens Terminal on `relais account add <email>`: Claude Code's sign-in page
/// then opens in a Chrome profile of its own.
#[tauri::command]
pub fn add_account(window: WebviewWindow, email: String) -> Result<(), String> {
    let email = email.trim().to_string();
    let valid =
        email.contains('@') && !email.chars().any(char::is_whitespace) && email.len() <= 254;
    if !valid {
        return Err("invalid email".into());
    }
    let layout = layout()?;
    let home = layout.home.display().to_string();
    platform::open_terminal_with(&layout, &home, &["account", "add", &email])?;
    let _ = window.hide();
    Ok(())
}

#[tauri::command]
pub fn set_statusline(enabled: bool) -> Result<Overview, String> {
    let layout = layout()?;
    if enabled {
        let cli = platform::relais_cli().ok_or("relais command not found")?;
        let command = format!(
            "{} statusline",
            platform::shell_quote(&cli.display().to_string())
        );
        integration::install_statusline(&layout, &command, now()).map_err(text)?;
    } else {
        integration::uninstall_statusline(&layout, now()).map_err(text)?;
    }
    overview()
}

#[tauri::command]
pub fn set_shell(enabled: bool) -> Result<Overview, String> {
    let layout = layout()?;
    if enabled {
        let cli = platform::relais_cli().ok_or("relais command not found")?;
        integration::install_shell(&layout, &cli, now()).map_err(text)?;
    } else {
        integration::uninstall_shell(&layout, now()).map_err(text)?;
    }
    overview()
}

#[tauri::command]
pub fn hide(window: WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
