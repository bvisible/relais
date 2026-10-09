//! End-to-end tests: the real `relais` binary, a temporary home and a fake
//! `claude` that plays sign-in and records how it was started.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
if [ -n "$CLAUDE_CONFIG_DIR" ]; then global="$CLAUDE_CONFIG_DIR/.claude.json"; else global="$HOME/.claude.json"; fi
case "$1 $2" in
  "-p /usage")
    reset=$(date -v+2d '+%b %-d' 2>/dev/null || date -d '+2 days' '+%b %-d')
    printf '{"type":"result","num_turns":0,"total_cost_usd":0,"result":"Current session: 3%% used\\nCurrent week (all models): 41%% used · resets %s at 10pm (Europe/Zurich)\\n"}' "$reset"
    exit 0;;
  "auth status")
    if [ -f "$dir/fake-account" ]; then
      email=$(cat "$dir/fake-account")
      printf '{"loggedIn":true,"email":"%s","orgId":"org-%s","subscriptionType":"max"}' "$email" "$email"
      exit 0
    fi
    printf '{"loggedIn":false}'
    exit 1;;
  "auth login")
    printf '%s' "$BROWSER" > "$HOME/browser-env"
    mkdir -p "$dir"
    printf '%s' "$FAKE_LOGIN_EMAIL" > "$dir/fake-account"
    if [ -f "$global" ]; then content=$(cat "$global"); else content='{}'; fi
    rest="${content#\{}"
    case "$rest" in "}"*) sep="";; *) sep=",";; esac
    printf '{"oauthAccount":{"accountUuid":"acc-%s","emailAddress":"%s","organizationUuid":"org-%s"}%s%s' \
      "$FAKE_LOGIN_EMAIL" "$FAKE_LOGIN_EMAIL" "$FAKE_LOGIN_EMAIL" "$sep" "$rest" > "$global"
    exit 0;;
esac
printf 'CONFIG_DIR=%s PWD=%s ARGS=%s\n' "${CLAUDE_CONFIG_DIR:-unset}" "$(pwd)" "$*" >> "$FAKE_LOG"
exit "${FAKE_EXIT:-0}"
"#;

/// Stands in for the Claude desktop app, so no test ever stops or starts the
/// real one: `running` answers from a file, `quit` and `open` are logged.
const FAKE_DESKTOP: &str = r#"#!/bin/sh
state="$HOME/desktop-running"
case "$1" in
  running) [ -f "$state" ] && exit 0; exit 1;;
  quit) printf 'quit\n' >> "$HOME/desktop.log"; rm -f "$state"; exit 0;;
  open) printf 'open\n' >> "$HOME/desktop.log"; touch "$state"
    log="$HOME/Library/Logs/Claude/main.log"; mkdir -p "$(dirname "$log")"
    if [ -n "$FAKE_DESKTOP_ACCOUNT" ]; then
      printf "%s [info] [ScheduledTasks] Initialized { accountId: '%s', orgId: 'org' }\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$FAKE_DESKTOP_ACCOUNT" >> "$log"
    else
      printf "%s [info] [account] User is logged out, skipping account ID wait\n" "$(date '+%Y-%m-%d %H:%M:%S')" >> "$log"
    fi
    exit 0;;
esac
exit 2
"#;

