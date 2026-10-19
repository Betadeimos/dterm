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

## Visibility and stars (standing rule)

Growing the GitHub repo's visibility and stars is an ongoing goal. In every session, look for one
cheap way to make the repo more attractive or easier to find, and suggest it: the README's top
screen (badge, one-line pitch, GIF), the repo description and topics, release notes, `good first
issue` labels, answering issues quickly. Keep every size claim (README badge and text, repo
description, posts) in step with the real size of `target\release\dterm.exe` after each release.
Never fake traction: no star-for-star swaps, no bought stars, no karma-farming subreddits, no
resubmitting to get around a site's rules.

## Public launch (status 2026-10-19)

Drafts of the Reddit and Show HN posts are in `launch-posts.md` (local only, listed in
`.git/info/exclude`). Before posting, re-check every number in them against the current release.

**Where things stand**
- v0.3.2 (522,240 bytes, ~510 KB) is the release to launch with. It fixed #14 (two Claude panes in
  one folder swapped conversations) and ships `SHA256SUMS.txt`. The README explains the
  SmartScreen warning, since the exe is not signed (#24).
- Reddit: the account is 6 years old but has almost no karma, and r/ClaudeCode blocks posting
  below an unpublished karma threshold. r/ClaudeAI needs 50 karma to post a project to the
  main feed; below that, use the pinned "Built with Claude" Project Showcase Megathread.
- Hacker News: Show HN is closed to new accounts for now ("temporarily restricting Show HNs").
- The user has no X/Twitter or Discord.
- awesome-claude-code (hesreallyhim, 56k stars) takes projects that are at least 14 days old or
  have 100+ stars: dterm qualifies from **2026-10-30**. Only a human, through the web form
  (https://github.com/hesreallyhim/awesome-claude-code/issues/new?template=recommend-resource.yml),
  never `gh` or a PR. Category `Agent Orchestration`. Description: "Native Windows terminal
  (single ~510 KB exe) that tiles Claude Code sessions in an auto-grid, marks background panes as
  working, done or waiting for input, and resumes each pane's conversation with claude --resume
  after a restart." Tick every checklist box except the last one (a trap).
- jqueryscript/awesome-claude-code merges nothing new and lists only projects with many stars:
  not worth it until dterm has traction.

**Plan, in order**
1. The user earns Reddit karma (target 50) with real comments, early on rising posts in big
   subreddits, without mentioning dterm.
2. The user comments on Hacker News a few times a day to build the account.
3. 2026-10-30 or later: the user submits dterm to awesome-claude-code (details above).
4. At 50 karma: post on r/ClaudeCode (Images & Video post with `assets/features/grid.gif`, body
   from `launch-posts.md`), then r/ClaudeAI a day or two later with a reworded first line. Stay
   about 2 hours after each post to answer comments; turn bug reports into issues.
5. After about 2 weeks of HN comments: email hn@ycombinator.com to ask whether the account may
   post a Show HN, then post it with the first comment from `launch-posts.md`.
