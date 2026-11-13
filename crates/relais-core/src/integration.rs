//! Hooking Relais into the user's tools: Claude Code's status line and the
//! shell's `claude` command. Shared by the CLI and the app.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;

/// Comment written above the line Relais adds to the shell profile.
pub const SHELL_MARKER: &str = "# Relais: `claude` runs with the active account";

/// The shell function printed by `relais shell-init`.
pub const SHELL_FUNCTION: &str = r#"# Relais: `claude` runs with the active account, or unchanged when relais is missing.
claude() {
  if command -v relais >/dev/null 2>&1; then
    relais exec -- "$@"
  else
    command claude "$@"
  fi
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Done,
    AlreadyDone,
}

fn statusline_state(layout: &Layout) -> PathBuf {
    layout.relais_dir.join("statusline.json")
}

fn settings_path(layout: &Layout) -> PathBuf {
    layout.claude_dir.join("settings.json")
}

/// Whether Claude Code's status line runs through Relais.
pub fn statusline_installed(layout: &Layout) -> bool {
    let command = fsutil::read_json(&settings_path(layout))
        .ok()
        .flatten()
        .and_then(|settings| {
            settings
                .pointer("/statusLine/command")
                .and_then(Value::as_str)
                .map(String::from)
        });
    command.is_some_and(|c| c.ends_with(" statusline")) && statusline_state(layout).exists()
}

/// The user's own status line command, kept when Relais took over.
pub fn statusline_original_command(layout: &Layout) -> Option<String> {
    let state = fsutil::read_json(&statusline_state(layout))
        .ok()
        .flatten()?;
    state
        .pointer("/original/command")
        .and_then(Value::as_str)
        .map(String::from)
}

/// Points the status line at `command` (`<relais> statusline`), keeping the
/// user's command so Relais can still render it. Settings are backed up first.
pub fn install_statusline(layout: &Layout, command: &str, now: i64) -> Result<Change> {
    let path = settings_path(layout);
    let mut settings = fsutil::read_json(&path)?.unwrap_or_else(|| json!({}));
    let current = settings.get("statusLine").cloned();
    let state_path = statusline_state(layout);
    let current_command = current
        .as_ref()
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str);
    if current_command == Some(command) && state_path.exists() {
        return Ok(Change::AlreadyDone);
    }
    // A reinstall keeps the command saved the first time, not Relais's own.
    let original = match fsutil::read_json(&state_path)? {
        Some(state) => state.get("original").cloned().unwrap_or(Value::Null),
        None => current.clone().unwrap_or(Value::Null),
    };
    fsutil::write_json_atomic(&state_path, &json!({ "original": original }))?;
    backup(&path, &layout.backups_dir().join("settings"), "json", now)?;
    let mut status_line = current.unwrap_or_else(|| json!({"type": "command"}));
    if let Some(object) = status_line.as_object_mut() {
        object.insert("type".into(), json!("command"));
        object.insert("command".into(), json!(command));
    }
    if !settings.is_object() {
        settings = json!({});
    }
    if let Some(object) = settings.as_object_mut() {
        object.insert("statusLine".into(), status_line);
    }
    fsutil::write_json_atomic(&path, &settings)?;
    Ok(Change::Done)
}

/// Puts the user's own status line back.
pub fn uninstall_statusline(layout: &Layout, now: i64) -> Result<Change> {
    let state_path = statusline_state(layout);
    let Some(state) = fsutil::read_json(&state_path)? else {
        return Ok(Change::AlreadyDone);
    };
    let path = settings_path(layout);
    let mut settings = fsutil::read_json(&path)?.unwrap_or_else(|| json!({}));
    backup(&path, &layout.backups_dir().join("settings"), "json", now)?;
    if let Some(object) = settings.as_object_mut() {
        match state.get("original").filter(|original| !original.is_null()) {
            Some(original) => {
                object.insert("statusLine".into(), original.clone());
            }
            None => {
                object.remove("statusLine");
            }
        }
    }
    fsutil::write_json_atomic(&path, &settings)?;
    fs::remove_file(&state_path).map_err(|e| Error::io(&state_path, e))?;
    Ok(Change::Done)
}

