//! Asks Claude Code for an account's limits, the way a person would.
//!
//! `claude -p /usage` is not safe to rely on: depending on the version, a
//! command name in print mode can reach the model as a prompt, and that only
//! shows once it has been sent. Here the official client runs in a private
//! pseudo-terminal in safe mode, with no tool, hook, MCP server or setting of
//! the user's, keeps no transcript, and Enter is pressed only after the
//! official `/usage` command is seen selected. No prompt goes to the model.
//! Authentication stays Claude Code's own: no value is read or logged here.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::layout::Layout;

const REFUSED: &str = "usage-probe:";

fn refused(code: &str) -> Error {
    Error::Command(format!("{REFUSED}{code}"))
}

/// Whether a measure stopped on a screen the user answers once: the first-run
/// questions, the folder's trust. The probe never answers for them, and a
/// missing sign-in is a matter of its own.
pub fn awaits_the_user(reason: &str) -> bool {
    reason.starts_with("setup-") && reason != "setup-login"
}

/// What stopped a measure: a code of the probe, or the error as it reads.
pub fn reason(error: &Error) -> String {
    let text = error.to_string();
    match text.split_once(REFUSED) {
        Some((_, code)) => code.to_string(),
        None => text,
    }
}

/// Never inherit provider credentials, nested-session state or startup injection.
/// Values are neither inspected nor logged. Claude owns authentication itself.
pub fn sanitized_command(binary: &Path, profile: Option<&Path>) -> Command {
    let mut command = Command::new(binary);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("ANTHROPIC_")
            || name.starts_with("CLAUDE_")
            || name.starts_with("DYLD_")
            || matches!(
                name.as_ref(),
                "CLAUDECODE"
                    | "AWS_ACCESS_KEY_ID"
                    | "AWS_SECRET_ACCESS_KEY"
                    | "AWS_SESSION_TOKEN"
                    | "AWS_BEARER_TOKEN_BEDROCK"
                    | "GOOGLE_APPLICATION_CREDENTIALS"
                    | "GOOGLE_API_KEY"
                    | "GEMINI_API_KEY"
                    | "BASH_ENV"
                    | "ENV"
                    | "NODE_OPTIONS"
                    | "LD_PRELOAD"
            )
        {
            command.env_remove(key);
        }
    }
    if let Some(profile) = profile {
        command.env("CLAUDE_CONFIG_DIR", profile);
    }
    command
        .env("DISABLE_AUTOUPDATER", "1")
        // Official interactive equivalent of print-only --no-session-persistence.
        // Reapply after stripping inherited CLAUDE_* values, so a caller cannot
        // enable transcript persistence for this child through its environment.
        .env("CLAUDE_CODE_SKIP_PROMPT_HISTORY", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("DISABLE_ERROR_REPORTING", "1")
        .env("TERM", "xterm-256color")
        .env("LC_ALL", "C");
    command
}

fn unsafe_policy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
            // A selected agent can prefix a positional /usage before slash
            // dispatch. Unknown shapes must not be treated as an empty choice.
            let agent_override = match key.as_str() {
                "agent" | "initialPrompt" => !value.is_null() && value.as_str() != Some(""),
                "agents" => !value.is_null() && value.as_object().is_none_or(|v| !v.is_empty()),
                _ => false,
            };
            let nonempty = !value.is_null()
                && value.as_object().is_none_or(|v| !v.is_empty())
                && value.as_array().is_none_or(|v| !v.is_empty())
                && value.as_str() != Some("");
            agent_override
                || (matches!(
                    key.as_str(),
                    "hooks"
                        | "statusLine"
                        | "mcpServers"
                        | "env"
                        | "apiKeyHelper"
                        | "otelHeadersHelper"
                        | "awsAuthRefresh"
                        | "awsCredentialExport"
                        | "anthropicBaseUrl"
                        | "baseUrl"
                ) && nonempty)
                || unsafe_policy(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(unsafe_policy),
        _ => false,
    }
}