struct Env {
    root: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let fake = root.path().join("bin/claude");
        fs::create_dir_all(fake.parent().unwrap()).unwrap();
        fs::write(&fake, FAKE_CLAUDE).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let desktop = root.path().join("bin/desktop");
        fs::write(&desktop, FAKE_DESKTOP).unwrap();
        fs::set_permissions(&desktop, fs::Permissions::from_mode(0o755)).unwrap();
        Env { root }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.path().join("claude.log")).unwrap_or_default()
    }

    fn relais(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_relais"));
        command
            .env("HOME", self.home())
            .env("RELAIS_CLAUDE_BIN", self.root.path().join("bin/claude"))
            .env("RELAIS_LANG", "en")
            .env("FAKE_LOG", self.root.path().join("claude.log"))
            .env("RELAIS_CHROME_APP", "Google Chrome")
            .env(
                "RELAIS_DESKTOP_CONTROL",
                self.root.path().join("bin/desktop"),
            )
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("RELAIS_HOME")
            .current_dir(self.root.path());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.relais().args(args).output().unwrap()
    }

    fn sign_in_default(&self, email: &str) {
        fs::write(self.home().join(".claude/fake-account"), email).unwrap();
        fs::write(
            self.home().join(".claude.json"),
            json!({"oauthAccount": {"accountUuid": "acc-main", "emailAddress": email, "organizationUuid": "org-main"},
                   "mcpServers": {"github": {"command": "gh"}}})
            .to_string(),
        )
        .unwrap();
    }

    fn registry(&self) -> Value {
        serde_json::from_slice(&fs::read(self.home().join(".relais/accounts.json")).unwrap())
            .unwrap()
    }

    fn add_account(&self, alias: &str, email: &str) -> Output {
        self.relais()
            .args(["account", "add", alias])
            .env("FAKE_LOGIN_EMAIL", email)
            .output()
            .unwrap()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_ok(output: &Output) {
    assert!(output.status.success(), "relais failed: {}", text(output));
}

fn profile(env: &Env, alias: &str) -> PathBuf {
    env.home()
        .join(".relais/accounts")
        .join(alias)
        .join("claude")
}

#[test]
fn init_registers_the_signed_in_default_profile() {
    let env = Env::new();
    env.sign_in_default("main@example.com");

    let output = env.run(&["init"]);

    assert_ok(&output);
    let registry = env.registry();
    assert_eq!(registry["active"], json!("main"));
    assert_eq!(registry["accounts"][0]["email"], json!("main@example.com"));
    assert_eq!(registry["accounts"][0]["accountUuid"], json!("acc-main"));
    assert!(registry["accounts"][0].get("configDir").is_none());
}

#[test]
fn account_add_creates_a_linked_profile_and_registers_the_new_login() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));

    let output = env.add_account("work", "work@example.com");

    assert_ok(&output);
    let dir = profile(&env, "work");
    assert_eq!(
        fs::read_link(dir.join("projects")).unwrap(),
        env.home().join(".claude/projects")
    );
    let config: Value =
        serde_json::from_slice(&fs::read(dir.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"],
        json!({"github": {"command": "gh"}}),
        "shared MCP servers are seeded"
    );
    assert_eq!(
        config["oauthAccount"]["emailAddress"],
        json!("work@example.com")
    );
    let registry = env.registry();
    assert_eq!(registry["accounts"][1]["alias"], json!("work"));
    assert_eq!(registry["accounts"][1]["email"], json!("work@example.com"));
    assert_eq!(
        registry["active"],
        json!("main"),
        "adding an account does not switch to it"
    );
}

#[test]
fn account_add_refuses_an_account_already_registered() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));

    let output = env.add_account("again", "MAIN@example.com");

    assert!(!output.status.success());
    assert!(
        text(&output).contains("main@example.com") || text(&output).contains("MAIN@example.com"),
        "{}",
        text(&output)
    );
    assert_eq!(env.registry()["accounts"].as_array().unwrap().len(), 1);
}

#[test]
fn switch_moves_the_active_account() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));

    assert_ok(&env.run(&["switch", "work"]));
    assert_eq!(env.registry()["active"], json!("work"));
    assert_ok(&env.run(&["switch", "next"]));
    assert_eq!(env.registry()["active"], json!("main"));
    assert_ok(&env.run(&["switch", "prev"]));
    assert_eq!(env.registry()["active"], json!("work"));
    assert!(!env.run(&["switch", "nobody"]).status.success());
}

#[test]
fn exec_runs_claude_in_the_profile_of_the_active_account() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));

    assert_ok(&env.run(&["exec", "--", "--version"]));
    assert_ok(&env.run(&["switch", "work"]));
    assert_ok(&env.run(&["exec", "--", "-p", "hello"]));

    let log = env.log();
    let lines: Vec<&str> = log.lines().collect();
    assert!(
        lines[0].starts_with("CONFIG_DIR=unset") && lines[0].ends_with("ARGS=--version"),
        "{log}"
    );
    let work_dir = profile(&env, "work");
    assert!(
        lines[1].starts_with(&format!("CONFIG_DIR={}", work_dir.display())),
        "{log}"
    );
    assert!(lines[1].ends_with("ARGS=-p hello"), "{log}");
}

