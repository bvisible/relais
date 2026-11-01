# Relais — rules for working in this repository

## Non-negotiable product rules

- **Never touch a credential.** No code may read, copy, refresh, store or log an OAuth token,
  a session cookie or a Keychain secret. Sign-in always goes through Anthropic's own flow
  (`claude auth login`, the desktop app, claude.ai in Chrome). Reading non-secret metadata is fine:
  account UUIDs, emails, the modification date of a Keychain entry.
- **Never call Anthropic's servers.** Usage comes from the status line input, the desktop app's
  own history file and the official client's `/usage` panel, opened in a private terminal.
- **Never switch silently.** Relais suggests; the user decides.
- **Fail open.** A Relais failure must never stop Claude from starting or the user's status line from
  rendering.

## Code

- Everything in source files is English: identifiers, comments, file names, flags, log messages.
- Every string a user reads goes through the CLI's translation table (`crates/relais-cli/locales`),
  English and French.
- Test first: write the test, watch it fail, then write the code.
- Files that Claude Code also writes (`.claude.json`, `settings.json`) are rewritten atomically
  (temporary file and rename), never in place.
- The repository will be public: no client names, no real account data in tests or fixtures
  (use `example.com` addresses and made-up UUIDs).
- Commits and pull requests carry no AI signature or co-author line.

## Running

Rust comes from Homebrew's `rustup` (keg-only):

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo test
cargo clippy --all-targets -- -D warnings
```
