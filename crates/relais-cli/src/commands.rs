//! What each `relais` subcommand does.

use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use relais_core::claude::cli as claude;
use relais_core::claude::desktop_app::{DesktopApp, DESKTOP_QUIT_WAIT};
use relais_core::claude::{desktop_profile, transcripts};
use relais_core::registry::{self, Account};
use relais_core::service::Relais;
use relais_core::usage::Availability;
use relais_core::{integration, Error, Layout};

use crate::format;
use crate::i18n::t;

/// A failure already worded for the user, or one from the core.
pub enum Failure {
    Core(Error),
    Message(String),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Failure::Core(error)
    }
}

impl Failure {
    pub fn describe(&self) -> String {
        match self {
            Failure::Message(message) => message.clone(),
            Failure::Core(Error::UnknownAccount(alias)) => {
                t("error.unknown_account", &[("alias", alias)])
            }
            Failure::Core(Error::DuplicateAlias(alias)) => {
                t("error.duplicate_alias", &[("alias", alias)])
            }
            Failure::Core(Error::DuplicateEmail(email)) => {
                t("error.duplicate_email", &[("email", email)])
            }
            Failure::Core(Error::NoAccounts) => t("error.no_accounts", &[]),
            Failure::Core(Error::ClaudeNotFound) => t("error.claude_not_found", &[]),
            Failure::Core(Error::DesktopStillRunning) => t("desktop.still_running", &[]),
            Failure::Core(Error::DesktopNotOpened) => t("desktop.open_failed", &[]),
            Failure::Core(other) => t("error.generic", &[("details", &other.to_string())]),
        }
    }
}

pub type Outcome = Result<i32, Failure>;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn warn(message: String) {
    eprintln!("{}", t("warn.prefix", &[("message", &message)]));
}

fn claude_binary() -> Result<PathBuf, Failure> {
    let override_path = std::env::var_os(claude::CLAUDE_BIN_ENV).map(PathBuf::from);
    claude::find_claude_binary(
        override_path.as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
    .ok_or(Failure::Core(Error::ClaudeNotFound))
}

fn load() -> Result<Relais, Failure> {
    Ok(Relais::load(Layout::from_env()?)?)
}

fn require_accounts(relais: &Relais) -> Result<(), Failure> {
    if relais.registry.accounts.is_empty() {
        return Err(Failure::Core(Error::NoAccounts));
    }
    Ok(())
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

/// Ignores Ctrl+C and Ctrl+\ in Relais while Claude runs in the foreground, so
/// they reach Claude only and Relais is still there when Claude exits.
struct IgnoreInterrupts {
    previous: [(i32, usize); 2],
}

extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

const SIGINT: i32 = 2;
const SIGQUIT: i32 = 3;
const SIG_IGN: usize = 1;

impl IgnoreInterrupts {
    fn new() -> Self {
        // SAFETY: `signal` with SIG_IGN only changes this process's disposition.
        let previous = unsafe {
            [
                (SIGINT, signal(SIGINT, SIG_IGN)),
                (SIGQUIT, signal(SIGQUIT, SIG_IGN)),
            ]
        };
        IgnoreInterrupts { previous }
    }
}

impl Drop for IgnoreInterrupts {
    fn drop(&mut self) {
        for (signum, handler) in self.previous {
            // SAFETY: restores the disposition saved in `new`.
            unsafe {
                signal(signum, handler);
            }
        }
    }
}

/// Runs Claude Code for one profile and waits for it.
fn run_claude(
    binary: &Path,
    config_dir: Option<&Path>,
    args: &[OsString],
    cwd: Option<&Path>,
) -> Result<ExitStatus, Failure> {
    let mut command = claude::command_for_profile(binary, config_dir);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|e| Failure::Message(t("error.generic", &[("details", &e.to_string())])))?;
    // Installed after the spawn: an ignored signal would be inherited by Claude.
    let _guard = IgnoreInterrupts::new();
    child
        .wait()
        .map_err(|e| Failure::Message(t("error.generic", &[("details", &e.to_string())])))
}

/// Links the profile and syncs shared settings; problems are reported, never fatal.
fn prepare(relais: &Relais, account: &Account) {
    if let Err(error) = relais.prepare_profile(account) {
        warn(t(
            "warn.profile",
            &[("alias", &account.alias), ("details", &error.to_string())],
        ));
    }
    if let Err(error) = relais.sync_shared_config(now()) {
        warn(t("warn.sync", &[("details", &error.to_string())]));
    }
}

pub fn init(alias: Option<String>) -> Outcome {
    let binary = claude_binary()?;
    let mut relais = load()?;
    if !relais.registry.accounts.is_empty() {
        println!("{}", t("init.already", &[]));
        return list();
    }
    let status = claude::auth_status(&binary, None)?;
    if !status.logged_in {
        return Err(Failure::Message(t("error.not_logged_in_default", &[])));
    }
    let identity = claude::read_account_identity(&relais.layout.claude_global_config)?;
    let email = status
        .email
        .clone()
        .or_else(|| identity.as_ref().and_then(|i| i.email.clone()));
    let alias = match alias {
        Some(alias) => validate_alias(&alias)?,
        None => alias_from_email(email.as_deref()),
    };
    relais.registry.add(Account {
        alias: alias.clone(),
        email: email.clone(),
        account_uuid: identity.as_ref().and_then(|i| i.account_uuid.clone()),
        org_id: identity
            .as_ref()
            .and_then(|i| i.org_id.clone())
            .or(status.org_id),
        config_dir: None,
        chrome_profile: None,
        color: None,
    })?;
    relais.save()?;
    println!(
        "{}",
        t(
            "init.done",
            &[
                ("alias", &alias),
                ("email", email.as_deref().unwrap_or("?"))
            ]
        )
    );
    println!("{}", t("init.next", &[]));
    Ok(0)
}

fn validate_alias(alias: &str) -> Result<String, Failure> {
    let valid = !alias.is_empty()
        && alias.len() <= 32
        && alias
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid {
        Ok(alias.to_string())
    } else {
        Err(Failure::Message(t("error.invalid_alias", &[])))
    }
}

fn alias_from_email(email: Option<&str>) -> String {
    let local = email.and_then(|e| e.split('@').next()).unwrap_or("main");
    let cleaned: String = local
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "main".to_string()
    } else {
        cleaned
    }
}

