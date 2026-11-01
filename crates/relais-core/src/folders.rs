//! Accounts tied to folders: Claude Code started in a folder, or anywhere under
//! it, runs on that folder's account whatever the active one is. Kept in
//! `~/.relais/folders.json`, as `{ "<folder>": "<alias>" }`.

use std::collections::BTreeMap;
use std::path::Path;

pub type FolderRules = BTreeMap<String, String>;

/// The account of the closest folder holding `cwd`: a rule on a subfolder wins
/// over one on its parent.
pub fn account_for<'a>(rules: &'a FolderRules, cwd: &Path) -> Option<&'a str> {
    rules
        .iter()
        .filter(|(folder, _)| cwd.starts_with(folder))
        .max_by_key(|(folder, _)| folder.len())
        .map(|(_, alias)| alias.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> FolderRules {
        [
            ("/work".to_string(), "work".to_string()),
            ("/work/client".to_string(), "studio".to_string()),
        ]
        .into()
    }

    #[test]
    fn a_folder_and_everything_under_it_take_its_account() {
        assert_eq!(account_for(&rules(), Path::new("/work")), Some("work"));
        assert_eq!(
            account_for(&rules(), Path::new("/work/app/src")),
            Some("work")
        );
    }

    #[test]
    fn the_closest_folder_wins() {
        assert_eq!(
            account_for(&rules(), Path::new("/work/client/app")),
            Some("studio")
        );
    }

    #[test]
    fn a_folder_with_a_similar_name_is_not_under_the_rule() {
        assert_eq!(account_for(&rules(), Path::new("/workshop")), None);
        assert_eq!(account_for(&rules(), Path::new("/home")), None);
    }
}
