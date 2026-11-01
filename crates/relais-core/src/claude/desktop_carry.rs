//! What the desktop app keeps per account inside the workspace, carried from
//! the account that leaves to the account that arrives.
//!
//! The app is built for accounts taking turns in one data folder: a few of
//! its settings are filed under the account's id, so an account arriving
//! for the first time finds a default sidebar. Carrying those entries makes
//! the arriving account see the workspace exactly as the user left it. The
//! entries are listed here as rules, so that a new keyed setting in a later
//! version of the app is one line to add.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::fsutil;

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

/// The conversation list of an account in the workspace: `root` is
/// `claude-code-sessions` (Code) or `local-agent-mode-sessions` (Cowork).
pub fn index_dir(root: &Path, ids: &AccountIds) -> PathBuf {
    root.join(&ids.account).join(&ids.org)
}

/// What the arriving account's list holds that the leaving one's does not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TargetConversations {
    /// Records of conversations of its own (`local_<id>`, sorted).
    pub own: Vec<String>,
    /// Records of a conversation the leaving list already has under another
    /// record: kept would show it twice.
    pub duplicates: Vec<String>,
}

/// Sorts the records only `target` has into its own and duplicates.
pub fn target_conversations(source: &Path, target: &Path) -> TargetConversations {
    let leaving = records(source);
    let names: HashSet<&str> = leaving.iter().map(|(name, _)| name.as_str()).collect();
    let transcripts: HashSet<&str> = leaving
        .iter()
        .filter_map(|(_, cli)| cli.as_deref())
        .collect();
    let mut sorted = TargetConversations::default();
    for (name, cli) in records(target) {
        if names.contains(name.as_str()) {
            continue;
        }
        if cli.as_deref().is_some_and(|cli| transcripts.contains(cli)) {
            sorted.duplicates.push(name);
        } else {
            sorted.own.push(name);
        }
    }
    sorted.own.sort();
    sorted.duplicates.sort();
    sorted
}

/// How many conversations a list holds.
pub fn conversation_count(dir: &Path) -> usize {
    entry_names(dir)
        .iter()
        .filter(|name| is_record(name))
        .count()
}

/// The conversation records of a list: `local_<id>` and its transcript id.
fn records(dir: &Path) -> Vec<(String, Option<String>)> {
    entry_names(dir)
        .into_iter()
        .filter(|name| is_record(name))
        .map(|name| {
            let cli = fsutil::read_json(&dir.join(&name))
                .ok()
                .flatten()
                .and_then(|value| value.get("cliSessionId")?.as_str().map(String::from));
            (name.trim_end_matches(".json").to_string(), cli)
        })
        .collect()
}

fn is_record(name: &str) -> bool {
    name.starts_with("local_") && name.ends_with(".json")
}

/// The names in `dir`, sorted; none when it cannot be read.
fn entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// What to do with the arriving account's own conversations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnConversations {
    Add,
    PutAside,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Records that joined the arriving list.
    pub moved: usize,
    /// Records both lists had, where the arriving one's copy was newer.
    pub kept_target: usize,
    pub own_added: usize,
    pub own_set_aside: usize,
    pub duplicates_set_aside: usize,
}

/// Moves the leaving account's list (`source`) into the arriving one's
/// (`target`). A record both have keeps the copy with the later activity;
/// the other copy goes to `replaced`, as does anything else `target` held
/// in the way. Scheduled tasks and the archive index are merged. Duplicates,
/// and own records when asked, go to `set_aside`. Nothing is deleted.
pub fn carry_index(
    source: &Path,
    target: &Path,
    own: OwnConversations,
    set_aside: &Path,
    replaced: &Path,
) -> Result<IndexReport> {
    let mut report = IndexReport::default();
    if !source.is_dir() {
        return Ok(report);
    }
    let sorted = target_conversations(source, target);
    for name in &sorted.duplicates {
        set_aside_record(target, name, set_aside)?;
        report.duplicates_set_aside += 1;
    }
    match own {
        OwnConversations::Add => report.own_added = sorted.own.len(),
        OwnConversations::PutAside => {
            for name in &sorted.own {
                set_aside_record(target, name, set_aside)?;
                report.own_set_aside += 1;
            }
        }
    }
    if !target.exists() {
        report.moved = records(source).len();
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        fs::rename(source, target).map_err(|e| Error::io(source, e))?;
        return Ok(report);
    }
    merge_dir(source, target, replaced, true, &mut report)?;
    Ok(report)
}