#[test]
fn exec_without_accounts_still_runs_claude() {
    let env = Env::new();

    let output = env
        .relais()
        .args(["exec", "--", "--version"])
        .env("FAKE_EXIT", "3")
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(3),
        "the exit code of claude is kept"
    );
    assert!(env.log().contains("ARGS=--version"));
}

#[test]
fn statusline_install_wraps_the_existing_command_and_uninstall_restores_it() {
    let env = Env::new();
    let settings = env.home().join(".claude/settings.json");
    fs::write(&settings, json!({"model": "x", "statusLine": {"type": "command", "command": "printf hello", "padding": 0}}).to_string())
        .unwrap();

    assert_ok(&env.run(&["statusline-install"]));
    let installed: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    let command = installed["statusLine"]["command"].as_str().unwrap();
    assert!(command.ends_with(" statusline"), "{command}");
    assert_eq!(installed["statusLine"]["padding"], json!(0));
    assert_eq!(installed["model"], json!("x"));

    assert_ok(&env.run(&["statusline-uninstall"]));
    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["command"], json!("printf hello"));
}

#[test]
fn statusline_records_usage_and_still_renders_the_users_status_line() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    let settings = env.home().join(".claude/settings.json");
    fs::write(&settings, json!({"statusLine": {"type": "command", "command": "cat > \"$HOME/seen.json\"; printf rendered"}}).to_string())
        .unwrap();
    assert_ok(&env.run(&["statusline-install"]));
    let input = json!({"session_id": "s-1", "rate_limits": {
        "five_hour": {"used_percentage": 42, "resets_at": 4_102_444_800_i64},
        "seven_day": {"used_percentage": 7, "resets_at": 4_102_444_800_i64}}});

    let mut child = env
        .relais()
        .arg("statusline")
        .env("CLAUDE_CONFIG_DIR", profile(&env, "work"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), input.to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();

    assert_ok(&output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "rendered");
    let seen: Value =
        serde_json::from_slice(&fs::read(env.home().join("seen.json")).unwrap()).unwrap();
    assert_eq!(seen, input, "the user's command receives the same input");
    let usage: Value =
        serde_json::from_slice(&fs::read(env.home().join(".relais/usage/work.json")).unwrap())
            .unwrap();
    assert_eq!(usage["fiveHour"]["usedPercentage"], json!(42.0));
}

#[test]
fn statusline_survives_input_it_cannot_read() {
    let env = Env::new();
    fs::write(
        env.home().join(".claude/settings.json"),
        json!({"statusLine": {"type": "command", "command": "printf still-here"}}).to_string(),
    )
    .unwrap();
    assert_ok(&env.run(&["statusline-install"]));

    let mut child = env
        .relais()
        .arg("statusline")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"not json at all").unwrap();
    let output = child.wait_with_output().unwrap();

    assert_ok(&output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "still-here");
}

#[test]
fn shell_init_prints_a_claude_function_that_falls_back_to_claude() {
    let env = Env::new();
    let output = env.run(&["shell-init", "zsh"]);
    assert_ok(&output);
    let script = String::from_utf8_lossy(&output.stdout);
    assert!(script.contains("claude()"), "{script}");
    assert!(script.contains("relais exec --"), "{script}");
    assert!(script.contains("command claude"), "{script}");
}

#[test]
fn list_shows_accounts_with_their_usage() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    fs::create_dir_all(env.home().join(".relais/usage")).unwrap();
    let now = chrono_now();
    fs::write(
        env.home().join(".relais/usage/work.json"),
        json!({"fiveHour": {"usedPercentage": 64.0, "resetsAt": now + 5400}, "sevenDay": {"usedPercentage": 21.0, "resetsAt": now + 30 * 3600},
               "observedAt": now - 120, "source": "statusline"})
        .to_string(),
    )
    .unwrap();

    let output = env.run(&["list"]);

    assert_ok(&output);
    let listing = String::from_utf8_lossy(&output.stdout);
    assert!(
        listing.contains("main") && listing.contains("main@example.com"),
        "{listing}"
    );
    assert!(
        listing.contains("work") && listing.contains("64%") && listing.contains("21%"),
        "{listing}"
    );
    assert!(listing.contains("resets in 1h 30m"), "{listing}");
    assert!(listing.contains("resets in 1d 6h"), "{listing}");
}

