//! Switching the desktop app to another account, its workspace unchanged.
//!
//! The data folder stays where it is. A switch, the app closed, parks the
//! sign-in in place, puts the next account's back, and carries to it what
//! the app files under an account's id: the conversation list and a few
//! settings. Every step is written to a journal before it runs, and what it
//! changes is copied first, so a switch cut short (a crash, a power cut) is
//! undone by the next run, and Desktop can always start on the account it
//! had.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::claude::desktop_app::{self, DesktopApp, DesktopSignIn, DESKTOP_QUIT_WAIT};
use crate::claude::desktop_carry::{self, AccountIds, IndexReport, OwnConversations};
use crate::claude::desktop_identity;
use crate::claude::desktop_profile;
use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;
use crate::registry::Registry;

/// The app's two conversation lists, inside its data folder: Code, Cowork.
pub const INDEX_ROOTS: [&str; 2] = ["claude-code-sessions", "local-agent-mode-sessions"];

/// Where the arriving account's sign-in comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    /// Parked by an earlier switch.
    Parked,
    /// Inside the whole data folder an earlier version of Relais kept for it.
    OldFolder,
    /// None: the app starts signed out and the user signs in.
    None,
}

/// What a switch will do, read before anything moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchPlan {
    pub from: String,
    pub to: String,
    pub from_ids: AccountIds,
    pub to_ids: AccountIds,
    pub sign_in: SignIn,
    /// Conversations of the list that follows the user.
    pub conversations: usize,
    /// The arriving account's conversations of its own.
    pub own: Vec<String>,
    /// The arriving account's records of conversations already listed.
    pub duplicates: usize,
    /// Settings filed under the account that will be carried.
    pub preference_keys: usize,
}

impl SwitchPlan {
    pub fn is_noop(&self) -> bool {
        self.from == self.to
    }
}

/// What a switch did.
#[derive(Debug, PartialEq, Eq)]
pub struct Applied {
    pub plan: SwitchPlan,
    pub index: IndexReport,
    pub cowork: IndexReport,
    pub preference_keys: usize,
    pub backup: PathBuf,
    /// The app will start signed out: the arriving account had no sign-in.
    pub signed_out: bool,
}

/// The steps of a switch, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Backup,
    TakeOldFolder,
    ParkSource,
    PutTargetBack,
    CarryIndex,
    CarryPreferences,
    RecordOwner,
}

/// Reads what switching to `to` would do. Nothing changes.
pub fn plan(layout: &Layout, registry: &Registry, to: &str) -> Result<SwitchPlan> {
    let target = registry
        .get(to)
        .ok_or_else(|| Error::UnknownAccount(to.to_string()))?;
    let from = source_alias(layout, registry)?;
    let source = registry
        .get(&from)
        .ok_or_else(|| Error::UnknownAccount(from.clone()))?;
    let from_ids = ids_of(source)?;
    let to_ids = ids_of(target)?;
    let sign_in = if from == to {
        SignIn::None
    } else {
        sign_in_of(layout, to, &to_ids)?
    };
    let codes = layout.desktop_dir.join(INDEX_ROOTS[0]);
    let leaving = desktop_carry::index_dir(&codes, &from_ids);
    let arriving =
        desktop_carry::target_conversations(&leaving, &desktop_carry::index_dir(&codes, &to_ids));
    let preference_keys = fsutil::read_json(&layout.desktop_config_file())
        .ok()
        .flatten()
        .map(|mut config| desktop_carry::carry_preferences(&mut config, &from_ids, &to_ids))
        .unwrap_or(0);
    Ok(SwitchPlan {
        conversations: desktop_carry::conversation_count(&leaving),
        own: arriving.own,
        duplicates: arriving.duplicates.len(),
        preference_keys,
        from: from.clone(),
        to: to.to_string(),
        from_ids,
        to_ids,
        sign_in,
    })
}

/// The account whose sign-in is in place, by the id it carries; without
/// one, the account recorded at the last switch.
pub fn signed_in_account(layout: &Layout, registry: &Registry) -> Result<String> {
    source_alias(layout, registry)
}

fn source_alias(layout: &Layout, registry: &Registry) -> Result<String> {
    match desktop_identity::account_of(&layout.desktop_dir) {
        Some(uuid) => registry
            .accounts
            .iter()
            .find(|a| a.account_uuid.as_deref() == Some(uuid.as_str()))
            .map(|a| a.alias.clone())
            .ok_or(Error::UnknownSignedInAccount(uuid)),
        None => desktop_profile::owner(layout, registry).ok_or(Error::UnknownDesktopOwner),
    }
}

