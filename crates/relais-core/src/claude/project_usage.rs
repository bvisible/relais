//! What each project consumed over the last days, from the transcripts on this
//! Mac. Transcripts weigh gigabytes, so each is read once, then only what was
//! added to it: `~/.relais/project-usage.json` remembers where reading stopped.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::fsutil;

const DAY: i64 = 86_400;
/// How far back the figures go.
pub const DAYS: i64 = 7;

/// One transcript, as far as it was read.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Read {
    offset: u64,
    /// The last message counted: a message comes back on several lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_message: Option<String>,
    /// Tokens by project and Unix day.
    tokens: BTreeMap<String, BTreeMap<i64, u64>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    transcripts: HashMap<PathBuf, Read>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUsage {
    pub project: String,
    pub tokens: u64,
}

/// The project a working folder belongs to: its last name, past any worktree
/// an agent made under the project.
fn project_of(cwd: &str) -> String {
    let root = [
        "/.claude/worktrees/",
        "/.codex/worktrees/",
        "/.claude-worktrees/",
    ]
    .iter()
    .find_map(|marker| cwd.split_once(marker).map(|(root, _)| root))
    .unwrap_or(cwd);
    Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string())
}

/// The tokens a line adds, its project, day and message id. What came from the
/// cache is left out: it costs far less, and would drown the rest.
fn line_tokens(line: &str) -> Option<(String, i64, u64, Option<String>)> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let tokens =
        count("input_tokens") + count("output_tokens") + count("cache_creation_input_tokens");
    let cwd = value.get("cwd")?.as_str()?;
    let at = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?).ok()?;
    let id = message.get("id").and_then(Value::as_str).map(String::from);
    Some((project_of(cwd), at.timestamp().div_euclid(DAY), tokens, id))
}

fn read_from(path: &Path, read: &mut Read) -> std::io::Result<()> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < read.offset {
        *read = Read::default();
    }
    file.seek(SeekFrom::Start(read.offset))?;
    let mut lines = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let length = lines.read_line(&mut line)?;
        // A line without its end is still being written: it is read next time.
        if length == 0 || !line.ends_with('\n') {
            return Ok(());
        }
        read.offset += length as u64;
        let Some((project, day, tokens, id)) = line_tokens(&line) else {
            continue;
        };
        if id.is_some() && id == read.last_message {
            continue;
        }
        read.last_message = id;
        *read
            .tokens
            .entry(project)
            .or_default()
            .entry(day)
            .or_default() += tokens;
    }
}

fn transcripts(projects: &Path, since: SystemTime) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(folders) = fs::read_dir(projects) else {
        return found;
    };
    for folder in folders.flatten() {
        let Ok(entries) = fs::read_dir(folder.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let recent = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified >= since);
            if recent && path.extension().is_some_and(|e| e == "jsonl") {
                found.push(path);
            }
        }
    }
    found
}

/// Reads what was added to the transcripts of `projects` since last time.
pub fn refresh(projects: &Path, cache_file: &Path, now: i64) -> Result<()> {
    let mut cache: Cache = fsutil::read_json(cache_file)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let since =
        SystemTime::UNIX_EPOCH + Duration::from_secs((now - (DAYS + 1) * DAY).max(0) as u64);
    let current = transcripts(projects, since);
    let wanted: HashSet<&PathBuf> = current.iter().collect();
    cache.transcripts.retain(|path, _| wanted.contains(path));
    for path in current {
        let read = cache.transcripts.entry(path.clone()).or_default();
        // Fail open: a transcript that cannot be read counts for nothing.
        let _ = read_from(&path, read);
    }
    fsutil::write_json_atomic(cache_file, &serde_json::json!(cache))
}

