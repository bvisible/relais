//! What the desktop app keeps per account inside the workspace, carried from
//! the account that leaves to the account that arrives.
//!
//! The app is built for accounts taking turns in one data folder: a few of
//! its settings are filed under the account's id, so an account arriving
//! for the first time finds a default sidebar. Carrying those entries makes
//! the arriving account see the workspace exactly as the user left it. The
//! entries are listed here as rules, so that a new keyed setting in a later
//! version of the app is one line to add.

use serde_json::{Map, Value};

/// An account as the desktop app files it: its id and its organization's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountIds {
    pub account: String,
    pub org: String,
}

/// Where a keyed entry sits in `claude_desktop_config.json`.
enum Rule {
    /// Every key of this object ending in `.<account>`.
    SuffixedKeys(&'static [&'static str]),
    /// The `<account>/<org>` entry of this object.
    ScopeEntry(&'static [&'static str]),
    /// The `<account>` entry of every object of this one whose key ends with
    /// the given text.
    AccountEntries(&'static [&'static str], &'static str),
}

const PREFERENCE_RULES: &[Rule] = &[
    // Folder order, folder permission modes and their acknowledgements, and
    // the other per-account switches of the Code tab.
    Rule::SuffixedKeys(&["preferences", "epitaxyPrefs"]),
    // Sidebar sections: pinned, routines, sessions, shown, folded, sorted.
    Rule::ScopeEntry(&["preferences", "epitaxyPrefs", "dframe-code-sections"]),
    // Custom sidebar groups and their members.
    Rule::ScopeEntry(&["preferences", "epitaxyPrefs", "dframe-group-scopes"]),
    // bypassPermissionsOptInByAccount and its siblings.
    Rule::AccountEntries(&["preferences"], "ByAccount"),
];

/// Carries `from`'s keyed settings in the app's `claude_desktop_config.json`
/// to `to`, replacing what `to` had. `from`'s own entries stay, for a later
/// switch back. Returns how many entries were written.
pub fn carry_preferences(config: &mut Value, from: &AccountIds, to: &AccountIds) -> usize {
    let mut written = 0;
    for rule in PREFERENCE_RULES {
        match rule {
            Rule::SuffixedKeys(path) => {
                let Some(object) = object_at(config, path) else {
                    continue;
                };
                let from_suffix = format!(".{}", from.account);
                let carried: Vec<(String, Value)> = object
                    .iter()
                    .filter_map(|(key, value)| {
                        let stem = key.strip_suffix(&from_suffix)?;
                        Some((format!("{stem}.{}", to.account), value.clone()))
                    })
                    .collect();
                written += carried.len();
                object.extend(carried);
            }
            Rule::ScopeEntry(path) => {
                if let Some(object) = object_at(config, path) {
                    let from_scope = format!("{}/{}", from.account, from.org);
                    if let Some(value) = object.get(&from_scope).cloned() {
                        object.insert(format!("{}/{}", to.account, to.org), value);
                        written += 1;
                    }
                }
            }
            Rule::AccountEntries(path, ending) => {
                let Some(object) = object_at(config, path) else {
                    continue;
                };
                for (_, map) in object.iter_mut().filter(|(key, _)| key.ends_with(ending)) {
                    if copy_entry(map, &from.account, &to.account) {
                        written += 1;
                    }
                }
            }
        }
    }
    written
}

/// The object at `path` inside `value`, when every step is an object.
fn object_at<'v>(value: &'v mut Value, path: &[&str]) -> Option<&'v mut Map<String, Value>> {
    path.iter()
        .try_fold(value, |value, key| value.as_object_mut()?.get_mut(*key))?
        .as_object_mut()
}

/// Copies `object[from]` to `object[to]` when `object` is an object with a
/// `from` entry.
fn copy_entry(object: &mut Value, from: &str, to: &str) -> bool {
    let Some(object) = object.as_object_mut() else {
        return false;
    };
    let Some(value) = object.get(from).cloned() else {
        return false;
    };
    object.insert(to.to_string(), value);
    true
}