fn ids_of(account: &crate::registry::Account) -> Result<AccountIds> {
    match (&account.account_uuid, &account.org_id) {
        (Some(account_uuid), Some(org)) => Ok(AccountIds {
            account: account_uuid.clone(),
            org: org.clone(),
        }),
        _ => Err(Error::MissingAccountIds(account.alias.clone())),
    }
}

/// Where `alias`'s sign-in will come from, checked to be its own.
fn sign_in_of(layout: &Layout, alias: &str, ids: &AccountIds) -> Result<SignIn> {
    let (sign_in, dir) = if desktop_identity::is_parked(layout, alias) {
        (SignIn::Parked, layout.identity_dir(alias))
    } else if desktop_identity::has_old_folder(layout, alias) {
        (SignIn::OldFolder, layout.desktop_profile_dir(alias))
    } else {
        return Ok(SignIn::None);
    };
    match desktop_identity::account_of(&dir) {
        Some(uuid) if uuid != ids.account => Err(Error::Conflict(dir)),
        _ => Ok(sign_in),
    }
}

/// Switches the app, closed, to `to`. `own` says what to do with the
/// arriving account's own conversations, and is needed when it has some.
pub fn apply(
    layout: &Layout,
    registry: &Registry,
    to: &str,
    own: Option<OwnConversations>,
    desktop_running: bool,
    now: i64,
) -> Result<Applied> {
    apply_with(layout, registry, to, own, desktop_running, now, None)
}

/// [`apply`], stopping without a word after `cut_after`, as a crash would:
/// how the tests cut a switch short.
fn apply_with(
    layout: &Layout,
    registry: &Registry,
    to: &str,
    own: Option<OwnConversations>,
    desktop_running: bool,
    now: i64,
    cut_after: Option<Step>,
) -> Result<Applied> {
    if desktop_running {
        return Err(Error::DesktopStillRunning);
    }
    if roll_back_interrupted(layout)?.is_some() {
        return Err(Error::SwitchInterrupted);
    }
    let plan = plan(layout, registry, to)?;
    let stamp = now.to_string();
    let mut applied = Applied {
        index: IndexReport::default(),
        cowork: IndexReport::default(),
        preference_keys: 0,
        backup: layout.workspace_backup_dir(&stamp),
        signed_out: plan.sign_in == SignIn::None,
        plan,
    };
    if applied.plan.is_noop() {
        applied.signed_out = false;
        return Ok(applied);
    }
    let own = match own {
        Some(own) => own,
        None if applied.plan.own.is_empty() => OwnConversations::Add,
        None => return Err(Error::OwnConversationsNeedChoice(applied.plan.own.len())),
    };
    let mut steps = vec![Step::Backup];
    if applied.plan.sign_in == SignIn::OldFolder {
        steps.push(Step::TakeOldFolder);
    }
    steps.push(Step::ParkSource);
    if applied.plan.sign_in != SignIn::None {
        steps.push(Step::PutTargetBack);
    }
    steps.extend([Step::CarryIndex, Step::CarryPreferences, Step::RecordOwner]);

    let mut journal = Journal {
        version: 1,
        from: applied.plan.from.clone(),
        to: applied.plan.to.clone(),
        from_ids: (&applied.plan.from_ids).into(),
        to_ids: (&applied.plan.to_ids).into(),
        backup: applied.backup.clone(),
        started: Vec::new(),
    };
    for step in steps {
        journal.started.push(step);
        journal.save(layout)?;
        if let Err(error) = run(layout, step, own, &stamp, now, &mut applied) {
            let _ = roll_back_interrupted(layout);
            return Err(error);
        }
        if cut_after == Some(step) {
            return Err(Error::Command(format!("cut short after {step:?}")));
        }
    }
    let path = layout.workspace_switch_journal();
    fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
    Ok(applied)
}

