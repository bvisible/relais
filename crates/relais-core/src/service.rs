//! The operations the CLI and the app share, built on the lower modules.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::claude::profile::{self, LinkReport};
use crate::claude::sync::{self, ConfigFile, SyncOutcome};
use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;
use crate::registry::{Account, Registry};
use crate::usage::{self, UsageReading, UsageStore};

pub struct Relais {
    pub layout: Layout,
    pub registry: Registry,
}

impl Relais {
    pub fn load(layout: Layout) -> Result<Self> {
        let registry = Registry::load(&layout.accounts_file())?;
        Ok(Relais { layout, registry })
    }

    pub fn save(&self) -> Result<()> {
        self.registry.save(&self.layout.accounts_file())
    }

    /// The `.claude.json` Claude Code uses for `account`.
    pub fn global_config_of(&self, account: &Account) -> PathBuf {
        match &account.config_dir {
            None => self.layout.claude_global_config.clone(),
            Some(dir) => dir.join(".claude.json"),
        }
    }

    /// Links the shared entries of `~/.claude` into a non-default profile.
    pub fn prepare_profile(&self, account: &Account) -> Result<Option<LinkReport>> {
        match &account.config_dir {
            None => Ok(None),
            Some(dir) => profile::ensure_links(&self.layout.claude_dir, dir).map(Some),
        }
    }

    /// Brings the shared part of every account's `.claude.json` to the same state.
    pub fn sync_shared_config(&self, now: i64) -> Result<SyncOutcome> {
        let files: Vec<ConfigFile> = self
            .registry
            .accounts
            .iter()
            .map(|account| ConfigFile {
                id: account.alias.clone(),
                path: self.global_config_of(account),
                primary: account.is_default_profile(),
            })
            .collect();
        let _lock = lock(&self.layout.lock_file())?;
        sync::sync_config_files(
            &files,
            &self.layout.sync_base_file(),
            &self.layout.backups_dir(),
            now,
        )
    }

    /// Stores the usage carried by one status line input under the account of
    /// the session (`CLAUDE_CONFIG_DIR`, or the default profile when unset).
    /// Returns the account it was stored for.
    pub fn record_statusline(
        &self,
        config_dir: Option<&Path>,
        input: &[u8],
        now: i64,
    ) -> Result<Option<String>> {
        let value: Value = serde_json::from_slice(input).map_err(|e| Error::Json {
            path: PathBuf::from("<status line input>"),
            source: e,
        })?;
        let Some(account) = self.registry.find_by_config_dir(config_dir) else {
            return Ok(None);
        };
        let Some(reading) = usage::reading_from_statusline(&value, now) else {
            return Ok(None);
        };
        UsageStore::new(&self.layout.usage_dir()).save(&account.alias, &reading)?;
        Ok(Some(account.alias.clone()))
    }

    /// The newest reading for an account, from the status line or the desktop app.
    pub fn usage_of(&self, alias: &str) -> Option<UsageReading> {
        let account = self.registry.get(alias)?;
        let stored = UsageStore::new(&self.layout.usage_dir())
            .load(alias)
            .ok()
            .flatten();
        let desktop = account.org_id.as_deref().and_then(|org| {
            let history = fsutil::read_json(&self.layout.desktop_usage_history())
                .ok()
                .flatten()?;
            usage::reading_from_desktop_history(&history, org)
        });
        match (stored, desktop) {
            (Some(stored), Some(desktop)) => Some(if desktop.observed_at > stored.observed_at {
                desktop
            } else {
                stored
            }),
            (stored, desktop) => stored.or(desktop),
        }
    }

    /// The account with the most room left, other than the active one.
    pub fn best_other_account(&self, now: i64) -> Option<String> {
        let readings: Vec<(String, Option<UsageReading>)> = self
            .registry
            .accounts
            .iter()
            .filter(|account| self.registry.active.as_deref() != Some(account.alias.as_str()))
            .map(|account| (account.alias.clone(), self.usage_of(&account.alias)))
            .collect();
        let candidates: Vec<(&str, Option<&UsageReading>)> = readings
            .iter()
            .map(|(alias, reading)| (alias.as_str(), reading.as_ref()))
            .collect();
        usage::best_account(&candidates, now).map(String::from)
    }
}

