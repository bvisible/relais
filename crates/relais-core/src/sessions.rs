//! Which account each Claude Code session runs on.
//!
//! The status line is the only place that sees both a session id and the
//! profile running it, so it records the pair here. The interface then shows
//! each conversation with the color of its account.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil;

/// Sessions kept in the index; the least recently seen go first.
pub const MAX_SESSIONS: usize = 500;

/// An unchanged session is written again at most this often, since the status
/// line renders several times a minute.
pub const RECORD_EVERY_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAccount {
    pub alias: String,
    /// Unix seconds when the status line last reported the session.
    pub seen_at: i64,
}

pub struct SessionAccounts {
    path: PathBuf,
    capacity: usize,
}

impl SessionAccounts {
    pub fn new(path: &Path) -> Self {
        Self::with_capacity(path, MAX_SESSIONS)
    }

    pub fn with_capacity(path: &Path, capacity: usize) -> Self {
        SessionAccounts {
            path: path.to_path_buf(),
            capacity,
        }
    }

    /// Every recorded session; a missing index is empty.
    pub fn load(&self) -> Result<BTreeMap<String, SessionAccount>> {
        match fsutil::read_json(&self.path)? {
            None => Ok(BTreeMap::new()),
            Some(value) => serde_json::from_value(value).map_err(|e| Error::Json {
                path: self.path.clone(),
                source: e,
            }),
        }
    }

    /// Notes that `session_id` runs on `alias`.
    pub fn record(&self, session_id: &str, alias: &str, now: i64) -> Result<()> {
        let mut sessions = self.load().unwrap_or_default();
        let fresh = sessions
            .get(session_id)
            .is_some_and(|s| s.alias == alias && now - s.seen_at < RECORD_EVERY_SECS);
        if fresh {
            return Ok(());
        }
        sessions.insert(
            session_id.to_string(),
            SessionAccount {
                alias: alias.to_string(),
                seen_at: now,
            },
        );
        while sessions.len() > self.capacity {
            let oldest = sessions
                .iter()
                .min_by_key(|(_, s)| s.seen_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => sessions.remove(&id),
                None => break,
            };
        }
        let value = serde_json::to_value(&sessions).map_err(|e| Error::Json {
            path: self.path.clone(),
            source: e,
        })?;
        fsutil::write_json_atomic(&self.path, &value)
    }

    /// The account `session_id` last ran on, when the status line saw it.
    pub fn account_of(&self, session_id: &str) -> Option<String> {
        self.load()
            .ok()?
            .remove(session_id)
            .map(|session| session.alias)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_270_000;

    fn index() -> (tempfile::TempDir, SessionAccounts) {
        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionAccounts::new(&dir.path().join("sessions.json"));
        (dir, sessions)
    }

    #[test]
    fn a_recorded_session_is_found_again() {
        let (_dir, sessions) = index();

        sessions.record("s-1", "work", NOW).unwrap();

        assert_eq!(sessions.account_of("s-1").as_deref(), Some("work"));
        assert_eq!(sessions.account_of("s-2"), None);
    }

    #[test]
    fn a_session_resumed_on_another_account_follows_it() {
        let (_dir, sessions) = index();
        sessions.record("s-1", "work", NOW).unwrap();

        sessions.record("s-1", "main", NOW + 5).unwrap();

        assert_eq!(sessions.account_of("s-1").as_deref(), Some("main"));
    }

    #[test]
    fn an_unchanged_session_is_rewritten_at_most_once_a_minute() {
        let (_dir, sessions) = index();
        sessions.record("s-1", "work", NOW).unwrap();

        sessions.record("s-1", "work", NOW + 10).unwrap();
        assert_eq!(sessions.load().unwrap()["s-1"].seen_at, NOW);

        sessions
            .record("s-1", "work", NOW + RECORD_EVERY_SECS)
            .unwrap();
        assert_eq!(
            sessions.load().unwrap()["s-1"].seen_at,
            NOW + RECORD_EVERY_SECS
        );
    }

    #[test]
    fn the_oldest_sessions_go_once_the_index_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionAccounts::with_capacity(&dir.path().join("sessions.json"), 2);

        sessions.record("old", "work", NOW).unwrap();
        sessions.record("mid", "work", NOW + 1).unwrap();
        sessions.record("new", "main", NOW + 2).unwrap();

        assert_eq!(sessions.account_of("old"), None);
        assert_eq!(sessions.account_of("mid").as_deref(), Some("work"));
        assert_eq!(sessions.account_of("new").as_deref(), Some("main"));
    }

    #[test]
    fn a_missing_index_is_empty() {
        let (_dir, sessions) = index();
        assert!(sessions.load().unwrap().is_empty());
    }
}
