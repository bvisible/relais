//! Index of the conversations stored in `~/.claude/projects`.
//!
//! Every account profile links `projects` back to `~/.claude`, so this one
//! index holds the conversations of all accounts, started in the terminal or in
//! the desktop app. Sessions launched by scripts through the SDK (`claude -p`)
//! are left out, as the desktop app does.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::fsutil;

/// Entrypoints of sessions started by scripts rather than by a person.
pub const SDK_ENTRYPOINTS: &[&str] = &["sdk-cli", "sdk-ts", "sdk-py"];

/// Longest title kept, in characters.
pub const TITLE_MAX_CHARS: usize = 80;

const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub session_id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub title: Option<String>,
    pub entrypoint: Option<String>,
    /// Unix seconds of the last write to the transcript.
    pub last_activity: i64,
}

impl Conversation {
    pub fn is_from_sdk(&self) -> bool {
        self.entrypoint
            .as_deref()
            .is_some_and(|e| SDK_ENTRYPOINTS.contains(&e))
    }
}

/// Reads what the index needs from one transcript, looking only at its first
/// and last kilobytes so large sessions stay cheap.
pub fn parse_transcript(path: &Path) -> Result<Conversation> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let (head, tail) = read_head_and_tail(path)?;
    let mut conversation = Conversation {
        session_id,
        path: path.to_path_buf(),
        cwd: None,
        git_branch: None,
        title: None,
        entrypoint: None,
        last_activity: fsutil::modified_secs(path),
    };

    let mut first_message = None;
    for value in head
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if conversation.entrypoint.is_none() {
            conversation.entrypoint = string_field(&value, "entrypoint");
        }
        if conversation.cwd.is_none() {
            conversation.cwd = string_field(&value, "cwd");
        }
        if conversation.git_branch.is_none() {
            conversation.git_branch = string_field(&value, "gitBranch");
        }
        if first_message.is_none() {
            first_message = user_text(&value);
        }
    }

    let mut custom_title = None;
    let mut ai_title = None;
    for value in head
        .lines()
        .chain(tail.lines())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match value.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                custom_title = string_field(&value, "customTitle").or(custom_title)
            }
            Some("ai-title") => ai_title = string_field(&value, "aiTitle").or(ai_title),
            _ => {}
        }
    }
    conversation.title = custom_title
        .or(ai_title)
        .or(first_message)
        .map(|title| shorten(&title));
    Ok(conversation)
}

/// The whole file when small, otherwise its first and last kilobytes.
/// Lines cut at a boundary fail to parse and are ignored.
fn read_head_and_tail(path: &Path) -> Result<(String, String)> {
    let mut file = fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = file.metadata().map_err(|e| Error::io(path, e))?.len();
    if len <= HEAD_BYTES + TAIL_BYTES {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| Error::io(path, e))?;
        return Ok((String::from_utf8_lossy(&bytes).into_owned(), String::new()));
    }
    let mut head = vec![0; HEAD_BYTES as usize];
    file.read_exact(&mut head).map_err(|e| Error::io(path, e))?;
    file.seek(SeekFrom::Start(len - TAIL_BYTES))
        .map_err(|e| Error::io(path, e))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)
        .map_err(|e| Error::io(path, e))?;
    Ok((
        String::from_utf8_lossy(&head).into_owned(),
        String::from_utf8_lossy(&tail).into_owned(),
    ))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// The text a person typed, or `None` for harness messages and command wrappers.
fn user_text(value: &Value) -> Option<String> {
    let is_user = value.get("type").and_then(Value::as_str) == Some("user");
    let is_meta = value
        .get("isMeta")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_sidechain = value
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !is_user || is_meta || is_sidechain {
        return None;
    }
    let content = value.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)?
            .to_string(),
        _ => return None,
    };
    let trimmed = text.trim_start();
    let is_wrapper = [
        "<command-",
        "<local-command-",
        "<system-reminder>",
        "Caveat:",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix));
    if is_wrapper {
        return None;
    }
    let cleaned = strip_tags(&text);
    (!cleaned.trim().is_empty()).then_some(cleaned)
}