/// Adds an account. `target` is an email (the name is derived from it and the
/// sign-in must match it) or a short name.
pub fn account_add(target: &str) -> Outcome {
    let binary = claude_binary()?;
    let mut relais = load()?;
    require_accounts(&relais)?;
    let taken: Vec<String> = relais
        .registry
        .accounts
        .iter()
        .map(|a| a.alias.clone())
        .collect();
    let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
    let (alias, expected_email) = if target.contains('@') {
        (
            registry::alias_from_email(target, &taken),
            Some(target.trim().to_string()),
        )
    } else {
        (validate_alias(target)?, None)
    };
    if let Some(email) = &expected_email {
        if is_registered(&relais, email, &alias) {
            return Err(Failure::Core(Error::DuplicateEmail(email.clone())));
        }
    }
    let config_dir = relais.layout.profile_dir(&alias);
    let account_dir = config_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_dir.clone());
    let chrome_profile = format!("Relais-{alias}");
    relais.registry.add(Account {
        alias: alias.clone(),
        email: None,
        account_uuid: None,
        org_id: None,
        config_dir: Some(config_dir.clone()),
        chrome_profile: Some(chrome_profile.clone()),
        color: None,
    })?;
    let account = relais
        .registry
        .get(&alias)
        .cloned()
        .ok_or_else(|| Error::UnknownAccount(alias.clone()))?;
    prepare(&relais, &account);

    println!("{}", t("add.signin", &[("alias", &alias)]));
    let mut login = claude::command_for_profile(&binary, Some(&config_dir));
    login.args(["auth", "login", "--claudeai"]);
    if let Some(opener) = write_browser_opener(&account_dir, &chrome_profile) {
        login.env("BROWSER", opener);
    }
    let login = login
        .status()
        .map_err(|e| Failure::Message(t("error.generic", &[("details", &e.to_string())])))?;
    let status = claude::auth_status(&binary, Some(&config_dir))
        .ok()
        .filter(|s| s.logged_in);
    let Some(status) = status.filter(|_| login.success()) else {
        forget_profile(&account_dir);
        return Err(Failure::Message(t(
            "error.login_failed",
            &[("alias", &alias)],
        )));
    };

    let email = status.email.clone();
    if let (Some(expected), Some(actual)) = (&expected_email, &email) {
        if !expected.eq_ignore_ascii_case(actual) {
            forget_profile(&account_dir);
            return Err(Failure::Message(t(
                "error.wrong_account",
                &[("expected", expected), ("actual", actual)],
            )));
        }
    }
    if let Some(email) = &email {
        if is_registered(&relais, email, &alias) {
            forget_profile(&account_dir);
            return Err(Failure::Core(Error::DuplicateEmail(email.clone())));
        }
    }
    let identity = claude::read_account_identity(&config_dir.join(".claude.json"))?;
    if let Some(entry) = relais
        .registry
        .accounts
        .iter_mut()
        .find(|a| a.alias == alias)
    {
        entry.email = email.clone();
        entry.account_uuid = identity.as_ref().and_then(|i| i.account_uuid.clone());
        entry.org_id = identity
            .as_ref()
            .and_then(|i| i.org_id.clone())
            .or(status.org_id.clone());
    }
    relais.save()?;
    prepare(&relais, &account);
    println!(
        "{}",
        t(
            "add.done",
            &[
                ("alias", &alias),
                ("email", email.as_deref().unwrap_or("?"))
            ]
        )
    );
    Ok(0)
}

