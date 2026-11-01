# Desktop workspace switch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One click moves Claude Desktop to another account while its workspace (sidebar, pins, folders, panes, window) stays exactly as it was.

**Architecture:** The Desktop data folder stays in place for good. Each account's sign-in is four files that Relais parks and puts back by renaming only. The few pieces of workspace state Desktop keys by account are carried from the source account to the target by a table of rules, inside a journaled, backed-up, reversible switch.

**Tech Stack:** Rust (relais-core, relais-cli with clap), serde_json, tempfile for tests; Tauri 2 + React for the panel.

**Spec:** `docs/superpowers/specs/2026-11-01-desktop-workspace-switch-design.md`

## Global Constraints

- Identity files `config.json`, `config.json.journal`, `Cookies`, `Cookies-journal` are only ever renamed: never opened, read, copied, hashed, logged, or put in a backup.
- Files Desktop also writes (`claude_desktop_config.json`, `ccd-session-presets.json`) are rewritten atomically with `fsutil::write_json_atomic`, and only while Desktop is closed.
- Never call Anthropic's servers; never drive Claude's interface; never switch without the user's click.
- Fail open: any failure leaves Desktop able to start on the source account.
- Nothing is deleted: replaced or set-aside entries are moved under `~/.relais/backups/…` or `~/.relais/set-aside/…`.
- Every user-facing string goes through the CLI translation table (`crates/relais-cli/locales/{en,fr}.json`) or the app's `i18n.ts`, English and French.
- Tests use made-up UUIDs and `example.com` addresses only; test first.
- Code, identifiers, comments, file names in English. Commits carry no AI signature.
- `cargo test` and `cargo clippy --all-targets -- -D warnings` green before every commit (`export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"`).

## Review Focus

- Desktop does not close within 20 s → nothing has moved, the message says so.
- A parked identity already sits where one is about to be parked → the switch stops before anything moves; nothing is overwritten.
- The switch is interrupted between two steps (crash, kill, power) → the next switch or `relais desktop` rolls it back to the source account, workspace identical to the backup.
- `claude_desktop_config.json` lacks `preferences` / `epitaxyPrefs`, or holds unexpected types → those rules carry nothing, every other key survives byte-for-byte in value.
- The same conversation exists in both accounts' index folders → the later `lastActivityAt` stays in the list, the other copy is kept under the switch's backup, never lost.

---

### Task 0: Land pending work

**Files:** the uncommitted `account adopt` work already in the tree (README.md, crates/relais-cli/{locales,src,tests}, crates/relais-core/src/{error,service,usage}.rs), `docs/desktop-session-continuity.md`, the spec and this plan.

- [ ] **Step 1:** `cargo test` → 183 passed; `cargo clippy --all-targets -- -D warnings` → no warning.
- [ ] **Step 2:** Commit the adopt work alone: `git commit -m "Adopt the account someone signed in on the default profile"`; then the docs: `git commit -m "Design the Desktop switch that keeps one workspace"`.

### Task 1: Park and put back a sign-in

**Files:**
- Create: `crates/relais-core/src/claude/desktop_identity.rs` (register in `claude/mod.rs`)
- Modify: `crates/relais-core/src/layout.rs` (paths below)

