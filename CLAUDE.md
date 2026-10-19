# dterm: notes for Claude Code

## Work is tracked in GitHub issues

GitHub issues are the single source of truth for open work: https://github.com/Betadeimos/dterm/issues
Use the GitHub CLI (`gh`, installed at `C:\Program Files\GitHub CLI\gh.exe`, logged in as Betadeimos).

- **Before starting something new:** find its issue (`gh issue list`), or open one first with what,
  why, where to look in the code, and "done when". Labels: `enhancement`, `bug`, `good first issue`.
- **Branch per issue:** e.g. `feat/17-shell-picker`, `fix/14-claude-swap`. Reference the issue in
  commits (`#17`).
- **When it is done:** merge to `main`, push, then close the issue with a comment that says what
  was built, the commit, and anything left out (open a follow-up issue for leftovers).
- **Ideas and bugs found along the way:** open an issue for each instead of a TODO in the code or a
  line in ROADMAP.md.
- **ROADMAP.md** holds only the direction, the history and lessons learned; it has no to-do list.
  After a release, add a short entry to its History section.

## Build, test, release

- `cargo test` and `cargo build --release` before saying something works; report the exe size
  (`target\release\dterm.exe`) when it changes. Binary size is a feature: no new crates unless they
  earn their bytes.
- After each release build, copy `target\release\dterm.exe` to `%LOCALAPPDATA%\Programs\dterm\dterm.exe`
  (the copy pinned to the taskbar). If it is locked, dterm is running: ask the user to close it.
- Releases: bump `Cargo.toml`, then `.\release.ps1 -Publish` (tags, builds `dist\`, creates the
  GitHub release). Keep README, CONTRIBUTING and the top comment of `src/main.rs` in sync with
  behavior and shortcuts.
