//! The list of accounts Relais manages, and which one is active.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil;

/// One Claude account and the Claude Code profile it lives in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// `None` for the default profile (`~/.claude`); otherwise the account's
    /// own Claude Code config directory, passed as `CLAUDE_CONFIG_DIR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<PathBuf>,
    /// The Chrome profile directory that belongs to this account: its sign-in
    /// page opens there, and Claude in Chrome runs there for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chrome_profile: Option<String>,
    /// The color the user picked for this account (`#RRGGBB`). Without one,
    /// the account gets a free color of [`PALETTE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// The colors offered for accounts, by name; accounts without a chosen color
/// take the first ones still free, in this order.
pub const PALETTE: [(&str, &str); 8] = [
    ("sage", "#4CC38A"),
    ("ocean", "#4C8DFF"),
    ("violet", "#A78BFA"),
    ("coral", "#FF7A6B"),
    ("amber", "#F2B33D"),
    ("rose", "#F472B6"),
    ("lime", "#C3D94E"),
    ("sky", "#38BDF8"),
];

impl Account {
    pub fn is_default_profile(&self) -> bool {
        self.config_dir.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registry {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            version: 1,
            active: None,
            accounts: Vec::new(),
        }
    }
}

impl Registry {
    /// Loads the registry; a missing file is an empty registry.
    pub fn load(path: &Path) -> Result<Self> {
        match fsutil::read_json(path)? {
            None => Ok(Registry::default()),
            Some(value) => serde_json::from_value(value).map_err(|e| Error::Json {
                path: path.into(),
                source: e,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let value = serde_json::to_value(self).map_err(|e| Error::Json {
            path: path.into(),
            source: e,
        })?;
        fsutil::write_json_atomic(path, &value)
    }

    pub fn get(&self, alias: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.alias == alias)
    }

    /// Adds an account. The first account becomes the active one.
    pub fn add(&mut self, account: Account) -> Result<()> {
        if self.get(&account.alias).is_some() {
            return Err(Error::DuplicateAlias(account.alias));
        }
        if let Some(email) = &account.email {
            let taken = self.accounts.iter().any(|a| {
                a.email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
            });
            if taken {
                return Err(Error::DuplicateEmail(email.clone()));
            }
        }
        if self.active.is_none() {
            self.active = Some(account.alias.clone());
        }
        self.accounts.push(account);
        Ok(())
    }

    /// Removes an account. When it was active, the first remaining account takes over.
    pub fn remove(&mut self, alias: &str) -> Result<Account> {
        let index = self
            .accounts
            .iter()
            .position(|a| a.alias == alias)
            .ok_or_else(|| Error::UnknownAccount(alias.to_string()))?;
        let removed = self.accounts.remove(index);
        if self.active.as_deref() == Some(alias) {
            self.active = self.accounts.first().map(|a| a.alias.clone());
        }
        Ok(removed)
    }

    pub fn set_active(&mut self, alias: &str) -> Result<()> {
        if self.get(alias).is_none() {
            return Err(Error::UnknownAccount(alias.to_string()));
        }
        self.active = Some(alias.to_string());
        Ok(())
    }

    pub fn active(&self) -> Option<&Account> {
        self.active.as_deref().and_then(|alias| self.get(alias))
    }

    /// The account `offset` places after the active one, wrapping around
    /// (`1` for the next account, `-1` for the previous one).
    pub fn neighbour(&self, offset: isize) -> Option<&Account> {
        let count = self.accounts.len() as isize;
        if count == 0 {
            return None;
        }
        let current = self
            .active
            .as_deref()
            .and_then(|alias| self.accounts.iter().position(|a| a.alias == alias))
            .unwrap_or(0) as isize;
        let index = (current + offset).rem_euclid(count) as usize;
        self.accounts.get(index)
    }

    /// The account's color: the one chosen, else the first palette color that
    /// neither a chosen color nor an earlier account already holds.
    pub fn color_of(&self, alias: &str) -> Option<String> {
        let mut taken: Vec<String> = self
            .accounts
            .iter()
            .filter_map(|a| a.color.clone())
            .collect();
        for (index, account) in self.accounts.iter().enumerate() {
            let color = match &account.color {
                Some(color) => color.clone(),
                None => {
                    let free = PALETTE
                        .iter()
                        .map(|(_, hex)| hex.to_string())
                        .find(|hex| !taken.contains(hex));
                    let color = free.unwrap_or_else(|| PALETTE[index % PALETTE.len()].1.into());
                    taken.push(color.clone());
                    color
                }
            };
            if account.alias == alias {
                return Some(color);
            }
        }
        None
    }

    /// Gives an account the color `hex` (`#RRGGBB`, any case).
    pub fn set_color(&mut self, alias: &str, hex: &str) -> Result<()> {
        let valid = hex.len() == 7
            && hex.starts_with('#')
            && hex[1..].chars().all(|c| c.is_ascii_hexdigit());
        let account = self
            .accounts
            .iter_mut()
            .find(|a| a.alias == alias)
            .ok_or_else(|| Error::UnknownAccount(alias.to_string()))?;
        if !valid {
            return Err(Error::InvalidColor(hex.to_string()));
        }
        account.color = Some(hex.to_ascii_uppercase());
        Ok(())
    }

    /// The account whose profile is `config_dir`; `None` means the default profile.
    pub fn find_by_config_dir(&self, config_dir: Option<&Path>) -> Option<&Account> {
        match config_dir {
            None => self.accounts.iter().find(|a| a.is_default_profile()),
            Some(dir) => {
                let wanted = normalize(dir);
                self.accounts
                    .iter()
                    .find(|a| a.config_dir.as_deref().map(normalize).as_ref() == Some(&wanted))
            }
        }
    }
}

/// Drops trailing separators, so `/p/c/` and `/p/c` name the same profile.
fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// A short account name derived from an email: its local part, lowercased,
/// with anything other than letters, digits, `-` and `_` turned into `-`.
/// A suffix (`-2`, `-3`…) avoids names already taken.
pub fn alias_from_email(email: &str, taken: &[&str]) -> String {
    let local = email.split('@').next().unwrap_or_default().to_lowercase();
    let mut base = String::new();
    for c in local.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '-'
        };
        if c == '-' && (base.is_empty() || base.ends_with('-')) {
            continue;
        }
        base.push(c);
    }
    let base = base.trim_end_matches('-');
    let base = if base.is_empty() { "account" } else { base };
    let base: String = base.chars().take(28).collect();
    if !taken.contains(&base.as_str()) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(alias: &str, email: &str, config_dir: Option<&str>) -> Account {
        Account {
            alias: alias.into(),
            email: Some(email.into()),
            account_uuid: None,
            org_id: None,
            config_dir: config_dir.map(PathBuf::from),
            chrome_profile: None,
            color: None,
        }
    }

