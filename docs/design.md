# One workspace, many accounts: switching Claude Desktop without losing anything

Status: built, and checked by a real switch between two accounts on 2026-11-01.

## Intent

When an account runs out, the user clicks one button and Claude Desktop comes back on another account
**exactly as it was**: the same conversations in the sidebar, the same pinned ones in the same order, the
same folders in the same order and folded the same way, the same open panes and window. Nothing to put back
by hand, ever.

Today it does not work because Relais gives each account a whole Desktop data folder and swaps folders.
That folder is not only the sign-in: it is the whole workspace, so every switch changes workspace, and the
previous attempt then tried to rebuild the workspace afterwards (copying session files, then driving the
app's interface). The fix is to stop moving the workspace at all.

## The model

- **The workspace** is the Desktop data folder in place (`~/Library/Application Support/Claude`). It never
  moves again. It belongs to the user, not to an account.
- **An identity** is what makes Desktop signed in as one account. It is four files at the root of the data
  folder: `config.json`, `config.json.journal`, `Cookies`, `Cookies-journal` (whichever exist).
  `config.json` holds the encrypted OAuth token cache and `lastKnownAccountUuid`; `Cookies` holds the
  claude.ai session cookies. No other place holds the claude.ai session (checked on Desktop 3.6556.2: the only
  partition has no claude.ai cookie, `Local State` holds no key).
- **A parked identity** lives in `~/.relais/accounts/<alias>/identity/`, same file names.
- Relais **only renames** identity files (same volume, atomic). It never copies, hashes, logs or backs them up.
  One exception, which the repository rules allow (account ids are non-secret metadata): it reads the single
  field `lastKnownAccountUuid` of a `config.json`, with a streaming reader that skips every other value unread.
  That is how Relais knows whose sign-in a set of files is.
- The other keys of `config.json` (locale, boot placeholders, update bookkeeping) follow the account. They are
  the same on every account here (`fr-FR`) or cosmetic (the first frame before the app paints): accepted.

## What Desktop keeps per account inside the workspace

Desktop is built for accounts taking turns in one folder: some of its state is keyed by account. On a switch,
that state must follow the user from the source account to the target account, or the target sees a default
workspace. Observed on Desktop 3.6556.2 (in its code and on disk):

| What the user sees | Where | Key |
|---|---|---|
| Code conversations list | `claude-code-sessions/<account>/<org>/` (`local_*.json`, `scheduled-tasks.json`, `archived-sessions.idx`, `waiting-input/`) | path |
| Cowork tasks | `local-agent-mode-sessions/<account>/<org>/` | path |
| Folder (project) order | `claude_desktop_config.json` → `preferences.epitaxyPrefs["code-projects-order.<account>"]` | key suffix |
| Sidebar sections (pinned, routines, sessions: shown, folded, sort) | `epitaxyPrefs["dframe-code-sections"]["<account>/<org>"]` | map key |
| Custom groups | `epitaxyPrefs["dframe-group-scopes"]["<account>/<org>"]` | map key |
| Folder permission modes and their acknowledgements | `epitaxyPrefs["epitaxy-folder-permission-mode(-v2).<account>"]`, `["epitaxy-perm-mode-acks(-v2).<account>"]`, other `*.<account>` keys | key suffix |
| Per-account switches | `preferences.*ByAccount["<account>"]` | map key |
| Session presets | `ccd-session-presets.json` → `byAccount["<account>"]` | map key |

Not keyed by account, so already identical after a switch: transcripts (`~/.claude/projects`), pinned order
and starred sessions (`epitaxyPrefs["dframe-local-slice"]`, `["starred-local-code-sessions"]`), folded groups,
grouping and sort (`Local Storage`, `dframe-store`), open panes (`epitaxyPrefs["desktop-frame.paneStore.v1"]`),
window (`window-state.json`).

Deliberately **not carried** (they belong to the account's own registration, Desktop recreates them):
`ant-device-registry.json`, `remote-control-state.json`, `spaces-present/`, `local-agent-mode-sessions/skills-plugin/`,
`plan-usage-history.json` (already per organization, read for usage).

The renderer reads `epitaxyPrefs` first and falls back to `localStorage` (`persisted.<key>`), so rewriting
`claude_desktop_config.json` is enough for the keyed preferences; `Local Storage` is never written.

The carry list is data, not scattered code: one table of rules (path rules, key-suffix rules, map-key rules)
so that a Desktop update that adds a keyed preference is one line to add.

## A switch, from account A to account B

Preconditions: A and B are in the registry with `accountUuid` and `orgId`; B has a parked identity (else see
"An account without identity"). The sign-in in place is checked before anything moves: its
`lastKnownAccountUuid` must be A's. If it is another registered account C (the user signed in to C directly in
Desktop), the switch parks it under C, carries from C, and says so. If it is an account Relais does not know,
the switch stops and asks to add it first. The same check applies to a parked identity before it is put back.

1. **Plan.** Compute everything that will move and show it (also what `--dry-run` prints): identity files,
   session entries carried, entries B already has of its own, preference keys rewritten.
2. **Close Desktop.** Ask it to quit (SIGTERM, as today) and wait up to 20 s. Still running: stop, nothing
   has changed. Sessions that were working stop with the app; Desktop offers to continue them after.
3. **Journal.** Write `~/.relais/workspace-switch.json` with the source, target and the ordered steps. Every
   step is idempotent, so an interrupted switch is finished (or undone) by running it again.
4. **Backup.** Copy the files that are about to change (`claude_desktop_config.json`,
   `ccd-session-presets.json`, both accounts' session index folders) to
   `~/.relais/backups/workspace/<timestamp>/`. Identity files are never part of a backup.
5. **Park A's identity** in `~/.relais/accounts/A/identity/`.
6. **Put B's identity in place.**
7. **Carry the keyed state from A to B**:
   - session index: B's folder doesn't exist → rename A's folder to B's. B's folder exists → move A's entries
     in one by one; an entry present on both sides keeps the one with the later `lastActivityAt` (the
     source's on a tie); `scheduled-tasks.json` and `archived-sessions.idx` are merged by id;
   - keyed preferences: B's key takes A's value (copy, so A's stays for a later switch back);
   - `claude_desktop_config.json` and `ccd-session-presets.json` are rewritten atomically (temporary file and
     rename), and only while Desktop is closed.
8. **Record** B as the identity in place (`~/.relais/desktop.json`), close the journal.
9. **Open Desktop** and wait up to 60 s for the proof that it is signed in as B: a new sample for B's
   organization in `plan-usage-history.json` (Desktop writes one at each start, after asking the server, so it
   needs a working sign-in). Desktop 3.6556.2 writes no `main.log` any more: the log parser stays as a second
   source only. No sample in time: "Claude is open on B; its sign-in is not confirmed yet". A sample for another
   organization: said plainly, with the way back.
10. **Terminal.** New terminals use B too (`registry.active = B`), so one click moves the user's whole
    working setup. (Builder's call, see below.)

**Run outside Desktop.** A command started from a Desktop session is killed when Desktop quits. The CLI
therefore refuses to switch from inside Desktop (`CLAUDE_CODE_ENTRYPOINT=claude-desktop`) and points to the
panel or a Terminal; `--dry-run` works anywhere. The panel is its own app, so it is unaffected.

Any failure between steps 5 and 8: put A's identity back, restore the backed-up files, reopen Desktop on A,
and say what happened in one sentence. Relais must never leave Desktop unable to start (fail open).

## B already has conversations of its own

Possible when the user signed in to B directly in Desktop (same workspace, so B got its own index folder).
Two records are the same conversation when they have the same file name (`local_<id>.json`). A record of B whose
transcript (`cliSessionId`) the list already has is a duplicate, not B's own: it is put aside whatever the
choice, so nothing shows twice. Only when B has records of its own, the confirmation asks one question:

> B already has 12 conversations of its own. **Add them to your list** / **Put them aside**

- Add: they join the list (step 7 merge).
- Put aside: they move to `~/.relais/set-aside/<timestamp>/<account>/<org>/`, recoverable with one command.
  Nothing is ever deleted or archived to make the list look like the source.

## An account without identity

First switch to an account that never signed in to Desktop: Relais closes Desktop, parks A's identity,
carries the keyed state to B (its ids come from the registry), and opens Desktop signed out. The user signs
in with Anthropic's own screen. From then on B has an identity like the others. If B's ids are not known,
Relais asks to add the account first (`relais account add`).

## Moving from the current layout (one-time)

On this Mac today: the folder in place is signed in as one account and already contains every conversation
of the four parked folders; each parked folder `~/.relais/accounts/<alias>/desktop` is signed in as its account.

Migration, run once, Desktop closed or not (parked folders are not used by Desktop):

- for each parked folder: move its identity files into `~/.relais/accounts/<alias>/identity/`, then rename the
  rest of the folder to `~/.relais/accounts/<alias>/desktop-archive-<date>`; never delete it;
- report the conversations an archive has that the workspace lacks (zero on this Mac). Importing them is
  left out until it is needed;
- the old switch journal (`desktop-switch.json`) and the previous version's state files are left as they are
  and ignored.

## The interface

Too much text and too many pages today. The panel becomes one screen:

- one line per account: its color, its name, two thin bars (5 hours, week) with when each comes back;
- the account Desktop uses is marked; every other line has one button, "Switch to B";
- clicking opens one short confirmation: "Claude will close and reopen on B. Everything stays." plus the
  question above only when it applies, and the button;
- while it works, one line of progress (closing, switching, reopening), then "You are on B".
- anything else (adding an account, colors, the account page, the recent conversations list with its resume
  buttons) goes behind one menu: kept, not deleted. The bar that chose where each account works goes, since
  the terminal now follows the switch (`relais switch` still moves the terminal alone).

Strings go through the translation table, English and French. Visual tokens follow the Neoservice design
system.

## Goals, non-goals, anti-goals

- Goal: after a switch, the sidebar and the window look the same as before, in one click, on this Mac.
- Goal: a switch can always be undone by switching back, and an interrupted switch can be finished.
- Non-goals: the Chat tab (server side, per account), cloud Code sessions (server side, per account), keeping
  running processes alive, Claude in Chrome (its sign-in lives in the browser).
- Anti-goals: driving Claude's interface, reading or copying a credential, calling Anthropic's servers,
  switching on its own, deleting or archiving a conversation, any "ready" message that was not checked.

## Code

- The core holds the workspace switch (identity parking, the carry rules, journal, backup), tested on
  temporary folders with made-up ids and `example.com` addresses.
- The CLI has `relais desktop <account> [--dry-run]` and `relais desktop --bring-back`.
- The Tauri app calls the same core.

## Decisions

- The terminal follows the Desktop switch (step 10).
- Merge rule for an entry on both sides: later `lastActivityAt` wins.
- Sessions of the source account under another organization than the recorded one are left where they are.
- Importing conversations found only in an archive is not built until needed.

## Checked by a real switch

- Desktop resets nothing when it starts on another account in the same folder.
- The sidebar shows the same thing before and after: conversations, pins, folder order, permissions and
  settings were compared one by one.
