//! A local copy of every artifact published from this Mac.
//!
//! An artifact belongs to the account that published it, and Claude writes its
//! page to a temporary folder before publishing it: after a switch, the other
//! account can neither change it nor find its source. A `PostToolUse` hook on
//! Claude Code's `Artifact` tool hands each publish to [`Library::record`],
//! which copies the page and its files and notes the link and the account.
//! Relais never publishes, reads or shares an artifact itself, and reads no
//! credential: only the tool's input and result, and the session's account id.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;
use crate::registry::Registry;

/// Earlier copies kept for each artifact.
pub const VERSIONS_KEPT: usize = 5;
/// A file larger than this is not copied.
pub const LARGEST_FILE: u64 = 64 * 1024 * 1024;

/// What the library knows about one artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The last part of the link, which names the artifact's folder.
    pub key: String,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The Relais account that published it, when the session told which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    /// Whether the publishing account owns it, as the tool reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned: Option<bool>,
    /// The folder the session worked in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Where the page was published from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The page, relative to the copy's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// Every file of the copy, relative to its folder.
    #[serde(default)]
    pub files: Vec<String>,
    /// The type the artifact was made from (slides, a design, a document).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_url: Option<String>,
    /// The artifact this one was published again from, on another account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub publishes: u32,
}

/// Who published, as the Claude Code session tells it in its environment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Publisher {
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    /// `CLAUDE_CONFIG_DIR`; `None` is the default profile.
    pub config_dir: Option<PathBuf>,
}

