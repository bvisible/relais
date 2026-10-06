# Relais

Relais lets several Claude subscriptions on one Mac work as a relay team. When one account
reaches its limit, another one takes over: Claude Code in the terminal, the Claude desktop app
and Claude in Chrome move together, and the conversation carries on where it stopped.

> **Early development.** Nothing here is ready for daily use yet.

## Why it is built this way

Relais never touches a credential. It does not read, copy, refresh or store any token: you sign
in through Anthropic's own apps, they keep and refresh their logins as usual, and Relais only
chooses which per-account profile is active. That removes the whole class of "dead token" bugs
that tools copying tokens have to chase, and it follows Anthropic's rule that third-party tools
must not collect or store Claude credentials.

Relais never talks to Anthropic either. The usage it shows comes from what Claude Code already
hands to its status line, and from the history the desktop app keeps.

Switching is your decision. Relais warns you before a limit and suggests the account with the most
room left; it never rotates accounts behind your back.

## How it works

Each account gets its own profile everywhere: a Claude Code config directory (the setup
Anthropic documents for several accounts), a desktop app data folder, and a Chrome profile.
Conversations, settings, skills and MCP servers are shared between the profiles, so switching
never loses history. The parts of `.claude.json` that are not tied to an account (MCP servers,
per-project trust and permissions) are kept in sync across profiles.

## Status

| Part | State |
|---|---|
| Shared config sync, account registry, usage readings, conversation index | done, tested |
| Terminal: `relais` command (accounts, switch, exec, continue, status line, doctor) | done, tested, in daily use on one Mac |
| Menu bar app (Tauri, React): accounts, gauges, switching, resume in Claude Desktop or the terminal, settings | first version, in daily use on one Mac |
| Switching the Claude desktop app's login and Chrome profiles | next |

```bash
relais init                      # register the account Claude Code is signed in with
relais account add work          # add another account (signs in through Claude Code)
relais statusline-install        # record usage from Claude Code's status line
eval "$(relais shell-init zsh)"  # make `claude` run with the active account
relais list                      # every account with its 5-hour and weekly usage
relais switch next               # or a name, prev, best
relais continue                  # carry the latest conversation over to the active account
```

## License

MIT. Relais is an independent project, not affiliated with or endorsed by Anthropic. Claude and
Claude Code are trademarks of Anthropic.
