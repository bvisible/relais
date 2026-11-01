//! The desktop app's sign-in, apart from its workspace.
//!
//! The app's data folder holds the user's whole workspace: the sidebar, the
//! pinned conversations, the folders, the panes. What makes it signed in as
//! one account is only four files at its root: `config.json` (the encrypted
//! token cache and the last account's id) and `Cookies` (the claude.ai
//! session), with their journals. To change account, Relais parks those files
//! under `~/.relais/accounts/<alias>/identity` and puts the next account's
//! back, so the workspace never moves. The files are only renamed: never
//! opened, read, copied or logged, so the sign-in stays the app's own.

use std::fs;
use std::path::Path;

use serde::de::{Deserializer as _, IgnoredAny, MapAccess, Visitor};

use crate::error::{Error, Result};
use crate::layout::Layout;

/// The files that hold the desktop app's sign-in, whichever of them exist.
pub const IDENTITY_FILES: [&str; 4] = [
    "config.json",
    "config.json.journal",
    "Cookies",
    "Cookies-journal",
];

/// Moves the sign-in in place to `alias`'s parking. Nothing moves when the
/// parking already holds one of the files: a sign-in is never overwritten.
pub fn park(layout: &Layout, alias: &str) -> Result<usize> {
    move_all(&layout.desktop_dir, &layout.identity_dir(alias))
}

/// Puts `alias`'s parked sign-in in place. Nothing moves when the data
/// folder still holds one of the files.
pub fn put_back(layout: &Layout, alias: &str) -> Result<usize> {
    move_all(&layout.identity_dir(alias), &layout.desktop_dir)
}

/// Whether `alias` has a sign-in parked.
pub fn is_parked(layout: &Layout, alias: &str) -> bool {
    holds_sign_in(&layout.identity_dir(alias))
}

/// The account a sign-in belongs to: the `lastKnownAccountUuid` of the
/// `config.json` in `dir`. That one string is kept; every other value of the
/// file, the token cache among them, is skipped as it is read, never held.
pub fn account_of(dir: &Path) -> Option<String> {
    let file = fs::File::open(dir.join("config.json")).ok()?;
    let mut reader = serde_json::Deserializer::from_reader(std::io::BufReader::new(file));
    reader.deserialize_map(LastAccount).ok().flatten()
}

/// Keeps `lastKnownAccountUuid` when it is a string; skips everything else.
struct LastAccount;

impl<'de> Visitor<'de> for LastAccount {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("the desktop app's config object")
    }

    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut account = None;
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            if key == "lastKnownAccountUuid" {
                account = match map.next_value::<serde_json::Value>()? {
                    serde_json::Value::String(uuid) => Some(uuid),
                    _ => None,
                };
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(account)
    }
}

/// Takes `alias`'s sign-in out of the whole data folder an earlier version
/// of Relais parked for it, and keeps the rest of that folder as an archive.
/// `Ok(false)` when there is no such folder, or a sign-in is already parked.
pub fn take_from_old_folder(layout: &Layout, alias: &str, date: &str) -> Result<bool> {
    let old = layout.desktop_profile_dir(alias);
    if is_parked(layout, alias) || !holds_sign_in(&old) {
        return Ok(false);
    }
    let archive = layout.desktop_archive_dir(alias, date);
    if archive.exists() {
        return Err(Error::Conflict(archive));
    }
    move_all(&old, &layout.identity_dir(alias))?;
    fs::rename(&old, &archive).map_err(|e| Error::io(&old, e))?;
    Ok(true)
}

fn holds_sign_in(dir: &Path) -> bool {
    ["config.json", "Cookies"]
        .iter()
        .any(|name| dir.join(name).is_file())
}