impl Publisher {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Publisher {
            account_uuid: var("CLAUDE_CODE_ACCOUNT_UUID"),
            email: var("CLAUDE_CODE_USER_EMAIL"),
            config_dir: var("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        }
    }
}

pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(layout: &Layout) -> Self {
        Library {
            dir: layout.relais_dir.join("artifacts"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Records what one `PostToolUse` event of the `Artifact` tool published.
    /// `Ok(None)` for anything else: another action, a failed publish.
    pub fn record(
        &self,
        registry: &Registry,
        event: &Value,
        publisher: &Publisher,
        now: i64,
    ) -> Result<Option<Entry>> {
        if event.get("tool_name").and_then(Value::as_str) != Some("Artifact") {
            return Ok(None);
        }
        let input = event.get("tool_input").cloned().unwrap_or(Value::Null);
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("publish");
        if action != "publish" || input.get("asset").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        let result = event.get("tool_response").cloned().unwrap_or(Value::Null);
        let Some(url) = result
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| is_artifact_link(u))
        else {
            return Ok(None);
        };
        let Some(key) = key_of(url) else {
            return Ok(None);
        };
        let cwd = event.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        let base = match (input.get("root").and_then(Value::as_str), &cwd) {
            (Some(root), Some(cwd)) => cwd.join(root),
            (Some(root), None) => PathBuf::from(root),
            (None, Some(cwd)) => cwd.clone(),
            (None, None) => PathBuf::new(),
        };
        let page_source = input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| resolve(&base, cwd.as_deref(), path))
            .filter(|path| path.is_file());
        let mut copies: Vec<(String, PathBuf)> = Vec::new();
        if let Some(source) = &page_source {
            let served = source
                .strip_prefix(&base)
                .ok()
                .and_then(|relative| safe_relative(&relative.to_string_lossy()))
                .or_else(|| source.file_name().map(|n| n.to_string_lossy().into_owned()));
            if let Some(served) = served {
                copies.push((served, source.clone()));
            }
        }
        for (served, from) in supporting_files(&input) {
            let Some(served) = safe_relative(&served) else {
                continue;
            };
            let mut source = resolve(&base, cwd.as_deref(), &from);
            if !source.is_file() {
                // Last resort: next to the page.
                if let Some(dir) = page_source.as_deref().and_then(Path::parent) {
                    source = dir.join(&from);
                }
            }
            if source.is_file() && !copies.iter().any(|(s, _)| *s == served) {
                copies.push((served, source));
            }
        }

        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        let index_path = self.index_path();
        let _lock = fsutil::ConfigLock::acquire(&index_path, Duration::from_secs(10))?;
        let mut entries = self.load_index()?;
        let continues = page_source
            .as_deref()
            .and_then(|source| self.followed(&entries, &key, source));
        let version = result
            .get("version")
            .and_then(Value::as_str)
            .map(String::from);
        let written = if copies.is_empty() {
            None
        } else {
            Some(self.store(&key, &copies, version.as_deref(), now)?)
        };

        let text =
            |value: &Value, name: &str| value.get(name).and_then(Value::as_str).map(String::from);
        let previous = entries
            .iter()
            .position(|e| e.key == key)
            .map(|i| entries.remove(i));
        let page_name = written
            .as_ref()
            .and(copies.first())
            .filter(|_| page_source.is_some())
            .map(|(s, _)| s.clone());
        let title = text(&result, "title")
            .or_else(|| text(&input, "title"))
            .or_else(|| page_source.as_deref().and_then(html_title))
            .or_else(|| previous.as_ref().map(|p| p.title.clone()))
            .unwrap_or_else(|| key.clone());
        let account = registry_account(registry, publisher);
        let entry = Entry {
            key: key.clone(),
            url: url.to_string(),
            title,
            icon: text(&input, "icon").or_else(|| previous.as_ref().and_then(|p| p.icon.clone())),
            description: text(&input, "description")
                .or_else(|| previous.as_ref().and_then(|p| p.description.clone())),
            account: account.as_ref().map(|a| a.alias.clone()),
            email: account
                .as_ref()
                .and_then(|a| a.email.clone())
                .or_else(|| publisher.email.clone()),
            account_uuid: account
                .as_ref()
                .and_then(|a| a.account_uuid.clone())
                .or_else(|| publisher.account_uuid.clone()),
            owned: result.get("owned").and_then(Value::as_bool),
            project: cwd
                .as_ref()
                .map(|c| c.display().to_string())
                .or_else(|| previous.as_ref().and_then(|p| p.project.clone())),
            source: page_source
                .as_ref()
                .map(|s| s.display().to_string())
                .or_else(|| previous.as_ref().and_then(|p| p.source.clone())),
            page: page_name.or_else(|| previous.as_ref().and_then(|p| p.page.clone())),
            files: written.unwrap_or_else(|| {
                previous
                    .as_ref()
                    .map(|p| p.files.clone())
                    .unwrap_or_default()
            }),
            type_url: text(&input, "type_url")
                .or_else(|| previous.as_ref().and_then(|p| p.type_url.clone())),
            continues: continues.or_else(|| previous.as_ref().and_then(|p| p.continues.clone())),
            version: version.or_else(|| previous.as_ref().and_then(|p| p.version.clone())),
            created: previous.as_ref().map(|p| p.created).unwrap_or(now),
            updated: now,
            publishes: previous.as_ref().map(|p| p.publishes).unwrap_or(0) + 1,
        };
        entries.insert(0, entry.clone());
        self.save_index(&entries)?;
        Ok(Some(entry))
    }

    /// Every artifact, the most recently published first.
    pub fn list(&self) -> Result<Vec<Entry>> {
        let mut entries = self.load_index()?;
        entries.sort_by_key(|e| std::cmp::Reverse(e.updated));
        Ok(entries)
    }

    /// The latest artifact whose key, link or title matches `query`.
    pub fn find(&self, query: &str) -> Result<Option<Entry>> {
        let wanted = query.trim();
        if wanted.is_empty() {
            return Ok(None);
        }
        let lower = wanted.to_lowercase();
        Ok(self.list()?.into_iter().find(|e| {
            e.key == wanted
                || e.url == wanted
                || key_of(wanted).as_deref() == Some(e.key.as_str())
                || e.title.to_lowercase().contains(&lower)
        }))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn load_index(&self) -> Result<Vec<Entry>> {
        let path = self.index_path();
        let Some(value) = fsutil::read_json(&path)? else {
            return Ok(Vec::new());
        };
        let list = value
            .get("artifacts")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        serde_json::from_value(list).map_err(|e| Error::Json { path, source: e })
    }

    fn save_index(&self, entries: &[Entry]) -> Result<()> {
        fsutil::write_json_atomic(
            &self.index_path(),
            &json!({ "version": 1, "artifacts": entries }),
        )
    }

    /// The artifact a page published to `key` came from: the page sits in
    /// another artifact's copy, or has exactly that copy's content.
    fn followed(&self, entries: &[Entry], key: &str, source: &Path) -> Option<String> {
        let source = fs::canonicalize(source).ok()?;
        let library = fs::canonicalize(&self.dir).ok()?;
        if let Ok(inside) = source.strip_prefix(&library) {
            let owner = inside
                .components()
                .next()?
                .as_os_str()
                .to_string_lossy()
                .into_owned();
            return (owner != key && entries.iter().any(|e| e.key == owner)).then_some(owner);
        }
        let content = fs::read(&source).ok()?;
        entries
            .iter()
            .filter(|e| e.key != key)
            .find(|e| {
                e.page
                    .as_ref()
                    .map(|page| self.current_dir(e).join(page))
                    .and_then(|page| fs::read(page).ok())
                    .is_some_and(|theirs| theirs == content)
            })
            .map(|e| e.key.clone())
    }

    /// Copies `copies` into the artifact's `current` folder, after moving the
    /// previous one to `versions`. Returns the files written.
    fn store(
        &self,
        key: &str,
        copies: &[(String, PathBuf)],
        version: Option<&str>,
        now: i64,
    ) -> Result<Vec<String>> {
        let home = self.dir.join(key);
        let incoming = home.join(format!(".incoming-{}", std::process::id()));
        if incoming.exists() {
            fs::remove_dir_all(&incoming).map_err(|e| Error::io(&incoming, e))?;
        }
        let mut written = Vec::new();
        for (served, source) in copies {
            let too_large = fs::metadata(source)
                .map(|m| m.len() > LARGEST_FILE)
                .unwrap_or(true);
            if too_large {
                continue;
            }
            let target = incoming.join(served);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            fs::copy(source, &target).map_err(|e| Error::io(&target, e))?;
            written.push(served.clone());
        }
        let current = home.join("current");
        if current.exists() {
            let versions = home.join("versions");
            fs::create_dir_all(&versions).map_err(|e| Error::io(&versions, e))?;
            let label = version
                .and_then(safe_relative)
                .filter(|v| !v.contains('/'))
                .unwrap_or_default();
            let mut aside = versions.join(format!("{now:012}-{label}"));
            let mut n = 1;
            while aside.exists() {
                aside = versions.join(format!("{now:012}-{label}-{n}"));
                n += 1;
            }
            fs::rename(&current, &aside).map_err(|e| Error::io(&aside, e))?;
            let mut kept: Vec<PathBuf> = fs::read_dir(&versions)
                .map_err(|e| Error::io(&versions, e))?
                .flatten()
                .map(|d| d.path())
                .collect();
            kept.sort();
            while kept.len() > VERSIONS_KEPT {
                let oldest = kept.remove(0);
                fs::remove_dir_all(&oldest).map_err(|e| Error::io(&oldest, e))?;
            }
        }
        fs::rename(&incoming, &current).map_err(|e| Error::io(&current, e))?;
        Ok(written)
    }

    /// The folder holding an artifact's latest copy.
    pub fn current_dir(&self, entry: &Entry) -> PathBuf {
        self.dir.join(&entry.key).join("current")
    }
}

/// The skill Relais installs for Claude, with the `relais` command it names.
pub fn skill(relais: &str) -> String {
    SKILL.replace("{relais}", relais)
}

const SKILL: &str = r#"---
name: relais-artifacts
description: Use before making or updating an artifact, and whenever an artifact link was published by another Claude account of this Mac or cannot be changed from this one (after a Relais account switch). Relais keeps a local copy of every artifact published here, with its link and its account.
---

# Artifacts kept by Relais

Relais copies every artifact published from this Mac into `~/.relais/artifacts/<id>/current/`, with its link, title, project folder and the account that published it. The copy outlives the temporary folder the page was written in, and a switch to another account.

## Look first

Before making an artifact that may already exist (same subject, same project), list them:

    {relais} artifacts           # newest first: title, account, link
    {relais} artifacts --json    # everything, for a closer look

Update the existing one rather than making a second.

## Update one

Publish with its link (`url`) as usual. The active account can do it when it owns the artifact or was given "Can edit" access.

If the publish is refused because this account can neither own nor edit it, publish the library copy as a new artifact:

    {relais} artifacts path "<title or link>"   # prints the page of the latest copy

Read that file, make the changes asked for, and publish it without `url` (with `root` set to its `current` folder when the copy has other files). Relais records the new link as following the old one. Tell the user the link changed, and why.

## Keep the same link

The account that owns an artifact can share it with the user's other accounts, with "Can edit" (the Share button on the page, invite by email). Any of them can then update the same link after a switch. `{relais} list` shows the other accounts' addresses. Suggest it once for an artifact the user shares with other people.

## Not kept

Comments, the page's shared database, its history on claude.ai, and assets uploaded with `asset: true` stay on the server. Upload such assets again when publishing a copy.
"#;

/// Whether `url` is a claude.ai artifact link.
fn is_artifact_link(url: &str) -> bool {
    url.starts_with("https://claude.ai/artifact/")
        || url.starts_with("https://claude.ai/code/artifact/")
}

/// The last part of an artifact link, which names its folder.
fn key_of(url: &str) -> Option<String> {
    if !is_artifact_link(url) {
        return None;
    }
    let last = url
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/')
        .rsplit('/')
        .next()?;
    let valid = !last.is_empty()
        && last.len() <= 64
        && last
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| last.to_string())
}

/// A path the tool was given: absolute, or relative to the root (`base`) the
/// publish named, falling back to the session's folder.
fn resolve(base: &Path, cwd: Option<&Path>, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let in_base = base.join(path);
    match cwd {
        Some(cwd) if !in_base.exists() => cwd.join(path),
        _ => in_base,
    }
}

/// `path` when it stays inside the folder it is joined to: relative, without `..`.
fn safe_relative(path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() {
        return None;
    }
    let parts: Option<Vec<String>> = path
        .components()
        .map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::CurDir => Some(String::new()),
            _ => None,
        })
        .collect();
    let parts: Vec<String> = parts?.into_iter().filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The `files` of a publish as (published path, source path): a map of
