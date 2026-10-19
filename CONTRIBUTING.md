# Contributing to dterm

Thanks for helping. dterm is small on purpose: one native exe, few dependencies, no config needed. Keep changes in that spirit.

## Build and test

Windows only. You need [Rust](https://rustup.rs/) (MSVC toolchain), Visual Studio Build Tools with
"Desktop development with C++", and [Git for Windows](https://gitforwindows.org/) (panes run its `bash.exe`).

```
cargo build --release          # target\release\dterm.exe
cargo run                      # debug build, quick to iterate
cargo test                     # pure-logic unit tests, no window
cargo test -- --ignored        # also starts Git Bash and types into it
```

The icon is linked from the prebuilt `dterm.res`, so no resource compiler is needed (see `build.rs` to change it).
Release builds use `opt-level = "z"`, LTO and `panic = "abort"` to stay tiny: check `target\release\dterm.exe`
size before and after your change, and avoid new crates unless they earn their bytes.

## Layout

Everything is in `src/main.rs` (about 4000 lines) on purpose, for now. Rough map, top to bottom:

| Area | What lives there |
| --- | --- |
| Constants, `Theme`, `THEMES` | Sizes in DIPs, alphas, the 4 color themes |
| `key_seq`, `paste_text` | Windows virtual keys to terminal escape sequences, bracketed paste |
| `Node`, `halves`, `auto_split`, `neighbor`, `place` | **Split tree** and layout math: pure functions on rectangles |
| `View`, tab and caption helpers | Grid vs Tabs, tab strip, the title bar dterm draws itself |
| `Action`, `ACTIONS`, `Chord`, `Settings` | Rebindable shortcuts, settings file parse/write |
| `Session`, `SavedPane`, `Landing` | Session files, the start page model |
| `Names`, `Status`, `Activity` | Pane names from OSC titles, busy/done/waiting status |
| `Signal`, `OscScan`, `Tapped` | OSC 9 / 777 / 133 read from the raw PTY output, feeding `Activity` |
| `Proxy`, `Pane`, `spawn` | One shell: PTY, terminal state, event bridge |
| `descendant`, `agent`, `agent_run` | Finds claude, codex, opencode or aider under a pane's shell, to tag the pane and resume the agent |
| `git`, `open_worktree` | The worktree pane shortcut |
| `mouse_report`, `report_mouse` | Mouse buttons for apps that ask for them |
| `App`, `with` | The single global app state (thread-local) |
| `layout`, `add_pane`, `remove_pane` | Fit panes to the window, resize their PTYs |
| `paint`, `draw_*` | Direct2D rendering |
| `on_key`, `run`, `wndproc` | Input and the Win32 message loop |
| `main`, `check`, tests | Startup; unit tests |

## How it works

**Win32 shell.** `main` registers a window class and runs a classic `GetMessage` loop. All UI state is one `App`
struct in a thread-local; `with(|a| ...)` borrows it. Never call anything inside `with` that sends a message back
to `wndproc`: it would borrow twice.

**PTY.** `spawn` starts `bash --login -i` through `alacritty_terminal`'s `tty` (ConPTY on Windows). Its
`EventLoop` thread reads the shell's output and updates a `Term` behind a `FairMutex`. Each pane's `Proxy` receives
terminal events (wakeup, bell, title, exit, replies to the app) and turns them into `PostMessageW` calls to the UI
thread (`WM_WAKE`, `WM_EXIT`) or writes back to the PTY. Status flags (`Activity`) are shared atomics, so the PTY
thread never touches `App`. The terminal parser ignores OSC 9, 777 and 133, so the PTY is wrapped in `Tapped`, whose
reader passes every byte through `OscScan` before the parser sees it (that wrapper is why `polling`, already a
dependency of `alacritty_terminal`, is named in `Cargo.toml`).

**Rendering.** `paint` draws everything with Direct2D, text with DirectWrite, into an `ID2D1HwndRenderTarget`:
landing page, settings page, or the panes. `draw_term` locks a pane's `Term`, walks its visible cells, and draws
backgrounds, glyph runs, cursor and selection. The window is acrylic (system backdrop on Windows 11 22H2+, accent
policy on Windows 10), and dterm paints the translucent tint itself.

**Layout.** Panes live in a binary split tree (`Node`). `place` turns the tree plus the window size into one
`Rect` per pane (or a single full-size pane in Tabs and zoom). `layout` then refits each pane's
columns and lines and sends `Msg::Resize` to its PTY. Drag-resizing edits split ratios in the tree.

**Sessions.** `save_last` / `write_session` serialize the tree, folders, names and view to small text files under
`%APPDATA%\dterm`. `agent_run` walks the process list under each pane's shell for a coding agent. For Claude Code it
matches the newest conversation in `~\.claude\projects\<folder>` and saves `claude --resume <id>`; aider gets
`aider --restore-chat-history`, codex and opencode their bare name. Only those exact command shapes are ever typed
back in (`resume_command` checks).

## How work is tracked

Everything goes through [GitHub issues](https://github.com/Betadeimos/dterm/issues):

1. **Find or open an issue first:** what, why, where to look in the code, and when it counts as done.
2. **One branch per issue** (`feat/17-shell-picker`, `fix/14-claude-swap`), and mention the issue
   in commits and the pull request (`Fixes #17`).
3. **Close it when merged,** saying what was built and what was left out; leftovers become new issues.
4. **Found a bug or an idea along the way?** Open an issue rather than leaving a TODO.

[ROADMAP.md](ROADMAP.md) only keeps the direction, the history and lessons learned.

## Making a change

- Match the surrounding style: terse code, comments that say why, sizes in DIPs.
- Put logic in pure functions where you can and add assertions to `check()` in `src/main.rs`; that is how most of
  dterm is tested. Rendering and input need a manual look: run it and try it.
- Keep the README shortcut tables and the module doc comment at the top of `main.rs` in sync with behavior.
- One focused change per pull request, and say what you tried by hand.

Issues labeled **good first issue** are a fair place to start.