/// Moves the identity files present in `from` to `to`, after checking that
/// none of them is already in `to`.
fn move_all(from: &Path, to: &Path) -> Result<usize> {
    let present: Vec<&str> = IDENTITY_FILES
        .iter()
        .copied()
        .filter(|name| from.join(name).exists())
        .collect();
    if present.is_empty() {
        return Ok(0);
    }
    if let Some(taken) = IDENTITY_FILES
        .iter()
        .map(|name| to.join(name))
        .find(|path| path.exists())
    {
        return Err(Error::Conflict(taken));
    }
    fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    for name in &present {
        let source = from.join(name);
        fs::rename(&source, to.join(name)).map_err(|e| Error::io(&source, e))?;
    }
    Ok(present.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::for_home(root.path());
        fs::create_dir_all(&layout.desktop_dir).unwrap();
        (root, layout)
    }

    fn write(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        fs::read_to_string(dir.join(name)).ok()
    }

    #[test]
    fn parking_then_putting_back_keeps_every_byte() {
        let (_root, layout) = layout();
        let live = layout.desktop_dir.clone();
        write(&live, "config.json", "token-a");
        write(&live, "config.json.journal", "journal-a");
        write(&live, "Cookies", "cookie-a");
        write(&live, "Cookies-journal", "");
        write(&live, "window-state.json", "workspace");

        assert_eq!(park(&layout, "a").unwrap(), 4);

        let parked = layout.identity_dir("a");
        assert_eq!(read(&parked, "config.json").as_deref(), Some("token-a"));
        assert_eq!(read(&parked, "Cookies").as_deref(), Some("cookie-a"));
        assert!(!live.join("config.json").exists());
        assert!(!live.join("Cookies").exists());
        assert_eq!(
            read(&live, "window-state.json").as_deref(),
            Some("workspace")
        );
        assert!(is_parked(&layout, "a"));

        assert_eq!(put_back(&layout, "a").unwrap(), 4);

        assert_eq!(read(&live, "config.json").as_deref(), Some("token-a"));
        assert_eq!(
            read(&live, "config.json.journal").as_deref(),
            Some("journal-a")
        );
        assert_eq!(read(&live, "Cookies").as_deref(), Some("cookie-a"));
        assert_eq!(read(&live, "Cookies-journal").as_deref(), Some(""));
        assert!(!is_parked(&layout, "a"));
    }

    #[test]
    fn a_parked_sign_in_is_never_overwritten() {
        let (_root, layout) = layout();
        write(&layout.desktop_dir, "config.json", "token-new");
        write(&layout.desktop_dir, "Cookies", "cookie-new");
        write(&layout.identity_dir("a"), "Cookies", "cookie-old");

        assert!(matches!(park(&layout, "a"), Err(Error::Conflict(_))));

        assert_eq!(
            read(&layout.desktop_dir, "config.json").as_deref(),
            Some("token-new")
        );
        assert_eq!(
            read(&layout.desktop_dir, "Cookies").as_deref(),
            Some("cookie-new")
        );
        assert_eq!(
            read(&layout.identity_dir("a"), "Cookies").as_deref(),
            Some("cookie-old")
        );
    }

    #[test]
    fn a_sign_in_in_place_is_never_overwritten() {
        let (_root, layout) = layout();
        write(&layout.desktop_dir, "config.json", "token-in-place");
        write(&layout.identity_dir("b"), "config.json", "token-b");
        write(&layout.identity_dir("b"), "Cookies", "cookie-b");

        assert!(matches!(put_back(&layout, "b"), Err(Error::Conflict(_))));

        assert_eq!(
            read(&layout.desktop_dir, "config.json").as_deref(),
            Some("token-in-place")
        );
        assert!(!layout.desktop_dir.join("Cookies").exists());
        assert_eq!(
            read(&layout.identity_dir("b"), "Cookies").as_deref(),
            Some("cookie-b")
        );
    }

    #[test]
    fn missing_journals_are_fine() {
        let (_root, layout) = layout();
        write(&layout.desktop_dir, "config.json", "token-a");
        write(&layout.desktop_dir, "Cookies", "cookie-a");

        assert_eq!(park(&layout, "a").unwrap(), 2);
        assert_eq!(put_back(&layout, "a").unwrap(), 2);
    }

    #[test]
    fn nothing_parked_puts_nothing_back() {
        let (_root, layout) = layout();
        assert!(!is_parked(&layout, "nobody"));
        assert_eq!(put_back(&layout, "nobody").unwrap(), 0);
    }

    #[test]
    fn the_old_folder_gives_its_sign_in_and_becomes_an_archive() {
        let (_root, layout) = layout();
        let old = layout.desktop_profile_dir("a");
        write(&old, "config.json", "token-a");
        write(&old, "Cookies", "cookie-a");
        write(
            &old.join("claude-code-sessions/acc-a/org-a"),
            "local_s1.json",
            "{}",
        );

        assert!(take_from_old_folder(&layout, "a", "2026-11-01").unwrap());

        assert_eq!(
            read(&layout.identity_dir("a"), "config.json").as_deref(),
            Some("token-a")
        );
        assert_eq!(
            read(&layout.identity_dir("a"), "Cookies").as_deref(),
            Some("cookie-a")
        );
        let archive = layout.desktop_archive_dir("a", "2026-11-01");
        assert!(archive
            .join("claude-code-sessions/acc-a/org-a/local_s1.json")
            .is_file());
        assert!(!archive.join("config.json").exists());
        assert!(!old.exists());
    }

    #[test]
    fn an_account_already_parked_keeps_its_old_folder() {
        let (_root, layout) = layout();
        write(&layout.identity_dir("a"), "config.json", "token-parked");
        let old = layout.desktop_profile_dir("a");
        write(&old, "config.json", "token-older");

        assert!(!take_from_old_folder(&layout, "a", "2026-11-01").unwrap());

        assert_eq!(
            read(&layout.identity_dir("a"), "config.json").as_deref(),
            Some("token-parked")
        );
        assert_eq!(read(&old, "config.json").as_deref(), Some("token-older"));
    }

    #[test]
    fn the_account_id_is_read_and_nothing_else() {
        let (_root, layout) = layout();
        let dir = layout.desktop_dir.clone();
        write(
            &dir,
            "config.json",
            r#"{"oauth:tokenCacheV2":"secret-blob","nested":{"x":[1,2,{"y":null}]},"lastKnownAccountUuid":"acc-a","after":true}"#,
        );
        assert_eq!(account_of(&dir).as_deref(), Some("acc-a"));

        write(
            &dir,
            "config.json",
            r#"{"oauth:tokenCacheV2":"secret-blob"}"#,
        );
        assert_eq!(account_of(&dir), None);
        write(&dir, "config.json", r#"{"lastKnownAccountUuid":3}"#);
        assert_eq!(account_of(&dir), None);
        write(&dir, "config.json", "{not json");
        assert_eq!(account_of(&dir), None);
        assert_eq!(account_of(&layout.identity_dir("nobody")), None);
    }

    #[test]
    fn a_taken_archive_name_stops_before_anything_moves() {
        let (_root, layout) = layout();
        let old = layout.desktop_profile_dir("a");
        write(&old, "config.json", "token-a");
        fs::create_dir_all(layout.desktop_archive_dir("a", "2026-11-01")).unwrap();

        assert!(matches!(
            take_from_old_folder(&layout, "a", "2026-11-01"),
            Err(Error::Conflict(_))
        ));
        assert_eq!(read(&old, "config.json").as_deref(), Some("token-a"));
        assert!(!is_parked(&layout, "a"));
    }

    #[test]
    fn without_an_old_folder_there_is_nothing_to_take() {
        let (_root, layout) = layout();
        assert!(!take_from_old_folder(&layout, "a", "2026-11-01").unwrap());
        assert!(!layout.identity_dir("a").exists());
    }
}