fn is_registered(relais: &Relais, email: &str, except_alias: &str) -> bool {
    relais.registry.accounts.iter().any(|other| {
        other.alias != except_alias
            && other
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(email))
    })
}

/// Removes an account folder that never got registered. `remove_dir_all`
/// deletes symbolic links themselves and never what they point to, so
/// `~/.claude` is untouched.
fn forget_profile(account_dir: &Path) {
    let _ = std::fs::remove_dir_all(account_dir);
}

/// The Chrome application to open sign-in pages with: `$RELAIS_CHROME_APP`,
/// else Google Chrome when it is installed.
fn chrome_app() -> Option<String> {
    if let Some(app) = std::env::var_os("RELAIS_CHROME_APP").and_then(|a| a.into_string().ok()) {
        return Some(app);
    }
    Path::new("/Applications/Google Chrome.app")
        .exists()
        .then(|| "Google Chrome".to_string())
}

/// Writes the script Claude Code runs as `BROWSER` during sign-in: it opens
/// the page in this account's own Chrome profile, so the browser where
/// another account is signed in to claude.ai is left alone.
fn write_browser_opener(account_dir: &Path, chrome_profile: &str) -> Option<PathBuf> {
    let app = chrome_app()?;
    std::fs::create_dir_all(account_dir).ok()?;
    let path = account_dir.join("open-browser");
    let script = format!(
        "#!/bin/sh\n# Opens Claude's sign-in page in this account's own Chrome profile.\nexec /usr/bin/open -na {} --args --profile-directory={} \"$@\"\n",
        shell_quote(&app),
        shell_quote(chrome_profile)
    );
    std::fs::write(&path, script).ok()?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).ok()?;
    Some(path)
}

/// Signs an account in again, in its own profile; the sign-in page opens in
/// the account's own Chrome profile. A sign-in with another email is signed
/// out again, so the profile never carries the wrong account.
pub fn account_login(alias: &str) -> Outcome {
    let binary = claude_binary()?;
    let relais = load()?;
    let account = relais
        .registry
        .get(alias)
        .cloned()
        .ok_or_else(|| Error::UnknownAccount(alias.to_string()))?;
    prepare(&relais, &account);
    println!("{}", t("login.signin", &[("alias", alias)]));
    let config_dir = account.config_dir.as_deref();
    let mut login = claude::command_for_profile(&binary, config_dir);
    login.args(["auth", "login", "--claudeai"]);
    if let (Some(dir), Some(chrome_profile)) = (config_dir, &account.chrome_profile) {
        let account_dir = dir.parent().unwrap_or(dir);
        if let Some(opener) = write_browser_opener(account_dir, chrome_profile) {
            login.env("BROWSER", opener);
        }
    }
    let login = login
        .status()
        .map_err(|e| Failure::Message(t("error.generic", &[("details", &e.to_string())])))?;
    let status = claude::auth_status(&binary, config_dir)
        .ok()
        .filter(|s| s.logged_in && login.success())
        .ok_or_else(|| Failure::Message(t("error.relogin_failed", &[("alias", alias)])))?;
    if let (Some(expected), Some(actual)) = (&account.email, &status.email) {
        if !expected.eq_ignore_ascii_case(actual) {
            let _ = claude::command_for_profile(&binary, config_dir)
                .args(["auth", "logout"])
                .status();
            return Err(Failure::Message(t(
                "error.relogin_wrong_account",
                &[("expected", expected), ("actual", actual)],
            )));
        }
    }
    println!(
        "{}",
        t(
            "login.done",
            &[
                ("alias", alias),
                ("email", status.email.as_deref().unwrap_or("?"))
            ]
        )
    );
    Ok(0)
}