/// Copies `path` into `dir` as `<now>.<extension>`, when it exists.
fn backup(path: &Path, dir: &Path, extension: &str, now: i64) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let mut target = dir.join(format!("{now:012}.{extension}"));
    let mut suffix = 1;
    while target.exists() {
        target = dir.join(format!("{now:012}-{suffix}.{extension}"));
        suffix += 1;
    }
    fs::copy(path, &target).map_err(|e| Error::io(&target, e))?;
    Ok(())
}

fn shell_quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

fn shell_profile(layout: &Layout) -> PathBuf {
    layout.home.join(".zshrc")
}

/// Whether the shell profile already sets up the `claude` function.
pub fn shell_installed(layout: &Layout) -> bool {
    // Also counts a line the user added by hand from `relais shell-init`.
    fs::read_to_string(shell_profile(layout)).is_ok_and(|profile| {
        profile.contains(SHELL_MARKER) || profile.contains("relais shell-init")
    })
}

/// Adds `eval "$(<relais> shell-init zsh)"` to `~/.zshrc`, after a backup.
pub fn install_shell(layout: &Layout, relais: &Path, now: i64) -> Result<Change> {
    let path = shell_profile(layout);
    let current = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&path, e)),
    };
    if current.contains(SHELL_MARKER) {
        return Ok(Change::AlreadyDone);
    }
    backup(&path, &layout.backups_dir().join("shell"), "zshrc", now)?;
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    let relais = shell_quote(&relais.display().to_string());
    next.push_str(&format!(
        "{SHELL_MARKER}\neval \"$({relais} shell-init zsh)\"\n"
    ));
    fsutil::write_atomic(&path, next.as_bytes())?;
    Ok(Change::Done)
}

/// Removes the lines [`install_shell`] added.
pub fn uninstall_shell(layout: &Layout, now: i64) -> Result<Change> {
    let path = shell_profile(layout);
    let current = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Change::AlreadyDone),
        Err(e) => return Err(Error::io(&path, e)),
    };
    if !current.contains(SHELL_MARKER) {
        return Ok(Change::AlreadyDone);
    }
    backup(&path, &layout.backups_dir().join("shell"), "zshrc", now)?;
    let mut kept = Vec::new();
    let mut lines = current.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim_end() == SHELL_MARKER {
            if lines.peek().is_some_and(|next| next.contains("shell-init")) {
                lines.next();
            }
            continue;
        }
        kept.push(line);
    }
    fsutil::write_atomic(&path, kept.concat().as_bytes())?;
    Ok(Change::Done)
}

/// What [`install_artifacts`] adds to `PostToolUse`: Claude Code hands every
/// use of its `Artifact` tool to this command, in the background.
const ARTIFACT_RECORD: &str = " artifact-record";

fn artifact_skill_dir(layout: &Layout) -> PathBuf {
    layout.claude_dir.join("skills").join("relais-artifacts")
}

fn is_artifact_hook(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.ends_with(ARTIFACT_RECORD))
}

/// Whether Claude Code hands its artifacts to Relais.
pub fn artifacts_installed(layout: &Layout) -> bool {
    let hooked = fsutil::read_json(&settings_path(layout))
        .ok()
        .flatten()
        .and_then(|settings| settings.pointer("/hooks/PostToolUse").cloned())
        .and_then(|entries| entries.as_array().cloned())
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| hooks.iter().any(is_artifact_hook))
            })
        });
    hooked && artifact_skill_dir(layout).join("SKILL.md").is_file()
}

