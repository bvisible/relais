//! The calls the interface makes, all built on `relais-core`.

use std::path::Path;

use relais_core::claude::desktop_app::{DesktopApp, DESKTOP_QUIT_WAIT, DESKTOP_SIGN_IN_WAIT};
use relais_core::claude::desktop_carry::OwnConversations;
use relais_core::claude::desktop_switch::{self, SignIn, Started};
use relais_core::claude::{cli as claude, desktop_profile, transcripts};
use relais_core::history;
use relais_core::integration;
use relais_core::registry;
use relais_core::service::Relais;
use relais_core::usage::{Availability, UsageReading, Window};
use relais_core::Error;
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

/// A weekly limit on one model only (`Fable`), as `/usage` last reported it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelWindowView {
    model: String,
    used: f64,
    resets_at: Option<i64>,
    observed_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    alias: String,
    email: Option<String>,
    /// The account's own Chrome profile; `None` for the account Claude Code
    /// started with, which uses the usual profile.
    chrome_profile: Option<String>,
    /// The Claude desktop app has a data folder for this account, in place or parked.
    desktop_folder: bool,
    /// `#RRGGBB`: the color chosen for the account, or the one it was given.
    color: String,
    active: bool,
    five_hour: Option<UsageWindowView>,
    seven_day: Option<UsageWindowView>,
    /// Limits on single models, which `/usage` reports and the status line does not.
    models: Vec<ModelWindowView>,
    observed_at: Option<i64>,
    limited: bool,
    limited_until: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteColor {
    name: &'static str,
    hex: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    accounts: Vec<AccountView>,
    /// The colors an account can take, named for the interface to translate.
    palette: Vec<PaletteColor>,
    /// The account whose folder the Claude desktop app uses.
    desktop_owner: Option<String>,
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
    /// The last folder of `cwd`, shown as the conversation's project.
    project: Option<String>,
    /// Unix seconds of the conversation's first message.
    started_at: Option<i64>,
    last_activity: i64,
    /// The account that last ran this session, in a terminal or in the desktop app.
    account: Option<String>,
    /// Started in the Claude desktop app rather than in a terminal.
    desktop: bool,
    /// Messages per five minutes over the last hour, oldest first.
    activity: Vec<u32>,
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
                chrome_profile: account.chrome_profile.clone(),
                desktop_folder: desktop_profile::has_folder(
                    &relais.layout,
                    &relais.registry,
                    &account.alias,
                ),
                color: relais
                    .registry
                    .color_of(&account.alias)
                    .unwrap_or_else(|| registry::PALETTE[0].1.to_string()),
                active: relais.registry.active.as_deref() == Some(account.alias.as_str()),
                five_hour: reading
                    .as_ref()
                    .and_then(|r| window_view(r.five_hour.as_ref(), r.five_hour_now(now))),
                seven_day: reading
                    .as_ref()
                    .and_then(|r| window_view(r.seven_day.as_ref(), r.seven_day_now(now))),
                models: relais
                    .model_limits_of(&account.alias)
                    .map(|limits| {
                        limits
                            .windows
                            .iter()
                            .map(|window| ModelWindowView {
                                model: window.model.clone(),
                                used: window.used_now(now),
                                resets_at: window.resets_at,
                                observed_at: limits.observed_at,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                observed_at: reading.as_ref().map(|r| r.observed_at),
                limited: limit.is_some(),
                limited_until: limit.flatten(),
            }
        })
        .collect();
    Overview {
        accounts,
        palette: registry::PALETTE
            .iter()
            .map(|&(name, hex)| PaletteColor { name, hex })
            .collect(),
        desktop_owner: desktop_profile::owner(&relais.layout, &relais.registry),
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

/// Async because the config sync may wait a few seconds for Claude Code to
/// release its file, which must not freeze the menu bar.
#[tauri::command]
pub async fn switch_account(app: AppHandle, alias: String) -> Result<Overview, String> {
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
pub fn set_color(alias: String, color: String) -> Result<Overview, String> {
    let mut relais = load()?;
    relais.registry.set_color(&alias, &color).map_err(text)?;
    relais.save().map_err(text)?;
    Ok(build_overview(&relais))
}

/// Async so that reading the ends of large transcripts never holds up the
/// menu bar: Tauri runs synchronous commands on the main thread.
#[tauri::command]
pub async fn conversations(limit: usize) -> Result<Vec<ConversationView>, String> {
    let relais = load()?;
    let home = relais.layout.home.display().to_string();
    let recent = relais.recent_conversations(limit, now()).map_err(text)?;
    Ok(recent
        .into_iter()
        .map(|recent| {
            let c = recent.conversation;
            let project = c.cwd.as_deref().and_then(|cwd| {
                Path::new(cwd)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
            ConversationView {
                id: c.session_id,
                desktop: c.entrypoint.as_deref() == Some("claude-desktop"),
                title: c.title,
                cwd: c.cwd.map(|cwd| cwd.replacen(&home, "~", 1)),
                project,
                started_at: c.started_at,
                last_activity: recent.last_activity,
                account: recent.account,
                activity: recent.activity,
            }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayView {
    /// Share of the weekly quota used that day; `None` without a reading that day.
    used: Option<f64>,
    /// A window was full that day.
    limited: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStats {
    alias: String,
    /// The last seven days, oldest first.
    days: Vec<DayView>,
    /// Highest weekly share in each of the last four weeks, oldest first.
    weeks: Vec<Option<f64>>,
    conversations_today: usize,
    conversations_week: usize,
    last_seen: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSignIn {
    alias: String,
    /// `None` when Claude Code could not be asked.
    signed_in: Option<bool>,
    email: Option<String>,
    plan: Option<String>,
}

/// Local midnight and the local offset, for the per-day figures.
fn local_day() -> (i64, chrono::FixedOffset) {
    let now = chrono::Local::now();
    let midnight = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp())
        .unwrap_or_else(|| now.timestamp());
    (midnight, *now.offset())
}

/// What each account used, day by day and week by week, and the
/// conversations it carried. Read from local files only.
#[tauri::command]
pub async fn account_stats() -> Result<Vec<AccountStats>, String> {
    let relais = load()?;
    let now = now();
    let (midnight, offset) = local_day();
    let counts = relais.conversation_counts(now, midnight);
    Ok(relais
        .registry
        .accounts
        .iter()
        .map(|account| {
            let samples = relais.usage_history(&account.alias);
            let count = counts.get(&account.alias).cloned().unwrap_or_default();
            AccountStats {
                alias: account.alias.clone(),
                days: history::daily_usage(&samples, now, 7, offset)
                    .into_iter()
                    .map(|day| DayView {
                        used: day.used,
                        limited: day.limited,
                    })
                    .collect(),
                weeks: history::weekly_peaks(&samples, now, 4),
                conversations_today: count.today,
                conversations_week: count.week,
                last_seen: count.last_seen,
            }
        })
        .collect())
}

/// Asks Claude Code for accounts' limits (`claude -p /usage`: nothing billed,
/// nothing saved), one account after the other since each run reads every local
/// transcript. In the background (`force` false), only accounts whose weekly
/// reset is unknown or past, at most every six hours; on request, every account
/// but the one the terminals run on.
#[tauri::command]
pub async fn refresh_usage(force: bool) -> Result<Overview, String> {
    let relais = load()?;
    if let Some(binary) = platform::claude_binary() {
        let now = now();
        for account in &relais.registry.accounts {
            let wanted = if force {
                !relais.measured_lately(&account.alias, now)
            } else {
                relais.needs_usage_refresh(&account.alias, now)
            };
            if wanted {
                let _ = relais.refresh_usage(&account.alias, &binary, chrono::Local::now());
            }
        }
    }
    Ok(build_overview(&load()?))
}

/// Whether each account is still signed in, as Claude Code reports it, or
/// only `only`. Each check starts Claude Code, so they run side by side.
#[tauri::command]
pub async fn account_sign_ins(only: Option<String>) -> Result<Vec<AccountSignIn>, String> {
    let relais = load()?;
    let binary = platform::claude_binary();
    let checks: Vec<_> = relais
        .registry
        .accounts
        .iter()
        .filter(|account| only.as_deref().is_none_or(|alias| alias == account.alias))
        .map(|account| {
            let alias = account.alias.clone();
            let config_dir = account.config_dir.clone();
            let binary = binary.clone();
            std::thread::spawn(move || {
                let status = binary
                    .as_deref()
                    .and_then(|b| claude::auth_status(b, config_dir.as_deref()).ok());
                AccountSignIn {
                    alias,
                    signed_in: status.as_ref().map(|s| s.logged_in),
                    email: status.as_ref().and_then(|s| s.email.clone()),
                    plan: status.and_then(|s| s.subscription_type),
                }
            })
        })
        .collect();
    Ok(checks
        .into_iter()
        .filter_map(|check| check.join().ok())
        .collect())
}

#[tauri::command]
pub fn open_chrome(alias: String) -> Result<(), String> {
    let relais = load()?;
    let account = relais.registry.get(&alias).ok_or("unknown account")?;
    platform::open_chrome(account.chrome_profile.as_deref())
}

/// Opens Terminal on `relais account login <alias>`: the sign-in page opens in
/// the account's own Chrome profile.
#[tauri::command]
pub fn sign_in_again(window: WebviewWindow, alias: String) -> Result<(), String> {
    let layout = layout()?;
    let home = layout.home.display().to_string();
    platform::open_terminal_with(&layout, &home, &["account", "login", &alias])?;
    let _ = window.hide();
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPlanView {
    /// The account in place, which the workspace leaves.
    from: String,
    /// Conversations that follow the user.
    conversations: usize,
    /// The arriving account's conversations of its own: the user chooses.
    own: usize,
    /// `parked`, `oldFolder` or `none` (the app will ask to sign in).
    sign_in: SignIn,
}

/// What switching the desktop app to `alias` would do. Changes nothing.
#[tauri::command]
pub fn desktop_plan(alias: String) -> Result<DesktopPlanView, String> {
    let relais = load()?;
    let plan =
        desktop_switch::plan(&relais.layout, &relais.registry, &alias).map_err(desktop_error)?;
    Ok(DesktopPlanView {
        from: plan.from,
        conversations: plan.conversations,
        own: plan.own.len(),
        sign_in: plan.sign_in,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSwitchView {
    /// `already` or `switched`.
    outcome: &'static str,
    /// How the app started: `signedIn`, `notConfirmed` or `signedOut`.
    started: Option<Started>,
    /// An earlier switch was cut short and has been undone first.
    undid_interrupted: bool,
    overview: Overview,
}

/// Errors the interface translates, by code.
fn desktop_error(error: Error) -> String {
    match error {
        Error::DesktopStillRunning => "desktop-still-running".into(),
        Error::DesktopNotOpened => "desktop-not-opened".into(),
        Error::UnknownDesktopOwner => "desktop-owner-unknown".into(),
        Error::UnknownSignedInAccount(_) => "desktop-unknown-account".into(),
        Error::MissingAccountIds(_) => "desktop-missing-ids".into(),
        Error::OwnConversationsNeedChoice(_) => "desktop-own-needed".into(),
        Error::Conflict(_) => "desktop-conflict".into(),
        other => other.to_string(),
    }
}

/// Switches the Claude desktop app to `alias`: closes it, parks the sign-in
/// in place, puts `alias`'s back with the workspace carried to it, and opens
/// it again. New terminals follow. `own` is `add` or `aside` when the
/// account has conversations of its own. Async, since the app takes a few
/// seconds to close and to start.
#[tauri::command]
pub async fn switch_desktop(
    app: AppHandle,
    alias: String,
    own: Option<String>,
) -> Result<DesktopSwitchView, String> {
    let mut relais = load()?;
    let desktop = DesktopApp::from_env();
    let undid_interrupted =
        desktop_switch::undo_interrupted(&relais.layout, &desktop).map_err(desktop_error)?;
    let plan =
        desktop_switch::plan(&relais.layout, &relais.registry, &alias).map_err(desktop_error)?;
    if plan.is_noop() {
        return Ok(DesktopSwitchView {
            outcome: "already",
            started: None,
            undid_interrupted,
            overview: build_overview(&relais),
        });
    }
    let own = match own.as_deref() {
        Some("add") => Some(OwnConversations::Add),
        Some("aside") => Some(OwnConversations::PutAside),
        _ => None,
    };
    if desktop.is_running() {
        desktop.quit(DESKTOP_QUIT_WAIT).map_err(desktop_error)?;
    }
    let applied = desktop_switch::apply(
        &relais.layout,
        &relais.registry,
        &alias,
        own,
        desktop.is_running(),
        now(),
    )
    .map_err(desktop_error)?;
    if relais.registry.set_active(&alias).is_ok() && relais.save().is_ok() {
        if let Some(account) = relais.registry.get(&alias).cloned() {
            let _ = relais.prepare_profile(&account);
            let _ = relais.sync_shared_config(now());
        }
    }
    tray::refresh_title(&app);
    let started = desktop_switch::open_and_confirm(
        &relais.layout,
        &desktop,
        &applied,
        now(),
        DESKTOP_SIGN_IN_WAIT,
    )
    .map_err(desktop_error)?;
    Ok(DesktopSwitchView {
        outcome: "switched",
        started: Some(started),
        undid_interrupted,
        overview: build_overview(&relais),
    })
}

/// Removes an account from Relais. Its sign-in and its profile stay on disk.
#[tauri::command]
pub fn remove_account(app: AppHandle, alias: String) -> Result<Overview, String> {
    let mut relais = load()?;
    relais.registry.remove(&alias).map_err(text)?;
    relais.save().map_err(text)?;
    tray::refresh_title(&app);
    Ok(build_overview(&relais))
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
