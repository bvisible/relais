//! Keeps the account-independent part of Claude Code's global config
//! (`.claude.json`) identical across every account profile.
//!
//! Each account profile has its own `.claude.json`, because that file also
//! holds the signed-in account (`oauthAccount`) and per-account caches. Only an
//! allowlist of keys is shared: user-scope MCP servers, per-project trust and
//! tool permissions, and a few onboarding preferences.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

/// Top-level keys that hold user preferences rather than account state.
pub const SHARED_TOP_LEVEL_KEYS: &[&str] = &[
    "hasCompletedOnboarding",
    "lastOnboardingVersion",
    "hasCompletedClaudeInChromeOnboarding",
    "claudeInChromeDefaultEnabled",
    "autoUpdates",
    "autoUpdatesProtectedForNative",
    "installMethod",
    "deepLinkTerminal",
    "diffSidebarOpen",
    "showSpinnerTree",
    "githubRepoPaths",
    "lastReleaseNotesSeen",
];

/// Keys inside `projects.<path>` that describe what the user allowed in that
/// project. Usage statistics (`last*`) stay per account.
pub const SHARED_PROJECT_KEYS: &[&str] = &[
    "allowedTools",
    "hasTrustDialogAccepted",
    "mcpServers",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "enabledMcpServers",
    "disabledMcpServers",
    "hasClaudeMdExternalIncludesApproved",
    "hasClaudeMdExternalIncludesWarningShown",
    "mcpContextUris",
];

/// One unit of shared configuration. Units are merged independently, so two
/// accounts can each add a different MCP server without a conflict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SharedKey {
    TopLevel(String),
    McpServer(String),
    Project { path: String, key: String },
}

/// The shared part of one `.claude.json`.
pub type SharedView = BTreeMap<SharedKey, Value>;

/// One `.claude.json` taking part in a merge.
#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub view: SharedView,
    /// Last modification time, in seconds since the Unix epoch.
    pub modified: i64,
    /// The default profile (`~/.claude.json`) wins conflicts on the first merge.
    pub primary: bool,
}

/// Extracts the shared part of a parsed `.claude.json`.
pub fn extract(config: &Value) -> SharedView {
    let mut view = SharedView::new();
    let Some(object) = config.as_object() else {
        return view;
    };
    for key in SHARED_TOP_LEVEL_KEYS {
        if let Some(value) = object.get(*key) {
            view.insert(SharedKey::TopLevel((*key).to_string()), value.clone());
        }
    }
    if let Some(servers) = object.get("mcpServers").and_then(Value::as_object) {
        for (name, value) in servers {
            view.insert(SharedKey::McpServer(name.clone()), value.clone());
        }
    }
    if let Some(projects) = object.get("projects").and_then(Value::as_object) {
        for (path, project) in projects {
            let Some(project) = project.as_object() else {
                continue;
            };
            for key in SHARED_PROJECT_KEYS {
                if let Some(value) = project.get(*key) {
                    let shared_key = SharedKey::Project {
                        path: path.clone(),
                        key: (*key).to_string(),
                    };
                    view.insert(shared_key, value.clone());
                }
            }
        }
    }
    view
}

/// Merges the shared views of every participant.
///
/// With a `base` (the result of the previous merge), a unit takes the value of
/// the most recently modified participant that changed it, and a removal counts
/// as a change. Without a base, values present anywhere are kept, and the
/// primary participant wins conflicts.
pub fn merge(base: Option<&SharedView>, participants: &[Participant]) -> SharedView {
    let mut keys: BTreeSet<&SharedKey> = participants.iter().flat_map(|p| p.view.keys()).collect();
    if let Some(base) = base {
        keys.extend(base.keys());
    }
    let mut merged = SharedView::new();
    for key in keys {
        let value = match base {
            None => first_merge_value(key, participants),
            Some(base) => incremental_value(key, base.get(key), participants),
        };
        if let Some(value) = value {
            merged.insert(key.clone(), value);
        }
    }
    merged
}

