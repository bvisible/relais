# Desktop session continuity audit

Date: 2026-10-18. Inspected Desktop version: 2.42304.0.
Status: investigation complete; implementation and live account-switch acceptance pending.

## Required behavior

Changing accounts should preserve local Code conversations, their Desktop parent/child
relationships, their project folders and their persistent subagent histories. Desktop and
Claude in Chrome must agree on both account and organization before browser work resumes.

Preserving history does not preserve a running process. Interrupted commands, browser tabs,
remote connections and background work require independent checks before resuming them.
Cloud Chat and Cowork histories are outside this local Code-session migration scope.

## Existing implementation

- `claude/profile.rs` shares local Claude Code files through profile symlinks.
- `claude/transcripts.rs` indexes top-level project transcripts, without a session tree.
- `claude/desktop.rs` reads Desktop sidebar records only to attribute sessions to accounts.
- `claude/desktop_profile.rs` renames entire Desktop data folders between per-account locations.
  Authentication stays inside the vendor-managed folders; Relais does not extract credentials.
- `relais switch` selects the account for subsequent CLI launches. Desktop switching is separate.
- `platform::resume_in_desktop` launches `claude --desktop --resume` using the terminal profile.
  Successful process creation is currently treated as success without verifying the import.
- `platform::open_chrome` opens a Chrome profile. It does not inspect extension sign-in.

## Two distinct relationship models

Desktop side sessions are independent local sessions. The installed Desktop build stores their
sidebar metadata under:

```text
Application Support/Claude/claude-code-sessions/<account>/<organization>/local_<id>.json
```

The relationship is carried by `spawnedFrom`, for example with fictional identifiers:

```json
{
  "sessionId": "local_child",
  "cliSessionId": "transcript-child",
  "spawnedFrom": {
    "sessionId": "local_parent",
    "taskId": "task_example",
    "sideSession": true,
    "sharedFolder": true,
    "name": "review"
  }
}
```

Other persisted fields include side-session notes, queued notes, workspace anchors and
interruption metadata. Their migration rules must be established individually; account-bound
connectors, permissions and remote-control identifiers must not be blindly copied.

Internal Claude Code subagents have a separate transcript layout:

```text
~/.claude/projects/<project>/<cli-session-id>/subagents/agent-<id>.jsonl
```

Preserving only the root JSONL misses this second set of histories. Preserving all JSONLs
still does not reconstruct Desktop's first relationship model.

## Installed Desktop import path

Read-only inspection of `/Applications/Claude.app/Contents/Resources/app.asar` found:

- `.vite/build/index.chunk-CFJssAFf.js`, `adoptCliSession` and `importCliSession`: adoption creates
  a single destination record, in the current account/organization, using `local_<cliId>`.
  This path does not recursively import descendants or retain `spawnedFrom` and side-session notes.
- The serializer and schema chunks persist `spawnedFrom` separately. Its required relationship
  fields include `sessionId` and `taskId`.

Therefore the current single-transcript resume path cannot implement the required tree transfer
by itself. These are private implementation details, not a supported migration API; a later
Desktop release needs a fresh schema check.

## Blocking defects

1. **Stale Desktop owner.** `desktop_profile::owner` trusts a saved alias before current identity.
   A manual account change can make this incorrect. The inspected workstation exhibited this
   mismatch. Moving the live folder under that alias can assign it to the wrong account.
2. **Unknown identity treated as success.** Startup verification relies on
   `~/Library/Logs/Claude/main.log`, absent on the inspected workstation. No evidence establishes
   that Desktop removed this log globally. An unavailable identity signal must remain unknown.
3. **No global switch transaction.** There is no lock shared by CLI and Tauri, durable operation
   journal or complete rollback after metadata, restart or identity-verification failure.
4. **No session-tree import or verification.** The conversation model is flat. Neither session
   relationships nor persistent subagent files are part of a switch acceptance check.
5. **Chrome is not qualified.** A profile directory is not proof of an installed, connected
   extension using the destination account and organization.
6. **Profile sharing needs a version review.** Newly introduced Claude files are shared by
   default unless excluded. New directories created first inside another profile can remain
   private. Review both cases without inspecting or moving credentials.

## Proposed implementation order

1. Add a read-only inventory: Desktop IDs, CLI IDs, parent links, account/organization metadata,
   transcript presence, subagent files, shared project folders and activity evidence. Detect
   missing parents, cycles, duplicates and unsupported records without mutating live files.
2. Add a switch preview with a non-secret manifest, source identity evidence, target profile,
   selected session tree, expected file counts and rollback conditions. Explicitly distinguish
   idle, active and unknown execution states.
3. Implement a serialized Desktop switch with a durable journal, no-overwrite behavior and
   recovery from partial moves. Require fresh source identity before moving folders, then
   verify the destination identity after restart. Keep unknown results visible.
4. Prototype a version-gated migration of explicitly supported session metadata while Desktop
   is closed. Preserve relationship IDs or remap every reference consistently. Reconcile
   tombstones, notes, workspace anchors and account-bound fields. Never overwrite conflicting
   destination sessions or copy authentication state from one account into another.
5. Complete Chrome onboarding per profile through official sign-in. Verify account and
   organization compatibility through the vendor's UI/connection flow. Preserve existing
   profiles; do not log out of the extension to rotate accounts.
6. Expose independent results: profile selected, app open, identity verified, tree present,
   transcript resumed and browser connected. Do not collapse them into one success toast.

## Acceptance checks

Begin with fictional sessions, not a real working project. Use a parent with descendants at
multiple depths, an internal subagent transcript, a side-session note and pending work.

- Test identity drift, missing metadata, conflicts, partial moves and concurrent switch attempts.
- Switch A → B and B → A. After each restart, independently inspect the account/organization,
  complete sidebar tree, transcript IDs, notes and file hashes.
- Resume existing sessions on the destination account without recreating conversations.
- Verify internal subagent context separately from Desktop side-session relationships.
- Demonstrate that interrupted work is reported rather than automatically duplicated.
- Connect the extension under the same destination account and organization, and perform a
  harmless browser read. Verify that unrelated Chrome profiles and sessions remain usable.

## Validation executed during this audit

- `cargo test --workspace`: 183 tests passed (150 core, 30 CLI integration, 2 CLI unit, 1 app).
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cd app && pnpm test`: 12 tests passed; coverage is format/calculation behavior.
- `cd app && pnpm build`: TypeScript and Vite passed.
- Read-only live UI, non-secret session metadata and installed application-code inspection.

Tests used temporary homes and fake Claude/Desktop controls. No real account switch, session
import, extension sign-out, model prompt or runtime installation was performed by this audit.
Passing these tests does not qualify the requested account-transfer workflow.

## Official references

- [Desktop sessions and CLI import](https://code.claude.com/docs/en/desktop#coming-from-the-cli)
- [Session restoration and interrupted work](https://code.claude.com/docs/en/sessions#what-a-resumed-session-restores)
- [Persistent subagent histories](https://code.claude.com/docs/en/sub-agents#resume-subagents)
- [Chrome account and organization requirements](https://code.claude.com/docs/en/chrome#extension-signed-in-to-a-different-organization)
