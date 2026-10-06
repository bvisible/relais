//! The Claude Code config directory of a non-default account.
//!
//! It is a thin directory: every entry of `~/.claude` that is not tied to an
//! account is a symbolic link back to `~/.claude`, so conversations, settings,
//! skills, plugins and hooks are the same whichever account runs. What stays
//! per account is the global config (`.claude.json`, which holds the signed-in
//! account), caches, telemetry and organization policies.

use std::fs;
use std::os::unix::fs::{symlink, DirBuilderExt};
use std::path::Path;

use crate::error::{Error, Result};

/// Entries of `~/.claude` that are never linked into a profile.
pub const PER_ACCOUNT_ENTRIES: &[&str] = &[
    ".claude.json",
    ".credentials.json",
    ".credentials.lock",
    ".last-update-result.json",
    ".last-cleanup",
    ".DS_Store",
    ".stignore",
    "backups",
    "cache",
    "debug",
    "telemetry",
    "statsig",
    "state",
    "daemon",
    "daemon.lock",
    "daemon.status.json",
    "daemon-auth-status.json",
    "daemon-auth-cooldown",
    "stats-cache.json",
    "mcp-needs-auth-cache.json",
    "policy-limits.json",
    "remote-settings.json",
];

/// Directories that must exist in `~/.claude` before profiles link them;
/// otherwise Claude Code would create a private copy inside the profile and
/// the conversation would stay with one account.
pub const ESSENTIAL_SHARED_DIRS: &[&str] = &[
    "projects",
    "sessions",
    "session-env",
    "shell-snapshots",
    "file-history",
    "todos",
    "tasks",
    "plans",
    "paste-cache",
    "ide",
];

/// What [`ensure_links`] found and did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LinkReport {
    pub created: Vec<String>,
    pub already_linked: Vec<String>,
    /// A real file or directory sits where a link is expected; left untouched.
    pub conflicts: Vec<String>,
    /// A link that points somewhere else; left untouched.
    pub foreign_links: Vec<String>,
}

/// Whether an entry of `~/.claude` is shared between accounts.
pub fn is_shared(name: &str) -> bool {
    if PER_ACCOUNT_ENTRIES.contains(&name) {
        return false;
    }
    let is_backup = name.starts_with(".claude.json.")
        || name.starts_with("settings.json.bak")
        || name.starts_with("settings.local.json.bak");
    !is_backup
}

/// Makes `profile_dir` hold a link to every shared entry of `claude_dir`.
pub fn ensure_links(claude_dir: &Path, profile_dir: &Path) -> Result<LinkReport> {
    for name in ESSENTIAL_SHARED_DIRS {
        let dir = claude_dir.join(name);
        fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(profile_dir)
        .map_err(|e| Error::io(profile_dir, e))?;

    let entries = fs::read_dir(claude_dir).map_err(|e| Error::io(claude_dir, e))?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_shared(name))
        .collect();
    names.sort();

    let mut report = LinkReport::default();
    for name in names {
        let target = claude_dir.join(&name);
        let link = profile_dir.join(&name);
        match fs::symlink_metadata(&link) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                symlink(&target, &link).map_err(|e| Error::io(&link, e))?;
                report.created.push(name);
            }
            Err(e) => return Err(Error::io(&link, e)),
            Ok(meta) if meta.file_type().is_symlink() => {
                if fs::read_link(&link).is_ok_and(|current| current == target) {
                    report.already_linked.push(name);
                } else {
                    report.foreign_links.push(name);
                }
            }
            Ok(_) => report.conflicts.push(name),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join(".claude");
        let profile = root.path().join("profile");
        fs::create_dir_all(claude.join("skills")).unwrap();
        fs::create_dir_all(claude.join("cache")).unwrap();
        fs::write(claude.join("settings.json"), "{}").unwrap();
        fs::write(claude.join("CLAUDE.md"), "# rules").unwrap();
        fs::write(claude.join(".claude.json"), "{}").unwrap();
        fs::write(claude.join("settings.json.bak-20260101"), "{}").unwrap();
        fs::write(claude.join(".claude.json.backup.123"), "{}").unwrap();
        (root, claude, profile)
    }

    #[test]
    fn shared_entries_are_linked_and_account_entries_are_not() {
        let (_root, claude, profile) = setup();

        let report = ensure_links(&claude, &profile).unwrap();

        for name in ["skills", "settings.json", "CLAUDE.md", "projects"] {
            let link = profile.join(name);
            assert_eq!(
                fs::read_link(&link).unwrap(),
                claude.join(name),
                "{name} links back"
            );
        }
        for name in [
            "cache",
            ".claude.json",
            "settings.json.bak-20260101",
            ".claude.json.backup.123",
        ] {
            assert!(
                fs::symlink_metadata(profile.join(name)).is_err(),
                "{name} stays out"
            );
        }
        assert!(report.created.contains(&"skills".to_string()));
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn essential_directories_are_created_before_being_linked() {
        let (_root, claude, profile) = setup();

        ensure_links(&claude, &profile).unwrap();

        for name in ESSENTIAL_SHARED_DIRS {
            assert!(claude.join(name).is_dir(), "{name} exists in ~/.claude");
            assert_eq!(
                fs::read_link(profile.join(name)).unwrap(),
                claude.join(name)
            );
        }
    }

    #[test]
    fn running_twice_changes_nothing() {
        let (_root, claude, profile) = setup();
        ensure_links(&claude, &profile).unwrap();

        let second = ensure_links(&claude, &profile).unwrap();

        assert!(second.created.is_empty());
        assert!(second.already_linked.contains(&"skills".to_string()));
    }

    #[test]
    fn a_real_directory_in_the_profile_is_reported_and_left_alone() {
        let (_root, claude, profile) = setup();
        fs::create_dir_all(profile.join("skills")).unwrap();
        fs::write(profile.join("skills/local.md"), "mine").unwrap();

        let report = ensure_links(&claude, &profile).unwrap();

        assert_eq!(report.conflicts, vec!["skills".to_string()]);
        assert!(profile.join("skills/local.md").exists());
    }

    #[test]
    fn a_link_pointing_elsewhere_is_reported_and_left_alone() {
        let (root, claude, profile) = setup();
        fs::create_dir_all(&profile).unwrap();
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        symlink(&elsewhere, profile.join("skills")).unwrap();

        let report = ensure_links(&claude, &profile).unwrap();

        assert_eq!(report.foreign_links, vec!["skills".to_string()]);
        assert_eq!(fs::read_link(profile.join("skills")).unwrap(), elsewhere);
    }

    #[test]
    fn backup_names_are_not_shared() {
        assert!(!is_shared(".claude.json.backup.1771500746500"));
        assert!(!is_shared("settings.json.bak"));
        assert!(!is_shared("settings.json.bak-pre-rtk-20260505-094518"));
        assert!(is_shared("settings.json"));
        assert!(is_shared("RTK.md"));
        assert!(!is_shared("telemetry"));
    }
}