fn run(
    layout: &Layout,
    step: Step,
    own: OwnConversations,
    stamp: &str,
    now: i64,
    applied: &mut Applied,
) -> Result<()> {
    let plan = &applied.plan;
    match step {
        Step::Backup => {
            for file in [layout.desktop_config_file(), layout.desktop_presets_file()] {
                if file.is_file() {
                    copy_file(
                        &file,
                        &applied.backup.join(file.file_name().unwrap_or_default()),
                    )?;
                }
            }
            for (root, ids) in index_dirs(plan) {
                let list = desktop_carry::index_dir(&layout.desktop_dir.join(root), ids);
                if list.is_dir() {
                    copy_dir(&list, &backed_up_list(&applied.backup, root, ids))?;
                }
            }
        }
        Step::TakeOldFolder => {
            desktop_identity::take_from_old_folder(layout, &plan.to, &date_of(now))?;
        }
        Step::ParkSource => {
            desktop_identity::park(layout, &plan.from)?;
        }
        Step::PutTargetBack => {
            desktop_identity::put_back(layout, &plan.to)?;
        }
        Step::CarryIndex => {
            for root in INDEX_ROOTS {
                let base = layout.desktop_dir.join(root);
                let report = desktop_carry::carry_index(
                    &desktop_carry::index_dir(&base, &plan.from_ids),
                    &desktop_carry::index_dir(&base, &plan.to_ids),
                    own,
                    &desktop_carry::index_dir(
                        &layout.set_aside_dir(stamp).join(root),
                        &plan.to_ids,
                    ),
                    &applied.backup.join("replaced").join(root),
                )?;
                if root == INDEX_ROOTS[0] {
                    applied.index = report;
                } else {
                    applied.cowork = report;
                }
            }
        }
        Step::CarryPreferences => {
            let config_file = layout.desktop_config_file();
            if let Some(mut config) = fsutil::read_json(&config_file)? {
                applied.preference_keys =
                    desktop_carry::carry_preferences(&mut config, &plan.from_ids, &plan.to_ids);
                write_private(&config_file, &config)?;
            }
            let presets_file = layout.desktop_presets_file();
            if let Some(mut presets) = fsutil::read_json(&presets_file)? {
                if desktop_carry::carry_presets(&mut presets, &plan.from_ids, &plan.to_ids) {
                    write_private(&presets_file, &presets)?;
                }
            }
        }
        Step::RecordOwner => desktop_profile::set_owner(layout, &plan.to)?,
    }
    Ok(())
}

/// Both accounts' lists in both of the app's roots.
fn index_dirs(plan: &SwitchPlan) -> Vec<(&'static str, &AccountIds)> {
    INDEX_ROOTS
        .iter()
        .flat_map(|root| [(*root, &plan.from_ids), (*root, &plan.to_ids)])
        .collect()
}

fn backed_up_list(backup: &Path, root: &str, ids: &AccountIds) -> PathBuf {
    desktop_carry::index_dir(&backup.join(root), ids)
}

fn date_of(now: i64) -> String {
    chrono::DateTime::from_timestamp(now, 0)
        .map(|at| at.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| now.to_string())
}

/// The journal of a switch in progress: who, where the copies are, and the
/// steps begun, so that they can be undone in reverse.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    version: u32,
    from: String,
    to: String,
    from_ids: Ids,
    to_ids: Ids,
    backup: PathBuf,
    started: Vec<Step>,
}

#[derive(Serialize, Deserialize)]
struct Ids {
    account: String,
    org: String,
}

impl From<&AccountIds> for Ids {
    fn from(ids: &AccountIds) -> Self {
        Ids {
            account: ids.account.clone(),
            org: ids.org.clone(),
        }
    }
}

impl From<&Ids> for AccountIds {
    fn from(ids: &Ids) -> Self {
        AccountIds {
            account: ids.account.clone(),
            org: ids.org.clone(),
        }
    }
}

impl Journal {
    fn save(&self, layout: &Layout) -> Result<()> {
        let path = layout.workspace_switch_journal();
        let value = serde_json::to_value(self).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        fsutil::write_json_atomic(&path, &value)
    }
}

/// Whether a switch was cut short and waits to be undone.
pub fn is_interrupted(layout: &Layout) -> bool {
    layout.workspace_switch_journal().exists()
}