/// Moves the records `set_aside` holds back into `list`, but for those `list`
/// already has: they would show twice. Returns how many came back.
pub fn bring_back(set_aside: &Path, list: &Path) -> Result<usize> {
    let sorted = target_conversations(list, set_aside);
    for name in &sorted.own {
        set_aside_record(set_aside, name, list)?;
    }
    Ok(sorted.own.len())
}

/// Moves a record and everything named after it (its folder, its waiting
/// input) from a list to `set_aside`.
fn set_aside_record(list: &Path, record: &str, set_aside: &Path) -> Result<()> {
    for dir in [list.to_path_buf(), list.join("waiting-input")] {
        for name in entry_names(&dir) {
            if name == record || name.starts_with(&format!("{record}.")) {
                let into = if dir == list {
                    set_aside.to_path_buf()
                } else {
                    set_aside.join("waiting-input")
                };
                move_into(&dir.join(&name), &into)?;
            }
        }
    }
    Ok(())
}

fn merge_dir(
    source: &Path,
    target: &Path,
    replaced: &Path,
    top: bool,
    report: &mut IndexReport,
) -> Result<()> {
    fs::create_dir_all(target).map_err(|e| Error::io(target, e))?;
    for name in entry_names(source) {
        let from = source.join(&name);
        let to = target.join(&name);
        let record = top && is_record(&name);
        if fs::symlink_metadata(&to).is_err() {
            fs::rename(&from, &to).map_err(|e| Error::io(&from, e))?;
            report.moved += usize::from(record);
            continue;
        }
        if from.is_dir() && to.is_dir() {
            merge_dir(&from, &to, &replaced.join(&name), false, report)?;
            continue;
        }
        let merge: Option<fn(&Value, &Value) -> Option<Value>> = match name.as_str() {
            "scheduled-tasks.json" if top => Some(merge_tasks),
            "archived-sessions.idx" if top => Some(merge_archive),
            _ => None,
        };
        if let Some(merged) = merge.and_then(|merge| merge(&read(&from)?, &read(&to)?)) {
            move_into(&to, replaced)?;
            fsutil::write_json_atomic(&to, &merged)?;
            restrict(&to)?;
            move_into(&from, replaced)?;
            continue;
        }
        if record && activity(&to) > activity(&from) {
            move_into(&from, replaced)?;
            report.kept_target += 1;
            continue;
        }
        move_into(&to, replaced)?;
        fs::rename(&from, &to).map_err(|e| Error::io(&from, e))?;
        report.moved += usize::from(record);
    }
    fs::remove_dir(source).map_err(|e| Error::io(source, e))
}

fn read(path: &Path) -> Option<Value> {
    fsutil::read_json(path).ok().flatten()
}

/// When a record was last active; unknown counts as never, so the leaving
/// account's copy wins a tie or a doubt.
fn activity(path: &Path) -> Option<f64> {
    read(path)?.get("lastActivityAt")?.as_f64()
}

/// The leaving account's scheduled tasks, then the arriving one's others.
fn merge_tasks(source: &Value, target: &Value) -> Option<Value> {
    merge_list(source, target, "scheduledTasks", |task| {
        task.get("id").cloned()
    })
}

/// The leaving account's archived records, then the arriving one's others.
fn merge_archive(source: &Value, target: &Value) -> Option<Value> {
    merge_list(source, target, "archived", |id| Some(id.clone()))
}

fn merge_list(
    source: &Value,
    target: &Value,
    field: &str,
    key: impl Fn(&Value) -> Option<Value>,
) -> Option<Value> {
    let ours = source.get(field)?.as_array()?;
    let theirs = target.get(field)?.as_array()?;
    let known: Vec<Value> = ours.iter().filter_map(&key).collect();
    let mut list = ours.clone();
    list.extend(
        theirs
            .iter()
            .filter(|item| key(item).is_none_or(|k| !known.contains(&k)))
            .cloned(),
    );
    let mut merged = source.clone();
    merged[field] = json!(list);
    Some(merged)
}

/// Moves `path` into `dir` under its own name, or with a number added when
/// that name is taken: nothing is ever overwritten.
fn move_into(path: &Path, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut destination = dir.join(name.as_ref());
    let mut n = 1;
    while fs::symlink_metadata(&destination).is_ok() {
        destination = dir.join(format!("{name}.{n}"));
        n += 1;
    }
    fs::rename(path, &destination).map_err(|e| Error::io(path, e))
}