/// Managed settings remain active even when all user/project settings are
/// excluded. Inspect policy metadata only and reject executable/provider or
/// agent overrides before any official authentication subprocess is started.
pub fn audit_policy(layout: &Layout, profile: Option<&Path>) -> Result<()> {
    let system = Path::new("/Library/Application Support/ClaudeCode");
    let mut paths = vec![
        system.join("managed-settings.json"),
        system.join("managed-mcp.json"),
        profile
            .unwrap_or(&layout.claude_dir)
            .join("remote-settings.json"),
    ];
    let fragments = system.join("managed-settings.d");
    if fragments.exists() {
        for entry in fs::read_dir(&fragments).map_err(|_| refused("managed-policy-unreadable"))? {
            let path = entry
                .map_err(|_| refused("managed-policy-unreadable"))?
                .path();
            if path.extension().is_some_and(|e| e == "json") {
                paths.push(path);
            }
        }
    }
    let managed = Path::new("/Library/Managed Preferences");
    paths.push(managed.join("com.anthropic.claudecode.plist"));
    if let Some(user) = layout.home.file_name() {
        paths.push(managed.join(user).join("com.anthropic.claudecode.plist"));
    }
    audit_policy_paths(&paths)
}

fn audit_policy_paths(paths: &[PathBuf]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    for path in paths {
        if Instant::now() >= deadline {
            return Err(refused("managed-policy-unreadable"));
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(refused("managed-policy-unreadable")),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024
        {
            return Err(refused("managed-policy-unreadable"));
        }
        let bytes = if path.extension().is_some_and(|e| e == "plist") {
            Command::new("/usr/bin/plutil")
                .args(["-convert", "json", "-o", "-"])
                .arg(path)
                .output()
                .map_err(|_| refused("managed-policy-unreadable"))?
                .stdout
        } else {
            fs::read(path).map_err(|_| refused("managed-policy-unreadable"))?
        };
        let value =
            serde_json::from_slice(&bytes).map_err(|_| refused("managed-policy-unreadable"))?;
        if unsafe_policy(&value) {
            return Err(refused("managed-policy-unsupported"));
        }
    }
    Ok(())
}

fn setup_reason(text: &str) -> Option<&'static str> {
    let text = text.to_ascii_lowercase();
    let reasons: &[(&str, &[&str])] = &[
        (
            "setup-trust",
            &[
                "do you trust",
                "accessing workspace",
                "yes, i trust",
                "yes i trust",
                "quick safety check:",
                "ready to code here?",
            ],
        ),
        (
            "setup-login",
            &[
                "please log in",
                "please sign in",
                "login method",
                "select login",
                "browser to sign in",
            ],
        ),
        (
            "setup-theme",
            &["choose the text style", "select a theme", "choose a theme"],
        ),
        ("setup-terms", &["accept the terms"]),
        ("setup-continue", &["press enter to continue"]),
        // Keep the former generic permission guard, without guessing its cause.
        ("setup-unknown", &["permission required:"]),
    ];
    reasons.iter().find_map(|(reason, needles)| {
        needles
            .iter()
            .any(|needle| text.contains(needle))
            .then_some(*reason)
    })
}

fn builtin_usage_selected(text: &str) -> bool {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    !normalized.to_ascii_lowercase().contains("unknown command")
        && [
            "/usage Show plan usage limits",
            "/usage Show session cost, plan usage, and activity stats",
            "/usage Show session cost and plan usage limits",
            "/usage Show session cost, plan usage, and what's contributing to your limits",
            "/usage Show session cost and plan usage",
        ]
        .iter()
        .any(|description| normalized.contains(description))
}

fn usage_panel_header(text: &str) -> bool {
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    normalized.contains("settings: usage")
        || normalized.contains("settings status config usage stats")
}

