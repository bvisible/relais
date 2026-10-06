//! Which account the Claude desktop app ran each Code session with.
//!
//! The desktop app keeps its Code sidebar per account, in
//! `claude-code-sessions/<accountUuid>/<orgUuid>/local_<id>.json`, and each
//! record names the transcript it shows (`cliSessionId`). These records hold
//! ids, folders and titles; no credential is read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::fsutil;

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopSession {
    /// The account whose sidebar lists the session.
    pub account_uuid: String,
    /// Unix seconds of the record's last write.
    pub seen_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    cli_session_id: Option<String>,
}

/// Every Code session in the desktop app's sidebars, by transcript id. A
/// session listed by two accounts (the app can import one from another
/// account) goes to the record written last.
pub fn session_accounts(desktop_dir: &Path) -> HashMap<String, DesktopSession> {
    let mut sessions: HashMap<String, DesktopSession> = HashMap::new();
    for account in subdirectories(&desktop_dir.join("claude-code-sessions")) {
        let Some(account_uuid) = account.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for org in subdirectories(&account) {
            let Ok(entries) = fs::read_dir(&org) else {
                continue;
            };
            for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
                let is_record = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("local_") && n.ends_with(".json"));
                if !is_record {
                    continue;
                }
                let Some(cli_session_id) = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
                    .and_then(|record| record.cli_session_id)
                else {
                    continue;
                };
                let seen_at = fsutil::modified_secs(&path);
                let newer = sessions
                    .get(&cli_session_id)
                    .is_none_or(|known| known.seen_at < seen_at);
                if newer {
                    sessions.insert(
                        cli_session_id,
                        DesktopSession {
                            account_uuid: account_uuid.to_string(),
                            seen_at,
                        },
                    );
                }
            }
        }
    }
    sessions
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn record(dir: &Path, account: &str, org: &str, file: &str, body: &str) -> PathBuf {
        let folder = dir.join("claude-code-sessions").join(account).join(org);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(file);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn each_sidebar_record_names_its_transcript_and_its_account() {
        let dir = tempfile::tempdir().unwrap();
        record(
            dir.path(),
            "acc-1",
            "org-1",
            "local_aaa.json",
            r#"{"sessionId":"local_aaa","cliSessionId":"cli-1","cwd":"/work"}"#,
        );
        record(
            dir.path(),
            "acc-2",
            "org-2",
            "local_bbb.json",
            r#"{"sessionId":"local_bbb","cliSessionId":"cli-2"}"#,
        );

        let sessions = session_accounts(dir.path());

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions["cli-1"].account_uuid, "acc-1");
        assert_eq!(sessions["cli-2"].account_uuid, "acc-2");
    }

    #[test]
    fn other_files_and_broken_records_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        record(
            dir.path(),
            "acc-1",
            "org-1",
            "scheduled-tasks.json",
            r#"{"cliSessionId":"no"}"#,
        );
        record(dir.path(), "acc-1", "org-1", "deleted_ccc", "");
        record(dir.path(), "acc-1", "org-1", "local_bad.json", "not json");
        record(
            dir.path(),
            "acc-1",
            "org-1",
            "local_bare.json",
            r#"{"sessionId":"local_bare"}"#,
        );

        assert!(session_accounts(dir.path()).is_empty());
    }

    #[test]
    fn a_record_carries_the_time_it_was_last_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = record(
            dir.path(),
            "acc-1",
            "org-1",
            "local_aaa.json",
            r#"{"cliSessionId":"cli-1"}"#,
        );
        let written = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_791_000_000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(written)
            .unwrap();

        assert_eq!(session_accounts(dir.path())["cli-1"].seen_at, 1_791_000_000);
    }

    #[test]
    fn no_desktop_app_means_no_sessions() {
        let dir = tempfile::tempdir().unwrap();
        assert!(session_accounts(&dir.path().join("absent")).is_empty());
    }
}