/// The app's own files are private to the user.
fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| Error::io(path, e))
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

    mod index {
        use super::super::*;
        use serde_json::json;
        use std::fs;
        use std::path::Path;

        struct Dirs {
            _root: tempfile::TempDir,
            source: PathBuf,
            target: PathBuf,
            set_aside: PathBuf,
            replaced: PathBuf,
        }

        fn dirs() -> Dirs {
            let root = tempfile::tempdir().unwrap();
            let base = root.path().to_path_buf();
            Dirs {
                source: base.join("sessions/acc-a/org-a"),
                target: base.join("sessions/acc-b/org-b"),
                set_aside: base.join("set-aside"),
                replaced: base.join("replaced"),
                _root: root,
            }
        }

        fn record(dir: &Path, id: &str, cli: &str, activity: i64) {
            fs::create_dir_all(dir).unwrap();
            let value = json!({"sessionId": id, "cliSessionId": cli, "lastActivityAt": activity});
            fs::write(dir.join(format!("{id}.json")), value.to_string()).unwrap();
        }

        fn json_at(path: &Path) -> Value {
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
        }

        fn names(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok()?.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }

        #[test]
        fn an_empty_target_takes_the_whole_list() {
            let d = dirs();
            record(&d.source, "local_s1", "cli-1", 100);
            record(&d.source, "local_s2", "cli-2", 200);
            fs::write(
                d.source.join("scheduled-tasks.json"),
                r#"{"scheduledTasks":[]}"#,
            )
            .unwrap();

            let report = carry_index(
                &d.source,
                &d.target,
                OwnConversations::Add,
                &d.set_aside,
                &d.replaced,
            )
            .unwrap();

            assert_eq!(report.moved, 2);
            assert_eq!(
                names(&d.target),
                ["local_s1.json", "local_s2.json", "scheduled-tasks.json"]
            );
            assert!(!d.source.exists());
        }

        #[test]
        fn the_later_activity_wins_and_the_other_copy_is_kept() {
            let d = dirs();
            record(&d.source, "local_s1", "cli-1", 100);
            record(&d.target, "local_s1", "cli-1", 200);
            record(&d.source, "local_s2", "cli-2", 300);
            record(&d.target, "local_s2", "cli-2", 250);
            record(&d.source, "local_s3", "cli-3", 50);
            record(&d.target, "local_s3", "cli-3", 50);

            let report = carry_index(
                &d.source,
                &d.target,
                OwnConversations::Add,
                &d.set_aside,
                &d.replaced,
            )
            .unwrap();

            assert_eq!(
                json_at(&d.target.join("local_s1.json"))["lastActivityAt"],
                200
            );
            assert_eq!(
                json_at(&d.replaced.join("local_s1.json"))["lastActivityAt"],
                100
            );
            assert_eq!(
                json_at(&d.target.join("local_s2.json"))["lastActivityAt"],
                300
            );
            assert_eq!(
                json_at(&d.replaced.join("local_s2.json"))["lastActivityAt"],
                250
            );
            assert!(d.target.join("local_s3.json").is_file());
            assert!(d.replaced.join("local_s3.json").is_file());
            assert_eq!(report.kept_target, 1);
            assert_eq!(report.moved, 2);
            assert!(!d.source.exists());
        }

        #[test]
        fn scheduled_tasks_and_archives_are_merged() {
            let d = dirs();
            fs::create_dir_all(&d.source).unwrap();
            fs::create_dir_all(&d.target).unwrap();
            fs::write(
                d.source.join("scheduled-tasks.json"),
                json!({"scheduledTasks": [{"id": "t1", "v": "source"}, {"id": "t2"}], "stamped": true})
                    .to_string(),
            )
            .unwrap();
            fs::write(
                d.target.join("scheduled-tasks.json"),
                json!({"scheduledTasks": [{"id": "t1", "v": "target"}, {"id": "t3"}], "stamped": false})
                    .to_string(),
            )
            .unwrap();
            fs::write(
                d.source.join("archived-sessions.idx"),
                json!({"v": 1, "archived": ["local_a", "local_b"]}).to_string(),
            )
            .unwrap();
            fs::write(
                d.target.join("archived-sessions.idx"),
                json!({"v": 1, "archived": ["local_b", "local_c"]}).to_string(),
            )
            .unwrap();

            carry_index(
                &d.source,
                &d.target,
                OwnConversations::Add,
                &d.set_aside,
                &d.replaced,
            )
            .unwrap();

            assert_eq!(
                json_at(&d.target.join("scheduled-tasks.json")),
                json!({"scheduledTasks": [{"id": "t1", "v": "source"}, {"id": "t2"}, {"id": "t3"}], "stamped": true})
            );
            assert_eq!(
                json_at(&d.target.join("archived-sessions.idx")),
                json!({"v": 1, "archived": ["local_a", "local_b", "local_c"]})
            );
            assert!(d.replaced.join("scheduled-tasks.json").is_file());
            assert!(!d.source.exists());
        }

        #[test]
        fn own_conversations_are_added_or_put_aside() {
            for (choice, in_target) in [
                (OwnConversations::Add, true),
                (OwnConversations::PutAside, false),
            ] {
                let d = dirs();
                record(&d.source, "local_s1", "cli-1", 100);
                record(&d.target, "local_own", "cli-own", 100);
                fs::create_dir_all(d.target.join("local_own")).unwrap();

                let report =
                    carry_index(&d.source, &d.target, choice, &d.set_aside, &d.replaced).unwrap();

                assert_eq!(
                    d.target.join("local_own.json").is_file(),
                    in_target,
                    "{choice:?}"
                );
                assert_eq!(
                    d.set_aside.join("local_own.json").is_file(),
                    !in_target,
                    "{choice:?}"
                );
                assert_eq!(
                    d.set_aside.join("local_own").is_dir(),
                    !in_target,
                    "{choice:?}"
                );
                assert_eq!(report.own_added, usize::from(in_target));
                assert_eq!(report.own_set_aside, usize::from(!in_target));
                assert!(d.target.join("local_s1.json").is_file());
            }
        }

        #[test]
        fn a_duplicate_is_put_aside_whatever_the_choice() {
            let d = dirs();
            record(&d.source, "local_s1", "cli-1", 100);
            record(&d.target, "local_dup", "cli-1", 300);

            let report = carry_index(
                &d.source,
                &d.target,
                OwnConversations::Add,
                &d.set_aside,
                &d.replaced,
            )
            .unwrap();

            assert!(!d.target.join("local_dup.json").exists());
            assert!(d.set_aside.join("local_dup.json").is_file());
            assert_eq!(report.duplicates_set_aside, 1);
        }

        #[test]
        fn target_conversations_tell_own_from_duplicates() {
            let d = dirs();
            record(&d.source, "local_s1", "cli-1", 100);
            record(&d.target, "local_s1", "cli-1", 100);
            record(&d.target, "local_dup", "cli-1", 100);
            record(&d.target, "local_own2", "cli-x", 100);
            record(&d.target, "local_own1", "cli-y", 100);
            fs::write(d.target.join("deleted_local_zz.json"), "{}").unwrap();

            assert_eq!(
                target_conversations(&d.source, &d.target),
                TargetConversations {
                    own: vec!["local_own1".into(), "local_own2".into()],
                    duplicates: vec!["local_dup".into()],
                }
            );
            assert_eq!(
                target_conversations(&d.source, &d.target.join("missing")),
                TargetConversations::default()
            );
        }

        #[test]
        fn folders_on_both_sides_are_merged_entry_by_entry() {
            let d = dirs();
            record(&d.source.join("waiting-input"), "local_s1", "cli-1", 1);
            record(&d.target.join("waiting-input"), "local_t1", "cli-2", 1);
            record(&d.source, "local_s1", "cli-1", 1);

            carry_index(
                &d.source,
                &d.target,
                OwnConversations::Add,
                &d.set_aside,
                &d.replaced,
            )
            .unwrap();

            assert_eq!(
                names(&d.target.join("waiting-input")),
                ["local_s1.json", "local_t1.json"]
            );
            assert!(!d.source.exists());
        }

        #[test]
        fn a_missing_source_changes_nothing() {
            let d = dirs();
            record(&d.target, "local_t1", "cli-1", 1);

            let report = carry_index(
                &d.source,
                &d.target,
                OwnConversations::PutAside,
                &d.set_aside,
                &d.replaced,
            )
            .unwrap();

            assert_eq!(report, IndexReport::default());
            assert_eq!(names(&d.target), ["local_t1.json"]);
            assert!(!d.set_aside.exists());
        }
    }
}
