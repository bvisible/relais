//! End-to-end tests: the real `relais` binary, a temporary home and a fake
//! `claude` that plays sign-in and records how it was started.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

// The fake keeps who is "signed in" in `$dir/.credentials.json`, a name Relais never links
// into a profile (the real Claude Code keeps its tokens in the Keychain, outside the folder).
// Any other name would be linked from `~/.claude` into every profile, and a sign-in in one
// profile would overwrite the others'.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
if [ -n "$CLAUDE_CONFIG_DIR" ]; then global="$CLAUDE_CONFIG_DIR/.claude.json"; else global="$HOME/.claude.json"; fi
case "$1 $2" in
  "auth status")
    if [ -f "$dir/.credentials.json" ]; then
      email=$(cat "$dir/.credentials.json")
      printf '{"loggedIn":true,"email":"%s","orgId":"org-%s","subscriptionType":"max"}' "$email" "$email"
      exit 0
    fi
    printf '{"loggedIn":false}'
    exit 1;;
  "auth login")
    printf '%s' "$BROWSER" > "$HOME/browser-env"
    mkdir -p "$dir"
    printf '%s' "$FAKE_LOGIN_EMAIL" > "$dir/.credentials.json"
    if [ -f "$global" ]; then content=$(cat "$global"); else content='{}'; fi
    rest="${content#\{}"
    case "$rest" in "}"*) sep="";; *) sep=",";; esac
    printf '{"oauthAccount":{"accountUuid":"acc-%s","emailAddress":"%s","organizationUuid":"org-%s"}%s%s' \
      "$FAKE_LOGIN_EMAIL" "$FAKE_LOGIN_EMAIL" "$FAKE_LOGIN_EMAIL" "$sep" "$rest" > "$global"
    exit 0;;
esac
if [ "$1" = "--safe-mode" ]; then
  printf 'CONFIG_DIR=%s PWD=%s ARGS=%s\n' "${CLAUDE_CONFIG_DIR:-unset}" "$(pwd)" "$*" >> "$FAKE_LOG"
  [ -t 0 ] || exit 0
  reset=$(date -v+2d '+%b %-d' 2>/dev/null || date -d '+2 days' '+%b %-d')
  stty raw -echo
  printf 'Claude Code\r\n\342\235\257\r\n'
  dd bs=1 count=6 >/dev/null 2>&1
  printf '/usage Show plan usage limits\r\n'
  dd bs=1 count=1 >/dev/null 2>&1
  printf '\033[2J\033[HSettings: Usage\r\nCurrent session: 3%% used\r\nCurrent week (all models): 41%% used \302\267 resets %s at 10pm (Europe/Zurich)\r\n' "$reset"
  sleep 20
  exit 0
fi
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
    if [ -n "$FAKE_DESKTOP_ORG" ]; then
      printf '{"samples":[{"t":%s000,"org":"%s"}]}' "$(date +%s)" "$FAKE_DESKTOP_ORG" > "$HOME/Library/Application Support/Claude/plan-usage-history.json"
    fi
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
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env_remove("RELAIS_HOME")
            .current_dir(self.root.path());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.relais().args(args).output().unwrap()
    }

    fn sign_in_default(&self, email: &str) {
        fs::write(self.home().join(".claude/.credentials.json"), email).unwrap();
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

/// The desktop app's sign-in for `uuid`, as the app leaves it in a folder.
fn desktop_sign_in(dir: &Path, uuid: &str, secret: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("config.json"),
        json!({"oauth:tokenCacheV2": secret, "lastKnownAccountUuid": uuid}).to_string(),
    )
    .unwrap();
    fs::write(dir.join("Cookies"), format!("cookie-{secret}")).unwrap();
}

const WORK_ACCOUNT: &str = "acc-work@example.com";
const WORK_ORG: &str = "org-work@example.com";

/// main signed in to the desktop app with one conversation and its folder
/// order; work's sign-in parked.
fn desktop_workspace(env: &Env) {
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    let live = desktop_folder(env);
    desktop_sign_in(&live, "acc-main", "main");
    let list = live.join("claude-code-sessions/acc-main/org-main");
    fs::create_dir_all(&list).unwrap();
    fs::write(
        list.join("local_1.json"),
        json!({"sessionId": "local_1", "cliSessionId": "cli-1", "lastActivityAt": 1}).to_string(),
    )
    .unwrap();
    fs::write(
        live.join("claude_desktop_config.json"),
        json!({"preferences": {"epitaxyPrefs": {"code-projects-order.acc-main": ["/w/one"]}}})
            .to_string(),
    )
    .unwrap();
    desktop_sign_in(
        &env.home().join(".relais/accounts/work/identity"),
        WORK_ACCOUNT,
        "work",
    );
}