#[test]
fn list_with_refresh_asks_claude_code_for_the_week_of_each_account() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));

    let output = env.run(&["list", "--refresh"]);

    assert_ok(&output);
    let listing = String::from_utf8_lossy(&output.stdout);
    let row = listing
        .lines()
        .find(|l| l.contains("work@example.com"))
        .unwrap_or_default();
    assert!(
        row.contains("41%") && row.contains("resets in"),
        "{listing}"
    );
}

#[test]
fn list_says_how_long_until_a_limited_account_is_back() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    fs::create_dir_all(env.home().join(".relais/usage")).unwrap();
    let now = chrono_now();
    fs::write(
        env.home().join(".relais/usage/main.json"),
        json!({"fiveHour": {"usedPercentage": 100.0, "resetsAt": now + 2 * 3600 + 600},
               "observedAt": now - 60, "source": "statusline"})
        .to_string(),
    )
    .unwrap();

    let output = env.run(&["list"]);

    assert_ok(&output);
    let listing = String::from_utf8_lossy(&output.stdout);
    assert!(listing.contains("limited · back in 2h 10m"), "{listing}");
}

#[test]
fn conversations_name_the_account_that_ran_each_one() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    let project = env.home().join(".claude/projects/-workdir");
    fs::create_dir_all(&project).unwrap();
    let line = json!({"type": "user", "cwd": "/workdir", "entrypoint": "cli", "message": {"role": "user", "content": "hello there"}});
    fs::write(project.join("s-42.jsonl"), format!("{line}\n")).unwrap();
    fs::write(
        env.home().join(".relais/sessions.json"),
        json!({"s-42": {"alias": "main", "seenAt": chrono_now()}}).to_string(),
    )
    .unwrap();

    let output = env.run(&["conversations"]);

    assert_ok(&output);
    let listing = String::from_utf8_lossy(&output.stdout);
    let row = listing
        .lines()
        .find(|l| l.contains("hello there"))
        .unwrap_or_default();
    assert!(row.contains("main"), "{listing}");
}

#[test]
fn continue_resumes_a_conversation_from_its_directory() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    let workdir = env.root.path().join("workdir");
    fs::create_dir_all(&workdir).unwrap();
    let project = env.home().join(".claude/projects/-workdir");
    fs::create_dir_all(&project).unwrap();
    let line = json!({"type": "user", "cwd": workdir, "entrypoint": "cli", "message": {"role": "user", "content": "hello"}});
    fs::write(project.join("s-42.jsonl"), format!("{line}\n")).unwrap();

    assert_ok(&env.run(&["continue", "s-42"]));

    let log = env.log();
    assert!(log.contains("ARGS=--resume s-42"), "{log}");
    let pwd = workdir.canonicalize().unwrap();
    assert!(log.contains(&format!("PWD={}", pwd.display())), "{log}");
}

#[test]
fn account_add_by_email_derives_the_name_and_signs_in_through_its_own_chrome_profile() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));

    let output = env.add_account("hello@example.com", "hello@example.com");

    assert_ok(&output);
    let registry = env.registry();
    assert_eq!(registry["accounts"][1]["alias"], json!("hello"));
    assert_eq!(registry["accounts"][1]["email"], json!("hello@example.com"));
    assert_eq!(
        registry["accounts"][1]["chromeProfile"],
        json!("Relais-hello")
    );
    let browser = fs::read_to_string(env.home().join("browser-env")).unwrap();
    let opener = fs::read_to_string(&browser).unwrap_or_default();
    assert!(
        opener.contains("--profile-directory=Relais-hello"),
        "BROWSER={browser}: {opener}"
    );
    assert!(opener.contains("Google Chrome"), "{opener}");
}

#[test]
fn account_login_signs_an_account_in_again_through_its_own_chrome_profile() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("hello@example.com", "hello@example.com"));
    fs::remove_file(env.home().join("browser-env")).unwrap();

    let output = env
        .relais()
        .args(["account", "login", "hello"])
        .env("FAKE_LOGIN_EMAIL", "hello@example.com")
        .output()
        .unwrap();

    assert_ok(&output);
    assert!(
        text(&output).contains("hello@example.com"),
        "{}",
        text(&output)
    );
    let browser = fs::read_to_string(env.home().join("browser-env")).unwrap();
    let opener = fs::read_to_string(&browser).unwrap_or_default();
    assert!(
        opener.contains("--profile-directory=Relais-hello"),
        "BROWSER={browser}: {opener}"
    );
}

