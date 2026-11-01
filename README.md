# Relais

Relais helps several Claude subscriptions on one Mac work as a relay team. When one account
runs out, one click moves Claude Desktop and new terminals to another account, and Desktop comes
back exactly as you left it: the same conversations, pins, folders and open panes.

> **Early development.** Used daily on one Mac, where a real switch between two accounts kept every
> conversation, pin and folder. See the [design](docs/superpowers/specs/2026-11-01-desktop-workspace-switch-design.md).

## Why it is built this way

Relais never touches a credential. It does not read, copy, refresh or store any token: you sign
in through Anthropic's own apps, they keep and refresh their logins as usual, and Relais only
moves the app's own sign-in files aside and back, by renaming them, without opening them. That removes the whole class of "dead token" bugs
that tools copying tokens have to chase, and it follows Anthropic's rule that third-party tools
must not collect or store Claude credentials.

Relais never talks to Anthropic either. The usage it shows comes from what Claude Code already
hands to its status line, from the history the desktop app keeps, and from Claude Code's own
`/usage` panel: every quarter of an hour the app opens the official client in a private terminal,
with nothing of yours loaded, reads the panel and closes it. No prompt is sent, nothing is billed
and no conversation is saved.

Switching is your decision. Relais warns you before a limit and suggests the account with the most
room left; it never rotates accounts behind your back.

## How it works

**Claude Desktop: one workspace, the sign-in changes.** Desktop's data folder holds your whole
workspace; only four files in it (`config.json`, `Cookies` and their journals) make it signed in
as one account. A switch, with Desktop closed, parks those files under the account that leaves,
puts back the arriving account's, and carries to it what Desktop files under an account's id:
the conversation list, the folder order, the sidebar sections, folder permissions and session
presets. Everything else stays where it is. Each step is journaled and backed up first: a step
that fails undoes the switch at once, and a switch cut short is undone by the next run.

**Claude Code in the terminal: one profile per account**, the setup Anthropic documents for
several accounts. Transcripts, settings, skills and MCP servers are shared between profiles, and
the parts of `.claude.json` not tied to an account are kept in sync.

## Status

| Part | State |
|---|---|
| Shared config sync, account registry, usage readings, conversation index | done, tested |
| Terminal: `relais` command (accounts, switch, exec, continue, status line, doctor) | done, tested, in daily use on one Mac |
| Desktop switch that keeps the workspace (`relais desktop <account> [--dry-run]`) | done, tested, used for real |
| Menu bar app (Tauri, React): a simple view, one line per account and one button to switch, and a full one | done, tested |
| Claude in Chrome | out of scope: its sign-in lives in the browser |

```bash
relais init                         # register the account Claude Code is signed in with
relais account add me@work.example  # add another account (signs in in its own Chrome profile)
relais account adopt                # someone signed another account in on the default profile: keep it, and move the recorded one to a profile of its own
relais statusline-install           # record usage, and which account runs each session
eval "$(relais shell-init zsh)"     # make `claude` run with the active account
relais list                         # every account, its usage and how long until each reset
relais desktop work --dry-run       # what switching Claude Desktop to work would do
relais desktop work                 # do it: Desktop closes, reopens on work, new terminals follow
relais desktop --bring-back         # Desktop closed: put back the conversations a switch set aside
relais switch next                  # move only the terminal: a name, next, prev or best
relais continue                     # carry the latest conversation over to the active account
```

## License

MIT. Relais is an independent project, not affiliated with or endorsed by Anthropic. Claude and
Claude Code are trademarks of Anthropic.
