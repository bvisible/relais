//! Stopping and starting the Claude desktop app, for a change of account.
//!
//! The app is found by its bundle identifier (`lsappinfo`), asked to quit with
//! SIGTERM (Electron then quits as from its menu, saving what it holds) and
//! opened again with `open`. No Apple Events are sent, so no permission to
//! control another app is asked. `RELAIS_DESKTOP_CONTROL` names a script that
//! does this instead (`running`, `quit`, `open`), so tests never touch the
//! real app.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// The Claude desktop app's bundle identifier.
pub const DESKTOP_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";
/// How long the app gets to close before the change of account is given up.
pub const DESKTOP_QUIT_WAIT: Duration = Duration::from_secs(20);
pub const DESKTOP_CONTROL_ENV: &str = "RELAIS_DESKTOP_CONTROL";

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

    #[test]
    fn the_pid_is_read_from_both_lsappinfo_formats() {
        let current = "[ NULL ]  [ NULL ]  \n    bundleID=[ NULL ] \n    pid = 75645 !cgsConnection !signalled type=[ NULL ]\n";
        assert_eq!(pid_from_lsappinfo(current), Some(75645));
        assert_eq!(pid_from_lsappinfo("\"pid\"=4242\n"), Some(4242));
        assert_eq!(pid_from_lsappinfo(""), None);
        assert_eq!(pid_from_lsappinfo("bundleID=[ NULL ]"), None);
    }
}