/// published path to a path or `{from}`, or a list of paths published as
/// spelled. Files another artifact supplies server side are left out.
fn supporting_files(input: &Value) -> Vec<(String, String)> {
    match input.get("files") {
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(served, source)| match source {
                Value::String(from) => Some((served.clone(), from.clone())),
                Value::Object(spec) => spec
                    .get("from")
                    .and_then(Value::as_str)
                    .map(|from| (served.clone(), from.to_string())),
                _ => None,
            })
            .collect(),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|item| item.get("path").and_then(Value::as_str))
            .map(|path| (path.to_string(), path.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The text of a page's `<title>`, when it has one.
fn html_title(page: &Path) -> Option<String> {
    let text = fs::read_to_string(page).ok()?;
    let lower = text.to_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = start + lower[start..].find("</title>")?;
    let title = text[start..end].trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// The registered account behind the session: its id, its email, then its profile.
fn registry_account<'a>(
    registry: &'a Registry,
    publisher: &Publisher,
) -> Option<&'a crate::registry::Account> {
    let by_uuid = publisher.account_uuid.as_deref().and_then(|uuid| {
        registry.accounts.iter().find(|a| {
            a.account_uuid
                .as_deref()
                .is_some_and(|known| known.eq_ignore_ascii_case(uuid))
        })
    });
    let by_email = || {
        publisher.email.as_deref().and_then(|email| {
            registry.accounts.iter().find(|a| {
                a.email
                    .as_deref()
                    .is_some_and(|known| known.eq_ignore_ascii_case(email))
            })
        })
    };
    by_uuid
        .or_else(by_email)
        .or_else(|| registry.find_by_config_dir(publisher.config_dir.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Account;

    struct Setup {
        _root: tempfile::TempDir,
        library: Library,
        registry: Registry,
        work: PathBuf,
    }

    fn setup() -> Setup {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::for_home(root.path());
        let work = root.path().join("scratch");
        fs::create_dir_all(&work).unwrap();
        let mut registry = Registry::default();
        registry
            .add(Account {
                alias: "work".into(),
                email: Some("work@example.com".into()),
                account_uuid: Some("00000000-0000-4000-8000-000000000001".into()),
                org_id: None,
                config_dir: Some(root.path().join(".relais/accounts/work/claude")),
                chrome_profile: None,
                color: None,
            })
            .unwrap();
        registry
            .add(Account {
                alias: "main".into(),
                email: Some("main@example.com".into()),
                account_uuid: Some("00000000-0000-4000-8000-000000000002".into()),
                org_id: None,
                config_dir: None,
                chrome_profile: None,
                color: None,
            })
            .unwrap();
        Setup {
            library: Library::new(&layout),
            _root: root,
            registry,
            work,
        }
    }

    fn publish(input: Value, url: &str, title: &str, cwd: &Path) -> Value {
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Artifact",
            "cwd": cwd,
            "tool_input": input,
            "tool_response": {
                "url": url,
                "artifact_id": "5248c593-0000-4000-8000-000000000000",
                "title": title,
                "owned": true,
                "version": "1794409840-6018",
                "updated": false,
            },
        })
    }

    fn by_uuid(uuid: &str) -> Publisher {
        Publisher {
            account_uuid: Some(uuid.into()),
            ..Publisher::default()
        }
    }

    const WORK: &str = "00000000-0000-4000-8000-000000000001";
    const PAGE_URL: &str = "https://claude.ai/artifact/BAKyKCdvQkXZD6ya9PpSar";

    #[test]
    fn a_published_page_and_its_files_are_copied_and_indexed() {
        let s = setup();
        let page = s.work.join("report.html");
        fs::write(
            &page,
            "<title>Weekly report</title><img src=\"img/chart.png\">",
        )
        .unwrap();
        fs::create_dir_all(s.work.join("img")).unwrap();
        fs::write(s.work.join("img/chart.png"), [1, 2, 3]).unwrap();
        let input = json!({"file_path": page, "files": {"img/chart.png": "img/chart.png"}, "icon": "chart"});
        let project = Path::new("/projects/example");

        let entry = s
            .library
            .record(
                &s.registry,
                &publish(input, PAGE_URL, "Weekly report", project),
                &by_uuid(WORK),
                100,
            )
            .unwrap()
            .expect("the publish is recorded");

        assert_eq!(entry.key, "BAKyKCdvQkXZD6ya9PpSar");
        assert_eq!(entry.title, "Weekly report");
        assert_eq!(entry.account.as_deref(), Some("work"));
        assert_eq!(entry.email.as_deref(), Some("work@example.com"));
        assert_eq!(entry.project.as_deref(), Some("/projects/example"));
        assert_eq!(entry.icon.as_deref(), Some("chart"));
        assert_eq!(entry.page.as_deref(), Some("report.html"));
        let copy = s.library.current_dir(&entry);
        assert!(fs::read_to_string(copy.join("report.html"))
            .unwrap()
            .contains("Weekly report"));
        assert_eq!(fs::read(copy.join("img/chart.png")).unwrap(), vec![1, 2, 3]);
        assert_eq!(s.library.list().unwrap(), vec![entry]);
    }

    #[test]
    fn the_files_are_found_against_the_root_when_one_is_given() {
        let s = setup();
        let root = s.work.join("site");
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("index.html"), "<p>home</p>").unwrap();
        fs::write(root.join("data/rows.json"), "[]").unwrap();
        let input = json!({"file_path": root.join("index.html"), "root": "site", "files": {"data/rows.json": "data/rows.json"}});

        let entry = s
            .library
            .record(
                &s.registry,
                &publish(input, PAGE_URL, "Site", &s.work),
                &by_uuid(WORK),
                100,
            )
            .unwrap()
            .unwrap();

        let copy = s.library.current_dir(&entry);
        assert_eq!(
            fs::read_to_string(copy.join("index.html")).unwrap(),
            "<p>home</p>"
        );
        assert_eq!(
            fs::read_to_string(copy.join("data/rows.json")).unwrap(),
            "[]"
        );
    }

    #[test]
    fn publishing_again_keeps_the_earlier_copies_up_to_a_limit() {
        let s = setup();
        let page = s.work.join("page.html");
        for n in 0..(VERSIONS_KEPT + 3) {
            fs::write(&page, format!("<p>{n}</p>")).unwrap();
            let mut event = publish(
                json!({"file_path": page, "url": PAGE_URL}),
                PAGE_URL,
                "Page",
                &s.work,
            );
            event["tool_response"]["version"] = json!(format!("v{n}"));
            s.library
                .record(&s.registry, &event, &by_uuid(WORK), 100 + n as i64)
                .unwrap();
        }

        let entry = s.library.find(PAGE_URL).unwrap().unwrap();
        assert_eq!(entry.publishes, (VERSIONS_KEPT + 3) as u32);
        assert_eq!(entry.created, 100);
        let latest = format!("<p>{}</p>", VERSIONS_KEPT + 2);
        assert_eq!(
            fs::read_to_string(s.library.current_dir(&entry).join("page.html")).unwrap(),
            latest
        );
        let versions = fs::read_dir(s.library.dir().join(&entry.key).join("versions"))
            .unwrap()
            .count();
        assert_eq!(versions, VERSIONS_KEPT);
    }

    #[test]
    fn a_copy_published_again_from_another_account_follows_the_old_link() {
        let s = setup();
        let page = s.work.join("plan.html");
        fs::write(&page, "<p>plan</p>").unwrap();
        let first = s
            .library
            .record(
                &s.registry,
                &publish(json!({"file_path": page}), PAGE_URL, "Plan", &s.work),
                &by_uuid(WORK),
                100,
            )
            .unwrap()
            .unwrap();
        let copy = s.library.current_dir(&first).join("plan.html");
        let other = "https://claude.ai/code/artifact/7c39b1c4-5575-4be1-9d24-a963c66ff53d";

        let second = s
            .library
            .record(
                &s.registry,
                &publish(json!({"file_path": copy}), other, "Plan", &s.work),
                &Publisher::default(),
                200,
            )
            .unwrap()
            .unwrap();

        assert_eq!(second.key, "7c39b1c4-5575-4be1-9d24-a963c66ff53d");
        assert_eq!(second.continues.as_deref(), Some(first.key.as_str()));
        assert_eq!(
            second.account.as_deref(),
            Some("main"),
            "no variable: the default profile"
        );
        assert_eq!(s.library.list().unwrap()[0].key, second.key);
    }

    #[test]
    fn the_same_content_published_from_elsewhere_follows_the_old_link_too() {
        let s = setup();
        let page = s.work.join("plan.html");
        fs::write(&page, "<p>plan</p>").unwrap();
        s.library
            .record(
                &s.registry,
                &publish(json!({"file_path": page}), PAGE_URL, "Plan", &s.work),
                &by_uuid(WORK),
                100,
            )
            .unwrap();
        let elsewhere = s.work.join("again.html");
        fs::write(&elsewhere, "<p>plan</p>").unwrap();
        let other = "https://claude.ai/artifact/QKN21svewxgyPb6SYRqWnd";

        let second = s
            .library
            .record(
                &s.registry,
                &publish(json!({"file_path": elsewhere}), other, "Plan", &s.work),
                &by_uuid(WORK),
                200,
            )
            .unwrap()
            .unwrap();

        assert_eq!(second.continues.as_deref(), Some("BAKyKCdvQkXZD6ya9PpSar"));
    }

    #[test]
    fn anything_but_a_successful_publish_is_ignored() {
        let s = setup();
        let page = s.work.join("p.html");
        fs::write(&page, "<p>p</p>").unwrap();
        let mut read = publish(
            json!({"action": "read", "url": PAGE_URL}),
            PAGE_URL,
            "P",
            &s.work,
        );
        read["tool_response"] = json!({"file_read": {"url": PAGE_URL}});
        let mut failed = publish(json!({"file_path": page}), PAGE_URL, "P", &s.work);
        failed["tool_response"] = json!("Refused: the edit did not apply");
        let mut other_tool = publish(json!({"file_path": page}), PAGE_URL, "P", &s.work);
        other_tool["tool_name"] = json!("Write");
        let asset = publish(
            json!({"file_path": page, "url": PAGE_URL, "asset": true}),
            PAGE_URL,
            "P",
            &s.work,
        );

        for event in [read, failed, other_tool, asset] {
            assert_eq!(
                s.library
                    .record(&s.registry, &event, &by_uuid(WORK), 100)
                    .unwrap(),
                None
            );
        }
        assert!(s.library.list().unwrap().is_empty());
    }

    #[test]
    fn a_file_outside_the_copy_is_never_written() {
        let s = setup();
        let page = s.work.join("p.html");
        fs::write(&page, "<p>p</p>").unwrap();
        fs::write(s.work.join("secret.txt"), "x").unwrap();
        let input = json!({"file_path": page, "files": {"../../escape.txt": "secret.txt", "/etc/escape.txt": "secret.txt"}});

        let entry = s
            .library
            .record(
                &s.registry,
                &publish(input, PAGE_URL, "P", &s.work),
                &by_uuid(WORK),
                100,
            )
            .unwrap()
            .unwrap();

        assert_eq!(entry.files, vec!["p.html".to_string()]);
        assert!(!s.library.dir().join("escape.txt").exists());
        assert!(!s
            .library
            .dir()
            .parent()
            .unwrap()
            .join("escape.txt")
            .exists());
    }

    #[test]
    fn a_type_created_without_a_file_is_recorded_with_its_link() {
        let s = setup();
        let mut event = publish(
            json!({"type_url": "https://claude.ai/artifact/QKN21svewxgyPb6SYRqWnd", "title": "Deck"}),
            PAGE_URL,
            "Deck",
            &s.work,
        );
        event["tool_response"]["created_from_type"] = json!(true);

        let entry = s
            .library
            .record(&s.registry, &event, &by_uuid(WORK), 100)
            .unwrap()
            .unwrap();

        assert_eq!(
            entry.type_url.as_deref(),
            Some("https://claude.ai/artifact/QKN21svewxgyPb6SYRqWnd")
        );
        assert!(entry.files.is_empty());
        assert_eq!(entry.page, None);
    }

    #[test]
    fn find_matches_a_title_a_link_or_a_key() {
        let s = setup();
        let page = s.work.join("p.html");
        fs::write(&page, "<p>p</p>").unwrap();
        s.library
            .record(
                &s.registry,
                &publish(
                    json!({"file_path": page}),
                    PAGE_URL,
                    "Quarterly Review",
                    &s.work,
                ),
                &by_uuid(WORK),
                100,
            )
            .unwrap();

        for query in ["quarterly", PAGE_URL, "BAKyKCdvQkXZD6ya9PpSar"] {
            assert_eq!(
                s.library.find(query).unwrap().unwrap().key,
                "BAKyKCdvQkXZD6ya9PpSar",
                "{query}"
            );
        }
        assert_eq!(s.library.find("nothing like it").unwrap(), None);
    }
}