**Interfaces:**
- Produces:
  - `Layout::identity_dir(&self, alias: &str) -> PathBuf` = `<relais>/accounts/<alias>/identity`
  - `Layout::desktop_archive_dir(&self, alias: &str, date: &str) -> PathBuf` = `<relais>/accounts/<alias>/desktop-archive-<date>`
  - `pub const IDENTITY_FILES: [&str; 4] = ["config.json", "config.json.journal", "Cookies", "Cookies-journal"];`
  - `pub fn park(layout: &Layout, alias: &str) -> Result<usize>` — moves the identity files present in `layout.desktop_dir` into `identity_dir(alias)`; `Error::Conflict(path)` before moving anything if `identity_dir(alias)` already holds one of them; returns how many moved.
  - `pub fn put_back(layout: &Layout, alias: &str) -> Result<usize>` — the reverse; `Error::Conflict` if `desktop_dir` still holds one.
  - `pub fn is_parked(layout: &Layout, alias: &str) -> bool` — `config.json` or `Cookies` in `identity_dir(alias)`.
  - `pub fn account_of(dir: &Path) -> Option<String>` — the `lastKnownAccountUuid` of `dir/config.json`, read with a streaming `serde` visitor that keeps that one string and skips every other value with `IgnoredAny`. `None` when the file or field is missing or malformed.
  - `pub fn take_from_old_folder(layout: &Layout, alias: &str, date: &str) -> Result<bool>` — when `identity_dir(alias)` is empty and `desktop_profile_dir(alias)` holds identity files: move them into `identity_dir(alias)`, then rename the rest of the folder to `desktop_archive_dir(alias, date)`. `Ok(false)` when there is nothing to take.

- [ ] **Step 1: Failing tests** in the module: `parking_then_putting_back_keeps_every_byte` (writes `"token-a"` / `"cookie-a"` markers, parks, asserts the files are gone from the workspace and equal in `identity_dir`, puts back, asserts equal again); `a_parked_sign_in_is_never_overwritten` (pre-existing `identity/Cookies` → `Error::Conflict`, workspace files still in place); `missing_journals_are_fine` (only `config.json` and `Cookies` → `Ok(2)`); `the_old_folder_gives_its_sign_in_and_becomes_an_archive` (old `desktop/` with identity files and `claude-code-sessions/x` → identity files in `identity/`, `desktop-archive-2026-11-01/claude-code-sessions/x` exists, `desktop/` gone); `an_account_already_parked_keeps_its_old_folder` (`Ok(false)`, nothing moves); `the_account_id_is_read_and_nothing_else` (`{"oauth:tokenCacheV2":"secret-blob","lastKnownAccountUuid":"acc-a","nested":{"x":[1,2]}}` → `Some("acc-a")`; missing field, missing file, `"lastKnownAccountUuid": 3`, invalid JSON → `None`).
- [ ] **Step 2:** `cargo test -p relais-core desktop_identity` → fails (module missing).
- [ ] **Step 3:** Implement with `std::fs::rename` only (create parent dirs first).
- [ ] **Step 4:** Tests pass, clippy clean.
- [ ] **Step 5:** Commit `Park and put back Claude Desktop's sign-in, by renaming its four files`.

### Task 2: Carry the preferences Desktop keys by account