#[test]
fn account_login_with_another_email_is_signed_out_again() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("hello@example.com", "hello@example.com"));

    let output = env
        .relais()
        .args(["account", "login", "hello"])
        .env("FAKE_LOGIN_EMAIL", "someone-else@example.com")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = text(&output);
    assert!(
        message.contains("someone-else@example.com") && message.contains("hello@example.com"),
        "{message}"
    );
    assert!(env.log().contains("ARGS=auth logout"), "{}", env.log());
}

#[test]
fn account_login_names_an_unknown_account() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));

    let output = env.run(&["account", "login", "nobody"]);

    assert!(!output.status.success());
    assert!(text(&output).contains("nobody"), "{}", text(&output));
}

fn desktop_folder(env: &Env) -> PathBuf {
    env.home().join("Library/Application Support/Claude")
}

#[test]
fn desktop_switch_closes_the_app_parks_its_folder_and_opens_it_again() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    fs::create_dir_all(desktop_folder(&env)).unwrap();
    fs::write(desktop_folder(&env).join("marker"), "main's data").unwrap();
    fs::write(
        desktop_folder(&env).join("plan-usage-history.json"),
        json!({"samples": [{"t": 1, "org": "org-main", "u": {"fh": 1}}]}).to_string(),
    )
    .unwrap();
    fs::write(env.home().join("desktop-running"), "").unwrap();
    let active_before = env.registry()["active"].clone();

    let output = env.run(&["desktop", "work"]);

    assert_ok(&output);
    let log = fs::read_to_string(env.home().join("desktop.log")).unwrap_or_default();
    assert_eq!(log, "quit\nopen\n");
    assert!(!desktop_folder(&env).join("marker").exists());
    let parked = env.home().join(".relais/accounts/main/desktop/marker");
    assert_eq!(fs::read_to_string(parked).unwrap(), "main's data");
    assert!(
        text(&output).contains("work@example.com"),
        "{}",
        text(&output)
    );
    assert_eq!(env.registry()["active"], active_before);
}

#[test]
fn desktop_switch_confirms_the_account_the_app_started_with() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    fs::create_dir_all(desktop_folder(&env)).unwrap();
    fs::write(
        env.home().join(".relais/desktop.json"),
        r#"{"owner":"main"}"#,
    )
    .unwrap();
    fs::create_dir_all(env.home().join(".relais/accounts/work/desktop")).unwrap();

    let output = env
        .relais()
        .args(["desktop", "work"])
        .env("FAKE_DESKTOP_ACCOUNT", "acc-work@example.com")
        .output()
        .unwrap();

    assert_ok(&output);
    assert!(text(&output).contains("signed in"), "{}", text(&output));
}

#[test]
fn desktop_without_an_account_says_which_account_the_app_uses() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    fs::create_dir_all(desktop_folder(&env)).unwrap();
    fs::write(
        desktop_folder(&env).join("plan-usage-history.json"),
        json!({"samples": [{"t": 1, "org": "org-work@example.com", "u": {"fh": 1}}]}).to_string(),
    )
    .unwrap();

    let output = env.run(&["desktop"]);

    assert_ok(&output);
    let listing = String::from_utf8_lossy(&output.stdout);
    let row = listing
        .lines()
        .find(|l| l.contains("work"))
        .unwrap_or_default();
    assert!(row.contains('●'), "{listing}");
    assert!(!fs::read_to_string(env.home().join("desktop.log"))
        .unwrap_or_default()
        .contains("quit"));
}

#[test]
fn account_add_refuses_a_sign_in_with_another_email() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));

    let output = env.add_account("hello@example.com", "someone-else@example.com");

    assert!(!output.status.success());
    let message = text(&output);
    assert!(
        message.contains("someone-else@example.com") && message.contains("hello@example.com"),
        "{message}"
    );
    assert_eq!(env.registry()["accounts"].as_array().unwrap().len(), 1);
    assert!(
        !profile(&env, "hello").exists(),
        "the half-made profile is removed"
    );
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[allow(dead_code)]
fn unused(_: &Path) {}