/// Every file under `dir` with its content.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn desktop_switch_keeps_the_list_moves_the_sign_in_and_the_terminal() {
    let env = Env::new();
    desktop_workspace(&env);
    fs::write(env.home().join("desktop-running"), "").unwrap();

    let output = env
        .relais()
        .args(["desktop", "work"])
        .env("FAKE_DESKTOP_ORG", WORK_ORG)
        .output()
        .unwrap();

    assert_ok(&output);
    let log = fs::read_to_string(env.home().join("desktop.log")).unwrap_or_default();
    assert_eq!(log, "quit\nopen\n");
    let live = desktop_folder(&env);
    assert_eq!(
        fs::read_to_string(live.join("Cookies")).unwrap(),
        "cookie-work"
    );
    assert_eq!(
        fs::read_to_string(env.home().join(".relais/accounts/main/identity/Cookies")).unwrap(),
        "cookie-main"
    );
    assert!(live
        .join(format!(
            "claude-code-sessions/{WORK_ACCOUNT}/{WORK_ORG}/local_1.json"
        ))
        .is_file());
    let config: Value =
        serde_json::from_slice(&fs::read(live.join("claude_desktop_config.json")).unwrap())
            .unwrap();
    assert_eq!(
        config["preferences"]["epitaxyPrefs"][format!("code-projects-order.{WORK_ACCOUNT}")],
        json!(["/w/one"])
    );
    assert_eq!(env.registry()["active"], "work");
    assert!(text(&output).contains("signed in"), "{}", text(&output));
}

#[test]
fn desktop_dry_run_says_what_would_move_and_changes_nothing() {
    let env = Env::new();
    desktop_workspace(&env);
    let before = snapshot(&env.home());

    let output = env.run(&["desktop", "work", "--dry-run"]);

    assert_ok(&output);
    assert_eq!(snapshot(&env.home()), before);
    let said = text(&output);
    assert!(said.contains("main"), "{said}");
    assert!(said.contains("1 conversation"), "{said}");
}

#[test]
fn desktop_switch_asks_what_to_do_with_the_account_s_own_conversations() {
    let env = Env::new();
    desktop_workspace(&env);
    let own = desktop_folder(&env).join(format!("claude-code-sessions/{WORK_ACCOUNT}/{WORK_ORG}"));
    fs::create_dir_all(&own).unwrap();
    fs::write(
        own.join("local_own.json"),
        json!({"sessionId": "local_own", "cliSessionId": "cli-own"}).to_string(),
    )
    .unwrap();
    let before = snapshot(&env.home());

    let asked = env.run(&["desktop", "work"]);

    assert_eq!(asked.status.code(), Some(2), "{}", text(&asked));
    assert!(text(&asked).contains("--own"), "{}", text(&asked));
    assert_eq!(snapshot(&env.home()), before);

    assert_ok(&env.run(&["desktop", "work", "--own", "add"]));
    assert!(own.join("local_own.json").is_file());
    assert!(own.join("local_1.json").is_file());
}

#[test]
fn conversations_set_aside_come_back_with_one_command_once_the_app_is_closed() {
    let env = Env::new();
    desktop_workspace(&env);
    let own = desktop_folder(&env).join(format!("claude-code-sessions/{WORK_ACCOUNT}/{WORK_ORG}"));
    fs::create_dir_all(&own).unwrap();
    fs::write(
        own.join("local_own.json"),
        json!({"sessionId": "local_own", "cliSessionId": "cli-own"}).to_string(),
    )
    .unwrap();
    assert_ok(&env.run(&["desktop", "work", "--own", "aside"]));
    assert!(!own.join("local_own.json").exists());

    // The switch leaves the app open, and it keeps its list in memory.
    let refused = env.run(&["desktop", "--bring-back"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(!own.join("local_own.json").exists());

    fs::remove_file(env.home().join("desktop-running")).unwrap();
    let back = env.run(&["desktop", "--bring-back"]);

    assert_ok(&back);
    assert!(text(&back).contains("Back in your list"), "{}", text(&back));
    assert!(own.join("local_own.json").is_file());
}

#[test]
fn allowing_the_measure_opens_the_client_in_the_folder_it_measures_from() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));

    let output = env.run(&["allow-measure"]);

    assert_ok(&output);
    assert!(text(&output).contains("Yes"), "{}", text(&output));
    let log = fs::read_to_string(env.root.path().join("claude.log")).unwrap();
    assert!(log.contains("probe/workspace"), "{log}");
    assert!(log.contains("--safe-mode"), "{log}");
}

