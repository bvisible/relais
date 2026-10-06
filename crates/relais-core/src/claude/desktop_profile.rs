//! One Claude desktop app data folder per account.
//!
//! The desktop app holds one signed-in account, in its data folder
//! (`~/Library/Application Support/Claude`). Relais gives each account a
//! folder of its own: while the app is closed, the folder in place is parked
//! under `~/.relais/accounts/<alias>/desktop` and the target account's parked
//! folder takes its place. Folders are only renamed, never read, copied or
//! deleted, so the sign-in each one holds stays the app's own.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;
use crate::registry::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// The app already uses that account's folder.
    AlreadyThere,
    /// The account's own folder is back in place, still signed in.
    Restored,
    /// The account has no folder yet: the app starts empty and asks to sign in.
    SignInNeeded,
}

#[derive(Serialize, Deserialize)]
struct State {
    owner: String,
}

/// Records that the folder in place belongs to `alias`.
pub fn set_owner(layout: &Layout, alias: &str) -> Result<()> {
    let path = layout.desktop_state_file();
    let value = serde_json::to_value(State {
        owner: alias.to_string(),
    })
    .map_err(|e| Error::Json {
        path: path.clone(),
        source: e,
    })?;
    fsutil::write_json_atomic(&path, &value)
}

/// The account whose folder is in place: the one recorded at the last switch,
/// else the account of the app's latest usage sample, since the app samples the
/// usage of the account it is signed in with.
pub fn owner(layout: &Layout, registry: &Registry) -> Option<String> {
    let recorded = fsutil::read_json(&layout.desktop_state_file())
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<State>(value).ok())
        .map(|state| state.owner)
        .filter(|alias| registry.get(alias).is_some());
    recorded.or_else(|| {
        let history = fsutil::read_json(&layout.desktop_usage_history())
            .ok()
            .flatten()?;
        let org = history
            .get("samples")?
            .as_array()?
            .iter()
            .filter_map(|s| Some((s.get("t")?.as_i64()?, s.get("org")?.as_str()?)))
            .max_by_key(|(t, _)| *t)
            .map(|(_, org)| org.to_string())?;
        registry
            .accounts
            .iter()
            .find(|a| a.org_id.as_deref() == Some(org.as_str()))
            .map(|a| a.alias.clone())
    })
}

/// Whether `alias` has a desktop app folder: in place or parked.
pub fn has_folder(layout: &Layout, registry: &Registry, alias: &str) -> bool {
    let in_place = owner(layout, registry).as_deref() == Some(alias) && layout.desktop_dir.exists();
    in_place || layout.desktop_profile_dir(alias).exists()
}

