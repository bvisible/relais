//! Where Relais and Claude Code keep their files.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The file in which the desktop app keeps its usage samples, in its data folder.
pub const DESKTOP_USAGE_HISTORY: &str = "plan-usage-history.json";

#[derive(Debug, Clone)]
pub struct Layout {
    /// The user's home directory.
    pub home: PathBuf,
    /// Claude Code's default config directory (`~/.claude`).
    pub claude_dir: PathBuf,
    /// Claude Code's global config for the default directory (`~/.claude.json`).
    pub claude_global_config: PathBuf,
    /// Relais's own directory (`~/.relais`, or `$RELAIS_HOME`).
    pub relais_dir: PathBuf,
    /// The Claude desktop app's data folder.
    pub desktop_dir: PathBuf,
}

impl Layout {
    pub fn for_home(home: &Path) -> Self {
        Layout {
            home: home.to_path_buf(),
            claude_dir: home.join(".claude"),
            claude_global_config: home.join(".claude.json"),
            relais_dir: home.join(".relais"),
            desktop_dir: home.join("Library/Application Support/Claude"),
        }
    }

    /// Reads `HOME`, and `RELAIS_HOME` when set.
    pub fn from_env() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or(Error::NoHome)?;
        let mut layout = Layout::for_home(Path::new(&home));
        if let Some(relais_home) = std::env::var_os("RELAIS_HOME").filter(|h| !h.is_empty()) {
            layout.relais_dir = PathBuf::from(relais_home);
        }
        Ok(layout)
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.relais_dir.join("accounts.json")
    }

    /// The Claude Code config directory of a non-default account.
    pub fn profile_dir(&self, alias: &str) -> PathBuf {
        self.relais_dir.join("accounts").join(alias).join("claude")
    }

    pub fn usage_dir(&self) -> PathBuf {
        self.relais_dir.join("usage")
    }

    /// The Claude desktop app's data folder of an account, while another
    /// account's folder is in place.
    pub fn desktop_profile_dir(&self, alias: &str) -> PathBuf {
        self.relais_dir.join("accounts").join(alias).join("desktop")
    }

    /// Which account's folder the desktop app uses.
    pub fn desktop_state_file(&self) -> PathBuf {
        self.relais_dir.join("desktop.json")
    }

    /// Which account each Claude Code session ran on, as the status line saw it.
    pub fn sessions_file(&self) -> PathBuf {
        self.relais_dir.join("sessions.json")
    }

    pub fn sync_base_file(&self) -> PathBuf {
        self.relais_dir.join("sync").join("base.json")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.relais_dir.join("backups")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.relais_dir.join("relais.lock")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.relais_dir.join("logs")
    }

    /// The desktop app's usage history, one sample per organization and time.
    pub fn desktop_usage_history(&self) -> PathBuf {
        self.desktop_dir.join(DESKTOP_USAGE_HISTORY)
    }
}