#[test]
fn projects_lists_what_each_project_consumed() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    let folder = env.home().join(".claude/projects/-work");
    fs::create_dir_all(&folder).unwrap();
    let when = transcript_date();
    fs::write(
        folder.join("a.jsonl"),
        format!(
            "{}\n",
            json!({"cwd": "/work/app", "timestamp": when, "message": {
                "id": "m1", "usage": {"input_tokens": 10, "output_tokens": 5}
            }})
        ),
    )
    .unwrap();

    let output = env.run(&["projects"]);

    assert_ok(&output);
    assert!(text(&output).contains("app"), "{}", text(&output));
    assert!(text(&output).contains("100%"), "{}", text(&output));
}

/// Now as a transcript writes it.
fn transcript_date() -> String {
    let out = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_folder_tied_to_an_account_starts_on_it_and_stops_when_untied() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    let folder = env.home().join("projects/app");
    fs::create_dir_all(folder.join("src")).unwrap();
    let folder_text = folder.display().to_string();
    assert_ok(&env.run(&["folder", "set", "work", &folder_text]));
    assert!(text(&env.run(&["folder"])).contains("work"));

    let started = env
        .relais()
        .args(["exec", "--", "--version"])
        .current_dir(folder.join("src"))
        .output()
        .unwrap();

    assert_ok(&started);
    let log = fs::read_to_string(env.root.path().join("claude.log")).unwrap();
    let last = log.lines().last().unwrap_or_default();
    assert!(last.contains("accounts/work"), "{last}");

    assert_ok(&env.run(&["folder", "remove", &folder_text]));
    let started = env
        .relais()
        .args(["exec", "--", "--version"])
        .current_dir(folder.join("src"))
        .output()
        .unwrap();
    assert_ok(&started);
    let log = fs::read_to_string(env.root.path().join("claude.log")).unwrap();
    let last = log.lines().last().unwrap_or_default();
    assert!(!last.contains("accounts/work"), "{last}");
}

#[test]
fn a_closed_pipe_ends_a_command_quietly() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);

    let output = env.relais().arg("list").stdout(writer).output().unwrap();

    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("panicked"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn desktop_switch_from_inside_the_desktop_app_only_previews() {
    let env = Env::new();
    desktop_workspace(&env);
    let before = snapshot(&env.home());

    let refused = env
        .relais()
        .args(["desktop", "work"])
        .env("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")
        .output()
        .unwrap();

    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert_eq!(snapshot(&env.home()), before);
    let preview = env
        .relais()
        .args(["desktop", "work", "--dry-run"])
        .env("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")
        .output()
        .unwrap();
    assert_ok(&preview);
}

#[test]
fn desktop_switch_cut_short_is_undone_then_done() {
    let env = Env::new();
    desktop_workspace(&env);
    let live = desktop_folder(&env);
    let parked = env.home().join(".relais/accounts/main/identity");
    fs::create_dir_all(&parked).unwrap();
    for name in ["config.json", "Cookies"] {
        fs::rename(live.join(name), parked.join(name)).unwrap();
    }
    fs::write(
        env.home().join(".relais/workspace-switch.json"),
        json!({
            "version": 1, "from": "main", "to": "work",
            "fromIds": {"account": "acc-main", "org": "org-main"},
            "toIds": {"account": WORK_ACCOUNT, "org": WORK_ORG},
            "backup": env.home().join(".relais/backups/workspace/1"),
            "started": ["backup", "parkSource"]
        })
        .to_string(),
    )
    .unwrap();

    let output = env.run(&["desktop", "work"]);

    assert_ok(&output);
    assert!(text(&output).contains("undone"), "{}", text(&output));
    assert_eq!(
        fs::read_to_string(live.join("Cookies")).unwrap(),
        "cookie-work"
    );
    assert_eq!(
        fs::read_to_string(parked.join("Cookies")).unwrap(),
        "cookie-main"
    );
    assert!(!env.home().join(".relais/workspace-switch.json").exists());
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

/// What `/login` inside Claude Code does to the default profile: another
/// account is signed in there, without Relais knowing.
fn sign_in_default_as(env: &Env, email: &str, uuid: &str) {
    fs::write(env.home().join(".claude/.credentials.json"), email).unwrap();
    fs::write(
        env.home().join(".claude.json"),
        json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email, "organizationUuid": format!("org-{uuid}")},
               "mcpServers": {"github": {"command": "gh"}}})
        .to_string(),
    )
    .unwrap();
}

/// Two accounts registered, then someone signs a third in on the default profile.
fn env_with_a_third_account_on_the_default_profile() -> Env {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    sign_in_default_as(&env, "new@example.com", "acc-new");
    env
}