/// Minimal terminal renderer: inspect the current screen rather than stale
/// concatenated frames, and discard escape sequences without executing them.
struct Screen {
    rows: Vec<Vec<char>>,
    x: usize,
    y: usize,
    escape: Vec<u8>,
    osc: bool,
    osc_escape: bool,
    utf8: Vec<u8>,
    position_requested: bool,
}
impl Screen {
    fn new() -> Self {
        Self {
            rows: vec![vec![' '; 160]; 100],
            x: 0,
            y: 0,
            escape: Vec::new(),
            osc: false,
            osc_escape: false,
            utf8: Vec::new(),
            position_requested: false,
        }
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.utf8.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.utf8) {
            Ok(_) => self.utf8.len(),
            Err(e) => e.valid_up_to(),
        };
        let text = String::from_utf8_lossy(&self.utf8[..valid]).into_owned();
        self.utf8.drain(..valid);
        if self.utf8.len() > 4 {
            self.utf8.clear();
        }
        for c in text.chars() {
            if self.osc {
                if c == '\x07' || (self.osc_escape && c == '\\') {
                    self.osc = false;
                    self.osc_escape = false;
                } else {
                    self.osc_escape = c == '\x1b';
                }
                continue;
            }
            if !self.escape.is_empty() {
                self.escape.extend(c.to_string().as_bytes());
                if self.escape == b"\x1b]" {
                    self.osc = true;
                    self.escape.clear();
                    continue;
                }
                if self.escape.len() == 2 && c != '[' {
                    self.escape.clear();
                    continue;
                }
                if self.escape.len() > 2 && c.is_ascii() && ('@'..='~').contains(&c) {
                    let sequence = String::from_utf8_lossy(&self.escape[2..self.escape.len() - 1]);
                    let p: Vec<usize> = sequence
                        .split(';')
                        .map(|s| s.parse().unwrap_or(0))
                        .collect();
                    let n = p.first().copied().unwrap_or(0);
                    let n1 = n.max(1);
                    match c {
                        'H' | 'f' => {
                            self.y = n1.saturating_sub(1).min(99);
                            self.x = p
                                .get(1)
                                .copied()
                                .unwrap_or(1)
                                .max(1)
                                .saturating_sub(1)
                                .min(159);
                        }
                        'A' => self.y = self.y.saturating_sub(n1),
                        'B' => self.y = (self.y + n1).min(99),
                        'C' => self.x = (self.x + n1).min(159),
                        'D' => self.x = self.x.saturating_sub(n1),
                        'G' => self.x = n1.saturating_sub(1).min(159),
                        'd' => self.y = n1.saturating_sub(1).min(99),
                        'E' => {
                            self.y = (self.y + n1).min(99);
                            self.x = 0;
                        }
                        'F' => {
                            self.y = self.y.saturating_sub(n1);
                            self.x = 0;
                        }
                        'n' if n == 6 => self.position_requested = true,
                        'J' if n == 2 || n == 3 => {
                            self.rows = vec![vec![' '; 160]; 100];
                        }
                        'J' => {
                            for y in 0..100 {
                                for x in 0..160 {
                                    if (n == 0 && (y > self.y || (y == self.y && x >= self.x)))
                                        || (n == 1 && (y < self.y || (y == self.y && x <= self.x)))
                                    {
                                        self.rows[y][x] = ' ';
                                    }
                                }
                            }
                        }
                        'K' => {
                            let range = if n == 1 {
                                0..self.x + 1
                            } else if n == 2 {
                                0..160
                            } else {
                                self.x..160
                            };
                            for x in range {
                                self.rows[self.y][x] = ' ';
                            }
                        }
                        _ => {}
                    }
                    self.escape.clear();
                }
                if self.escape.len() > 80 {
                    self.escape.clear();
                }
                continue;
            }
            match c {
                '\x1b' => self.escape.push(27),
                '\r' => self.x = 0,
                '\n' => {
                    self.y += 1;
                    if self.y >= 100 {
                        self.rows.remove(0);
                        self.rows.push(vec![' '; 160]);
                        self.y = 99;
                    }
                }
                '\x08' => self.x = self.x.saturating_sub(1),
                c if !c.is_control() => {
                    self.rows[self.y][self.x] = c;
                    self.x = (self.x + 1).min(159);
                }
                _ => {}
            }
        }
    }
    fn text(&self) -> String {
        self.rows
            .iter()
            .map(|r| r.iter().collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// How long a client asked to stop gets to finish what it is saving.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long a killed client is waited for.
const KILL_WAIT: Duration = Duration::from_secs(2);

struct OwnedChild(Child);
impl OwnedChild {
    fn exited(&self) -> std::io::Result<bool> {
        Ok(self.exit_success()?.is_some())
    }

    /// Asks the client to stop, and lets it. A renewed sign-in is saved after
    /// the server has replaced the old one: a client killed in between loses
    /// both, and the account is signed out. So only the client is signaled, not
    /// what it started to save it, and its terminal stays open and read: closing
    /// it hangs up all of them, and a session leader does not finish exiting
    /// until its output is read.
    fn stop(&self, terminal: &mut File) {
        // SAFETY: the child is not reaped before Drop, so this pid is still its own.
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + STOP_GRACE;
        let mut bytes = [0; 16384];
        while Instant::now() < deadline && matches!(self.exited(), Ok(false)) {
            let _ = terminal.read(&mut bytes);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn ended_within(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            match self.exited() {
                Ok(true) => return true,
                Ok(false) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => return false,
            }
        }
    }

    fn exit_success(&self) -> std::io::Result<Option<bool>> {
        // WNOWAIT deliberately keeps the child unreaped. Its pid/group cannot
        // be recycled before Drop signals the group and then reaps it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::waitid(
                libc::P_PID,
                self.0.id() as _,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if status < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if info.si_signo == 0 {
            return Ok(None);
        }
        // SAFETY: waitid initialized this siginfo for our retained child pid.
        Ok(Some(
            info.si_code == libc::CLD_EXITED && unsafe { info.si_status() == 0 },
        ))
    }
}
impl Drop for OwnedChild {
    /// Kills what is left of the client. One that never ends is left behind,
    /// so that a stuck measure cannot stop all the others.
    fn drop(&mut self) {
        // SAFETY: setsid below creates a new process group whose id is this
        // newly spawned child's pid. Never signal any existing Claude session.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        if self.ended_within(KILL_WAIT) {
            let _ = self.0.wait();
        }
    }
}

/// Opens Claude Code in a private terminal, with nothing of the user's loaded,
/// types `/usage` and presses Enter only once the official command is on
/// screen, then returns the panel. A login, trust or setup screen stops it
/// unanswered, and nothing is ever sent to the model.
pub fn report(
    binary: &Path,
    profile: Option<&Path>,
    cwd: &Path,
    timeout: Duration,
) -> Result<String> {
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: 100,
        ws_col: 160,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes two owned descriptors and reads the valid winsize.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } != 0
    {
        return Err(refused("pty-unavailable"));
    }
    // SAFETY: descriptors were freshly returned by openpty and are uniquely owned.
    let terminal = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut command = sanitized_command(binary, profile);
    command.args(["--safe-mode","--permission-mode","manual","--tools","","--strict-mcp-config","--mcp-config",r#"{"mcpServers":{}}"#,"--no-chrome","--setting-sources","","--settings",r#"{"remoteControlAtStartup":false,"disableAllHooks":true,"autoContinueAtUsageLimit":false}"#,"--ax-screen-reader"])
        .current_dir(cwd).stdin(Stdio::from(slave.try_clone().map_err(|_|refused("pty-unavailable"))?)).stdout(Stdio::from(slave.try_clone().map_err(|_|refused("pty-unavailable"))?)).stderr(Stdio::from(slave));
    // SAFETY: both calls are async-signal-safe and run only in this child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = OwnedChild(command.spawn().map_err(|_| refused("start-failed"))?);
    // Bound after the child, so it closes before the child is killed and waited for.
    let mut terminal = terminal;
    // SAFETY: master is a live descriptor owned by terminal, fcntl changes its flags only.
    unsafe {
        libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK);
    }
    let panel = read_panel(&child, &mut terminal, timeout);
    child.stop(&mut terminal);
    panel
}

/// Why the panel holds no figures to take: none came, or those shown are not
/// new. Anthropic sometimes holds back requests to the usage page for a
/// while; Claude Code then shows the last figures it had, which must not be
/// stored as if they had just been read.
fn panel_refusal(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| lower.contains(phrase));
    if says(&[
        "limiting requests to the usage page",
        "showing last-known usage",
        "could not refresh usage",
    ]) {
        Some("usage-rate-limited")
    } else if says(&[
        "failed to load usage",
        "usage data is unavailable",
        "unknown command",
        "invalid api key",
        "authentication error",
    ]) {
        Some("usage-unavailable")
    } else {
        None
    }
}

fn read_panel(child: &OwnedChild, terminal: &mut File, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(45));
    let mut screen = Screen::new();
    let mut phase = 0;
    let mut stable = Instant::now();
    let mut last = String::new();
    let mut total = 0usize;
    while Instant::now() < deadline {
        let mut bytes = [0; 16384];
        match terminal.read(&mut bytes) {
            Ok(n) if n > 0 => {
                total += n;
                if total > 2 * 1024 * 1024 {
                    return Err(refused("screen-too-large"));
                }
                screen.feed(&bytes[..n]);
                if screen.position_requested {
                    screen.position_requested = false;
                    terminal
                        .write_all(b"\x1b[1;1R")
                        .map_err(|_| refused("pty-failed"))?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(_) => {}
            Err(_) => return Err(refused("pty-failed")),
        }
        let text = screen.text();
        if text != last {
            stable = Instant::now();
            last = text.clone();
        }
        if let Some(reason) = setup_reason(&text) {
            return Err(refused(reason));
        }
        let lower = text.to_ascii_lowercase();
        if let Some(reason) = panel_refusal(&text) {
            return Err(refused(reason));
        }
        if phase == 0
            && stable.elapsed() >= Duration::from_millis(200)
            && (lower.contains("claude code") || lower.contains("claudecode"))
            && text
                .lines()
                .any(|line| matches!(line.trim(), "❯" | ">" | "$" | "$$"))
        {
            terminal
                .write_all(b"/usage")
                .map_err(|_| refused("pty-failed"))?;
            phase = 1;
        } else if phase == 1
            && builtin_usage_selected(&text)
            && stable.elapsed() >= Duration::from_millis(200)
        {
            terminal
                .write_all(b"\r")
                .map_err(|_| refused("pty-failed"))?;
            phase = 2;
        } else if phase == 2
            && stable.elapsed() >= Duration::from_millis(250)
            // Some plans no longer have the 5-hour limit: only the week is sure.
            && text.contains("Current week")
            && usage_panel_header(&text)
            && !lower.contains("loading usage")
            && crate::usage::parse_usage_report(&text, chrono::Local::now()).is_some()
        {
            return Ok(text);
        }
        if child.exited().map_err(|_| refused("process-failed"))? {
            return Err(refused("unsupported-cli"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(refused(if phase == 0 {
        "ready-timeout"
    } else if phase == 1 {
        "builtin-not-confirmed"
    } else {
        "usage-timeout"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_name_without_the_official_description_never_authorizes_enter() {
        assert!(!builtin_usage_selected("❯ /usage"));
        assert!(!builtin_usage_selected("Unknown command: /usage"));
        assert!(!builtin_usage_selected("/usage Show your quota skill"));
        assert!(!builtin_usage_selected("/usage Show session cost"));
        assert!(!builtin_usage_selected(
            "/usage A custom command for plan usage and limits"
        ));
        assert!(builtin_usage_selected(
            "❯ /usage\n  /usage  Show plan usage limits"
        ));
    }

    #[test]
    fn figures_claude_code_could_not_refresh_are_never_taken_for_new_ones() {
        for (text, reason) in [
            (
                "Claude is temporarily limiting requests to the usage page (not your usage limit). Try again in a moment.",
                "usage-rate-limited",
            ),
            (
                "Current week (all models)\n100% used\nShowing last-known usage (could not refresh)",
                "usage-rate-limited",
            ),
            ("Could not refresh usage data", "usage-rate-limited"),
            ("Failed to load usage data", "usage-unavailable"),
            ("Unknown command: /usage", "usage-unavailable"),
        ] {
            assert_eq!(panel_refusal(text), Some(reason), "{text}");
        }
        assert_eq!(
            panel_refusal("Current week (all models)\n5% used\nPartial usage data"),
            None
        );
    }

    #[test]
    fn setup_and_authentication_prompts_stop_the_probe() {
        for (text, reason) in [
            ("Do you trust the files in this folder?", "setup-trust"),
            ("Quick safety check:", "setup-trust"),
            ("Please log in", "setup-login"),
            ("Select a theme", "setup-theme"),
            ("Accept the terms. Press enter to continue", "setup-terms"),
            ("Press enter to continue", "setup-continue"),
        ] {
            assert_eq!(setup_reason(text), Some(reason), "{text}");
        }
        assert_eq!(setup_reason("Claude Code\n$"), None);
    }

    #[test]
    fn only_executable_provider_or_agent_policy_is_refused() {
        assert!(!unsafe_policy(
            &serde_json::json!({"permissions":{"defaultMode":"default"},"language":"French"})
        ));
        assert!(unsafe_policy(
            &serde_json::json!({"hooks":{"SessionStart":[{"command":"touch sentinel"}]}})
        ));
        assert!(unsafe_policy(
            &serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}})
        ));
        assert!(unsafe_policy(&serde_json::json!({"agent":"fixture"})));
        assert!(!unsafe_policy(
            &serde_json::json!({"mcpServers":{},"hooks":{},"agents":{}})
        ));
    }

    /// A stand-in for Claude Code: a prompt, the command palette once
    /// `/usage` is typed, the panel once Enter is pressed. It records whether
    /// Enter came, and what it was given.
    fn fake_client(dir: &Path, panel: &str, palette: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-claude");
        let script = format!(
            r#"#!/usr/bin/python3
import os,sys,time,tty
root={root:?}
open(os.path.join(root,'env'),'w').write(os.environ.get('CLAUDE_CODE_SKIP_PROMPT_HISTORY','unset'))
open(os.path.join(root,'args'),'w').write(' '.join(sys.argv[1:]))
tty.setraw(0)
sys.stdout.write('Claude Code\r\n❯\r\n');sys.stdout.flush()
typed=b''
while not typed.endswith(b'/usage'):
    typed+=os.read(0,1)
sys.stdout.write('{palette}\r\n');sys.stdout.flush()
os.read(0,1)
open(os.path.join(root,'entered'),'w').write('yes')
sys.stdout.write('\x1b[2J\x1b[H{panel}');sys.stdout.flush()
time.sleep(20)
"#,
            root = dir.to_string_lossy(),
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    const OFFICIAL: &str = "/usage Show plan usage limits";

    #[test]
    fn a_plan_without_the_five_hour_limit_is_measured() {
        let dir = tempfile::tempdir().unwrap();
        let panel = r"Settings: Usage\r\nCurrent week (all models): 41% used\r\n";
        let binary = fake_client(dir.path(), panel, OFFICIAL);

        let text = report(&binary, None, dir.path(), Duration::from_secs(10)).unwrap();

        let reading = crate::usage::parse_usage_report(&text, chrono::Local::now()).unwrap();
        assert!(reading.five_hour.is_none());
        assert_eq!(reading.seven_day.unwrap().used_percentage, 41.0);
        assert_eq!(fs::read_to_string(dir.path().join("env")).unwrap(), "1");
        let args = fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.contains("--safe-mode"), "{args}");
    }

    #[test]
    fn both_limits_are_read_when_the_plan_has_them() {
        let dir = tempfile::tempdir().unwrap();
        let panel = r"Settings: Usage\r\nCurrent session: 12% used\r\nCurrent week (all models): 41% used\r\n";
        let binary = fake_client(dir.path(), panel, OFFICIAL);

        let text = report(&binary, None, dir.path(), Duration::from_secs(10)).unwrap();

        let reading = crate::usage::parse_usage_report(&text, chrono::Local::now()).unwrap();
        assert_eq!(reading.five_hour.unwrap().used_percentage, 12.0);
    }

    #[test]
    fn enter_is_never_pressed_on_anything_but_the_official_command() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_client(dir.path(), "", "/usage Show your quota skill");

        assert!(report(&binary, None, dir.path(), Duration::from_secs(2)).is_err());

        assert!(!dir.path().join("entered").exists());
    }

    fn client(dir: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("client");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    /// What a stand-in prints once it is ready: the measure ends there, however
    /// long the interpreter took to start.
    const READY: &str = "print('authentication error', flush=True)";

    /// Runs `report` on its own thread: a measure that never returns fails
    /// the test instead of hanging the suite.
    fn measure_within(limit: Duration, binary: PathBuf, cwd: PathBuf) -> Result<String> {
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(report(&binary, None, &cwd, limit));
        });
        result
            .recv_timeout(limit + Duration::from_secs(30))
            .expect("the measure never returned")
    }

    #[test]
    fn a_client_flooding_its_terminal_never_blocks_the_measure() {
        let dir = tempfile::tempdir().unwrap();
        let binary = client(dir.path(), "#!/bin/sh\nexec yes 'Claude Code is busy'\n");

        let result = measure_within(Duration::from_secs(1), binary, dir.path().into());

        assert!(result.is_err());
    }

    #[test]
    fn a_stopped_client_gets_time_to_finish_what_it_is_saving() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("saved");
        let script = format!(
            r#"#!/usr/bin/python3
import signal, time
def finish(*_):
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(0.5)
    open({saved:?}, 'w').write('yes')
    raise SystemExit
signal.signal(signal.SIGHUP, finish)
signal.signal(signal.SIGTERM, finish)
{READY}
time.sleep(30)
"#
        );
        let binary = client(dir.path(), &script);

        let result = measure_within(Duration::from_secs(30), binary, dir.path().into());

        assert!(result.is_err());
        assert!(saved.exists(), "the client was killed before it finished");
    }

    #[test]
    fn a_stopped_client_keeps_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("saved");
        let script = format!(
            r#"#!/usr/bin/python3
import signal, subprocess, sys
writer = subprocess.Popen(['/bin/sh', '-c', 'sleep 2; echo yes > ' + {saved:?}])
def finish(*_):
    writer.wait()
    sys.exit(0)
signal.signal(signal.SIGTERM, finish)
{READY}
signal.pause()
"#
        );
        let binary = client(dir.path(), &script);

        let result = measure_within(Duration::from_secs(30), binary, dir.path().into());

        assert!(result.is_err());
        assert!(saved.exists(), "what the client was writing was killed");
    }

    #[test]
    fn a_client_refusing_to_stop_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!(
            r#"#!/usr/bin/python3
import os, signal, time
signal.signal(signal.SIGHUP, signal.SIG_IGN)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
open({pid_file:?}, 'w').write(str(os.getpid()))
{READY}
time.sleep(60)
"#
        );
        let binary = client(dir.path(), &script);

        let result = measure_within(Duration::from_secs(30), binary, dir.path().into());

        assert!(result.is_err());
        let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "the client is still alive"
        );
    }

    #[test]
    fn inherited_credentials_never_reach_the_client() {
        let command = sanitized_command(Path::new("/bin/true"), Some(Path::new("/profile")));
        let envs: Vec<_> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_os_string()),
                )
            })
            .collect();
        assert!(envs.iter().any(|(k, v)| k == "CLAUDE_CONFIG_DIR"
            && v.as_deref() == Some(std::ffi::OsStr::new("/profile"))));
        assert!(envs
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_SKIP_PROMPT_HISTORY"
                && v.as_deref() == Some(std::ffi::OsStr::new("1"))));
    }
}