/// Adds the hook that records artifacts, in the background, and the skill
/// that uses them. Settings are backed up first. A hook already there is
/// pointed at `relais` when the command moved.
pub fn install_artifacts(layout: &Layout, relais: &Path, now: i64) -> Result<Change> {
    let quoted = shell_quote(&relais.display().to_string());
    let command = format!("{quoted}{ARTIFACT_RECORD}");
    let skill_path = artifact_skill_dir(layout).join("SKILL.md");
    let skill = crate::artifacts::skill(&quoted);
    let path = settings_path(layout);
    let mut settings = fsutil::read_json(&path)?.unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        settings = json!({});
    }
    let hook = json!({"type": "command", "command": command, "async": true});
    let mut changed = false;
    let mut found = false;
    {
        let hooks = settings
            .as_object_mut()
            .expect("settings is an object")
            .entry("hooks")
            .or_insert_with(|| json!({}));
        if !hooks.is_object() {
            *hooks = json!({});
        }
        let post = hooks
            .as_object_mut()
            .expect("hooks is an object")
            .entry("PostToolUse")
            .or_insert_with(|| json!([]));
        if !post.is_array() {
            *post = json!([]);
        }
        let entries = post.as_array_mut().expect("PostToolUse is a list");
        for entry in entries.iter_mut() {
            if let Some(list) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                for existing in list.iter_mut().filter(|h| is_artifact_hook(h)) {
                    found = true;
                    if *existing != hook {
                        *existing = hook.clone();
                        changed = true;
                    }
                }
            }
        }
        if !found {
            entries.push(json!({"matcher": "Artifact", "hooks": [hook]}));
            changed = true;
        }
    }
    let skill_current = fs::read_to_string(&skill_path).is_ok_and(|text| text == skill);
    if !changed && skill_current {
        return Ok(Change::AlreadyDone);
    }
    if changed {
        backup(&path, &layout.backups_dir().join("settings"), "json", now)?;
        fsutil::write_json_atomic(&path, &settings)?;
    }
    if !skill_current {
        fsutil::write_atomic(&skill_path, skill.as_bytes())?;
    }
    Ok(Change::Done)
}