/// Puts `to`'s folder in place of the app's data folder. The app must be closed.
/// Nothing is overwritten: a folder already parked where the current one would
/// go stops the switch before anything moves.
pub fn switch(layout: &Layout, registry: &Registry, to: &str) -> Result<Switch> {
    if registry.get(to).is_none() {
        return Err(Error::UnknownAccount(to.to_string()));
    }
    let current = owner(layout, registry);
    if current.as_deref() == Some(to) {
        return Ok(Switch::AlreadyThere);
    }
    let live = &layout.desktop_dir;
    let target = layout.desktop_profile_dir(to);
    let parked = if live.exists() {
        let owner = current.ok_or(Error::UnknownDesktopOwner)?;
        let destination = layout.desktop_profile_dir(&owner);
        if destination.exists() {
            return Err(Error::Conflict(destination));
        }
        rename(live, &destination)?;
        Some(destination)
    } else {
        None
    };
    let outcome = if target.exists() {
        if let Err(error) = rename(&target, live) {
            if let Some(destination) = &parked {
                let _ = fs::rename(destination, live);
            }
            return Err(error);
        }
        Switch::Restored
    } else {
        Switch::SignInNeeded
    };
    set_owner(layout, to)?;
    Ok(outcome)
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    fs::rename(from, to).map_err(|e| Error::io(from, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Account, Registry};
    use serde_json::json;
    use std::fs;

    fn account(alias: &str, uuid: &str, org: &str) -> Account {
        Account {
            alias: alias.into(),
            email: Some(format!("{alias}@example.com")),
            account_uuid: Some(uuid.into()),
            org_id: Some(org.into()),
            config_dir: None,
            chrome_profile: None,
            color: None,
        }
    }

    fn fixture() -> (tempfile::TempDir, Layout, Registry) {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::for_home(root.path());
        let mut registry = Registry::default();
        registry
            .add(account("main", "acc-main", "org-main"))
            .unwrap();
        let mut work = account("work", "acc-work", "org-work");
        work.config_dir = Some(layout.profile_dir("work"));
        registry.add(work).unwrap();
        (root, layout, registry)
    }

    fn live_folder(layout: &Layout, marker: &str) {
        fs::create_dir_all(&layout.desktop_dir).unwrap();
        fs::write(layout.desktop_dir.join("marker"), marker).unwrap();
    }

    fn marker(dir: &Path) -> Option<String> {
        fs::read_to_string(dir.join("marker")).ok()
    }

    #[test]
    fn the_first_switch_parks_the_live_folder_and_leaves_room_for_a_sign_in() {
        let (_root, layout, registry) = fixture();
        live_folder(&layout, "main's data");
        set_owner(&layout, "main").unwrap();

        let outcome = switch(&layout, &registry, "work").unwrap();

        assert_eq!(outcome, Switch::SignInNeeded);
        assert!(!layout.desktop_dir.exists());
        assert_eq!(
            marker(&layout.desktop_profile_dir("main")).as_deref(),
            Some("main's data")
        );
        assert_eq!(owner(&layout, &registry).as_deref(), Some("work"));
    }

    #[test]
    fn switching_back_puts_the_parked_folder_in_place() {
        let (_root, layout, registry) = fixture();
        live_folder(&layout, "main's data");
        set_owner(&layout, "main").unwrap();
        switch(&layout, &registry, "work").unwrap();
        live_folder(&layout, "work's data");

        let outcome = switch(&layout, &registry, "main").unwrap();

        assert_eq!(outcome, Switch::Restored);
        assert_eq!(marker(&layout.desktop_dir).as_deref(), Some("main's data"));
        assert_eq!(
            marker(&layout.desktop_profile_dir("work")).as_deref(),
            Some("work's data")
        );
        assert!(!layout.desktop_profile_dir("main").exists());
        assert!(has_folder(&layout, &registry, "work"));
        assert!(has_folder(&layout, &registry, "main"));
    }

    #[test]
    fn switching_to_the_account_already_in_place_changes_nothing() {
        let (_root, layout, registry) = fixture();
        live_folder(&layout, "main's data");
        set_owner(&layout, "main").unwrap();

        assert_eq!(
            switch(&layout, &registry, "main").unwrap(),
            Switch::AlreadyThere
        );
        assert_eq!(marker(&layout.desktop_dir).as_deref(), Some("main's data"));
    }

    #[test]
    fn a_folder_already_parked_for_the_owner_stops_the_switch_untouched() {
        let (_root, layout, registry) = fixture();
        live_folder(&layout, "main's data");
        set_owner(&layout, "main").unwrap();
        let parked = layout.desktop_profile_dir("main");
        fs::create_dir_all(&parked).unwrap();
        fs::write(parked.join("marker"), "older copy").unwrap();

        let error = switch(&layout, &registry, "work").unwrap_err();

        assert!(matches!(error, Error::Conflict(_)));
        assert_eq!(marker(&layout.desktop_dir).as_deref(), Some("main's data"));
        assert_eq!(marker(&parked).as_deref(), Some("older copy"));
        assert_eq!(owner(&layout, &registry).as_deref(), Some("main"));
    }

    #[test]
    fn without_a_record_the_owner_is_the_account_of_the_latest_usage_sample() {
        let (_root, layout, registry) = fixture();
        live_folder(&layout, "someone's data");
        let history = json!({"samples": [
            {"t": 1_000, "org": "org-main", "u": {"fh": 1}},
            {"t": 2_000, "org": "org-work", "u": {"fh": 2}}
        ]});
        fs::write(layout.desktop_usage_history(), history.to_string()).unwrap();

        assert_eq!(owner(&layout, &registry).as_deref(), Some("work"));
    }

    #[test]
    fn an_unknown_account_is_refused() {
        let (_root, layout, registry) = fixture();
        assert!(matches!(
            switch(&layout, &registry, "nobody"),
            Err(Error::UnknownAccount(_))
        ));
    }
}
