//! Talking to the official `claude` binary.
//!
//! Relais starts Claude Code with the right `CLAUDE_CONFIG_DIR` and asks it
//! who is signed in (`claude auth status`). It never reads the credentials
//! themselves.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::fsutil;

/// Environment variable that points Relais at a specific `claude` binary.
pub const CLAUDE_BIN_ENV: &str = "RELAIS_CLAUDE_BIN";

/// What `claude auth status` reports for one profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub logged_in: bool,
    pub email: Option<String>,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    pub subscription_type: Option<String>,
}

/// Non-secret account metadata Claude Code keeps in `.claude.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountIdentity {
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    pub org_id: Option<String>,
}

/// Finds the real `claude` executable: `override_path` first, then `PATH`.
pub fn find_claude_binary(
    override_path: Option<&Path>,
    path_env: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(path) = override_path.filter(|path| is_executable(path)) {
        return Some(path.to_path_buf());
    }
    std::env::split_paths(path_env?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("claude"))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Builds the command that runs `binary` for one profile: `config_dir` set as
/// `CLAUDE_CONFIG_DIR`, or the variable removed for the default profile.
pub fn command_for_profile(binary: &Path, config_dir: Option<&Path>) -> Command {
    let mut command = Command::new(binary);
    match config_dir {
        Some(dir) => {
            command.env("CLAUDE_CONFIG_DIR", dir);
        }
        None => {
            command.env_remove("CLAUDE_CONFIG_DIR");
        }
    }
    command
}

/// Asks `claude auth status --json` who is signed in for a profile.
pub fn auth_status(binary: &Path, config_dir: Option<&Path>) -> Result<AuthStatus> {
    let output = command_for_profile(binary, config_dir)
        .args(["auth", "status", "--json"])
        .output()
        .map_err(|e| Error::io(binary, e))?;
    // A signed-out profile may exit with an error code but still print its JSON.
    if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) {
        if value.get("loggedIn").is_some() {
            let text = |key: &str| value.get(key).and_then(Value::as_str).map(String::from);
            return Ok(AuthStatus {
                logged_in: value
                    .get("loggedIn")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                email: text("email"),
                org_id: text("orgId"),
                org_name: text("orgName"),
                subscription_type: text("subscriptionType"),
            });
        }
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Error::Command(format!(
        "{} auth status: {}",
        binary.display(),
        stderr.trim()
    )))
}

/// Reads `oauthAccount` from a profile's `.claude.json`.
pub fn read_account_identity(global_config: &Path) -> Result<Option<AccountIdentity>> {
    let Some(config) = fsutil::read_json(global_config)? else {
        return Ok(None);
    };
    let Some(account) = config.get("oauthAccount") else {
        return Ok(None);
    };
    let text = |key: &str| account.get(key).and_then(Value::as_str).map(String::from);
    Ok(Some(AccountIdentity {
        account_uuid: text("accountUuid"),
        email: text("emailAddress"),
        org_id: text("organizationUuid"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_claude(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("claude");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const ECHO_STATUS: &str = r#"printf '{"loggedIn":true,"email":"a@example.com","orgId":"org-1","orgName":"A","subscriptionType":"max","configDirectory":"%s"}' "${CLAUDE_CONFIG_DIR:-unset}""#;

    #[test]
    fn binary_is_found_in_path_and_skips_files_that_are_not_executable() {
        let dir = tempfile::tempdir().unwrap();
        let not_exec = dir.path().join("first");
        let exec = dir.path().join("second");
        fs::create_dir_all(&not_exec).unwrap();
        fs::create_dir_all(&exec).unwrap();
        fs::write(not_exec.join("claude"), "x").unwrap();
        let expected = fake_claude(&exec, "true");
        let path = std::env::join_paths([&not_exec, &exec]).unwrap();

        assert_eq!(find_claude_binary(None, Some(&path)), Some(expected));
        assert_eq!(find_claude_binary(None, Some(OsStr::new(""))), None);
    }

    #[test]
    fn override_wins_over_path() {
        let dir = tempfile::tempdir().unwrap();
        let chosen = fake_claude(dir.path(), "true");
        assert_eq!(
            find_claude_binary(Some(&chosen), Some(OsStr::new("/nowhere"))),
            Some(chosen)
        );
    }

    #[test]
    fn auth_status_runs_in_the_profile_directory() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_claude(dir.path(), ECHO_STATUS);

        let status = auth_status(&binary, Some(Path::new("/profiles/work"))).unwrap();

        assert!(status.logged_in);
        assert_eq!(status.email.as_deref(), Some("a@example.com"));
        assert_eq!(status.org_id.as_deref(), Some("org-1"));
        assert_eq!(status.subscription_type.as_deref(), Some("max"));
    }

    #[test]
    fn default_profile_runs_without_a_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_claude(dir.path(), ECHO_STATUS);

        let output = command_for_profile(&binary, None)
            .env("CLAUDE_CONFIG_DIR", "/leaked")
            .env_remove("X")
            .output();
        let leaked = String::from_utf8(output.unwrap().stdout).unwrap();
        assert!(
            leaked.contains("/leaked"),
            "the test harness can still set it explicitly"
        );

        let mut command = command_for_profile(&binary, None);
        let clean = String::from_utf8(command.output().unwrap().stdout).unwrap();
        assert!(clean.contains(r#""configDirectory":"unset""#), "{clean}");

        let mut command = command_for_profile(&binary, Some(Path::new("/profiles/work")));
        let set = String::from_utf8(command.output().unwrap().stdout).unwrap();
        assert!(
            set.contains(r#""configDirectory":"/profiles/work""#),
            "{set}"
        );
    }

    #[test]
    fn signed_out_profile_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_claude(dir.path(), r#"printf '{"loggedIn":false}'"#);

        let status = auth_status(&binary, None).unwrap();

        assert!(!status.logged_in);
        assert_eq!(status.email, None);
    }

    #[test]
    fn failing_command_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_claude(dir.path(), "echo boom >&2; exit 3");
        assert!(matches!(auth_status(&binary, None), Err(Error::Command(_))));
    }

    #[test]
    fn identity_comes_from_the_oauth_account_block() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        fs::write(
            &config,
            r#"{"oauthAccount":{"accountUuid":"acc-1","emailAddress":"a@example.com","organizationUuid":"org-1"}}"#,
        )
        .unwrap();

        let identity = read_account_identity(&config).unwrap().unwrap();

        assert_eq!(identity.account_uuid.as_deref(), Some("acc-1"));
        assert_eq!(identity.email.as_deref(), Some("a@example.com"));
        assert_eq!(identity.org_id.as_deref(), Some("org-1"));
        assert_eq!(
            read_account_identity(&dir.path().join("absent.json")).unwrap(),
            None
        );
    }
}