/// Removes what [`install_artifacts`] added, and nothing else: a hook list
/// left empty goes, the user's hooks stay.
pub fn uninstall_artifacts(layout: &Layout, now: i64) -> Result<Change> {
    let path = settings_path(layout);
    let skill_dir = artifact_skill_dir(layout);
    let mut changed = false;
    if let Some(mut settings) = fsutil::read_json(&path)? {
        let mut removed = false;
        if let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
            if let Some(entries) = hooks.get_mut("PostToolUse").and_then(Value::as_array_mut) {
                for entry in entries.iter_mut() {
                    if let Some(list) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                        let before = list.len();
                        list.retain(|h| !is_artifact_hook(h));
                        removed |= list.len() != before;
                    }
                }
                if removed {
                    entries.retain(|entry| {
                        entry
                            .get("hooks")
                            .and_then(Value::as_array)
                            .is_none_or(|list| !list.is_empty())
                    });
                }
                if removed && entries.is_empty() {
                    hooks.remove("PostToolUse");
                }
            }
            if removed && hooks.is_empty() {
                settings.as_object_mut().map(|s| s.remove("hooks"));
            }
        }
        if removed {
            backup(&path, &layout.backups_dir().join("settings"), "json", now)?;
            fsutil::write_json_atomic(&path, &settings)?;
            changed = true;
        }
    }
    let skill = skill_dir.join("SKILL.md");
    if fs::read_to_string(&skill)
        .is_ok_and(|text| text.starts_with("---\nname: relais-artifacts\n"))
    {
        fs::remove_file(&skill).map_err(|e| Error::io(&skill, e))?;
        let _ = fs::remove_dir(&skill_dir);
        changed = true;
    }
    Ok(if changed {
        Change::Done
    } else {
        Change::AlreadyDone
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::for_home(root.path());
        fs::create_dir_all(&layout.claude_dir).unwrap();
        (root, layout)
    }

    fn settings(layout: &Layout) -> Value {
        serde_json::from_slice(&fs::read(settings_path(layout)).unwrap()).unwrap()
    }

    #[test]
    fn statusline_install_keeps_the_original_and_uninstall_restores_it() {
        let (_root, layout) = layout();
        fs::write(
            settings_path(&layout),
            r#"{"model":"x","statusLine":{"type":"command","command":"bun line.ts","padding":0}}"#,
        )
        .unwrap();

        assert_eq!(
            install_statusline(&layout, "/bin/relais statusline", 1).unwrap(),
            Change::Done
        );
        assert!(statusline_installed(&layout));
        assert_eq!(
            settings(&layout)["statusLine"]["command"],
            json!("/bin/relais statusline")
        );
        assert_eq!(settings(&layout)["statusLine"]["padding"], json!(0));
        assert_eq!(settings(&layout)["model"], json!("x"));
        assert_eq!(
            statusline_original_command(&layout).as_deref(),
            Some("bun line.ts")
        );
        assert_eq!(
            install_statusline(&layout, "/bin/relais statusline", 2).unwrap(),
            Change::AlreadyDone
        );

        assert_eq!(uninstall_statusline(&layout, 3).unwrap(), Change::Done);
        assert!(!statusline_installed(&layout));
        assert_eq!(
            settings(&layout)["statusLine"]["command"],
            json!("bun line.ts")
        );
        assert_eq!(
            uninstall_statusline(&layout, 4).unwrap(),
            Change::AlreadyDone
        );
        assert!(
            layout
                .backups_dir()
                .join("settings")
                .read_dir()
                .unwrap()
                .count()
                >= 2
        );
    }

    #[test]
    fn statusline_without_previous_command_is_removed_on_uninstall() {
        let (_root, layout) = layout();
        fs::write(settings_path(&layout), r#"{"model":"x"}"#).unwrap();

        install_statusline(&layout, "/bin/relais statusline", 1).unwrap();
        uninstall_statusline(&layout, 2).unwrap();

        assert!(settings(&layout).get("statusLine").is_none());
        assert_eq!(statusline_original_command(&layout), None);
    }

    #[test]
    fn shell_install_appends_once_and_uninstall_removes_only_its_lines() {
        let (_root, layout) = layout();
        fs::write(shell_profile(&layout), "export A=1\n").unwrap();

        assert_eq!(
            install_shell(&layout, Path::new("/opt/relais"), 1).unwrap(),
            Change::Done
        );
        assert!(shell_installed(&layout));
        let profile = fs::read_to_string(shell_profile(&layout)).unwrap();
        assert!(profile.contains(SHELL_MARKER));
        assert!(profile.contains(r#"eval "$(/opt/relais shell-init zsh)""#));
        assert_eq!(
            install_shell(&layout, Path::new("/opt/relais"), 2).unwrap(),
            Change::AlreadyDone
        );

        assert_eq!(uninstall_shell(&layout, 3).unwrap(), Change::Done);
        assert!(!shell_installed(&layout));
        assert_eq!(
            fs::read_to_string(shell_profile(&layout)).unwrap(),
            "export A=1\n"
        );
        assert_eq!(uninstall_shell(&layout, 4).unwrap(), Change::AlreadyDone);
    }

    #[test]
    fn shell_profile_is_created_when_missing() {
        let (_root, layout) = layout();
        install_shell(&layout, Path::new("/opt/relais"), 1).unwrap();
        assert!(shell_installed(&layout));
    }

    #[test]
    fn shell_function_falls_back_to_plain_claude() {
        assert!(SHELL_FUNCTION.contains("relais exec --"));
        assert!(SHELL_FUNCTION.contains("command claude"));
    }

    #[test]
    fn invalid_settings_are_an_error_not_a_rewrite() {
        let (_root, layout) = layout();
        fs::write(settings_path(&layout), "{not json").unwrap();
        assert!(matches!(
            install_statusline(&layout, "/bin/relais statusline", 1),
            Err(Error::Json { .. })
        ));
        assert_eq!(
            fs::read_to_string(settings_path(&layout)).unwrap(),
            "{not json"
        );
    }

    #[test]
    fn artifacts_install_adds_its_hook_and_skill_and_keeps_the_others() {
        let (_root, layout) = layout();
        fs::write(
            settings_path(&layout),
            r#"{"model":"x","hooks":{"PostToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"lint.sh"}]}],"SessionStart":[{"hooks":[{"type":"command","command":"hello.sh"}]}]}}"#,
        )
        .unwrap();
        let relais = Path::new("/Applications/Relais.app/Contents/MacOS/relais");

        assert_eq!(install_artifacts(&layout, relais, 1).unwrap(), Change::Done);

        let after = settings(&layout);
        assert_eq!(after["model"], json!("x"));
        assert_eq!(
            after["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!("hello.sh")
        );
        let post = after["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["hooks"][0]["command"], json!("lint.sh"));
        assert_eq!(post[1]["matcher"], json!("Artifact"));
        assert_eq!(
            post[1]["hooks"][0]["command"],
            json!("/Applications/Relais.app/Contents/MacOS/relais artifact-record")
        );
        assert_eq!(post[1]["hooks"][0]["async"], json!(true));
        let skill = fs::read_to_string(artifact_skill_dir(&layout).join("SKILL.md")).unwrap();
        assert!(
            skill.starts_with("---\nname: relais-artifacts\n"),
            "{skill}"
        );
        assert!(
            skill.contains("/Applications/Relais.app/Contents/MacOS/relais artifacts"),
            "{skill}"
        );
        assert!(artifacts_installed(&layout));
        assert_eq!(
            install_artifacts(&layout, relais, 2).unwrap(),
            Change::AlreadyDone
        );
        assert_eq!(
            settings(&layout)["hooks"]["PostToolUse"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn artifacts_install_follows_the_command_when_it_moves() {
        let (_root, layout) = layout();
        install_artifacts(&layout, Path::new("/old/relais"), 1).unwrap();

        assert_eq!(
            install_artifacts(&layout, Path::new("/new/relais"), 2).unwrap(),
            Change::Done
        );

        let post = settings(&layout)["hooks"]["PostToolUse"].clone();
        assert_eq!(post.as_array().unwrap().len(), 1);
        assert_eq!(
            post[0]["hooks"][0]["command"],
            json!("/new/relais artifact-record")
        );
    }

    #[test]
    fn artifacts_uninstall_removes_only_what_it_added() {
        let (_root, layout) = layout();
        fs::write(
            settings_path(&layout),
            r#"{"hooks":{"PostToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"lint.sh"}]}]}}"#,
        )
        .unwrap();
        install_artifacts(&layout, Path::new("/usr/local/bin/relais"), 1).unwrap();

        assert_eq!(uninstall_artifacts(&layout, 2).unwrap(), Change::Done);

        assert_eq!(
            settings(&layout),
            json!({"hooks":{"PostToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"lint.sh"}]}]}})
        );
        assert!(!artifact_skill_dir(&layout).exists());
        assert!(!artifacts_installed(&layout));
        assert_eq!(
            uninstall_artifacts(&layout, 3).unwrap(),
            Change::AlreadyDone
        );
    }

    #[test]
    fn artifacts_uninstall_leaves_no_empty_hook_list_behind() {
        let (_root, layout) = layout();
        fs::write(settings_path(&layout), r#"{"model":"x"}"#).unwrap();
        install_artifacts(&layout, Path::new("/usr/local/bin/relais"), 1).unwrap();

        uninstall_artifacts(&layout, 2).unwrap();

        assert_eq!(settings(&layout), json!({"model":"x"}));
    }
}
