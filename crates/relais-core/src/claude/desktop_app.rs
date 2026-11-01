//! Stopping and starting the Claude desktop app, for a change of account.
//!
//! The app is found by its bundle identifier (`lsappinfo`), asked to quit with
//! SIGTERM (Electron then quits as from its menu, saving what it holds) and
//! opened again with `open`. No Apple Events are sent, so no permission to
//! control another app is asked. `RELAIS_DESKTOP_CONTROL` names a script that
//! does this instead (`running`, `quit`, `open`), so tests never touch the
//! real app.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// The Claude desktop app's bundle identifier.
pub const DESKTOP_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";
/// How long the app gets to close before the change of account is given up.
pub const DESKTOP_QUIT_WAIT: Duration = Duration::from_secs(20);
pub const DESKTOP_CONTROL_ENV: &str = "RELAIS_DESKTOP_CONTROL";
/// How long a fresh start gets to log the account it loaded.
pub const DESKTOP_SIGN_IN_WAIT: Duration = Duration::from_secs(60);

pub struct DesktopApp {
    script: Option<PathBuf>,
}

impl DesktopApp {
    pub fn from_env() -> Self {
        DesktopApp {
            script: std::env::var_os(DESKTOP_CONTROL_ENV).map(PathBuf::from),
        }
    }

    fn pid(&self) -> Option<u32> {
        let output = Command::new("lsappinfo")
            .args(["info", "-only", "pid", "-app", DESKTOP_BUNDLE_ID])
            .output()
            .ok()?;
        pid_from_lsappinfo(&String::from_utf8_lossy(&output.stdout))
    }

    /// Driven by a script rather than the real app.
    pub fn is_scripted(&self) -> bool {
        self.script.is_some()
    }

    pub fn is_running(&self) -> bool {
        match &self.script {
            Some(script) => Command::new(script)
                .arg("running")
                .status()
                .is_ok_and(|s| s.success()),
            None => self.pid().is_some(),
        }
    }

    /// Asks the app to quit and waits until it has, up to `wait`. Nothing is
    /// forced: an app still open after that is reported, not killed.
    pub fn quit(&self, wait: Duration) -> Result<()> {
        match &self.script {
            Some(script) => {
                let _ = Command::new(script).arg("quit").status();
            }
            None => {
                if let Some(pid) = self.pid() {
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                }
            }
        }
        let started = Instant::now();
        while self.is_running() {
            if started.elapsed() > wait {
                return Err(Error::DesktopStillRunning);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }

    pub fn open(&self) -> Result<()> {
        let status = match &self.script {
            Some(script) => Command::new(script).arg("open").status(),
            None => Command::new("open")
                .args(["-b", DESKTOP_BUNDLE_ID])
                .status(),
        };
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => Err(Error::DesktopNotOpened),
        }
    }
}

/// What the desktop app's log says about the account it started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopSignIn {
    SignedIn { account_uuid: String },
    SignedOut,
}

/// The latest sign-in state the app logged at or after `since` (local time, as
/// the log writes it): `[ScheduledTasks] Initialized { accountId: '…' }` once
/// an account is loaded, `User is logged out` when there is none. Only ids
/// are read; the log holds no credential.
pub fn sign_in_since(log: &str, since: chrono::NaiveDateTime) -> Option<DesktopSignIn> {
    let mut state = None;
    for line in log.lines() {
        let Some(stamp) = line.get(..19) else {
            continue;
        };
        let Ok(at) = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S") else {
            continue;
        };
        if at < since {
            continue;
        }
        if line.contains("[ScheduledTasks] Initialized") {
            let account_uuid = line
                .split("accountId: '")
                .nth(1)
                .and_then(|rest| rest.split('\'').next())
                .filter(|uuid| !uuid.is_empty());
            if let Some(account_uuid) = account_uuid {
                state = Some(DesktopSignIn::SignedIn {
                    account_uuid: account_uuid.to_string(),
                });
            }
        } else if line.contains("User is logged out") {
            state = Some(DesktopSignIn::SignedOut);
        }
    }
    state
}

/// Waits up to `wait` for the app to log the account it started with after
/// `since`. `None` when nothing was logged in time.
pub fn wait_for_sign_in(
    log: &Path,
    since: chrono::NaiveDateTime,
    wait: Duration,
) -> Option<DesktopSignIn> {
    let started = Instant::now();
    loop {
        let found = std::fs::read(log)
            .ok()
            .map(|bytes| tail(&bytes))
            .and_then(|text| sign_in_since(&text, since));
        if found.is_some() || started.elapsed() > wait {
            return found;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The last part of the log: a start writes a few hundred lines.
fn tail(bytes: &[u8]) -> String {
    const KEEP: usize = 512 * 1024;
    String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(KEEP)..]).into_owned()
}

/// The process id in what `lsappinfo info -only pid` prints: `pid = 75645`
/// inside a block of other fields, or `"pid"=75645` on older systems.
pub fn pid_from_lsappinfo(text: &str) -> Option<u32> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    tokens.iter().enumerate().find_map(|(index, token)| {
        if *token == "pid" && tokens.get(index + 1) == Some(&"=") {
            return tokens.get(index + 2)?.parse().ok();
        }
        token.strip_prefix("\"pid\"=")?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
2026-10-06 21:36:52 [info] [account] User is logged out, skipping account ID wait
2026-10-06 21:39:42 [info] [ScheduledTasks] Initialized { accountId: '8f5febc5-278d', orgId: 'c89c1123' }
2026-10-06 21:40:14 [info] [account] Account details received via IPC
2026-10-06 21:40:14 [info] [ScheduledTasks] Initialized { accountId: '291ce996-d39f', orgId: '416c6079' }
";

    fn at(text: &str) -> chrono::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn the_log_tells_the_account_the_app_started_with_since_a_time() {
        assert_eq!(
            sign_in_since(LOG, at("2026-10-06 21:40:00")),
            Some(DesktopSignIn::SignedIn {
                account_uuid: "291ce996-d39f".into()
            })
        );
        assert_eq!(
            sign_in_since(LOG, at("2026-10-06 21:39:00")),
            Some(DesktopSignIn::SignedIn {
                account_uuid: "291ce996-d39f".into()
            })
        );
    }

    #[test]
    fn a_start_without_an_account_is_reported_and_nothing_new_is_none() {
        assert_eq!(
            sign_in_since(LOG, at("2026-10-06 21:36:00")).map(|_| ()),
            Some(())
        );
        let only_signed_out =
            "2026-10-06 21:36:52 [info] [account] User is logged out, skipping account ID wait\n";
        assert_eq!(
            sign_in_since(only_signed_out, at("2026-10-06 21:36:00")),
            Some(DesktopSignIn::SignedOut)
        );
        assert_eq!(sign_in_since(LOG, at("2026-10-06 22:00:00")), None);
        assert_eq!(sign_in_since("garbage\n", at("2026-10-06 21:00:00")), None);
    }

    #[test]
    fn the_pid_is_read_from_both_lsappinfo_formats() {
        let current = "[ NULL ]  [ NULL ]  \n    bundleID=[ NULL ] \n    pid = 75645 !cgsConnection !signalled type=[ NULL ]\n";
        assert_eq!(pid_from_lsappinfo(current), Some(75645));
        assert_eq!(pid_from_lsappinfo("\"pid\"=4242\n"), Some(4242));
        assert_eq!(pid_from_lsappinfo(""), None);
        assert_eq!(pid_from_lsappinfo("bundleID=[ NULL ]"), None);
    }
}
