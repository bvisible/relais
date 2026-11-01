//! The operations the CLI and the app share, built on the lower modules.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::claude::cli as claude;
use crate::claude::desktop;
use crate::claude::profile::{self, LinkReport};
use crate::claude::sync::{self, ConfigFile, SyncOutcome};
use crate::claude::transcripts::{self, Conversation, ACTIVITY_BUCKETS};
use crate::claude::usage_probe;
use crate::error::{Error, Result};
use crate::fsutil;
use crate::history::{self, HistoryStore, Sample};
use crate::layout::Layout;
use crate::registry::{self, Account, Registry};
use crate::sessions::SessionAccounts;
use crate::usage::{self, ModelLimits, UsageReading, UsageStore};

/// A conversation as the menu bar app and `relais conversations` show it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentConversation {
    pub conversation: Conversation,
    /// The account that last ran it, when Relais saw it run.
    pub account: Option<String>,
    /// Unix seconds of its last message: Claude Code also writes bookkeeping
    /// lines to an idle session, so the transcript's own date runs ahead.
    /// Transcripts untouched for an hour keep that date and are not read again.
    pub last_activity: i64,
    /// Messages per five minutes over the last hour, oldest first.
    pub activity: Vec<u32>,
}

/// The conversations an account carried, as Relais saw them run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConversationCounts {
    /// Seen since the start of the local day.
    pub today: usize,
    /// Seen in the last seven days.
    pub week: usize,
    /// Unix seconds of the latest sighting.
    pub last_seen: Option<i64>,
}

/// Who Claude Code says is signed in on a profile. Metadata only, never a token.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedIn {
    pub email: String,
    pub account_uuid: Option<String>,
    pub org_id: Option<String>,
}

/// What [`Relais::adopt_default_profile`] changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Adoption {
    /// The account that now holds the default profile.
    pub alias: String,
    /// The account that held it before. It keeps its place in the registry,
    /// now with a profile of its own, still to be signed in.
    pub previous: String,
}

/// The status line measured an account this recently: the terminals run on it.
pub const MEASURED_LATELY: i64 = 10 * 60;
/// How often each account is measured in the background. An account nobody
/// uses does not move, and the one in use has the status line besides.
pub const USAGE_EVERY: i64 = 15 * 60;
/// How long Claude Code gets to show an account's limits.
const USAGE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// How long after its last message a conversation counts as working.
const LIVE_FOR: i64 = 120;
/// More conversations than can be working at once.
const LIVE_LOOKUP: usize = 64;

/// Sets or clears an account's entry in one of the measure's notes. Measures
/// started together must not erase each other's.
fn note_for_account(file: &Path, alias: &str, value: Option<Value>) {
    static ONE_WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _writing = ONE_WRITER.lock();
    let mut notes: HashMap<String, Value> = fsutil::read_json(file)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    match value {
        Some(value) => notes.insert(alias.to_string(), value),
        None => notes.remove(alias),
    };
    let _ = fsutil::write_json_atomic(file, &json!(notes));
}