/// What each project consumed over the last `DAYS` days, the heaviest first.
pub fn by_project(cache_file: &Path, now: i64) -> Vec<ProjectUsage> {
    let cache: Cache = fsutil::read_json(cache_file)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let first_day = now.div_euclid(DAY) - DAYS + 1;
    let mut totals: HashMap<String, u64> = HashMap::new();
    for read in cache.transcripts.values() {
        for (project, days) in &read.tokens {
            let tokens: u64 = days.range(first_day..).map(|(_, tokens)| tokens).sum();
            if tokens > 0 {
                *totals.entry(project.clone()).or_default() += tokens;
            }
        }
    }
    let mut usage: Vec<ProjectUsage> = totals
        .into_iter()
        .map(|(project, tokens)| ProjectUsage { project, tokens })
        .collect();
    usage.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.project.cmp(&b.project)));
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    const NOW: i64 = 1_793_527_200;

    fn at(secs: i64) -> String {
        DateTime::from_timestamp(secs, 0).unwrap().to_rfc3339()
    }

    fn line(cwd: &str, when: i64, id: &str, tokens: u64) -> String {
        format!(
            "{}\n",
            json!({"type": "assistant", "cwd": cwd, "timestamp": at(when), "message": {
                "id": id, "usage": {
                    "input_tokens": tokens, "output_tokens": 1,
                    "cache_creation_input_tokens": 2, "cache_read_input_tokens": 1000
                }
            }})
        )
    }

    struct Fixture {
        _root: tempfile::TempDir,
        projects: PathBuf,
        cache: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let projects = root.path().join("projects");
        fs::create_dir_all(projects.join("-work")).unwrap();
        Fixture {
            cache: root.path().join("projects.json"),
            projects,
            _root: root,
        }
    }

    fn append(f: &Fixture, name: &str, text: &str) {
        let path = f.projects.join("-work").join(name);
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn usage(project: &str, tokens: u64) -> ProjectUsage {
        ProjectUsage {
            project: project.into(),
            tokens,
        }
    }

    #[test]
    fn tokens_add_up_by_project_without_what_came_from_the_cache() {
        let f = fixture();
        append(&f, "a.jsonl", &line("/work/app", NOW - 60, "m1", 10));
        append(&f, "a.jsonl", &line("/work/app", NOW - 50, "m2", 20));
        append(&f, "b.jsonl", &line("/work/site", NOW - 60, "m3", 5));

        refresh(&f.projects, &f.cache, NOW).unwrap();

        assert_eq!(
            by_project(&f.cache, NOW),
            vec![usage("app", 36), usage("site", 8)]
        );
    }

    #[test]
    fn a_message_written_on_several_lines_counts_once() {
        let f = fixture();
        append(&f, "a.jsonl", &line("/work/app", NOW - 60, "m1", 10));
        append(&f, "a.jsonl", &line("/work/app", NOW - 59, "m1", 10));

        refresh(&f.projects, &f.cache, NOW).unwrap();

        assert_eq!(by_project(&f.cache, NOW), vec![usage("app", 13)]);
    }

    #[test]
    fn only_what_was_added_is_read_the_next_time() {
        let f = fixture();
        append(&f, "a.jsonl", &line("/work/app", NOW - 60, "m1", 10));
        refresh(&f.projects, &f.cache, NOW).unwrap();

        append(&f, "a.jsonl", &line("/work/app", NOW - 30, "m2", 10));
        refresh(&f.projects, &f.cache, NOW).unwrap();

        assert_eq!(by_project(&f.cache, NOW), vec![usage("app", 26)]);
    }

    #[test]
    fn a_line_still_being_written_waits_for_its_end() {
        let f = fixture();
        let whole = line("/work/app", NOW - 60, "m1", 10);
        let (start, end) = whole.split_at(40);
        append(&f, "a.jsonl", start);
        refresh(&f.projects, &f.cache, NOW).unwrap();
        assert_eq!(by_project(&f.cache, NOW), vec![]);

        append(&f, "a.jsonl", end);
        refresh(&f.projects, &f.cache, NOW).unwrap();

        assert_eq!(by_project(&f.cache, NOW), vec![usage("app", 13)]);
    }

    #[test]
    fn days_past_the_span_no_longer_count() {
        let f = fixture();
        append(&f, "a.jsonl", &line("/work/app", NOW - 60, "m1", 10));
        append(&f, "a.jsonl", &line("/work/app", NOW - 9 * DAY, "m2", 500));

        refresh(&f.projects, &f.cache, NOW).unwrap();

        assert_eq!(by_project(&f.cache, NOW), vec![usage("app", 13)]);
    }

    #[test]
    fn a_worktree_counts_for_its_project() {
        assert_eq!(project_of("/work/app/.claude/worktrees/fix-1"), "app");
        assert_eq!(project_of("/home/me/.codex/worktrees/x/app"), "me");
        assert_eq!(project_of("/work/site"), "site");
    }
}