/// Undoes a switch that was cut short, from its journal. Returns the account
/// the app is back on, or `None` when no switch was in progress.
pub fn roll_back_interrupted(layout: &Layout) -> Result<Option<String>> {
    let path = layout.workspace_switch_journal();
    let Some(value) = fsutil::read_json(&path)? else {
        return Ok(None);
    };
    let journal: Journal = serde_json::from_value(value).map_err(|e| Error::Json {
        path: path.clone(),
        source: e,
    })?;
    let from_ids = AccountIds::from(&journal.from_ids);
    let to_ids = AccountIds::from(&journal.to_ids);
    let backup = &journal.backup;
    let rolled_back = backup.join("rolled-back");
    for step in journal.started.iter().rev() {
        match step {
            Step::RecordOwner => desktop_profile::set_owner(layout, &journal.from)?,
            Step::CarryPreferences => {
                for file in [layout.desktop_config_file(), layout.desktop_presets_file()] {
                    let copy = backup.join(file.file_name().unwrap_or_default());
                    restore(&file, &copy, &rolled_back)?;
                }
            }
            Step::CarryIndex => {
                for root in INDEX_ROOTS {
                    for ids in [&from_ids, &to_ids] {
                        let list = desktop_carry::index_dir(&layout.desktop_dir.join(root), ids);
                        let copy = backed_up_list(backup, root, ids);
                        restore(&list, &copy, &rolled_back.join(root).join(&ids.account))?;
                    }
                }
            }
            Step::PutTargetBack => {
                desktop_identity::rejoin_parked(layout, &journal.to)?;
            }
            Step::ParkSource => {
                desktop_identity::rejoin_in_place(layout, &journal.from)?;
            }
            Step::TakeOldFolder | Step::Backup => {}
        }
    }
    fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
    Ok(Some(journal.from))
}

/// Puts the copy of `live` back: what is there now goes to `rolled_back`,
/// and a `live` that did not exist before the switch stays away.
fn restore(live: &Path, copy: &Path, rolled_back: &Path) -> Result<()> {
    if fs::symlink_metadata(live).is_ok() {
        fs::create_dir_all(rolled_back).map_err(|e| Error::io(rolled_back, e))?;
        let mut aside = rolled_back.join(live.file_name().unwrap_or_default());
        let mut n = 1;
        while fs::symlink_metadata(&aside).is_ok() {
            aside = rolled_back.join(format!(
                "{}.{n}",
                live.file_name().unwrap_or_default().to_string_lossy()
            ));
            n += 1;
        }
        fs::rename(live, &aside).map_err(|e| Error::io(live, e))?;
    }
    if copy.is_dir() {
        copy_dir(copy, live)
    } else if copy.is_file() {
        copy_file(copy, live)
    } else {
        Ok(())
    }
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    fs::copy(from, to).map_err(|e| Error::io(from, e))?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    for entry in fs::read_dir(from).map_err(|e| Error::io(from, e))? {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let path = entry.path();
        let destination = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &destination)?;
        } else if path.is_file() {
            copy_file(&path, &destination)?;
        }
    }
    Ok(())
}

/// Rewrites one of the app's files atomically, private to the user as the
/// app keeps them.
fn write_private(path: &Path, value: &serde_json::Value) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fsutil::write_json_atomic(path, value)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| Error::io(path, e))
}

/// How the app started after a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Started {
    /// Proven signed in to the arriving account.
    SignedIn,
    /// Open, without proof yet of the account it is on.
    NotConfirmed,
    /// Open on its sign-in page: the arriving account had no sign-in.
    SignedOut,
}