#[test]
fn account_adopt_hands_the_default_profile_to_the_account_signed_in_there() {
    let env = env_with_a_third_account_on_the_default_profile();

    let output = env.run(&["account", "adopt"]);

    assert_ok(&output);
    let registry = env.registry();
    let accounts = registry["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 3);
    assert_eq!(accounts[2]["alias"], json!("new"));
    assert_eq!(accounts[2]["email"], json!("new@example.com"));
    assert_eq!(accounts[2]["accountUuid"], json!("acc-new"));
    assert!(
        accounts[2].get("configDir").is_none(),
        "the account signed in keeps the default profile"
    );
    assert_eq!(accounts[0]["alias"], json!("main"));
    assert_eq!(
        accounts[0]["configDir"],
        json!(profile(&env, "main").display().to_string())
    );
    assert_eq!(accounts[0]["chromeProfile"], json!("Relais-main"));
    assert_eq!(registry["active"], json!("new"));
    let message = text(&output);
    assert!(
        message.contains("new@example.com") && message.contains("relais account login main"),
        "{message}"
    );
}

#[test]
fn account_adopt_names_the_account_with_the_alias_asked() {
    let env = env_with_a_third_account_on_the_default_profile();

    assert_ok(&env.run(&["account", "adopt", "--alias", "fresh"]));

    assert_eq!(env.registry()["accounts"][2]["alias"], json!("fresh"));
    assert_eq!(env.registry()["active"], json!("fresh"));
}

#[test]
fn doctor_points_at_adopt_when_the_default_profile_changed_hands() {
    let env = env_with_a_third_account_on_the_default_profile();

    let before = text(&env.run(&["doctor"]));
    assert!(
        before.contains("signed in as new@example.com, registered as main@example.com")
            && before.contains("relais account adopt"),
        "{before}"
    );

    assert_ok(&env.run(&["account", "adopt"]));

    let after = text(&env.run(&["doctor"]));
    assert!(
        !after.contains("registered as main@example.com"),
        "the mismatch is gone: {after}"
    );
    assert!(
        after.contains("new: signed in as new@example.com"),
        "{after}"
    );
    assert!(
        after.contains("main: not signed in"),
        "the previous account waits for its own sign-in: {after}"
    );
}

#[test]
fn the_previous_account_signs_in_again_in_its_own_profile_after_an_adoption() {
    let env = env_with_a_third_account_on_the_default_profile();
    assert_ok(&env.run(&["account", "adopt"]));

    let login = env
        .relais()
        .args(["account", "login", "main"])
        .env("FAKE_LOGIN_EMAIL", "main@example.com")
        .output()
        .unwrap();

    assert_ok(&login);
    let doctor = text(&env.run(&["doctor"]));
    assert!(
        doctor.contains("main: signed in as main@example.com")
            && doctor.contains("new: signed in as new@example.com"),
        "both accounts are signed in, each in its own profile: {doctor}"
    );
    let browser = fs::read_to_string(env.home().join("browser-env")).unwrap();
    assert!(
        browser.contains("accounts/main/open-browser"),
        "the sign-in opens the account's own Chrome profile: {browser}"
    );
}

#[test]
fn account_adopt_has_nothing_to_do_when_the_registered_account_is_still_there() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    let before = env.registry();

    let output = env.run(&["account", "adopt"]);

    assert!(!output.status.success());
    assert!(text(&output).contains("main"), "{}", text(&output));
    assert_eq!(env.registry(), before);
}

#[test]
fn account_adopt_refuses_an_account_that_has_its_own_profile() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    assert_ok(&env.add_account("work", "work@example.com"));
    sign_in_default_as(&env, "work@example.com", "acc-work");
    let before = env.registry();

    let output = env.run(&["account", "adopt"]);

    assert!(!output.status.success());
    assert!(
        text(&output).contains("work@example.com"),
        "{}",
        text(&output)
    );
    assert_eq!(env.registry(), before);
}

#[test]
fn account_adopt_needs_something_signed_in_on_the_default_profile() {
    let env = Env::new();
    env.sign_in_default("main@example.com");
    assert_ok(&env.run(&["init"]));
    fs::remove_file(env.home().join(".claude/.credentials.json")).unwrap();
    let before = env.registry();

    let output = env.run(&["account", "adopt"]);

    assert!(!output.status.success());
    assert!(
        text(&output).contains("claude auth login"),
        "{}",
        text(&output)
    );
    assert_eq!(env.registry(), before);
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[allow(dead_code)]
fn unused(_: &Path) {}
