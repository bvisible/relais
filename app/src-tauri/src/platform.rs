//! What the app needs from macOS: finding the command-line tools, opening a
//! Terminal, handing a conversation to Claude Desktop.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use relais_core::claude::cli as claude;
use relais_core::service::Relais;
use relais_core::Layout;

/// Apps opened from the Finder get a minimal `PATH`; add the usual install
/// places of `claude`, `relais` and the tools Claude Code relies on.
pub fn search_path() -> OsString {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    for extra in [
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        home.join(".bun/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The `relais` command: `$RELAIS_CLI`, then a copy bundled next to the app,
/// then the search path.
pub fn relais_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RELAIS_CLI")
        .map(PathBuf::from)
        .filter(|p| is_executable(p))
    {
        return Some(path);
    }
    let bundled = std::env::current_exe().ok()?.parent()?.join("relais");
    if is_executable(&bundled) {
        return Some(bundled);
    }
    std::env::split_paths(&search_path())
        .map(|dir| dir.join("relais"))
        .find(|path| is_executable(path))
}

pub fn claude_binary() -> Option<PathBuf> {
    let override_path = std::env::var_os(claude::CLAUDE_BIN_ENV).map(PathBuf::from);
    claude::find_claude_binary(override_path.as_deref(), Some(&search_path()))
}

pub fn shell_quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+~".contains(c))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// Opens Terminal on `relais <args>` run from `cwd`, through a `.command`
/// file: `open` needs no automation permission, unlike AppleScript.
pub fn open_terminal_with(layout: &Layout, cwd: &str, args: &[&str]) -> Result<(), String> {
    let cli = relais_cli().ok_or("relais command not found")?;
    let dir = layout.relais_dir.join("launch");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    forget_old_scripts(&dir);
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let script = dir.join(format!("relais-{stamp}.command"));
    let command: Vec<String> = std::iter::once(shell_quote(&cli.display().to_string()))
        .chain(args.iter().map(|arg| shell_quote(arg)))
        .collect();
    let body = format!(
        "#!/bin/zsh -l\ncd {}\n{}\n",
        shell_quote(cwd),
        command.join(" ")
    );
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(&script)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn forget_old_scripts(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 3600));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Hands a conversation to Claude Desktop with `claude --desktop --resume`,
/// the official way, from the conversation's own folder.
pub fn resume_in_desktop(layout: &Layout, session_id: &str, cwd: &str) -> Result<(), String> {
    let relais = Relais::load(layout.clone()).map_err(|e| e.to_string())?;
    let config_dir = relais
        .registry
        .active()
        .and_then(|account| account.config_dir.clone());
    let binary = claude_binary().ok_or("claude command not found")?;
    claude::command_for_profile(&binary, config_dir.as_deref())
        .args(["--desktop", "--resume", session_id])
        .current_dir(cwd)
        .env("PATH", search_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// French when macOS or the environment prefers it, for the menu bar title.
/// Opens Chrome in `profile`, the account's own Chrome profile, or in the
/// usual profile when the account has none (the account Claude Code started with).
pub fn open_chrome(profile: Option<&str>) -> Result<(), String> {
    let app = "/Applications/Google Chrome.app";
    if !Path::new(app).exists() {
        return Err("Google Chrome is not installed".into());
    }
    let mut open = Command::new("/usr/bin/open");
    match profile {
        Some(profile) => {
            open.args(["-na", app, "--args"])
                .arg(format!("--profile-directory={profile}"));
        }
        None => {
            open.args(["-a", app]);
        }
    }
    let status = open
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "Chrome did not open".to_string())
}

pub fn prefers_french() -> bool {
    static FRENCH: OnceLock<bool> = OnceLock::new();
    *FRENCH.get_or_init(|| {
        for var in ["RELAIS_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(var) {
                let value = value.to_lowercase();
                if !value.is_empty() && !value.starts_with("c.") && value != "c" && value != "posix"
                {
                    return value.starts_with("fr");
                }
            }
        }
        Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split(['(', ',', ')', '\n'])
                    .map(|s| s.trim().trim_matches('"'))
                    .find(|s| !s.is_empty())
                    .is_some_and(|first| first.starts_with("fr"))
            })
            .unwrap_or(false)
    })
}
