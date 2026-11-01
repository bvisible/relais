//! Which account the desktop app was last switched to.
//!
//! The app holds one signed-in account at a time, in its data folder
//! (`~/Library/Application Support/Claude`). Relais records the account each
//! switch leaves in place (`~/.relais/desktop.json`); the switch itself is in
//! [`crate::claude::desktop_switch`].

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;
use crate::registry::Registry;

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

/// Whether `alias` can use the desktop app straight away: it is the account
/// in place, or its sign-in waits, parked or in its former folder.
pub fn has_folder(layout: &Layout, registry: &Registry, alias: &str) -> bool {
    owner(layout, registry).as_deref() == Some(alias)
        || crate::claude::desktop_identity::is_parked(layout, alias)
        || crate::claude::desktop_identity::has_old_folder(layout, alias)
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
    fn an_account_is_ready_when_in_place_or_its_sign_in_waits() {
        let (_root, layout, registry) = fixture();
        live_folder(&layout, "main's workspace");
        set_owner(&layout, "main").unwrap();
        assert!(has_folder(&layout, &registry, "main"));
        assert!(!has_folder(&layout, &registry, "work"));

        fs::create_dir_all(layout.identity_dir("work")).unwrap();
        fs::write(layout.identity_dir("work").join("Cookies"), "cookie").unwrap();
        assert!(has_folder(&layout, &registry, "work"));
    }
}