/// Carries `from`'s session presets (`ccd-session-presets.json`) to `to`.
pub fn carry_presets(presets: &mut Value, from: &AccountIds, to: &AccountIds) -> bool {
    presets
        .get_mut("byAccount")
        .is_some_and(|by_account| copy_entry(by_account, &from.account, &to.account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a() -> AccountIds {
        AccountIds {
            account: "acc-a".into(),
            org: "org-a".into(),
        }
    }

    fn b() -> AccountIds {
        AccountIds {
            account: "acc-b".into(),
            org: "org-b".into(),
        }
    }

    fn config() -> Value {
        json!({
            "coworkUserFilesPath": "/w",
            "preferences": {
                "sidebarMode": "epitaxy",
                "bypassPermissionsOptInByAccount": {"acc-a": true},
                "bypassPermissionsGateByAccount": {"acc-a": {"on": 1}, "acc-c": {"on": 0}},
                "keepAwakeEnabled": true,
                "epitaxyPrefs": {
                    "code-projects-order.acc-a": ["/w/one", "/w/two"],
                    "code-projects-order.acc-c": ["/w/three"],
                    "epitaxy-folder-permission-mode-v2.acc-a": {"/w/one": "bypassPermissions"},
                    "ultrathinkAvailable.model-x.acc-a": true,
                    "dframe-local-slice": {"pinnedOrder": ["code:local_1"]},
                    "starred-local-code-sessions": ["local_1"],
                    "dframe-code-sections": {
                        "acc-a/org-a": {"sections": [{"id": "pinned", "collapsed": false}]},
                        "acc-c/org-c": {"sections": []}
                    },
                    "dframe-group-scopes": {}
                }
            }
        })
    }

    #[test]
    fn suffixed_keys_follow_the_account() {
        let mut value = config();
        carry_preferences(&mut value, &a(), &b());
        let prefs = &value["preferences"]["epitaxyPrefs"];
        assert_eq!(
            prefs["code-projects-order.acc-b"],
            json!(["/w/one", "/w/two"])
        );
        assert_eq!(
            prefs["epitaxy-folder-permission-mode-v2.acc-b"],
            json!({"/w/one": "bypassPermissions"})
        );
        assert_eq!(prefs["ultrathinkAvailable.model-x.acc-b"], json!(true));
        assert_eq!(
            prefs["code-projects-order.acc-a"],
            json!(["/w/one", "/w/two"])
        );
    }

    #[test]
    fn scoped_sections_follow_the_account_and_its_org() {
        let mut value = config();
        carry_preferences(&mut value, &a(), &b());
        let sections = &value["preferences"]["epitaxyPrefs"]["dframe-code-sections"];
        assert_eq!(
            sections["acc-b/org-b"],
            json!({"sections": [{"id": "pinned", "collapsed": false}]})
        );
        assert_eq!(sections["acc-a/org-a"], sections["acc-b/org-b"]);
    }

    #[test]
    fn by_account_maps_follow_the_account() {
        let mut value = config();
        carry_preferences(&mut value, &a(), &b());
        let prefs = &value["preferences"];
        assert_eq!(
            prefs["bypassPermissionsOptInByAccount"]["acc-b"],
            json!(true)
        );
        assert_eq!(
            prefs["bypassPermissionsGateByAccount"]["acc-b"],
            json!({"on": 1})
        );
    }

    #[test]
    fn the_count_is_every_entry_written() {
        let mut value = config();
        // Three suffixed keys, one section scope, two by-account maps.
        assert_eq!(carry_preferences(&mut value, &a(), &b()), 6);
    }

    #[test]
    fn unrelated_keys_and_other_accounts_are_untouched() {
        let mut value = config();
        carry_preferences(&mut value, &a(), &b());
        let mut expected = config();
        let prefs = expected["preferences"]["epitaxyPrefs"]
            .as_object_mut()
            .unwrap();
        prefs.insert(
            "code-projects-order.acc-b".into(),
            json!(["/w/one", "/w/two"]),
        );
        prefs.insert(
            "epitaxy-folder-permission-mode-v2.acc-b".into(),
            json!({"/w/one": "bypassPermissions"}),
        );
        prefs.insert("ultrathinkAvailable.model-x.acc-b".into(), json!(true));
        prefs["dframe-code-sections"]["acc-b/org-b"] =
            json!({"sections": [{"id": "pinned", "collapsed": false}]});
        expected["preferences"]["bypassPermissionsOptInByAccount"]["acc-b"] = json!(true);
        expected["preferences"]["bypassPermissionsGateByAccount"]["acc-b"] = json!({"on": 1});
        assert_eq!(value, expected);
    }

    #[test]
    fn what_the_leaving_account_lacks_leaves_the_arriving_one_as_it_is() {
        let mut value = json!({"preferences": {"epitaxyPrefs": {
            "code-projects-order.acc-b": ["/w/kept"],
            "dframe-code-sections": {"acc-b/org-b": {"sections": [1]}}
        }}});
        let before = value.clone();
        assert_eq!(carry_preferences(&mut value, &a(), &b()), 0);
        assert_eq!(value, before);
    }

    #[test]
    fn odd_shapes_carry_nothing_and_break_nothing() {
        for odd in [
            json!({}),
            json!({"preferences": "x"}),
            json!({"preferences": {"epitaxyPrefs": []}}),
            json!({"preferences": {"epitaxyPrefs": {"dframe-code-sections": [1, 2]}}}),
            json!({"preferences": {"bypassPermissionsOptInByAccount": "acc-a"}}),
            json!([1, 2]),
        ] {
            let mut value = odd.clone();
            assert_eq!(carry_preferences(&mut value, &a(), &b()), 0, "{odd}");
            assert_eq!(value, odd);
        }
    }

    #[test]
    fn a_key_that_only_contains_the_id_is_not_carried() {
        let mut value = json!({"preferences": {"epitaxyPrefs": {
            "acc-a.something": 1,
            "x.acc-a.y": 2
        }}});
        assert_eq!(carry_preferences(&mut value, &a(), &b()), 0);
    }

    #[test]
    fn presets_follow_the_account() {
        let mut presets = json!({"byAccount": {"acc-a": {"recent": ["p1"]}, "acc-c": {}}});
        assert!(carry_presets(&mut presets, &a(), &b()));
        assert_eq!(presets["byAccount"]["acc-b"], json!({"recent": ["p1"]}));
        assert_eq!(presets["byAccount"]["acc-a"], json!({"recent": ["p1"]}));

        let mut none = json!({"byAccount": {"acc-c": {}}});
        assert!(!carry_presets(&mut none, &a(), &b()));
        assert_eq!(none, json!({"byAccount": {"acc-c": {}}}));
        assert!(!carry_presets(&mut json!("odd"), &a(), &b()));
    }
}
