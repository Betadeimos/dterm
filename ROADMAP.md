# dterm roadmap

Goal: be a better daily terminal than Windows Terminal for people who run several shells and
coding agents (Claude Code, Codex, OpenCode, aider) side by side. One small native exe, zero
config, looks like Windows Terminal, built around the grid.

**All open work lives in [GitHub issues](https://github.com/Betadeimos/dterm/issues).** This file
is the direction and the history; it has no to-do list of its own.

## Direction

1. **Grid first:** auto grid, mouse resize, grid ⇄ tabs, zoom. Built in, no config.
2. **Agent-aware:** which pane is busy, done, or waiting for you; agents that come back with the
   session; worktrees and broadcast for running several agents at once.
3. **Sessions that come back** with layout, names, folders and agents, from a quick start page.
4. **Stays tiny:** one native exe (~500 KB), instant start, no Electron, no new crates unless they
   earn their bytes.
5. **Easy for newcomers:** works out of the box, with the shell you already have
   ([#17](https://github.com/Betadeimos/dterm/issues/17)).

Main open threads: agents (#7, #14, #15, #16, #20), panes and input (#8, #9, #10, #18, #19, #21),
shells and sessions (#13, #17), look (#4, #5, #12), project (#6, #11, #22, #23, #24).

## History

**v0.3.0, the Native Agent Orchestrator Release**
- OSC 9 / 777 notifications turn a background pane's dot yellow at once; an OSC 133 prompt after
  a command turns it green at once. dterm scans the raw PTY output before the terminal parser,
  which drops these sequences. The 4.5 s quiet timer and the bell stay as the fallback.
- claude, codex, opencode and aider are found in each pane's process tree, tagged on the pane
  (`[claude] proj`) and restored with the session. Only whitelisted command shapes are ever typed
  back from a session file.
- Broadcast input (`Ctrl+Alt+B`), a one-key git worktree pane (`Ctrl+Alt+N`), mouse on the
  settings page, mouse clicks and drags reported to apps (Shift+drag still selects).
- New green icon (D_ in JetBrains Mono ExtraBold, centered by its ink) with a frame for every
  taskbar size, One Half Light theme, 80% default opacity,
  `D_term` logo, windows open centered.

**v0.2.2:** the status became a solid dot before the pane's name (blue busy, green done after 4.5 s
of quiet, yellow after a bell); fixed window title `dterm — <session>`.

**v0.2.1:** agent status per pane (as a name tint); a saved session stays the same session, and
`Ctrl+Alt+S` saves over its file.

**v0.2.0:** every shortcut moved to Ctrl+Alt (Ctrl+Shift+S never reached dterm on some machines),
with AltGr characters still winning; Claude Code resumes its conversation on restore; start page
button and `Ctrl+Alt+H`; settings page (theme, opacity, every shortcut); versioned releases.

**Before:** mouse and keyboard selection, right-click copy / paste, wheel scrollback; WT-style
splits, resize, swap, zoom, grid ⇄ tabs; pane names and focus border; session restore and the
start page; acrylic that stays when unfocused and works on Windows 10 (dterm draws its own title
bar there); font size keys.

## Lessons worth keeping

- Keep the dterm shortcuts on Ctrl+Alt: Ctrl+Shift combinations can be eaten before they reach the
  window, and the shell and apps want them.
- Acrylic halves the frame rate while the window is dragged, so moves and resizes use plain blur.
  On Windows 10, host backdrop brushes render black and a window region brings back the old frame.
- Watch the exe size on every change: `std::process::Command` alone added 73 KB (replaced by
  `CreateProcessW`).
- Icons need a frame for every size Windows asks for (16, 20, 24, 32, 40, 48, 64, 256); the
  taskbar's 24 px scaled from 32 px blurs.

## Landscape (checked October 2026)

**Windows Terminal** has splits, focus / resize / swap / zoom, search, command palette, mark mode,
Quake mode, layout restore (no contents), acrylic and font fallback. It does **not** have
mouse-drag pane resizing ([#992](https://github.com/microsoft/terminal/issues/992), open since 2019),
an automatic grid, grid ⇄ tabs for the same panes, or any agent awareness.

**Other grid terminals:** [Terminal-Grid](https://github.com/LiamDotPro/Terminal-Grid) (Tauri,
agent "working / done" glow, Ctrl+Alt+1-9, no sessions), [TermGrid](https://github.com/mayberks/termgrid)
(Electron, auto grid, no resize), [MulmoTerminal](https://github.com/receptron/mulmoterminal)
(browser + tmux, Claude Code grid). General: WezTerm, Rio, Tabby; Ghostty has no Windows build.