pub fn account_remove(alias: &str) -> Outcome {
    let mut relais = load()?;
    let removed = relais.registry.remove(alias)?;
    relais.save()?;
    let dir = removed
        .config_dir
        .unwrap_or_else(|| relais.layout.claude_dir.clone());
    println!(
        "{}",
        t(
            "remove.done",
            &[("alias", alias), ("dir", &dir.display().to_string())]
        )
    );
    Ok(0)
}

pub fn list() -> Outcome {
    let relais = load()?;
    require_accounts(&relais)?;
    let now = now();
    let width = relais
        .registry
        .accounts
        .iter()
        .map(|a| a.alias.len())
        .max()
        .unwrap_or(0);
    let email_width = relais
        .registry
        .accounts
        .iter()
        .map(|a| a.email.as_deref().unwrap_or("").len())
        .max()
        .unwrap_or(0);
    for account in &relais.registry.accounts {
        let marker = if relais.registry.active.as_deref() == Some(account.alias.as_str()) {
            "●"
        } else {
            " "
        };
        let usage = format::usage(relais.usage_of(&account.alias).as_ref(), now);
        println!(
            "{marker} {:<width$}  {:<email_width$}  {usage}",
            account.alias,
            account.email.as_deref().unwrap_or(""),
        );
    }
    Ok(0)
}

pub fn switch(target: &str) -> Outcome {
    let mut relais = load()?;
    require_accounts(&relais)?;
    let now = now();
    let alias = match target {
        "next" => relais.registry.neighbour(1).map(|a| a.alias.clone()),
        "prev" | "previous" => relais.registry.neighbour(-1).map(|a| a.alias.clone()),
        "best" => Some(
            relais
                .best_other_account(now)
                .ok_or_else(|| Failure::Message(t("error.no_better_account", &[])))?,
        ),
        alias => Some(alias.to_string()),
    }
    .ok_or(Failure::Core(Error::NoAccounts))?;
    relais.registry.set_active(&alias)?;
    relais.save()?;
    if let Some(account) = relais.registry.get(&alias).cloned() {
        prepare(&relais, &account);
    }
    let usage = format!(
        " ({})",
        format::usage(relais.usage_of(&alias).as_ref(), now)
    );
    println!(
        "{}",
        t("switch.done", &[("alias", &alias), ("usage", &usage)])
    );
    println!("{}", t("switch.note", &[]));
    Ok(0)
}

pub fn sync() -> Outcome {
    let relais = load()?;
    require_accounts(&relais)?;
    relais.sync_shared_config(now())?;
    Ok(0)
}

/// Runs Claude Code with the active account, or with `account_override`.
/// Never stands in Claude's way: without accounts, it runs Claude unchanged.
pub fn exec(account_override: Option<&str>, args: Vec<OsString>) -> Outcome {
    let binary = claude_binary()?;
    let mut relais = match load() {
        Ok(relais) if !relais.registry.accounts.is_empty() => relais,
        _ => return run_claude(&binary, None, &args, None).map(exit_code),
    };
    let alias = match account_override {
        Some(alias) => alias.to_string(),
        None => match relais.registry.active.clone() {
            Some(alias) => alias,
            None => return run_claude(&binary, None, &args, None).map(exit_code),
        },
    };
    let account = relais
        .registry
        .get(&alias)
        .cloned()
        .ok_or_else(|| Error::UnknownAccount(alias.clone()))?;
    prepare(&relais, &account);
    let status = run_claude(&binary, account.config_dir.as_deref(), &args, None)?;

    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let print_mode = args.iter().any(|a| a == "-p" || a == "--print");
    if interactive && !print_mode {
        if let Some(continued) = offer_to_continue(&mut relais, &account, &binary)? {
            return Ok(exit_code(continued));
        }
    }
    Ok(exit_code(status))
}