/// Opens the app after `applied` and waits up to `wait` for the proof that
/// it is signed in to the arriving account: a usage sample of its
/// organization, or the app's log when it writes one.
pub fn open_and_confirm(
    layout: &Layout,
    app: &DesktopApp,
    applied: &Applied,
    now: i64,
    wait: Duration,
) -> Result<Started> {
    let since = chrono::Local::now().naive_local() - chrono::Duration::seconds(1);
    app.open()?;
    if applied.signed_out {
        return Ok(Started::SignedOut);
    }
    let ids = &applied.plan.to_ids;
    let started = Instant::now();
    loop {
        if signed_in_since(layout, &ids.org, now - 1) {
            return Ok(Started::SignedIn);
        }
        let logged = std::fs::read(layout.desktop_log_file())
            .ok()
            .and_then(|log| desktop_app::sign_in_since(&String::from_utf8_lossy(&log), since));
        if let Some(DesktopSignIn::SignedIn { account_uuid }) = logged {
            return Ok(if account_uuid == ids.account {
                Started::SignedIn
            } else {
                Started::NotConfirmed
            });
        }
        if started.elapsed() > wait {
            return Ok(Started::NotConfirmed);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Undoes a switch cut short, closing the app first since its files move.
/// Returns whether there was one.
pub fn undo_interrupted(layout: &Layout, app: &DesktopApp) -> Result<bool> {
    if !is_interrupted(layout) {
        return Ok(false);
    }
    if app.is_running() {
        app.quit(DESKTOP_QUIT_WAIT)?;
    }
    roll_back_interrupted(layout)?;
    Ok(true)
}

/// Whether the app, since `since` (seconds), measured the usage of `org`:
/// it does so at each start, after asking the server, so only with a
/// working sign-in for that organization.
pub fn signed_in_since(layout: &Layout, org: &str, since: i64) -> bool {
    let Ok(Some(history)) = fsutil::read_json(&layout.desktop_usage_history()) else {
        return false;
    };
    history
        .get("samples")
        .and_then(|samples| samples.as_array())
        .is_some_and(|samples| {
            samples.iter().any(|sample| {
                let at = sample.get("t").and_then(|t| t.as_i64()).unwrap_or(0);
                let at = if at > 100_000_000_000 { at / 1000 } else { at };
                sample.get("org").and_then(|o| o.as_str()) == Some(org) && at >= since
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Account;
    use serde_json::json;
    use std::collections::BTreeMap;

    const NOW: i64 = 1_793_500_000;

    fn account(alias: &str, uuid: Option<&str>, org: Option<&str>) -> Account {
        Account {
            alias: alias.into(),
            email: Some(format!("{alias}@example.com")),
            account_uuid: uuid.map(String::from),
            org_id: org.map(String::from),
            config_dir: None,
            chrome_profile: None,
            color: None,
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        layout: Layout,
        registry: Registry,
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn sign_in(dir: &Path, uuid: &str, secret: &str) {
        write(
            &dir.join("config.json"),
            &json!({"oauth:tokenCacheV2": secret, "lastKnownAccountUuid": uuid}).to_string(),
        );
        write(&dir.join("Cookies"), &format!("cookie-{secret}"));
    }

    fn conversation(dir: &Path, id: &str, cli: &str) {
        write(
            &dir.join(format!("{id}.json")),
            &json!({"sessionId": id, "cliSessionId": cli, "lastActivityAt": 100}).to_string(),
        );
    }

    /// Account a signed in, with three conversations and its settings; b's
    /// sign-in parked.
    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::for_home(root.path());
        let mut registry = Registry::default();
        registry
            .add(account("a", Some("acc-a"), Some("org-a")))
            .unwrap();
        registry
            .add(account("b", Some("acc-b"), Some("org-b")))
            .unwrap();
        let live = &layout.desktop_dir;
        sign_in(live, "acc-a", "token-a");
        write(&live.join("window-state.json"), r#"{"x":1}"#);
        write(
            &layout.desktop_config_file(),
            &json!({"preferences": {"epitaxyPrefs": {
                "code-projects-order.acc-a": ["/w/one", "/w/two"],
                "dframe-local-slice": {"pinnedOrder": ["code:local_1"]},
                "dframe-code-sections": {"acc-a/org-a": {"sections": [{"id": "pinned"}]}}
            }}})
            .to_string(),
        );
        write(
            &layout.desktop_presets_file(),
            &json!({"byAccount": {"acc-a": {"recent": ["p"]}}}).to_string(),
        );
        let list = live.join("claude-code-sessions/acc-a/org-a");
        for n in 1..=3 {
            conversation(&list, &format!("local_{n}"), &format!("cli-{n}"));
        }
        write(
            &list.join("scheduled-tasks.json"),
            r#"{"scheduledTasks":[]}"#,
        );
        sign_in(&layout.identity_dir("b"), "acc-b", "token-b");
        desktop_profile::set_owner(&layout, "a").unwrap();
        Fixture {
            _root: root,
            layout,
            registry,
        }
    }

    /// Every file under `dir`, by relative path.
    fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    let rel = path
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    /// Whether every value of `part` is in `whole`, objects allowed to have
    /// more keys: the entries a switch adds for the other account.
    fn contains(whole: &serde_json::Value, part: &serde_json::Value) -> bool {
        match (whole, part) {
            (serde_json::Value::Object(whole), serde_json::Value::Object(part)) => part
                .iter()
                .all(|(key, value)| whole.get(key).is_some_and(|w| contains(w, value))),
            _ => whole == part,
        }
    }

    fn prefs(layout: &Layout) -> serde_json::Value {
        fsutil::read_json(&layout.desktop_config_file())
            .unwrap()
            .unwrap()["preferences"]["epitaxyPrefs"]
            .clone()
    }

    #[test]
    fn the_plan_says_what_will_move_and_changes_nothing() {
        let f = fixture();
        let before = tree(f.layout.home.as_path());

        let plan = plan(&f.layout, &f.registry, "b").unwrap();

        assert_eq!(plan.from, "a");
        assert_eq!(plan.to, "b");
        assert_eq!(plan.sign_in, SignIn::Parked);
        assert_eq!(plan.conversations, 3);
        assert!(plan.own.is_empty());
        assert_eq!(plan.preference_keys, 2);
        assert_eq!(tree(f.layout.home.as_path()), before);
    }

    #[test]
    fn after_a_switch_b_sees_a_s_list_and_settings() {
        let f = fixture();

        let applied = apply(&f.layout, &f.registry, "b", None, false, NOW).unwrap();

        let live = &f.layout.desktop_dir;
        assert_eq!(desktop_identity::account_of(live).as_deref(), Some("acc-b"));
        assert_eq!(
            fs::read_to_string(live.join("Cookies")).unwrap(),
            "cookie-token-b"
        );
        assert_eq!(
            desktop_identity::account_of(&f.layout.identity_dir("a")).as_deref(),
            Some("acc-a")
        );
        let list = live.join("claude-code-sessions/acc-b/org-b");
        for n in 1..=3 {
            assert!(list.join(format!("local_{n}.json")).is_file());
        }
        assert!(!live.join("claude-code-sessions/acc-a/org-a").exists());
        assert_eq!(
            prefs(&f.layout)["code-projects-order.acc-b"],
            json!(["/w/one", "/w/two"])
        );
        assert_eq!(
            prefs(&f.layout)["dframe-code-sections"]["acc-b/org-b"],
            json!({"sections": [{"id": "pinned"}]})
        );
        let presets = fsutil::read_json(&f.layout.desktop_presets_file())
            .unwrap()
            .unwrap();
        assert_eq!(presets["byAccount"]["acc-b"], json!({"recent": ["p"]}));
        assert_eq!(
            fs::read_to_string(live.join("window-state.json")).unwrap(),
            r#"{"x":1}"#
        );
        assert_eq!(
            desktop_profile::owner(&f.layout, &f.registry).as_deref(),
            Some("b")
        );
        assert!(!f.layout.workspace_switch_journal().exists());
        assert!(applied
            .backup
            .join("claude-code-sessions/acc-a/org-a/local_1.json")
            .is_file());
        assert!(!applied.backup.join("Cookies").exists());
        assert!(!applied.backup.join("config.json").exists());
        assert_eq!(applied.index.moved, 3);
        assert!(!applied.signed_out);
    }

    #[test]
    fn a_round_trip_brings_the_same_workspace_back() {
        let f = fixture();
        let mut before = tree(&f.layout.desktop_dir);
        let config_before = before.remove("claude_desktop_config.json").unwrap();
        before.remove("ccd-session-presets.json");

        apply(&f.layout, &f.registry, "b", None, false, NOW).unwrap();
        apply(&f.layout, &f.registry, "a", None, false, NOW + 60).unwrap();

        let mut after = tree(&f.layout.desktop_dir);
        let config_after = after.remove("claude_desktop_config.json").unwrap();
        after.remove("ccd-session-presets.json");
        assert_eq!(after, before);
        let original: serde_json::Value = serde_json::from_slice(&config_before).unwrap();
        let back: serde_json::Value = serde_json::from_slice(&config_after).unwrap();
        assert!(contains(&back, &original), "{back}");
        assert!(desktop_identity::is_parked(&f.layout, "b"));
    }

    #[test]
    fn a_running_desktop_stops_the_switch_before_anything_moves() {
        let f = fixture();
        let before = tree(f.layout.home.as_path());

        let error = apply(&f.layout, &f.registry, "b", None, true, NOW).unwrap_err();

        assert!(matches!(error, Error::DesktopStillRunning));
        assert_eq!(tree(f.layout.home.as_path()), before);
    }

    #[test]
    fn a_switch_cut_after_any_step_is_undone() {
        for step in [
            Step::Backup,
            Step::ParkSource,
            Step::PutTargetBack,
            Step::CarryIndex,
            Step::CarryPreferences,
            Step::RecordOwner,
        ] {
            let f = fixture();
            let before = tree(&f.layout.desktop_dir);

            let cut = apply_with(&f.layout, &f.registry, "b", None, false, NOW, Some(step));
            assert!(cut.is_err(), "{step:?}");
            assert!(f.layout.workspace_switch_journal().exists(), "{step:?}");

            assert_eq!(
                roll_back_interrupted(&f.layout).unwrap().as_deref(),
                Some("a"),
                "{step:?}"
            );
            assert_eq!(tree(&f.layout.desktop_dir), before, "{step:?}");
            assert!(desktop_identity::is_parked(&f.layout, "b"), "{step:?}");
            assert!(!desktop_identity::is_parked(&f.layout, "a"), "{step:?}");
            assert_eq!(
                desktop_profile::owner(&f.layout, &f.registry).as_deref(),
                Some("a"),
                "{step:?}"
            );
            assert!(!f.layout.workspace_switch_journal().exists(), "{step:?}");
        }
    }

    #[test]
    fn an_interrupted_switch_is_undone_before_the_next_one() {
        let f = fixture();
        let before = tree(&f.layout.desktop_dir);
        let _ = apply_with(
            &f.layout,
            &f.registry,
            "b",
            None,
            false,
            NOW,
            Some(Step::CarryIndex),
        );

        let error = apply(&f.layout, &f.registry, "b", None, false, NOW + 5).unwrap_err();

        assert!(matches!(error, Error::SwitchInterrupted));
        assert_eq!(tree(&f.layout.desktop_dir), before);
        assert!(apply(&f.layout, &f.registry, "b", None, false, NOW + 10).is_ok());
    }

    #[test]
    fn a_step_that_fails_undoes_the_switch_at_once() {
        let f = fixture();
        write(
            &f.layout.identity_dir("a").join("Cookies"),
            "an older parked cookie",
        );
        let before = tree(&f.layout.desktop_dir);

        let error = apply(&f.layout, &f.registry, "b", None, false, NOW).unwrap_err();

        assert!(matches!(error, Error::Conflict(_)));
        assert_eq!(tree(&f.layout.desktop_dir), before);
        assert_eq!(
            fs::read_to_string(f.layout.identity_dir("a").join("Cookies")).unwrap(),
            "an older parked cookie"
        );
        assert!(desktop_identity::is_parked(&f.layout, "b"));
        assert!(!f.layout.workspace_switch_journal().exists());
    }

    #[test]
    fn unreadable_settings_undo_the_switch_and_stay_as_they_were() {
        let f = fixture();
        write(&f.layout.desktop_config_file(), "{not json");
        let before = tree(&f.layout.desktop_dir);

        assert!(apply(&f.layout, &f.registry, "b", None, false, NOW).is_err());

        assert_eq!(tree(&f.layout.desktop_dir), before);
        assert!(desktop_identity::is_parked(&f.layout, "b"));
        assert!(!f.layout.workspace_switch_journal().exists());
    }

    #[test]
    fn nothing_to_undo_without_a_journal() {
        let f = fixture();
        assert_eq!(roll_back_interrupted(&f.layout).unwrap(), None);
    }

    #[test]
    fn an_account_without_a_sign_in_starts_signed_out() {
        let f = fixture();
        fs::remove_dir_all(f.layout.identity_dir("b")).unwrap();
        assert_eq!(
            plan(&f.layout, &f.registry, "b").unwrap().sign_in,
            SignIn::None
        );

        let applied = apply(&f.layout, &f.registry, "b", None, false, NOW).unwrap();

        assert!(applied.signed_out);
        assert!(!f.layout.desktop_dir.join("config.json").exists());
        assert!(!f.layout.desktop_dir.join("Cookies").exists());
        assert!(desktop_identity::is_parked(&f.layout, "a"));
        assert!(f
            .layout
            .desktop_dir
            .join("claude-code-sessions/acc-b/org-b/local_1.json")
            .is_file());
        assert_eq!(
            prefs(&f.layout)["code-projects-order.acc-b"],
            json!(["/w/one", "/w/two"])
        );
    }

    #[test]
    fn the_old_folder_gives_its_sign_in_on_the_first_switch() {
        let f = fixture();
        fs::remove_dir_all(f.layout.identity_dir("b")).unwrap();
        let old = f.layout.desktop_profile_dir("b");
        sign_in(&old, "acc-b", "token-b");
        conversation(
            &old.join("claude-code-sessions/acc-b/org-b"),
            "local_old",
            "cli-old",
        );
        assert_eq!(
            plan(&f.layout, &f.registry, "b").unwrap().sign_in,
            SignIn::OldFolder
        );

        apply(&f.layout, &f.registry, "b", None, false, NOW).unwrap();

        assert_eq!(
            fs::read_to_string(f.layout.desktop_dir.join("Cookies")).unwrap(),
            "cookie-token-b"
        );
        assert!(!old.exists());
        let archive = f.layout.desktop_archive_dir("b", "2026-11-01");
        assert!(archive
            .join("claude-code-sessions/acc-b/org-b/local_old.json")
            .is_file());
    }

    #[test]
    fn missing_ids_are_refused() {
        let mut f = fixture();
        f.registry.remove("b").unwrap();
        f.registry.add(account("b", Some("acc-b"), None)).unwrap();

        assert!(matches!(
            plan(&f.layout, &f.registry, "b"),
            Err(Error::MissingAccountIds(alias)) if alias == "b"
        ));
    }

    #[test]
    fn the_sign_in_in_place_decides_where_the_switch_starts() {
        let mut f = fixture();
        f.registry
            .add(account("c", Some("acc-c"), Some("org-c")))
            .unwrap();
        desktop_profile::set_owner(&f.layout, "c").unwrap();

        assert_eq!(plan(&f.layout, &f.registry, "b").unwrap().from, "a");

        fs::remove_file(f.layout.desktop_dir.join("config.json")).unwrap();
        sign_in(&f.layout.desktop_dir.join("x"), "acc-z", "token-z");
        fs::rename(
            f.layout.desktop_dir.join("x/config.json"),
            f.layout.desktop_dir.join("config.json"),
        )
        .unwrap();
        assert!(matches!(
            plan(&f.layout, &f.registry, "b"),
            Err(Error::UnknownSignedInAccount(uuid)) if uuid == "acc-z"
        ));
    }

    #[test]
    fn a_parked_sign_in_of_another_account_is_refused() {
        let f = fixture();
        fs::remove_dir_all(f.layout.identity_dir("b")).unwrap();
        sign_in(&f.layout.identity_dir("b"), "acc-z", "token-z");

        assert!(matches!(
            plan(&f.layout, &f.registry, "b"),
            Err(Error::Conflict(_))
        ));
    }

    #[test]
    fn own_conversations_need_a_choice_before_anything_moves() {
        let f = fixture();
        conversation(
            &f.layout
                .desktop_dir
                .join("claude-code-sessions/acc-b/org-b"),
            "local_own",
            "cli-own",
        );
        let before = tree(f.layout.home.as_path());
        assert_eq!(
            plan(&f.layout, &f.registry, "b").unwrap().own,
            ["local_own"]
        );

        let error = apply(&f.layout, &f.registry, "b", None, false, NOW).unwrap_err();
        assert!(matches!(error, Error::OwnConversationsNeedChoice(1)));
        assert_eq!(tree(f.layout.home.as_path()), before);

        let applied = apply(
            &f.layout,
            &f.registry,
            "b",
            Some(OwnConversations::PutAside),
            false,
            NOW,
        )
        .unwrap();
        assert_eq!(applied.index.own_set_aside, 1);
        assert!(f
            .layout
            .set_aside_dir(&NOW.to_string())
            .join("claude-code-sessions/acc-b/org-b/local_own.json")
            .is_file());
    }

    #[test]
    fn switching_to_the_account_in_place_changes_nothing() {
        let f = fixture();
        let before = tree(f.layout.home.as_path());

        let applied = apply(&f.layout, &f.registry, "a", None, false, NOW).unwrap();

        assert!(applied.plan.is_noop());
        assert_eq!(tree(f.layout.home.as_path()), before);
    }

    #[test]
    fn a_start_is_proven_by_a_usage_sample_of_the_org() {
        let f = fixture();
        write(
            &f.layout.desktop_usage_history(),
            &json!({"version": 1, "samples": [
                {"t": (NOW - 100) * 1000, "org": "org-b"},
                {"t": NOW + 5, "org": "org-a"},
                {"t": (NOW + 7) * 1000, "org": "org-b"}
            ]})
            .to_string(),
        );

        assert!(signed_in_since(&f.layout, "org-b", NOW));
        assert!(signed_in_since(&f.layout, "org-a", NOW));
        assert!(!signed_in_since(&f.layout, "org-b", NOW + 10));
        assert!(!signed_in_since(&f.layout, "org-c", 0));
    }
}