**Files:**
- Create: `crates/relais-core/src/claude/desktop_carry.rs`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct AccountIds { pub account: String, pub org: String }`
  - `pub fn carry_preferences(config: &mut serde_json::Value, from: &AccountIds, to: &AccountIds) -> usize` — on `claude_desktop_config.json`'s value; returns keys written. Rules, as one `const` table the function walks:
    - in `preferences.epitaxyPrefs`: each key ending in `.<from.account>` → same key ending in `.<to.account>` takes a clone of its value;
    - in `preferences.epitaxyPrefs["dframe-code-sections"]` and `["dframe-group-scopes"]`: entry `"<from.account>/<from.org>"` → entry `"<to.account>/<to.org>"`;
    - in `preferences`: each object key ending in `ByAccount` with a `<from.account>` entry → its `<to.account>` entry.
    A rule whose source is absent leaves the target as it is. Non-object shapes are skipped.
  - `pub fn carry_presets(presets: &mut serde_json::Value, from: &AccountIds, to: &AccountIds) -> bool` — `byAccount[from.account]` → `byAccount[to.account]`.

- [ ] **Step 1: Failing tests:** `suffixed_keys_follow_the_account` (`code-projects-order.acc-a` = `["/w/one","/w/two"]` → `code-projects-order.acc-b` equal, `.acc-a` still there; `ultrathinkAvailable.claude-x.acc-a` = `true` carried too); `scoped_sections_follow_the_account_and_org` (`dframe-code-sections["acc-a/org-a"]` → `["acc-b/org-b"]` equal); `by_account_maps_follow` (`bypassPermissionsOptInByAccount.acc-a` → `.acc-b`); `unrelated_keys_and_other_accounts_are_untouched` (whole value equal except the added keys; `code-projects-order.acc-c` untouched); `odd_shapes_carry_nothing_and_break_nothing` (`preferences` = `"x"`, `epitaxyPrefs` missing, `dframe-code-sections` = `[]` → returns 0, value unchanged); `presets_follow_the_account`.
- [ ] **Step 2:** Run → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass, clippy clean.
- [ ] **Step 5:** Commit `Carry the preferences Claude Desktop keys by account`.

### Task 3: Carry the conversation list

**Files:**
- Modify: `crates/relais-core/src/claude/desktop_carry.rs`

**Interfaces:**
- Consumes: `AccountIds`.
- Produces:
  - `pub fn index_dir(root: &Path, ids: &AccountIds) -> PathBuf` = `root/<account>/<org>` (root is `claude-code-sessions` or `local-agent-mode-sessions` in the workspace).
  - `pub fn own_conversations(source: &Path, target: &Path) -> Vec<String>` — sorted `local_*.json` file stems in `target` absent from `source`.
  - `pub enum OwnConversations { Add, PutAside(PathBuf) }`
  - `#[derive(Debug, Default, PartialEq, Eq)] pub struct IndexReport { pub moved: usize, pub kept_newer: usize, pub own_added: usize, pub own_set_aside: usize }`
  - `pub fn carry_index(source: &Path, target: &Path, own: &OwnConversations, replaced: &Path) -> Result<IndexReport>`:
    - `PutAside(dir)`: first move each own `local_<id>.json`, and any entry named `local_<id>` or `deleted_<id>…`, from `target` to `dir`;
    - `target` missing → rename `source` to `target` (`moved` = entries);
    - else each entry of `source`: absent from `target` → rename; `local_*.json` on both sides → keep the one with the later `lastActivityAt` (source on tie, or when either is unreadable), move the other under `replaced`; `scheduled-tasks.json` → `scheduledTasks` merged by `id` (source wins), other fields from source; `archived-sessions.idx` (`{"v":1,"archived":[…]}`) → union keeping source order first; any other entry on both sides → source wins, target's under `replaced`; finally remove the emptied `source` dir;
    - `source` missing → `Ok(IndexReport::default())`.

- [ ] **Step 1: Failing tests:** `an_empty_target_takes_the_whole_list`; `the_later_activity_wins_and_the_other_copy_is_kept` (both have `local_s1.json`, target newer → target stays, source copy in `replaced/`); `scheduled_tasks_and_archives_are_merged`; `own_conversations_are_added_or_put_aside` (two cases; put aside → files in the given dir, absent from target); `a_missing_source_changes_nothing`.
- [ ] **Step 2:** Run → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass, clippy clean.
- [ ] **Step 5:** Commit `Carry Claude Desktop's conversation list to the next account`.

### Task 4: The journaled switch

**Files:**
- Create: `crates/relais-core/src/claude/desktop_switch.rs`
- Modify: `crates/relais-core/src/layout.rs` (`workspace_switch_journal()` = `<relais>/workspace-switch.json`, `workspace_backup_dir(stamp)` = `<relais>/backups/workspace/<stamp>`, `set_aside_dir(stamp)` = `<relais>/set-aside/<stamp>`, `desktop_config_file()` = `<desktop>/claude_desktop_config.json`, `desktop_presets_file()` = `<desktop>/ccd-session-presets.json`), `crates/relais-core/src/error.rs` (`MissingAccountIds(String)`, `SwitchInterrupted`)
- Modify: `crates/relais-core/src/claude/desktop_profile.rs`: remove the folder swap (`switch`, `Switch`, its tests); `has_folder` becomes "in place, or `desktop_identity::is_parked`, or an old folder".

