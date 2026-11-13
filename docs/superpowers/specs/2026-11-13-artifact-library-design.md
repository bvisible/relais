# Artifact library: keep every artifact when the account changes

Status: approved in conversation on 2026-11-13 ("Dans Relais"); the details marked *builder's call* were
decided while building and are open to review.

## Intent

An artifact published from Claude Code or Claude Desktop belongs to the account that published it. After a
switch, the other account can neither see nor change it, and its source is gone: Claude writes the page to a
temporary folder before publishing it. Work done on one account has to be redone on the next.

Relais keeps a local copy of every artifact published on the Mac, with its link and the account that owns
it, so any account can find it again and publish it once more instead of rebuilding it.

## Goals

- Every published artifact is copied without Claude having to remember: a `PostToolUse` hook on the
  `Artifact` tool, installed by Relais, records it.
- Claude can find an artifact by title or link and publish its copy again from another account. The new
  link is recorded as following the old one.
- The panel lists the artifacts, with their account, and opens them or shows their folder.
- The way to keep the very same link is explained: sharing the artifact with the other accounts' addresses
  with "Can edit".

## Non-goals and anti-goals

- No call to Anthropic: Relais never publishes, reads or shares an artifact itself. Claude does, with its
  own Artifact tool.
- No credential is read. The hook reads the tool's input and result, and the account's id from the
  session's environment.
- A failure never reaches Claude: the hook runs in the background, prints nothing and always exits 0.
- Comments, shared database rows and version history on claude.ai are not copied: they live on the server.

## How it works

1. `relais artifacts-install` adds to `~/.claude/settings.json`, after a backup, a `PostToolUse` entry
   matching `Artifact` that runs `relais artifact-record` with `async: true`, and writes the skill
   `~/.claude/skills/relais-artifacts/SKILL.md`. Every account's profile links these files, so one install
   serves them all. `relais artifacts-uninstall` removes only what Relais added.
2. `relais artifact-record` reads the hook's JSON. It keeps a successful `publish` only (a result with a
   claude.ai artifact link), and copies the page and the files of the `files` map into
   `~/.relais/artifacts/<id>/current/`, at the path each is served at. The previous copy moves to
   `versions/`. `index.json` records the link, title, account, project folder and dates.
3. The skill tells Claude to look the library up before making an artifact that may exist, and to publish
   the library copy when the active account cannot change the artifact.

## Builder's calls

- The account comes from `CLAUDE_CODE_ACCOUNT_UUID`, then `CLAUDE_CODE_USER_EMAIL`, then the profile
  (`CLAUDE_CONFIG_DIR`, or the default profile), matched against the registry.
- A new link "follows" an older artifact when the published page sits in that artifact's library copy, or
  has exactly the same content.
- Five earlier versions are kept per artifact. Files over 64 MB, and paths that leave the folder (`..`,
  absolute), are not copied.
- Assets uploaded with `asset: true` and files copied from another artifact server side are not kept in
  this first version: the skill says to upload them again.
- The index is written atomically under Claude Code's own lock convention (`index.json.lock`).