/// How long a sync waits for Claude Code to release a config file.
pub const SYNC_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

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

    /// Registers `signed_in` as the account of the default profile (`~/.claude`),
    /// when it is no longer the account Relais recorded there: someone ran
    /// `/login` with another account.
    ///
    /// The account recorded until now is not dropped. Its identity is still
    /// right, and so is what Relais keeps under its name: the desktop app's
    /// sign-in, its color. It moves to a profile of its own, to be signed in
    /// again. The new account joins at the end, and takes the active marker
    /// when the previous one held it, since the terminals still run on the
    /// default profile.
    ///
    /// The registry changes in memory only: the caller saves it. On disk, this
    /// first copies the registry file and sets aside the usage reading stored
    /// under the previous name, both in `backups/adopt/<now>`: the status line
    /// wrote that reading while the default profile held another account, so
    /// nothing says whose it is. The next status line fills in the right one.
    pub fn adopt_default_profile(
        &mut self,
        signed_in: SignedIn,
        alias: Option<&str>,
        now: i64,
    ) -> Result<Adoption> {
        let previous = self
            .registry
            .accounts
            .iter()
            .find(|a| a.is_default_profile())
            .cloned()
            .ok_or(Error::NoDefaultProfileAccount)?;
        let email = signed_in.email.trim().to_string();
        let same_email = |a: &Account| {
            a.email
                .as_deref()
                .is_some_and(|known| known.eq_ignore_ascii_case(&email))
        };
        if same_email(&previous) {
            return Err(Error::AlreadyOnDefaultProfile(previous.alias));
        }
        if self.registry.accounts.iter().any(same_email) {
            return Err(Error::DuplicateEmail(email));
        }
        let alias = match alias {
            Some(alias) => alias.to_string(),
            None => {
                let taken: Vec<&str> = self
                    .registry
                    .accounts
                    .iter()
                    .map(|a| a.alias.as_str())
                    .collect();
                registry::alias_from_email(&email, &taken)
            }
        };
        if self.registry.get(&alias).is_some() {
            return Err(Error::DuplicateAlias(alias));
        }

        let saved = self
            .layout
            .backups_dir()
            .join("adopt")
            .join(now.to_string());
        fs::create_dir_all(&saved).map_err(|e| Error::io(&saved, e))?;
        let accounts_file = self.layout.accounts_file();
        if accounts_file.is_file() {
            fs::copy(&accounts_file, saved.join("accounts.json"))
                .map_err(|e| Error::io(&accounts_file, e))?;
        }
        UsageStore::new(&self.layout.usage_dir()).move_to(
            &previous.alias,
            &saved.join(format!("usage-{}.json", previous.alias)),
        )?;

        let profile_dir = self.layout.profile_dir(&previous.alias);
        if let Some(entry) = self
            .registry
            .accounts
            .iter_mut()
            .find(|a| a.alias == previous.alias)
        {
            entry.config_dir = Some(profile_dir);
            entry.chrome_profile = Some(format!("Relais-{}", previous.alias));
        }
        self.registry.add(Account {
            alias: alias.clone(),
            email: Some(email),
            account_uuid: signed_in.account_uuid,
            org_id: signed_in.org_id,
            config_dir: None,
            chrome_profile: None,
            color: None,
        })?;
        if self.registry.active.as_deref() == Some(previous.alias.as_str()) {
            self.registry.active = Some(alias.clone());
        }
        Ok(Adoption {
            alias,
            previous: previous.alias,
        })
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
            SYNC_LOCK_WAIT,
        )
    }

    /// Stores the usage carried by one status line input under the account of
    /// the session (`CLAUDE_CONFIG_DIR`, or the default profile when unset),
    /// and notes which account the session runs on.
    /// Returns the account the usage was stored for.
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
        if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
            // Best effort: a lost write only delays the account's color by one render.
            let _ = SessionAccounts::new(&self.layout.sessions_file()).record(
                session_id,
                &account.alias,
                now,
            );
        }
        let Some(reading) = usage::reading_from_statusline(&value, now) else {
            return Ok(None);
        };
        UsageStore::new(&self.layout.usage_dir()).save(&account.alias, &reading)?;
        // Best effort: a missed sample leaves a gap in the figures, nothing more.
        let _ = HistoryStore::new(&self.layout.usage_dir())
            .record(&account.alias, Sample::from_reading(&reading));
        Ok(Some(account.alias.clone()))
    }

    /// Every session Relais can place, by transcript id, with the alias of the
    /// account that last ran it and when it was seen. The status line sees the
    /// terminal sessions and the desktop app's sidebar lists its own; the later
    /// sighting wins.
    pub fn session_sightings(&self) -> HashMap<String, (String, i64)> {
        let mut seen: HashMap<String, (String, i64)> =
            SessionAccounts::new(&self.layout.sessions_file())
                .load()
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, session)| self.registry.get(&session.alias).is_some())
                .map(|(id, session)| (id, (session.alias, session.seen_at)))
                .collect();
        let desktop_sessions = self
            .desktop_dirs()
            .iter()
            .flat_map(|dir| desktop::session_accounts(dir))
            .collect::<Vec<_>>();
        for (id, session) in desktop_sessions {
            let account = self
                .registry
                .accounts
                .iter()
                .find(|a| a.account_uuid.as_deref() == Some(session.account_uuid.as_str()));
            let Some(account) = account else {
                continue;
            };
            if seen.get(&id).is_none_or(|(_, at)| *at < session.seen_at) {
                seen.insert(id, (account.alias.clone(), session.seen_at));
            }
        }
        seen
    }

    /// Every session Relais can place, with the alias of the account that last ran it.
    pub fn session_accounts(&self) -> HashMap<String, String> {
        self.session_sightings()
            .into_iter()
            .map(|(id, (alias, _))| (id, alias))
            .collect()
    }

    /// Per account, the conversations seen since `day_start` (the local
    /// midnight) and in the last seven days.
    pub fn conversation_counts(
        &self,
        now: i64,
        day_start: i64,
    ) -> HashMap<String, ConversationCounts> {
        let mut counts: HashMap<String, ConversationCounts> = HashMap::new();
        for (alias, seen_at) in self.session_sightings().into_values() {
            let entry = counts.entry(alias).or_default();
            if seen_at >= day_start {
                entry.today += 1;
            }
            if seen_at >= now - 7 * 86_400 {
                entry.week += 1;
            }
            entry.last_seen = Some(entry.last_seen.map_or(seen_at, |last| last.max(seen_at)));
        }
        counts
    }

    /// What `alias` used over time, oldest first: Relais's own readings and
    /// the desktop app's history for the account's organization.
    pub fn usage_history(&self, alias: &str) -> Vec<Sample> {
        let Some(account) = self.registry.get(alias) else {
            return Vec::new();
        };
        let mut samples = HistoryStore::new(&self.layout.usage_dir()).load(alias);
        if let Some(org) = account.org_id.as_deref() {
            for desktop in self.desktop_histories() {
                samples.extend(history::from_desktop_history(&desktop, org));
            }
        }
        samples.sort_by_key(|s| s.t);
        samples.dedup_by_key(|s| s.t);
        samples
    }

    /// The desktop app's conversations working right now: closing the app
    /// stops them.
    pub fn live_desktop_conversations(&self, now: i64) -> usize {
        self.recent_conversations(LIVE_LOOKUP, now)
            .map(|recent| {
                recent
                    .iter()
                    .filter(|c| now - c.last_activity < LIVE_FOR)
                    .filter(|c| c.conversation.entrypoint.as_deref() == Some("claude-desktop"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The latest conversations started by a person, most recent message first,
    /// with the account that ran each one and its activity over the last hour.
    pub fn recent_conversations(&self, limit: usize, now: i64) -> Result<Vec<RecentConversation>> {
        let accounts = self.session_accounts();
        let projects = self.layout.claude_dir.join("projects");
        let mut recent: Vec<RecentConversation> =
            transcripts::list_conversations(&projects, limit)?
                .into_iter()
                .map(|conversation| {
                    let activity = (now - conversation.last_activity < 3600)
                        .then(|| transcripts::activity(&conversation.path, now).ok())
                        .flatten();
                    RecentConversation {
                        account: accounts.get(&conversation.session_id).cloned(),
                        last_activity: activity
                            .as_ref()
                            .and_then(|a| a.last_message)
                            .unwrap_or(conversation.last_activity),
                        activity: activity.map_or_else(|| vec![0; ACTIVITY_BUCKETS], |a| a.buckets),
                        conversation,
                    }
                })
                .collect();
        recent.sort_by_key(|c| std::cmp::Reverse(c.last_activity));
        Ok(recent)
    }

    /// The account a session last ran on, in a terminal or in the desktop app.
    pub fn account_of_session(&self, session_id: &str) -> Option<String> {
        self.session_accounts().remove(session_id)
    }

    /// The desktop app's data folders: the one in place, and those parked for
    /// the accounts the app is not using right now.
    fn desktop_dirs(&self) -> Vec<PathBuf> {
        std::iter::once(self.layout.desktop_dir.clone())
            .chain(
                self.registry
                    .accounts
                    .iter()
                    .map(|account| self.layout.desktop_profile_dir(&account.alias)),
            )
            .filter(|dir| dir.is_dir())
            .collect()
    }

    /// The usage history each desktop app folder keeps.
    fn desktop_histories(&self) -> Vec<Value> {
        self.desktop_dirs()
            .iter()
            .filter_map(|dir| {
                fsutil::read_json(&dir.join(crate::layout::DESKTOP_USAGE_HISTORY))
                    .ok()
                    .flatten()
            })
            .collect()
    }

    /// Whether the status line measured `alias` in the last ten minutes, that
    /// is, terminals are running on it: Claude Code is not asked about it then.
    pub fn measured_lately(&self, alias: &str, now: i64) -> bool {
        UsageStore::new(&self.layout.usage_dir())
            .load(alias)
            .ok()
            .flatten()
            .is_some_and(|reading| {
                reading.source == usage::UsageSource::Statusline
                    && now - reading.observed_at < MEASURED_LATELY
            })
    }

    /// Whether Claude Code should be asked for `alias`'s limits in the
    /// background: the status line did not measure it lately, and it was not
    /// asked in the last quarter of an hour.
    pub fn needs_usage_refresh(&self, alias: &str, now: i64) -> bool {
        if self.measured_lately(alias, now) {
            return false;
        }
        self.usage_attempts()
            .get(alias)
            .is_none_or(|at| now - at >= USAGE_EVERY)
    }

    fn usage_attempts_file(&self) -> PathBuf {
        self.layout.relais_dir.join("probe").join("attempts.json")
    }

    fn usage_attempts(&self) -> HashMap<String, i64> {
        fsutil::read_json(&self.usage_attempts_file())
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn note_usage_attempt(&self, alias: &str, at: i64) {
        note_for_account(&self.usage_attempts_file(), alias, Some(json!(at)));
    }

    fn usage_failures_file(&self) -> PathBuf {
        self.layout.relais_dir.join("probe").join("failures.json")
    }

    fn usage_failures(&self) -> HashMap<String, String> {
        fsutil::read_json(&self.usage_failures_file())
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// Why the last measure of `alias` failed: a code of the probe, like
    /// `setup-trust`. Nothing once one succeeded.
    pub fn usage_failure(&self, alias: &str) -> Option<String> {
        self.usage_failures().remove(alias)
    }

    /// Claude Code asks whether to trust the folder Relais measures from. The
    /// measure never answers for the user: `relais allow-measure` lets them.
    pub fn measure_needs_trust(&self) -> bool {
        self.usage_failures()
            .values()
            .any(|reason| reason == usage_probe::TRUST_NEEDED)
    }

    /// The empty folder of Relais's own that Claude Code is opened in to
    /// measure an account.
    pub fn measure_workspace(&self) -> Result<PathBuf> {
        let workspace = self.layout.relais_dir.join("probe").join("workspace");
        fs::create_dir_all(&workspace).map_err(|e| Error::io(&workspace, e))?;
        Ok(workspace)
    }

    /// Asks Claude Code for `alias`'s limits and stores the reading, reset
    /// times included: `/usage` in a private terminal, nothing billed, nothing
    /// saved. A profile signed out, or holding another account, is not asked.
    pub fn refresh_usage<Tz: chrono::TimeZone>(
        &self,
        alias: &str,
        binary: &Path,
        now: chrono::DateTime<Tz>,
    ) -> Result<Option<UsageReading>> {
        let account = self
            .registry
            .get(alias)
            .ok_or_else(|| Error::UnknownAccount(alias.to_string()))?;
        // A signed-out profile has no limits to tell, and a profile holding
        // another account would tell someone else's. The check takes a quarter
        // of a second; when it fails, the question is asked anyway.
        if let Ok(status) = claude::auth_status(binary, account.config_dir.as_deref()) {
            let another = match (&status.email, &account.email) {
                (Some(there), Some(ours)) => !there.eq_ignore_ascii_case(ours),
                _ => false,
            };
            if !status.logged_in || another {
                return Ok(None);
            }
        }
        let workspace = self.measure_workspace()?;
        self.note_usage_attempt(alias, now.timestamp());
        let profile = account.config_dir.as_deref();
        let text = usage_probe::audit_policy(&self.layout, profile)
            .and_then(|()| usage_probe::report(binary, profile, &workspace, USAGE_PROBE_TIMEOUT));
        note_for_account(
            &self.usage_failures_file(),
            alias,
            text.as_ref().err().map(|e| json!(usage_probe::reason(e))),
        );
        let text = text?;
        let Some(reading) = usage::parse_usage_report(&text, now.clone()) else {
            return Ok(None);
        };
        let store = UsageStore::new(&self.layout.usage_dir());
        store.save(alias, &reading)?;
        // Best effort: without them, the account shows its overall limits only.
        let _ = store.save_models(alias, &usage::parse_model_limits(&text, now));
        let _ = HistoryStore::new(&self.layout.usage_dir())
            .record(alias, Sample::from_reading(&reading));
        Ok(Some(reading))
    }

    /// The limits on single models (`Fable`) Claude Code last reported for an account.
    pub fn model_limits_of(&self, alias: &str) -> Option<ModelLimits> {
        UsageStore::new(&self.layout.usage_dir())
            .load_models(alias)
            .ok()
            .flatten()
    }

    /// The newest reading for an account, from the status line or the desktop app.
    pub fn usage_of(&self, alias: &str) -> Option<UsageReading> {
        let account = self.registry.get(alias)?;
        let stored = UsageStore::new(&self.layout.usage_dir())
            .load(alias)
            .ok()
            .flatten();
        let desktop = account.org_id.as_deref().and_then(|org| {
            self.desktop_histories()
                .iter()
                .filter_map(|history| usage::reading_from_desktop_history(history, org))
                .max_by_key(|reading| reading.observed_at)
        });
        match (stored, desktop) {
            (Some(stored), Some(desktop)) => Some(if desktop.observed_at > stored.observed_at {
                desktop.with_resets_from(&stored)
            } else {
                stored.with_resets_from(&desktop)
            }),
            (stored, desktop) => stored.or(desktop),
        }
    }

    /// The account with the most room left, other than `alias`.
    pub fn best_account_other_than(&self, alias: &str, now: i64) -> Option<String> {
        let readings: Vec<(String, Option<UsageReading>)> = self
            .registry
            .accounts
            .iter()
            .filter(|account| account.alias != alias)
            .map(|account| (account.alias.clone(), self.usage_of(&account.alias)))
            .collect();
        let candidates: Vec<(&str, Option<&UsageReading>)> = readings
            .iter()
            .map(|(alias, reading)| (alias.as_str(), reading.as_ref()))
            .collect();
        usage::best_account(&candidates, now).map(String::from)
    }

    /// The account with the most room left, other than the active one.
    pub fn best_other_account(&self, now: i64) -> Option<String> {
        let active = self.registry.active.clone().unwrap_or_default();
        self.best_account_other_than(&active, now)
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
                chrome_profile: None,
                color: None,
            })
            .unwrap();
        registry
            .add(Account {
                alias: "work".into(),
                email: Some("work@example.com".into()),
                account_uuid: Some("acc-work".into()),
                org_id: Some("org-work".into()),
                config_dir: Some(layout.profile_dir("work")),
                chrome_profile: None,
                color: None,
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

    #[test]
    fn best_account_other_than_ignores_the_given_account_even_when_not_active() {
        let (_root, mut relais) = fixture();
        relais
            .registry
            .add(Account {
                alias: "spare".into(),
                email: Some("spare@example.com".into()),
                account_uuid: None,
                org_id: None,
                config_dir: Some(relais.layout.profile_dir("spare")),
                chrome_profile: None,
                color: None,
            })
            .unwrap();
        relais
            .record_statusline(None, &statusline_input(1.0), NOW)
            .unwrap();
        relais
            .record_statusline(
                Some(&relais.layout.profile_dir("work")),
                &statusline_input(30.0),
                NOW,
            )
            .unwrap();
        relais
            .record_statusline(
                Some(&relais.layout.profile_dir("spare")),
                &statusline_input(60.0),
                NOW,
            )
            .unwrap();

        assert_eq!(
            relais.best_account_other_than("main", NOW).as_deref(),
            Some("work")
        );
        assert_eq!(
            relais.best_account_other_than("work", NOW).as_deref(),
            Some("main")
        );
    }

    #[test]
    fn statusline_records_which_account_runs_each_session() {
        let (_root, relais) = fixture();

        relais
            .record_statusline(
                Some(&relais.layout.profile_dir("work")),
                &statusline_input(42.0),
                NOW,
            )
            .unwrap();
        relais
            .record_statusline(None, br#"{"session_id":"s-2"}"#, NOW)
            .unwrap();

        assert_eq!(relais.account_of_session("s-1").as_deref(), Some("work"));
        assert_eq!(relais.account_of_session("s-2").as_deref(), Some("main"));
        assert_eq!(relais.account_of_session("s-3"), None);
    }

    fn desktop_record(relais: &Relais, account_uuid: &str, cli_session: &str, written: i64) {
        let folder = relais
            .layout
            .desktop_dir
            .join("claude-code-sessions")
            .join(account_uuid)
            .join("org");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("local_{cli_session}.json"));
        fs::write(&path, format!(r#"{{"cliSessionId":"{cli_session}"}}"#)).unwrap();
        let written = std::time::UNIX_EPOCH + std::time::Duration::from_secs(written as u64);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(written)
            .unwrap();
    }

    #[test]
    fn a_desktop_session_belongs_to_the_account_whose_sidebar_lists_it() {
        let (_root, relais) = fixture();
        desktop_record(&relais, "acc-work", "cli-9", NOW);

        assert_eq!(relais.account_of_session("cli-9").as_deref(), Some("work"));
        assert_eq!(
            relais.session_accounts().get("cli-9").map(String::as_str),
            Some("work")
        );
    }

    #[test]
    fn the_latest_sighting_wins_between_status_line_and_desktop() {
        let (_root, relais) = fixture();
        relais
            .record_statusline(None, &statusline_input(5.0), NOW)
            .unwrap();
        desktop_record(&relais, "acc-work", "s-1", NOW - 3600);
        assert_eq!(relais.account_of_session("s-1").as_deref(), Some("main"));

        desktop_record(&relais, "acc-work", "s-1", NOW + 60);
        assert_eq!(relais.account_of_session("s-1").as_deref(), Some("work"));
    }

    #[test]
    fn a_desktop_account_relais_does_not_know_is_ignored() {
        let (_root, relais) = fixture();
        desktop_record(&relais, "acc-stranger", "cli-9", NOW);
        assert_eq!(relais.account_of_session("cli-9"), None);
    }

    fn transcript(relais: &Relais, session: &str, lines: &[Value], written: i64) {
        let project = relais.layout.claude_dir.join("projects").join("-work");
        fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{session}.jsonl"));
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(&path, body).unwrap();
        let written = std::time::UNIX_EPOCH + std::time::Duration::from_secs(written as u64);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(written)
            .unwrap();
    }

    fn at(secs: i64) -> String {
        chrono::DateTime::from_timestamp(secs, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    #[test]
    fn only_the_desktop_conversations_writing_right_now_count_as_live() {
        let (_root, relais) = fixture();
        let line = |entrypoint: &str, when: i64| {
            json!({"type": "user", "timestamp": at(when), "cwd": "/work", "entrypoint": entrypoint,
                   "message": {"role": "user", "content": "go"}})
        };
        let desktop = "claude-desktop";
        transcript(&relais, "live", &[line(desktop, NOW - 30)], NOW - 30);
        transcript(&relais, "over", &[line(desktop, NOW - 600)], NOW - 600);
        transcript(&relais, "terminal", &[line("cli", NOW - 30)], NOW - 30);

        assert_eq!(relais.live_desktop_conversations(NOW), 1);
    }

    #[test]
    fn recent_conversations_carry_their_account_and_their_last_message() {
        let (_root, relais) = fixture();
        let user = |text: &str, when: i64| {
            json!({"type": "user", "timestamp": at(when), "cwd": "/work", "entrypoint": "cli",
                   "message": {"role": "user", "content": text}})
        };
        // s-1: last message 10 minutes ago, but Claude Code wrote a bookkeeping
        // line 30 seconds ago. s-2: a message 2 minutes ago.
        transcript(
            &relais,
            "s-1",
            &[
                user("first", NOW - 600),
                json!({"type": "mode", "timestamp": at(NOW - 30)}),
            ],
            NOW - 30,
        );
        transcript(&relais, "s-2", &[user("second", NOW - 120)], NOW - 120);
        relais
            .record_statusline(
                Some(&relais.layout.profile_dir("work")),
                &serde_json::to_vec(&json!({"session_id": "s-1"})).unwrap(),
                NOW,
            )
            .unwrap();

        let recent = relais.recent_conversations(10, NOW).unwrap();

        let ids: Vec<&str> = recent
            .iter()
            .map(|c| c.conversation.session_id.as_str())
            .collect();
        assert_eq!(ids, vec!["s-2", "s-1"]);
        assert_eq!(recent[1].account.as_deref(), Some("work"));
        assert_eq!(recent[1].last_activity, NOW - 600);
        assert_eq!(recent[1].activity.iter().sum::<u32>(), 1);
        assert_eq!(recent[0].account, None);
        assert_eq!(recent[0].last_activity, NOW - 120);
    }

    #[test]
    fn status_line_readings_build_the_usage_history() {
        let (_root, relais) = fixture();

        relais
            .record_statusline(None, &statusline_input(20.0), NOW)
            .unwrap();
        relais
            .record_statusline(None, &statusline_input(35.0), NOW + 60)
            .unwrap();

        let history = relais.usage_history("main");
        let five: Vec<Option<f64>> = history.iter().map(|s| s.fh).collect();
        assert_eq!(five, vec![Some(20.0), Some(35.0)]);
    }

    #[test]
    fn the_desktop_history_joins_the_usage_history_of_its_account() {
        let (_root, relais) = fixture();
        fs::create_dir_all(&relais.layout.desktop_dir).unwrap();
        let history = json!({"samples": [{"t": (NOW - 3600) * 1000, "org": "org-main", "u": {"fh": 5, "sd": 9}}]});
        fs::write(relais.layout.desktop_usage_history(), history.to_string()).unwrap();
        relais
            .record_statusline(None, &statusline_input(20.0), NOW)
            .unwrap();

        let times: Vec<i64> = relais.usage_history("main").iter().map(|s| s.t).collect();

        assert_eq!(times, vec![NOW - 3600, NOW]);
    }

    #[test]
    fn conversations_are_counted_per_account_for_today_and_the_week() {
        let (_root, relais) = fixture();
        let seen = |session: &str, dir: Option<PathBuf>, when: i64| {
            relais
                .record_statusline(
                    dir.as_deref(),
                    &serde_json::to_vec(&json!({"session_id": session})).unwrap(),
                    when,
                )
                .unwrap();
        };
        let work = Some(relais.layout.profile_dir("work"));
        seen("s-1", None, NOW);
        seen("s-2", work.clone(), NOW - 2 * 86_400);
        seen("s-3", work, NOW - 10 * 86_400);
        desktop_record(&relais, "acc-work", "cli-9", NOW - 3600);

        let counts = relais.conversation_counts(NOW, NOW - 2 * 3600);

        assert_eq!(counts["main"].today, 1);
        assert_eq!(counts["main"].week, 1);
        assert_eq!(counts["work"].today, 1);
        assert_eq!(counts["work"].week, 2);
        assert_eq!(counts["work"].last_seen, Some(NOW - 3600));
    }

    #[test]
    fn a_parked_desktop_folder_still_counts_for_usage_and_sessions() {
        let (_root, relais) = fixture();
        let parked = relais.layout.desktop_profile_dir("main");
        fs::create_dir_all(parked.join("claude-code-sessions/acc-main/org")).unwrap();
        let history = json!({"samples": [{"t": (NOW - 600) * 1000, "org": "org-main", "u": {"fh": 7, "sd": 21}}]});
        fs::write(parked.join("plan-usage-history.json"), history.to_string()).unwrap();
        fs::write(
            parked.join("claude-code-sessions/acc-main/org/local_x.json"),
            r#"{"cliSessionId":"cli-parked"}"#,
        )
        .unwrap();

        let times: Vec<i64> = relais.usage_history("main").iter().map(|s| s.t).collect();

        assert_eq!(times, vec![NOW - 600]);
        assert_eq!(
            relais.usage_of("main").unwrap().five_hour_now(NOW),
            Some(7.0)
        );
        assert_eq!(
            relais.account_of_session("cli-parked").as_deref(),
            Some("main")
        );
    }

    #[test]
    fn a_newer_reading_without_reset_times_keeps_the_known_ones() {
        let (_root, relais) = fixture();
        relais
            .record_statusline(None, &statusline_input(20.0), NOW - 600)
            .unwrap();
        fs::create_dir_all(&relais.layout.desktop_dir).unwrap();
        let history =
            json!({"samples": [{"t": NOW * 1000, "org": "org-main", "u": {"fh": 55, "sd": 12}}]});
        fs::write(relais.layout.desktop_usage_history(), history.to_string()).unwrap();

        let reading = relais.usage_of("main").unwrap();

        assert_eq!(reading.five_hour_now(NOW), Some(55.0));
        assert_eq!(
            reading.seven_day.as_ref().unwrap().resets_at,
            Some(NOW + 86_400)
        );
        assert_eq!(
            reading.five_hour.as_ref().unwrap().resets_at,
            Some(NOW + 3600)
        );
    }

    #[test]
    fn a_reset_time_already_past_is_not_carried_over() {
        let (_root, relais) = fixture();
        relais
            .record_statusline(None, &statusline_input(20.0), NOW - 600)
            .unwrap();
        fs::create_dir_all(&relais.layout.desktop_dir).unwrap();
        let later = NOW + 2 * 3600;
        let history =
            json!({"samples": [{"t": later * 1000, "org": "org-main", "u": {"fh": 5, "sd": 12}}]});
        fs::write(relais.layout.desktop_usage_history(), history.to_string()).unwrap();

        let reading = relais.usage_of("main").unwrap();

        assert_eq!(reading.five_hour.as_ref().unwrap().resets_at, None);
        assert_eq!(
            reading.seven_day.as_ref().unwrap().resets_at,
            Some(NOW + 86_400)
        );
    }

    /// A stand-in for Claude Code signed in with `email`: a prompt, the
    /// official command once `/usage` is typed, `panel` once Enter is pressed.
    /// Each run of the client is written down in `runs`.
    fn fake_client(dir: &Path, name: &str, email: &str, panel: &str) -> PathBuf {
        let path = dir.join(name);
        let script = format!(
            r#"#!/bin/sh
if [ "$1 $2" = "auth status" ]; then printf '{{"loggedIn":true,"email":"{email}"}}'; exit 0; fi
printf 'run\n' >> "{runs}"
stty raw -echo
printf 'Claude Code\r\n❯\r\n'
dd bs=1 count=6 >/dev/null 2>&1
printf '/usage Show plan usage limits\r\n'
dd bs=1 count=1 >/dev/null 2>&1
printf '\033[2J\033[HSettings: Usage\r\n{panel}'
sleep 20
"#,
            runs = dir.join("runs").display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        path
    }

    const WORK_EMAIL: &str = "work@example.com";

    fn fake_usage_command(dir: &Path) -> PathBuf {
        fake_client(
            dir,
            "claude",
            WORK_EMAIL,
            "Current session: 4%% used\\r\\nCurrent week (all models): 37%% used · resets Oct 8 at 10pm (Europe/Zurich)\\r\\nCurrent week (Fable): 12%% used · resets Oct 8 at 10pm (Europe/Zurich)\\r\\n",
        )
    }

    fn zurich(at: i64) -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::from_timestamp(at, 0)
            .unwrap()
            .with_timezone(&chrono::FixedOffset::east_opt(2 * 3600).unwrap())
    }

    #[test]
    fn the_usage_command_gives_an_account_its_reset_times() {
        let (root, relais) = fixture();
        let binary = fake_usage_command(root.path());

        let reading = relais
            .refresh_usage("work", &binary, zurich(NOW))
            .unwrap()
            .unwrap();

        assert_eq!(reading.seven_day.as_ref().unwrap().used_percentage, 37.0);
        let stored = relais.usage_of("work").unwrap();
        assert!(stored.seven_day.unwrap().resets_at.is_some());
        assert!(relais.layout.relais_dir.join("probe").is_dir());
    }

    #[test]
    fn the_usage_command_also_gives_the_limit_of_each_model() {
        let (root, relais) = fixture();
        let binary = fake_usage_command(root.path());
        assert!(relais.model_limits_of("work").is_none());

        relais.refresh_usage("work", &binary, zurich(NOW)).unwrap();

        let limits = relais.model_limits_of("work").unwrap();
        assert_eq!(limits.observed_at, NOW);
        assert_eq!(limits.windows.len(), 1);
        assert_eq!(limits.windows[0].model, "Fable");
        assert_eq!(limits.windows[0].used_percentage, 12.0);
    }

    /// A profile nobody is signed in to: the sign-in check says so, and a run
    /// of `/usage` is written down in `runs`.
    fn fake_signed_out(dir: &Path) -> PathBuf {
        let path = dir.join("claude-signed-out-profile");
        let script = format!(
            r#"#!/bin/sh
if [ "$1 $2" = "auth status" ]; then printf '{{"loggedIn":false}}'; exit 1; fi
printf 'run\n' >> "{runs}"
printf '{{"type":"result","num_turns":0,"result":"Not logged in"}}'
"#,
            runs = dir.join("runs").display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_signed_out_account_is_not_asked_and_not_held_back() {
        let (root, relais) = fixture();
        let binary = fake_signed_out(root.path());

        assert!(relais
            .refresh_usage("work", &binary, zurich(NOW))
            .unwrap()
            .is_none());

        assert!(!root.path().join("runs").exists());
        assert!(relais.needs_usage_refresh("work", NOW + 60));
    }

    #[test]
    fn a_report_without_limits_leaves_the_model_limits_alone() {
        let (root, relais) = fixture();
        relais
            .refresh_usage("work", &fake_usage_command(root.path()), zurich(NOW))
            .unwrap();
        let quiet = root.path().join("claude-quiet");
        fs::write(&quiet, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&quiet, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        assert!(relais
            .refresh_usage("work", &quiet, zurich(NOW + 60))
            .is_err());

        assert_eq!(relais.model_limits_of("work").unwrap().windows.len(), 1);
    }

    #[test]
    fn an_account_is_measured_again_every_quarter_of_an_hour_even_with_its_reset_known() {
        let (root, relais) = fixture();
        let binary = fake_usage_command(root.path());
        assert!(relais.needs_usage_refresh("work", NOW));

        relais.refresh_usage("work", &binary, zurich(NOW)).unwrap();

        assert!(!relais.needs_usage_refresh("work", NOW + 14 * 60));
        assert!(relais.needs_usage_refresh("work", NOW + 15 * 60));
    }

    #[test]
    fn measures_started_together_each_leave_their_attempt() {
        let (_root, relais) = fixture();
        fs::create_dir_all(relais.layout.relais_dir.join("probe")).unwrap();
        let aliases: Vec<String> = (0..16).map(|n| format!("account-{n}")).collect();

        std::thread::scope(|scope| {
            for alias in &aliases {
                let relais = &relais;
                scope.spawn(move || relais.note_usage_attempt(alias, NOW));
            }
        });

        assert_eq!(relais.usage_attempts().len(), aliases.len());
    }

    #[test]
    fn a_failed_measure_waits_its_turn_like_the_others() {
        let (root, relais) = fixture();
        let quiet = root.path().join("claude-quiet");
        fs::write(&quiet, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&quiet, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        assert!(relais.refresh_usage("work", &quiet, zurich(NOW)).is_err());

        assert!(!relais.needs_usage_refresh("work", NOW + 60));
    }

    #[test]
    fn a_refused_measure_says_why_until_one_succeeds() {
        let (root, relais) = fixture();
        let wary = root.path().join("claude-wary");
        fs::write(
            &wary,
            "#!/bin/sh\nif [ \"$1 $2\" = \"auth status\" ]; then printf '{\"loggedIn\":true}'; exit 0; fi\nprintf 'Do you trust the files in this folder?\\r\\n'\nsleep 20\n",
        )
        .unwrap();
        fs::set_permissions(&wary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        assert!(relais.refresh_usage("work", &wary, zurich(NOW)).is_err());

        assert_eq!(relais.usage_failure("work").as_deref(), Some("setup-trust"));
        assert!(relais.measure_needs_trust());

        relais
            .refresh_usage("work", &fake_usage_command(root.path()), zurich(NOW))
            .unwrap();

        assert_eq!(relais.usage_failure("work"), None);
        assert!(!relais.measure_needs_trust());
    }

    #[test]
    fn a_profile_signed_in_with_another_account_is_not_measured() {
        let (root, relais) = fixture();
        let binary = fake_client(
            root.path(),
            "claude-other",
            "other@example.com",
            "Current week (all models): 37%% used\\r\\n",
        );

        assert!(relais
            .refresh_usage("work", &binary, zurich(NOW))
            .unwrap()
            .is_none());

        assert!(!root.path().join("runs").exists());
        assert!(relais.usage_of("work").is_none());
    }

    #[test]
    fn the_address_of_a_profile_is_compared_without_its_case() {
        let (root, relais) = fixture();
        let binary = fake_client(
            root.path(),
            "claude-case",
            "WORK@Example.com",
            "Current week (all models): 37%% used\\r\\n",
        );

        let reading = relais.refresh_usage("work", &binary, zurich(NOW)).unwrap();

        assert!(reading.is_some());
    }

    #[test]
    fn the_account_the_terminals_run_on_counts_as_measured_lately() {
        let (_root, relais) = fixture();
        relais
            .record_statusline(None, &statusline_input(5.0), NOW)
            .unwrap();

        assert!(relais.measured_lately("main", NOW + 60));
        assert!(!relais.measured_lately("main", NOW + 11 * 60));
        assert!(!relais.measured_lately("work", NOW));
    }

    fn signed_in(email: &str) -> SignedIn {
        SignedIn {
            email: email.into(),
            account_uuid: Some(format!("acc-{email}")),
            org_id: Some(format!("org-{email}")),
        }
    }

    #[test]
    fn adopting_gives_the_default_profile_to_the_account_signed_in_there() {
        let (_root, mut relais) = fixture();

        let adoption = relais
            .adopt_default_profile(signed_in("new@example.com"), None, NOW)
            .unwrap();

        assert_eq!(adoption.alias, "new");
        assert_eq!(adoption.previous, "main");
        let aliases: Vec<&str> = relais
            .registry
            .accounts
            .iter()
            .map(|a| a.alias.as_str())
            .collect();
        assert_eq!(aliases, ["main", "work", "new"]);
        let new = relais.registry.get("new").unwrap();
        assert!(new.is_default_profile());
        assert_eq!(new.email.as_deref(), Some("new@example.com"));
        assert_eq!(new.account_uuid.as_deref(), Some("acc-new@example.com"));
        assert_eq!(new.org_id.as_deref(), Some("org-new@example.com"));
        let main = relais.registry.get("main").unwrap();
        assert_eq!(main.config_dir, Some(relais.layout.profile_dir("main")));
        assert_eq!(main.chrome_profile.as_deref(), Some("Relais-main"));
        assert_eq!(main.email.as_deref(), Some("main@example.com"));
        assert_eq!(main.account_uuid.as_deref(), Some("acc-main"));
        assert_eq!(
            relais.registry.get("work").unwrap().config_dir,
            Some(relais.layout.profile_dir("work")),
            "the other accounts are not touched"
        );
    }

    #[test]
    fn the_active_marker_follows_the_default_profile() {
        let (_root, mut relais) = fixture();
        assert_eq!(relais.registry.active.as_deref(), Some("main"));
        relais
            .adopt_default_profile(signed_in("new@example.com"), None, NOW)
            .unwrap();
        assert_eq!(
            relais.registry.active.as_deref(),
            Some("new"),
            "the terminals still run on the default profile"
        );

        let (_root, mut relais) = fixture();
        relais.registry.set_active("work").unwrap();
        relais
            .adopt_default_profile(signed_in("new@example.com"), None, NOW)
            .unwrap();
        assert_eq!(relais.registry.active.as_deref(), Some("work"));
    }

    #[test]
    fn the_reading_under_the_previous_name_is_set_aside() {
        let (_root, mut relais) = fixture();
        relais.save().unwrap();
        relais
            .record_statusline(None, &statusline_input(42.0), NOW)
            .unwrap();
        assert!(relais.usage_of("main").is_some());

        relais
            .adopt_default_profile(signed_in("new@example.com"), None, NOW)
            .unwrap();

        assert!(
            relais.usage_of("main").is_none(),
            "nothing says whose reading it was"
        );
        let saved = relais
            .layout
            .backups_dir()
            .join("adopt")
            .join(NOW.to_string());
        assert!(saved.join("usage-main.json").is_file());
        let registry: Value =
            serde_json::from_slice(&fs::read(saved.join("accounts.json")).unwrap()).unwrap();
        assert_eq!(registry["accounts"][0]["alias"], json!("main"));
        assert!(
            registry["accounts"][0].get("configDir").is_none(),
            "the registry as it was before"
        );
    }

    #[test]
    fn the_desktop_app_stays_with_the_previous_account() {
        let (_root, mut relais) = fixture();
        crate::claude::desktop_profile::set_owner(&relais.layout, "main").unwrap();

        relais
            .adopt_default_profile(signed_in("new@example.com"), None, NOW)
            .unwrap();

        assert_eq!(
            crate::claude::desktop_profile::owner(&relais.layout, &relais.registry).as_deref(),
            Some("main"),
            "its sign-in did not change"
        );
    }

    #[test]
    fn an_account_registered_with_its_own_profile_is_not_adopted() {
        let (_root, mut relais) = fixture();
        let before = relais.registry.clone();

        let error = relais
            .adopt_default_profile(signed_in("WORK@example.com"), None, NOW)
            .unwrap_err();

        assert!(matches!(error, Error::DuplicateEmail(_)), "{error:?}");
        assert_eq!(relais.registry, before);
    }

    #[test]
    fn the_account_already_on_the_default_profile_has_nothing_to_adopt() {
        let (_root, mut relais) = fixture();
        let before = relais.registry.clone();

        let error = relais
            .adopt_default_profile(signed_in("Main@Example.com"), None, NOW)
            .unwrap_err();

        assert!(
            matches!(&error, Error::AlreadyOnDefaultProfile(alias) if alias == "main"),
            "{error:?}"
        );
        assert_eq!(relais.registry, before);
    }

    #[test]
    fn adopting_uses_the_alias_asked_and_refuses_a_taken_one() {
        let (_root, mut relais) = fixture();
        let adoption = relais
            .adopt_default_profile(signed_in("new@example.com"), Some("fresh"), NOW)
            .unwrap();
        assert_eq!(adoption.alias, "fresh");

        let (_root, mut relais) = fixture();
        let before = relais.registry.clone();
        let error = relais
            .adopt_default_profile(signed_in("new@example.com"), Some("work"), NOW)
            .unwrap_err();
        assert!(matches!(error, Error::DuplicateAlias(_)), "{error:?}");
        assert_eq!(relais.registry, before);
    }

    #[test]
    fn adopting_needs_an_account_registered_on_the_default_profile() {
        let (_root, mut relais) = fixture();
        relais.registry.remove("main").unwrap();

        let error = relais
            .adopt_default_profile(signed_in("new@example.com"), None, NOW)
            .unwrap_err();

        assert!(matches!(error, Error::NoDefaultProfileAccount), "{error:?}");
    }
}