/// When the account that just ran is limited, offers to carry the
/// conversation over to the account with the most room left.
fn offer_to_continue(
    relais: &mut Relais,
    account: &Account,
    binary: &Path,
) -> Result<Option<ExitStatus>, Failure> {
    let now = now();
    let reading = relais.usage_of(&account.alias);
    let Some(Availability::Limited { until, exact }) =
        reading.as_ref().map(|r| r.availability(now))
    else {
        return Ok(None);
    };
    let Some(best) = relais.best_account_other_than(&account.alias, now) else {
        return Ok(None);
    };
    let best_used = format::used_now(relais.usage_of(&best).as_ref(), now)
        .map(format::percent)
        .unwrap_or_else(|| "?".to_string());
    let prompt = if exact {
        t(
            "exec.limited_prompt",
            &[
                ("alias", &account.alias),
                ("in", &format::remaining(until, now)),
                ("time", &format::time(until, now)),
                ("best", &best),
                ("pct", &best_used),
            ],
        )
    } else {
        t(
            "exec.limited_prompt_unknown",
            &[
                ("alias", &account.alias),
                ("best", &best),
                ("pct", &best_used),
            ],
        )
    };
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    let answer = answer.trim().to_lowercase();
    if !(answer.is_empty() || ["y", "yes", "o", "oui"].contains(&answer.as_str())) {
        return Ok(None);
    }

    relais.registry.set_active(&best)?;
    relais.save()?;
    let next = relais
        .registry
        .get(&best)
        .cloned()
        .ok_or_else(|| Error::UnknownAccount(best.clone()))?;
    prepare(relais, &next);
    let session = reading.and_then(|r| r.session_id);
    let resume_args = resume_args(&relais.layout, session.as_deref());
    eprintln!("{}", t("exec.continuing", &[("alias", &best)]));
    run_claude(binary, next.config_dir.as_deref(), &resume_args, None).map(Some)
}

/// `--resume <id>` when the session belongs to this directory, else `--continue`.
fn resume_args(layout: &Layout, session_id: Option<&str>) -> Vec<OsString> {
    let projects = layout.claude_dir.join("projects");
    let here = std::env::current_dir()
        .ok()
        .and_then(|d| d.canonicalize().ok());
    if let Some(id) = session_id {
        if let Ok(Some(conversation)) = transcripts::find_conversation(&projects, id) {
            let same_dir = conversation
                .cwd
                .as_deref()
                .and_then(|cwd| Path::new(cwd).canonicalize().ok())
                .is_some_and(|cwd| Some(cwd) == here);
            if same_dir {
                return vec!["--resume".into(), id.into()];
            }
        }
    }
    vec!["--continue".into()]
}

pub fn conversations(limit: usize) -> Outcome {
    let relais = load()?;
    let now = now();
    let recent = relais.recent_conversations(limit, now)?;
    if recent.is_empty() {
        println!("{}", t("conversations.empty", &[]));
        return Ok(0);
    }
    let home = relais.layout.home.display().to_string();
    let width = recent
        .iter()
        .filter_map(|c| c.account.as_deref())
        .map(|alias| alias.chars().count())
        .max()
        .unwrap_or(1);
    for recent in recent {
        let conversation = &recent.conversation;
        let cwd = conversation
            .cwd
            .as_deref()
            .unwrap_or("")
            .replacen(&home, "~", 1);
        println!(
            "{}  {:<12}  {:<width$}  {}  {}",
            conversation.session_id.chars().take(8).collect::<String>(),
            format::age(recent.last_activity, now),
            recent.account.as_deref().unwrap_or("—"),
            conversation.title.as_deref().unwrap_or("—"),
            cwd
        );
    }
    Ok(0)
}