/// Removes markup such as `<pasted_content id="…">` that wraps what was typed.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let starts_tag = c == '<'
            && chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphabetic() || *next == '/');
        if starts_tag {
            for skipped in chars.by_ref() {
                if skipped == '>' {
                    break;
                }
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapses whitespace and cuts the title to [`TITLE_MAX_CHARS`].
fn shorten(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= TITLE_MAX_CHARS {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(TITLE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The most recent conversations started by a person, newest first.
pub fn list_conversations(projects_dir: &Path, limit: usize) -> Result<Vec<Conversation>> {
    let mut files = transcript_files(projects_dir)?;
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    let mut conversations = Vec::new();
    for (_, path) in files {
        if conversations.len() >= limit {
            break;
        }
        if let Ok(conversation) = parse_transcript(&path) {
            if !conversation.is_from_sdk() {
                conversations.push(conversation);
            }
        }
    }
    Ok(conversations)
}

/// Top-level `*.jsonl` files of every project directory, with their modification time.
fn transcript_files(projects_dir: &Path) -> Result<Vec<(i64, PathBuf)>> {
    let projects = match fs::read_dir(projects_dir) {
        Ok(projects) => projects,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(projects_dir, e)),
    };
    let mut files = Vec::new();
    for project in projects
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
    {
        let Ok(entries) = fs::read_dir(&project) else {
            continue;
        };
        for path in entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
        {
            if path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file() {
                files.push((fsutil::modified_secs(&path), path));
            }
        }
    }
    Ok(files)
}

/// Finds the transcript of `session_id` in any project directory.
pub fn find_conversation(projects_dir: &Path, session_id: &str) -> Result<Option<Conversation>> {
    let projects = match fs::read_dir(projects_dir) {
        Ok(projects) => projects,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(projects_dir, e)),
    };
    for project in projects
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
    {
        let candidate = project.join(format!("{session_id}.jsonl"));
        if candidate.is_file() {
            return parse_transcript(&candidate).map(Some);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_transcript(dir: &Path, project: &str, session: &str, lines: &[Value]) -> PathBuf {
        let project_dir = dir.join(project);
        fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join(format!("{session}.jsonl"));
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(&path, body).unwrap();
        path
    }

    fn user(text: Value, entrypoint: &str) -> Value {
        json!({"type": "user", "sessionId": "x", "cwd": "/work/app", "gitBranch": "main",
               "entrypoint": entrypoint, "message": {"role": "user", "content": text}})
    }

    #[test]
    fn custom_title_wins_over_ai_title_and_first_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "-work-app",
            "s1",
            &[
                user(json!("Fix the login form"), "cli"),
                json!({"type": "ai-title", "aiTitle": "Login form fix", "sessionId": "s1"}),
                json!({"type": "custom-title", "customTitle": "Login, take two", "sessionId": "s1"}),
                json!({"type": "ai-title", "aiTitle": "Later AI title", "sessionId": "s1"}),
            ],
        );

        let conversation = parse_transcript(&path).unwrap();

        assert_eq!(conversation.title.as_deref(), Some("Login, take two"));
        assert_eq!(conversation.session_id, "s1");
        assert_eq!(conversation.cwd.as_deref(), Some("/work/app"));
        assert_eq!(conversation.git_branch.as_deref(), Some("main"));
        assert_eq!(conversation.entrypoint.as_deref(), Some("cli"));
    }

    #[test]
    fn latest_ai_title_is_used_without_custom_title() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "-work-app",
            "s1",
            &[
                user(json!("Fix the login form"), "cli"),
                json!({"type": "ai-title", "aiTitle": "First idea", "sessionId": "s1"}),
                json!({"type": "ai-title", "aiTitle": "Login form fix", "sessionId": "s1"}),
            ],
        );

        assert_eq!(
            parse_transcript(&path).unwrap().title.as_deref(),
            Some("Login form fix")
        );
    }

    #[test]
    fn first_real_user_message_is_the_fallback_title() {
        let dir = tempfile::tempdir().unwrap();
        let mut meta = user(json!("Caveat: injected by the harness"), "cli");
        meta["isMeta"] = json!(true);
        let path = write_transcript(
            dir.path(),
            "-work-app",
            "s1",
            &[
                meta,
                user(json!("<command-name>/clear</command-name>"), "cli"),
                user(
                    json!([{"type": "text", "text": "Add a dark mode\nwith a toggle"}]),
                    "cli",
                ),
            ],
        );

        assert_eq!(
            parse_transcript(&path).unwrap().title.as_deref(),
            Some("Add a dark mode with a toggle")
        );
    }

    #[test]
    fn long_titles_are_cut() {
        let dir = tempfile::tempdir().unwrap();
        let long = "word ".repeat(40);
        let path = write_transcript(dir.path(), "-work-app", "s1", &[user(json!(long), "cli")]);

        let title = parse_transcript(&path).unwrap().title.unwrap();

        assert_eq!(title.chars().count(), TITLE_MAX_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn list_skips_sdk_sessions_and_sorts_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let old = write_transcript(
            dir.path(),
            "-work-app",
            "old",
            &[user(json!("old one"), "cli")],
        );
        write_transcript(
            dir.path(),
            "-work-app",
            "script",
            &[user(json!("from a script"), "sdk-cli")],
        );
        let new = write_transcript(
            dir.path(),
            "-work-other",
            "new",
            &[user(json!("new one"), "claude-desktop")],
        );
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let _ = new;

        let list = list_conversations(dir.path(), 10).unwrap();

        let ids: Vec<&str> = list.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "old"]);
        assert_eq!(list_conversations(dir.path(), 1).unwrap().len(), 1);
    }

    #[test]
    fn a_session_is_found_by_its_id_in_any_project() {
        let dir = tempfile::tempdir().unwrap();
        write_transcript(dir.path(), "-work-app", "s1", &[user(json!("one"), "cli")]);
        write_transcript(
            dir.path(),
            "-work-other",
            "s2",
            &[user(json!("two"), "cli")],
        );

        let found = find_conversation(dir.path(), "s2").unwrap().unwrap();

        assert_eq!(found.title.as_deref(), Some("two"));
        assert!(find_conversation(dir.path(), "zz").unwrap().is_none());
    }

    #[test]
    fn missing_projects_directory_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_conversations(&dir.path().join("absent"), 5)
            .unwrap()
            .is_empty());
    }
}