fn first_merge_value(key: &SharedKey, participants: &[Participant]) -> Option<Value> {
    let primary_value = participants
        .iter()
        .find(|p| p.primary)
        .and_then(|p| p.view.get(key));
    if let Some(value) = primary_value {
        return Some(value.clone());
    }
    participants
        .iter()
        .filter(|p| p.view.contains_key(key))
        .max_by_key(|p| p.modified)
        .and_then(|p| p.view.get(key).cloned())
}

fn incremental_value(
    key: &SharedKey,
    base_value: Option<&Value>,
    participants: &[Participant],
) -> Option<Value> {
    let latest_change = participants
        .iter()
        .filter(|p| p.view.get(key) != base_value)
        .max_by(|a, b| a.modified.cmp(&b.modified).then(a.primary.cmp(&b.primary)));
    match latest_change {
        Some(participant) => participant.view.get(key).cloned(),
        None => base_value.cloned(),
    }
}

/// Writes `view` into the shared part of `config`, leaving every other key untouched.
pub fn apply(config: &mut Value, view: &SharedView) {
    if !config.is_object() {
        *config = Value::Object(Map::new());
    }
    let Some(object) = config.as_object_mut() else {
        return;
    };

    for key in SHARED_TOP_LEVEL_KEYS {
        match view.get(&SharedKey::TopLevel((*key).to_string())) {
            Some(value) => {
                object.insert((*key).to_string(), value.clone());
            }
            None => {
                object.remove(*key);
            }
        }
    }

    let servers: Map<String, Value> = view
        .iter()
        .filter_map(|(key, value)| match key {
            SharedKey::McpServer(name) => Some((name.clone(), value.clone())),
            _ => None,
        })
        .collect();
    if !servers.is_empty() || object.contains_key("mcpServers") {
        object.insert("mcpServers".to_string(), Value::Object(servers));
    }

    let has_project_keys = view
        .keys()
        .any(|key| matches!(key, SharedKey::Project { .. }));
    if !has_project_keys && !object.contains_key("projects") {
        return;
    }
    let projects = object
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    if !projects.is_object() {
        *projects = Value::Object(Map::new());
    }
    let Some(projects) = projects.as_object_mut() else {
        return;
    };
    for (path, project) in projects.iter_mut() {
        if let Some(project) = project.as_object_mut() {
            for key in SHARED_PROJECT_KEYS {
                let shared_key = SharedKey::Project {
                    path: path.clone(),
                    key: (*key).to_string(),
                };
                if !view.contains_key(&shared_key) {
                    project.remove(*key);
                }
            }
        }
    }
    for (key, value) in view {
        if let SharedKey::Project { path, key } = key {
            let project = projects
                .entry(path.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            if !project.is_object() {
                *project = Value::Object(Map::new());
            }
            if let Some(project) = project.as_object_mut() {
                project.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Serializes a view for storage as the next merge base.
pub fn view_to_json(view: &SharedView) -> Value {
    let mut top_level = Map::new();
    let mut servers = Map::new();
    let mut projects: Map<String, Value> = Map::new();
    for (key, value) in view {
        match key {
            SharedKey::TopLevel(name) => {
                top_level.insert(name.clone(), value.clone());
            }
            SharedKey::McpServer(name) => {
                servers.insert(name.clone(), value.clone());
            }
            SharedKey::Project { path, key } => {
                let project = projects
                    .entry(path.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(project) = project.as_object_mut() {
                    project.insert(key.clone(), value.clone());
                }
            }
        }
    }
    serde_json::json!({"topLevel": top_level, "mcpServers": servers, "projects": projects})
}

/// Reads a view stored by [`view_to_json`].
pub fn view_from_json(value: &Value) -> SharedView {
    let mut view = SharedView::new();
    if let Some(top_level) = value.get("topLevel").and_then(Value::as_object) {
        for (name, value) in top_level {
            view.insert(SharedKey::TopLevel(name.clone()), value.clone());
        }
    }
    if let Some(servers) = value.get("mcpServers").and_then(Value::as_object) {
        for (name, value) in servers {
            view.insert(SharedKey::McpServer(name.clone()), value.clone());
        }
    }
    if let Some(projects) = value.get("projects").and_then(Value::as_object) {
        for (path, project) in projects {
            if let Some(project) = project.as_object() {
                for (key, value) in project {
                    view.insert(
                        SharedKey::Project {
                            path: path.clone(),
                            key: key.clone(),
                        },
                        value.clone(),
                    );
                }
            }
        }
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn participant(id: &str, config: Value, modified: i64, primary: bool) -> Participant {
        Participant {
            id: id.into(),
            view: extract(&config),
            modified,
            primary,
        }
    }

    fn server(name: &str) -> SharedKey {
        SharedKey::McpServer(name.into())
    }

    #[test]
    fn extract_keeps_shared_keys_and_drops_account_state() {
        let config = json!({
            "oauthAccount": {"emailAddress": "a@example.com"},
            "userID": "u1",
            "hasCompletedOnboarding": true,
            "mcpServers": {"github": {"command": "gh-mcp"}},
            "projects": {
                "/work/app": {"hasTrustDialogAccepted": true, "lastCost": 1.5, "allowedTools": ["Bash"]}
            }
        });

        let view = extract(&config);

        assert_eq!(
            view.get(&SharedKey::TopLevel("hasCompletedOnboarding".into())),
            Some(&json!(true))
        );
        assert_eq!(
            view.get(&server("github")),
            Some(&json!({"command": "gh-mcp"}))
        );
        let trust = SharedKey::Project {
            path: "/work/app".into(),
            key: "hasTrustDialogAccepted".into(),
        };
        assert_eq!(view.get(&trust), Some(&json!(true)));
        let tools = SharedKey::Project {
            path: "/work/app".into(),
            key: "allowedTools".into(),
        };
        assert_eq!(view.get(&tools), Some(&json!(["Bash"])));
        assert!(!view
            .keys()
            .any(|k| matches!(k, SharedKey::TopLevel(n) if n == "oauthAccount" || n == "userID")));
        assert!(!view
            .keys()
            .any(|k| matches!(k, SharedKey::Project { key, .. } if key == "lastCost")));
        assert_eq!(view.len(), 4);
    }

    #[test]
    fn first_merge_unions_values_and_primary_wins_conflicts() {
        let primary = participant(
            "default",
            json!({"mcpServers": {"a": {"v": 1}, "b": {"v": 1}}}),
            10,
            true,
        );
        let other = participant(
            "work",
            json!({"mcpServers": {"b": {"v": 2}, "c": {"v": 1}}}),
            20,
            false,
        );

        let merged = merge(None, &[primary, other]);

        assert_eq!(merged.get(&server("a")), Some(&json!({"v": 1})));
        assert_eq!(
            merged.get(&server("b")),
            Some(&json!({"v": 1})),
            "primary wins on the first merge"
        );
        assert_eq!(merged.get(&server("c")), Some(&json!({"v": 1})));
    }

    #[test]
    fn server_added_in_one_profile_reaches_the_others() {
        let base = extract(&json!({"mcpServers": {"a": {"v": 1}}}));
        let default = participant("default", json!({"mcpServers": {"a": {"v": 1}}}), 10, true);
        let work = participant(
            "work",
            json!({"mcpServers": {"a": {"v": 1}, "new": {"v": 1}}}),
            20,
            false,
        );

        let merged = merge(Some(&base), &[default, work]);

        assert_eq!(merged.get(&server("new")), Some(&json!({"v": 1})));
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn removal_in_one_profile_propagates() {
        let base = extract(&json!({"mcpServers": {"a": {"v": 1}, "gone": {"v": 1}}}));
        let default = participant(
            "default",
            json!({"mcpServers": {"a": {"v": 1}, "gone": {"v": 1}}}),
            10,
            true,
        );
        let work = participant("work", json!({"mcpServers": {"a": {"v": 1}}}), 20, false);

        let merged = merge(Some(&base), &[default, work]);

        assert!(!merged.contains_key(&server("gone")));
        assert_eq!(merged.get(&server("a")), Some(&json!({"v": 1})));
    }

    #[test]
    fn most_recent_change_wins_a_conflict() {
        let base = extract(&json!({"mcpServers": {"a": {"v": 1}}}));
        let default = participant("default", json!({"mcpServers": {"a": {"v": 2}}}), 30, true);
        let work = participant("work", json!({"mcpServers": {"a": {"v": 3}}}), 20, false);

        let merged = merge(Some(&base), &[default, work]);

        assert_eq!(merged.get(&server("a")), Some(&json!({"v": 2})));
    }

    #[test]
    fn unchanged_units_keep_the_base_value() {
        let base = extract(&json!({"hasCompletedOnboarding": true}));
        let default = participant("default", json!({"hasCompletedOnboarding": true}), 10, true);
        let work = participant("work", json!({"hasCompletedOnboarding": true}), 20, false);

        let merged = merge(Some(&base), &[default, work]);

        assert_eq!(
            merged.get(&SharedKey::TopLevel("hasCompletedOnboarding".into())),
            Some(&json!(true))
        );
    }

    #[test]
    fn apply_replaces_shared_parts_and_keeps_account_state() {
        let mut config = json!({
            "oauthAccount": {"emailAddress": "b@example.com"},
            "numStartups": 7,
            "mcpServers": {"old": {"v": 1}},
            "projects": {"/work/app": {"lastCost": 2.0, "hasTrustDialogAccepted": false}}
        });
        let view = extract(&json!({
            "hasCompletedOnboarding": true,
            "mcpServers": {"a": {"v": 1}},
            "projects": {
                "/work/app": {"hasTrustDialogAccepted": true},
                "/work/new": {"allowedTools": ["Read"]}
            }
        }));

        apply(&mut config, &view);

        assert_eq!(
            config["oauthAccount"],
            json!({"emailAddress": "b@example.com"})
        );
        assert_eq!(config["numStartups"], json!(7));
        assert_eq!(config["hasCompletedOnboarding"], json!(true));
        assert_eq!(config["mcpServers"], json!({"a": {"v": 1}}));
        assert_eq!(config["projects"]["/work/app"]["lastCost"], json!(2.0));
        assert_eq!(
            config["projects"]["/work/app"]["hasTrustDialogAccepted"],
            json!(true)
        );
        assert_eq!(
            config["projects"]["/work/new"]["allowedTools"],
            json!(["Read"])
        );
        assert_eq!(extract(&config), view);
    }

    #[test]
    fn apply_removes_shared_keys_missing_from_the_view() {
        let mut config = json!({
            "hasCompletedOnboarding": true,
            "projects": {"/work/app": {"allowedTools": ["Bash"], "lastCost": 1.0}}
        });

        apply(&mut config, &SharedView::new());

        assert!(config.get("hasCompletedOnboarding").is_none());
        assert!(config["projects"]["/work/app"]
            .get("allowedTools")
            .is_none());
        assert_eq!(config["projects"]["/work/app"]["lastCost"], json!(1.0));
    }

    #[test]
    fn view_survives_a_json_round_trip() {
        let view = extract(&json!({
            "hasCompletedOnboarding": true,
            "mcpServers": {"a": {"v": 1}},
            "projects": {"/work/app": {"allowedTools": ["Bash"]}}
        }));

        assert_eq!(view_from_json(&view_to_json(&view)), view);
    }

    #[test]
    fn config_that_is_not_an_object_yields_an_empty_view() {
        assert!(extract(&json!(null)).is_empty());
    }
}