/// Resumes a conversation with the active account, from its own directory.
pub fn continue_conversation(session_id: Option<&str>, desktop: bool) -> Outcome {
    let binary = claude_binary()?;
    let relais = load()?;
    let projects = relais.layout.claude_dir.join("projects");
    let conversation = match session_id {
        Some(id) => transcripts::find_conversation(&projects, id)?,
        None => transcripts::list_conversations(&projects, 1)?
            .into_iter()
            .next(),
    }
    .ok_or_else(|| Failure::Message(t("continue.none", &[])))?;
    let account = relais.registry.active().cloned();
    if let Some(account) = &account {
        prepare(&relais, account);
    }
    let config_dir = account.as_ref().and_then(|a| a.config_dir.clone());
    let cwd = conversation
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir());
    let mut args: Vec<OsString> = Vec::new();
    if desktop {
        args.push("--desktop".into());
    }
    args.push("--resume".into());
    args.push(conversation.session_id.clone().into());
    run_claude(&binary, config_dir.as_deref(), &args, cwd.as_deref()).map(exit_code)
}

/// Records the usage Claude Code hands to the status line, then renders the
/// user's own status line with the same input. Never fails the status line.
pub fn statusline(command: Vec<OsString>) -> Outcome {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);

    let layout = Layout::from_env().ok();
    if let Some(layout) = &layout {
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let recorded = Relais::load(layout.clone())
            .and_then(|relais| relais.record_statusline(config_dir.as_deref(), &input, now()));
        if let Err(error) = recorded {
            log_line(layout, "statusline", &error.to_string());
        }
    }

    let mut user_command = if let Some((program, rest)) = command.split_first() {
        let mut c = Command::new(program);
        c.args(rest);
        c
    } else {
        let Some(text) = layout
            .as_ref()
            .and_then(integration::statusline_original_command)
        else {
            return Ok(0);
        };
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(text);
        c
    };
    let Ok(mut child) = user_command.stdin(Stdio::piped()).spawn() else {
        return Ok(0);
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input);
    }
    Ok(child.wait().map(exit_code).unwrap_or(0))
}

fn log_line(layout: &Layout, name: &str, message: &str) {
    let dir = layout.log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{name}.log")))
    {
        let _ = writeln!(file, "{} {message}", chrono::Utc::now().to_rfc3339());
    }
}

fn own_executable() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("relais"))
}

fn own_statusline_command() -> String {
    format!(
        "{} statusline",
        shell_quote(&own_executable().display().to_string())
    )
}

fn shell_quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

fn report(change: integration::Change, done: &str, already: &str) -> Outcome {
    let key = match change {
        integration::Change::Done => done,
        integration::Change::AlreadyDone => already,
    };
    println!("{}", t(key, &[]));
    Ok(0)
}

/// Points Claude Code's status line at `relais statusline`.
pub fn statusline_install() -> Outcome {
    let layout = Layout::from_env()?;
    let change = integration::install_statusline(&layout, &own_statusline_command(), now())?;
    report(change, "statusline.installed", "statusline.already")
}

pub fn statusline_uninstall() -> Outcome {
    let layout = Layout::from_env()?;
    let change = integration::uninstall_statusline(&layout, now())?;
    report(change, "statusline.uninstalled", "statusline.not_installed")
}

pub fn shell_init(shell: &str) -> Outcome {
    match shell {
        "zsh" | "bash" | "sh" => {
            print!("{}", integration::SHELL_FUNCTION);
            Ok(0)
        }
        _ => Err(Failure::Message(t(
            "error.generic",
            &[("details", &format!("unsupported shell: {shell}"))],
        ))),
    }
}

/// Adds the `claude` function to `~/.zshrc`.
pub fn shell_install() -> Outcome {
    let layout = Layout::from_env()?;
    let change = integration::install_shell(&layout, &own_executable(), now())?;
    report(change, "shell.installed", "shell.already")
}

pub fn shell_uninstall() -> Outcome {
    let layout = Layout::from_env()?;
    let change = integration::uninstall_shell(&layout, now())?;
    report(change, "shell.uninstalled", "shell.not_installed")
}