**Interfaces:**
- Consumes: Tasks 1–3; `desktop_profile::{owner, set_owner}`.
- Produces:
  - `pub enum SignIn { Parked, OldFolder, None }` (where the target's sign-in comes from)
  - `pub struct SwitchPlan { pub from: String, pub to: String, pub from_ids: AccountIds, pub to_ids: AccountIds, pub sign_in: SignIn, pub conversations: usize, pub own: Vec<String>, pub preference_keys: usize }`
  - `pub fn plan(layout: &Layout, registry: &Registry, to: &str) -> Result<SwitchPlan>` — reads only. The source is the registered account whose `account_uuid` equals `desktop_identity::account_of(desktop_dir)`; without that field, `desktop_profile::owner`. `UnknownSignedInAccount(uuid)` when the id is not registered, `UnknownDesktopOwner` without any owner, `MissingAccountIds(alias)` when an id is missing, `UnknownAccount`. A parked identity of B whose `account_of` is another account → `Conflict`.
  - `pub fn signed_in_since(layout: &Layout, org: &str, since: i64) -> bool` — a sample of `plan-usage-history.json` with `org` and `t >= since` (seconds or milliseconds, as the file mixes).
  - `pub enum OwnChoice { Add, PutAside }` (own records whose `cliSessionId` the source already lists are always put aside)
  - `pub struct Applied { pub index: IndexReport, pub cowork: IndexReport, pub preference_keys: usize, pub backup: PathBuf, pub signed_out: bool }`
  - `pub fn apply(layout: &Layout, plan: &SwitchPlan, own: OwnChoice, desktop_running: bool, now: i64) -> Result<Applied>` — `DesktopStillRunning` when `desktop_running`. Steps, each recorded in the journal before it runs: `Backup` (copy config, presets, both accounts' index dirs of both roots), `TakeOldFolder`, `ParkSource`, `PutTargetBack` (skipped when `SignIn::None`), `CarryIndex`, `CarryPreferences`, `RecordOwner`; then the journal is removed.
  - `pub fn roll_back_interrupted(layout: &Layout) -> Result<Option<String>>` — with a journal present: undo recorded steps in reverse (owner back, files and index dirs restored from the backup with current ones moved under `<backup>/rolled-back/`, target parked again, source put back), remove the journal, return the source alias. `apply` calls it first and fails with `SwitchInterrupted` if it had to roll back, so the caller re-plans.

- [ ] **Step 1: Failing tests** (temp layout, two accounts `acc-a/org-a`, `acc-b/org-b`, a fake workspace with identity markers, an index of 3 conversations, prefs with `code-projects-order.acc-a`): `a_round_trip_brings_the_same_workspace_back` (A→B→A: identity markers back, index under A with the same 3 files, prefs value equal to the original plus the `.acc-b` keys); `after_a_switch_b_sees_a_s_list_and_preferences`; `a_running_desktop_stops_the_switch_before_anything_moves`; `a_switch_cut_after_each_step_rolls_back_to_the_source` (for each step: apply with an injected failure after it, then `roll_back_interrupted` → workspace equal to the initial tree, identity markers of A in place); `an_account_without_a_sign_in_starts_signed_out` (`signed_out: true`, A parked, prefs carried); `the_old_folder_gives_its_sign_in_on_the_first_switch`; `missing_ids_are_refused`.
  The failure injection is a `#[cfg(test)]` hook (`fail_after: Option<Step>`) passed through a private `apply_with`.
- [ ] **Step 2:** Run → fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass, `cargo test` all green, clippy clean.
- [ ] **Step 5:** Commit `Switch Claude Desktop's account and keep its workspace, journaled and reversible`.

### Task 5: `relais desktop`

**Files:**
- Modify: `crates/relais-cli/src/main.rs` (`Desktop { alias: Option<String>, #[arg(long)] dry_run: bool, #[arg(long, value_enum)] own: Option<Own> }` with `Own::{Add, Aside}`), `crates/relais-cli/src/commands.rs::desktop`, locales en/fr, `crates/relais-cli/tests/cli.rs`.

**Interfaces:**
- Consumes: `desktop_switch::{plan, apply, roll_back_interrupted}`, `DesktopApp`, `wait_for_sign_in`.
- Behavior: no alias → one line per account (in place / ready / sign-in needed). `--dry-run` → prints the plan (conversations carried, own conversations, preference keys, where the sign-in comes from) and changes nothing, Desktop left alone. Own conversations present and no `--own` → prints them and asks to rerun with `--own add` or `--own aside` (exit 2). Inside Desktop (`CLAUDE_CODE_ENTRYPOINT=claude-desktop`) and not `--dry-run` → refuse (exit 2) and point to the panel or a Terminal. Otherwise: quit Desktop, apply, set the terminal's active account to the target (`registry.set_active`, save), open Desktop, wait up to 60 s for `signed_in_since(target org, launch time)`, then the log as a second source, and say which.

- [ ] **Step 1: Failing e2e tests** with `RELAIS_DESKTOP_CONTROL` script and a fake home: `dry_run_changes_nothing`; `the_switch_keeps_the_list_and_moves_the_terminal_too`; `own_conversations_need_a_choice`; `an_interrupted_switch_is_rolled_back_by_the_next_command`; `inside_desktop_only_the_preview_runs`.
- [ ] **Step 2:** Run → fail. **Step 3:** Implement. **Step 4:** Pass, clippy clean.
- [ ] **Step 5:** Commit `relais desktop: switch with a preview, and the terminal follows`.

### Task 6: First real switch (before the panel)

- [ ] **Step 1:** Install the CLI (`cargo install --path crates/relais-cli --locked`); `relais desktop <next> --dry-run` on the real folders: the plan names the right sign-in in place, every conversation of the list, sign-in from the old folder, no own conversations; `ls -lR` of the Desktop folder and `~/.relais/accounts` identical before and after.
- [ ] **Step 2:** With the user's go-ahead only, run from Terminal.app (not from a Desktop session): sidebar screenshot before, `relais desktop <next>`, screenshot after, compare; usage sample seen for the next account's org. Anything different: switch back and report.

### Task 7: The panel

**Files:**
- Modify: `app/src-tauri/src/commands.rs` (`desktop_plan(alias) -> DesktopPlanView { conversations, own, signIn }`, `switch_desktop(alias, own: Option<String>)` on the new core, terminal follows), `app/src/App.tsx`, `app/src/api.ts`, `app/src/i18n.ts`, `app/src/styles.css`, `app/tests/*.test.ts`.

- [ ] **Step 1:** Load the `neoffice-design` skill; take tokens (colors, type, radius) from it.
- [ ] **Step 2: Failing UI tests** (node test runner, existing pattern): the confirmation text for a plan without own conversations; with own conversations it shows the two choices; progress labels; French and English strings exist for every new key.
- [ ] **Step 3:** Home = account rows (color, name, two bars with when they come back, "Desktop" mark on the account in place, "Switch to …" button on the others); one confirmation sheet; one progress line; result line. Everything else behind one menu. Remove the conversation list and the per-account page from the default view.
- [ ] **Step 4:** `pnpm test`, `pnpm build`, `cargo build -p relais-app` green.
- [ ] **Step 5:** Commit `A one-screen panel: every account, one button to switch`.

### Task 8: Install the panel, document

- [ ] **Step 1:** Build the app, replace `/Applications/Relais.app`, keep the previous one in `~/.relais/backups/Relais-codex.app`.
- [ ] **Step 2:** Obsidian note, hub line, log entry.