/// An exclusive lock on `path`, released when the returned file is dropped.
fn lock(path: &Path) -> Result<fs::File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let file = fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    file.lock().map_err(|e| Error::io(path, e))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_791_270_000;

    fn fixture() -> (tempfile::TempDir, Relais) {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::for_home(root.path());
        fs::create_dir_all(&layout.claude_dir).unwrap();
        let mut registry = Registry::default();
        registry
            .add(Account {
                alias: "main".into(),
                email: Some("main@example.com".into()),
                account_uuid: Some("acc-main".into()),
                org_id: Some("org-main".into()),
                config_dir: None,
            })
            .unwrap();
        registry
            .add(Account {
                alias: "work".into(),
                email: Some("work@example.com".into()),
                account_uuid: Some("acc-work".into()),
                org_id: Some("org-work".into()),
                config_dir: Some(layout.profile_dir("work")),
            })
            .unwrap();
        (root, Relais { layout, registry })
    }

    fn statusline_input(five: f64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "session_id": "s-1",
            "rate_limits": {
                "five_hour": {"used_percentage": five, "resets_at": NOW + 3600},
                "seven_day": {"used_percentage": 10, "resets_at": NOW + 86400}
            }
        }))
        .unwrap()
    }

    #[test]
    fn statusline_reading_goes_to_the_account_of_the_profile() {
        let (_root, relais) = fixture();
        let work_dir = relais.layout.profile_dir("work");

        let stored = relais
            .record_statusline(Some(&work_dir), &statusline_input(42.0), NOW)
            .unwrap();

        assert_eq!(stored.as_deref(), Some("work"));
        let reading = relais.usage_of("work").unwrap();
        assert_eq!(reading.five_hour_now(NOW), Some(42.0));
        assert!(relais.usage_of("main").is_none());
    }

    #[test]
    fn statusline_without_profile_counts_for_the_default_account() {
        let (_root, relais) = fixture();
        let stored = relais
            .record_statusline(None, &statusline_input(5.0), NOW)
            .unwrap();
        assert_eq!(stored.as_deref(), Some("main"));
    }

    #[test]
    fn unknown_profile_or_missing_limits_store_nothing() {
        let (_root, relais) = fixture();
        assert_eq!(
            relais
                .record_statusline(Some(Path::new("/elsewhere")), &statusline_input(5.0), NOW)
                .unwrap(),
            None
        );
        assert_eq!(
            relais
                .record_statusline(None, br#"{"session_id":"s"}"#, NOW)
                .unwrap(),
            None
        );
        assert!(relais.record_statusline(None, b"not json", NOW).is_err());
    }

    #[test]
    fn newest_reading_wins_between_statusline_and_desktop_history() {
        let (_root, relais) = fixture();
        relais
            .record_statusline(None, &statusline_input(20.0), NOW - 600)
            .unwrap();
        fs::create_dir_all(&relais.layout.desktop_dir).unwrap();
        let history = json!({"version": 1, "samples": [{"t": NOW * 1000, "org": "org-main", "u": {"fh": 55, "sd": 12}}]});
        fs::write(relais.layout.desktop_usage_history(), history.to_string()).unwrap();

        let reading = relais.usage_of("main").unwrap();

        assert_eq!(reading.five_hour_now(NOW), Some(55.0));
    }

    #[test]
    fn shared_config_reaches_a_new_profile() {
        let (_root, relais) = fixture();
        fs::write(
            &relais.layout.claude_global_config,
            r#"{"mcpServers":{"github":{"command":"gh"}}}"#,
        )
        .unwrap();
        let work = relais.registry.get("work").unwrap().clone();
        relais.prepare_profile(&work).unwrap();

        relais.sync_shared_config(NOW).unwrap();

        let work_config: Value =
            serde_json::from_slice(&fs::read(relais.global_config_of(&work)).unwrap()).unwrap();
        assert_eq!(
            work_config["mcpServers"],
            json!({"github": {"command": "gh"}})
        );
        assert!(relais.layout.profile_dir("work").join("projects").exists());
    }

    #[test]
    fn default_profile_needs_no_preparation() {
        let (_root, relais) = fixture();
        let main = relais.registry.get("main").unwrap().clone();
        assert_eq!(relais.prepare_profile(&main).unwrap(), None);
        assert_eq!(
            relais.global_config_of(&main),
            relais.layout.claude_global_config
        );
    }

    #[test]
    fn best_other_account_skips_the_active_one() {
        let (_root, relais) = fixture();
        relais
            .record_statusline(None, &statusline_input(1.0), NOW)
            .unwrap();
        relais
            .record_statusline(
                Some(&relais.layout.profile_dir("work")),
                &statusline_input(90.0),
                NOW,
            )
            .unwrap();

        assert_eq!(relais.best_other_account(NOW).as_deref(), Some("work"));
    }
}