    fn three_accounts() -> Registry {
        let mut registry = Registry::default();
        registry.add(account("a", "a@example.com", None)).unwrap();
        registry
            .add(account("b", "b@example.com", Some("/p/b")))
            .unwrap();
        registry
            .add(account("c", "c@example.com", Some("/p/c")))
            .unwrap();
        registry
    }

    #[test]
    fn missing_file_is_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::load(&dir.path().join("accounts.json")).unwrap();
        assert_eq!(registry, Registry::default());
    }

    #[test]
    fn first_account_becomes_active_and_survives_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let registry = three_accounts();

        registry.save(&path).unwrap();
        let loaded = Registry::load(&path).unwrap();

        assert_eq!(loaded.active().map(|a| a.alias.as_str()), Some("a"));
        assert_eq!(loaded, registry);
    }

    #[test]
    fn duplicate_alias_is_refused() {
        let mut registry = three_accounts();
        let err = registry
            .add(account("b", "other@example.com", Some("/p/x")))
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateAlias(alias) if alias == "b"));
    }

    #[test]
    fn duplicate_email_is_refused_whatever_the_case() {
        let mut registry = three_accounts();
        let err = registry
            .add(account("d", "B@Example.com", Some("/p/d")))
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateEmail(_)));
    }

    #[test]
    fn neighbours_wrap_around() {
        let mut registry = three_accounts();
        registry.set_active("b").unwrap();
        assert_eq!(registry.neighbour(1).unwrap().alias, "c");
        assert_eq!(registry.neighbour(-1).unwrap().alias, "a");

        registry.set_active("c").unwrap();
        assert_eq!(registry.neighbour(1).unwrap().alias, "a");
    }

    #[test]
    fn removing_the_active_account_hands_over_to_the_first_one() {
        let mut registry = three_accounts();
        registry.set_active("b").unwrap();

        let removed = registry.remove("b").unwrap();

        assert_eq!(removed.alias, "b");
        assert_eq!(registry.active().map(|a| a.alias.as_str()), Some("a"));
        assert!(registry.get("b").is_none());
    }

    #[test]
    fn unknown_accounts_are_reported() {
        let mut registry = three_accounts();
        assert!(matches!(
            registry.set_active("zz"),
            Err(Error::UnknownAccount(_))
        ));
        assert!(matches!(
            registry.remove("zz"),
            Err(Error::UnknownAccount(_))
        ));
    }

    #[test]
    fn profile_lookup_maps_no_directory_to_the_default_profile() {
        let registry = three_accounts();
        assert_eq!(registry.find_by_config_dir(None).unwrap().alias, "a");
        assert_eq!(
            registry
                .find_by_config_dir(Some(Path::new("/p/c")))
                .unwrap()
                .alias,
            "c"
        );
        assert_eq!(
            registry
                .find_by_config_dir(Some(Path::new("/p/c/")))
                .unwrap()
                .alias,
            "c"
        );
        assert!(registry
            .find_by_config_dir(Some(Path::new("/p/zz")))
            .is_none());
    }

    #[test]
    fn alias_comes_from_the_local_part_of_the_email() {
        assert_eq!(alias_from_email("hello@company.example", &[]), "hello");
        assert_eq!(
            alias_from_email("Ada.Lovelace@mail.example", &[]),
            "ada-lovelace"
        );
        assert_eq!(alias_from_email("a+b@example.com", &[]), "a-b");
    }

    #[test]
    fn alias_gets_a_suffix_when_taken() {
        assert_eq!(alias_from_email("sam@other.example", &["sam"]), "sam-2");
        assert_eq!(
            alias_from_email("sam@other.example", &["sam", "sam-2"]),
            "sam-3"
        );
    }

    #[test]
    fn alias_never_comes_out_empty() {
        assert_eq!(alias_from_email("@example.com", &[]), "account");
        assert_eq!(alias_from_email("...@example.com", &[]), "account");
    }

    #[test]
    fn accounts_without_a_color_get_distinct_palette_colors_in_order() {
        let registry = three_accounts();
        assert_eq!(registry.color_of("a").as_deref(), Some(PALETTE[0].1));
        assert_eq!(registry.color_of("b").as_deref(), Some(PALETTE[1].1));
        assert_eq!(registry.color_of("c").as_deref(), Some(PALETTE[2].1));
        assert_eq!(registry.color_of("zz"), None);
    }

    #[test]
    fn a_chosen_color_is_kept_and_the_others_avoid_it() {
        let mut registry = three_accounts();
        registry.set_color("b", PALETTE[0].1).unwrap();
        assert_eq!(registry.color_of("b").as_deref(), Some(PALETTE[0].1));
        assert_eq!(registry.color_of("a").as_deref(), Some(PALETTE[1].1));
        assert_eq!(registry.color_of("c").as_deref(), Some(PALETTE[2].1));
    }

    #[test]
    fn colors_are_hex_and_survive_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let mut registry = three_accounts();

        registry.set_color("c", "#38bdf8").unwrap();
        registry.save(&path).unwrap();

        assert_eq!(
            Registry::load(&path).unwrap().color_of("c").as_deref(),
            Some("#38BDF8")
        );
        assert!(matches!(
            registry.set_color("c", "blue"),
            Err(Error::InvalidColor(_))
        ));
        assert!(matches!(
            registry.set_color("c", "#12345"),
            Err(Error::InvalidColor(_))
        ));
        assert!(matches!(
            registry.set_color("zz", "#123456"),
            Err(Error::UnknownAccount(_))
        ));
    }

    #[test]
    fn colors_wrap_around_once_the_palette_is_used_up() {
        let mut registry = Registry::default();
        for n in 0..=PALETTE.len() {
            registry
                .add(account(
                    &format!("n{n}"),
                    &format!("n{n}@example.com"),
                    Some(&format!("/p/{n}")),
                ))
                .unwrap();
        }
        let last = format!("n{}", PALETTE.len());
        assert_eq!(registry.color_of(&last).as_deref(), Some(PALETTE[0].1));
    }
}