pub fn doctor() -> Outcome {
    let relais = load()?;
    match claude_binary() {
        Ok(binary) => {
            let version = Command::new(&binary)
                .arg("--version")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            println!(
                "✓ {}",
                t(
                    "doctor.claude",
                    &[
                        ("path", &binary.display().to_string()),
                        ("version", &version)
                    ]
                )
            );
            let active = relais.registry.active.clone().unwrap_or_else(|| "—".into());
            println!(
                "✓ {}",
                t(
                    "doctor.accounts",
                    &[
                        ("count", &relais.registry.accounts.len().to_string()),
                        ("active", &active)
                    ]
                )
            );
            for account in &relais.registry.accounts {
                doctor_account(&relais, account, &binary);
            }
        }
        Err(failure) => println!("✗ {}", failure.describe()),
    }
    if integration::statusline_installed(&relais.layout) {
        println!("✓ {}", t("doctor.statusline_on", &[]));
    } else {
        println!("✗ {}", t("doctor.statusline_off", &[]));
    }
    if integration::shell_installed(&relais.layout) {
        println!("✓ {}", t("doctor.shell_on", &[]));
    } else {
        println!("✗ {}", t("doctor.shell_off", &[]));
    }
    Ok(0)
}

fn doctor_account(relais: &Relais, account: &Account, binary: &Path) {
    match claude::auth_status(binary, account.config_dir.as_deref()) {
        Ok(status) if status.logged_in => {
            let actual = status.email.unwrap_or_default();
            let expected = account.email.clone().unwrap_or_default();
            if expected.is_empty() || actual.eq_ignore_ascii_case(&expected) {
                println!(
                    "✓ {}",
                    t(
                        "doctor.account_ok",
                        &[("alias", &account.alias), ("email", &actual)]
                    )
                );
            } else {
                println!(
                    "✗ {}",
                    t(
                        "doctor.account_mismatch",
                        &[
                            ("alias", &account.alias),
                            ("actual", &actual),
                            ("expected", &expected)
                        ]
                    )
                );
            }
        }
        _ => {
            let dir = account
                .config_dir
                .clone()
                .unwrap_or_else(|| relais.layout.claude_dir.clone());
            println!(
                "✗ {}",
                t(
                    "doctor.account_signed_out",
                    &[
                        ("alias", &account.alias),
                        ("dir", &dir.display().to_string())
                    ]
                )
            );
        }
    }
    if let Ok(Some(report)) = relais.prepare_profile(account) {
        let count = report.created.len() + report.already_linked.len();
        println!(
            "✓ {}",
            t(
                "doctor.links",
                &[("alias", &account.alias), ("count", &count.to_string())]
            )
        );
        let odd: Vec<String> = report
            .conflicts
            .iter()
            .chain(&report.foreign_links)
            .cloned()
            .collect();
        if !odd.is_empty() {
            println!(
                "✗ {}",
                t(
                    "doctor.conflicts",
                    &[("alias", &account.alias), ("names", &odd.join(", "))]
                )
            );
        }
    }
}

/// Without an account: which account the Claude desktop app uses, and which
/// accounts have a folder of their own. With one: closes the app, puts that
/// account's folder in place and opens the app again. The terminal's active
/// account is left alone.
pub fn desktop(alias: Option<&str>) -> Outcome {
    let relais = load()?;
    let owner = desktop_profile::owner(&relais.layout, &relais.registry);
    let Some(alias) = alias else {
        for account in &relais.registry.accounts {
            let marker = if owner.as_deref() == Some(account.alias.as_str()) {
                "●"
            } else {
                " "
            };
            let state =
                if desktop_profile::has_folder(&relais.layout, &relais.registry, &account.alias) {
                    t("desktop.ready", &[])
                } else {
                    t("desktop.no_folder", &[])
                };
            println!("{marker} {:<20}  {state}", account.alias);
        }
        return Ok(0);
    };
    let account = relais
        .registry
        .get(alias)
        .cloned()
        .ok_or_else(|| Error::UnknownAccount(alias.to_string()))?;
    if owner.as_deref() == Some(alias) {
        println!("{}", t("desktop.already", &[("alias", alias)]));
        return Ok(0);
    }
    let app = DesktopApp::from_env();
    if app.is_running() {
        println!("{}", t("desktop.closing", &[]));
        app.quit(DESKTOP_QUIT_WAIT)?;
    }
    let outcome = desktop_profile::switch(&relais.layout, &relais.registry, alias)?;
    app.open()?;
    match outcome {
        desktop_profile::Switch::SignInNeeded => println!(
            "{}",
            t(
                "desktop.sign_in",
                &[
                    ("alias", alias),
                    ("email", account.email.as_deref().unwrap_or(alias))
                ]
            )
        ),
        _ => println!("{}", t("desktop.switched", &[("alias", alias)])),
    }
    Ok(0)
}
