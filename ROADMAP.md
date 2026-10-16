# dterm roadmap

Goal: be a better daily terminal than Windows Terminal for people who run several shells
(and coding agents like Claude Code) side by side. Small native exe, zero config, looks like
Windows Terminal, but built around the grid.

## Requested features

Ordered roughly by how much they hurt today.

### Selection and clipboard
- [ ] **Mouse selection.** Drag to select, double-click word, triple-click line.
- [ ] **Shift+Left/Right selects** character by character (today it types a capital letter / sends
      an escape sequence). Shift+Up/Down extends by line. Like Windows Terminal's mark mode.
- [ ] **Right-click:** copies when there is a selection, pastes when there is none (Windows Terminal behavior).
- [ ] **Ctrl+C copies** when there is a selection, otherwise sends interrupt as today.
- [ ] **Mouse-wheel scrollback** through history.

### Layout and shortcuts
Windows Terminal defaults in brackets, so muscle memory carries over.
- [ ] **Alt+Shift+Plus / Alt+Shift+Minus split the focused pane**, exactly like WT:
      Plus = new pane to the right, Minus = new pane below.
      Moving from the auto-grid to real splits means a split tree instead of rows/cols fractions.
- [ ] **Alt+Shift+arrows resize the focused pane** by a fixed step [WT: same].
- [ ] **Ctrl+Alt+arrows swap** the focused pane with its neighbour [WT: swapPane, unbound by default].
- [ ] **Zoom** the focused pane to fill the window, and back to the grid [WT: togglePaneZoom, unbound].
- [ ] **Grid ⇄ tabs toggle.** One shortcut (and one small button) switches the same sessions
      between the grid and tab view. WT can't do this: panes and tabs are separate there.
- [ ] Ctrl+Shift+W closes the focused pane [WT: same].
- [ ] Keep Ctrl+Shift+T (add pane, auto grid) and Alt+arrows (move focus, done).

### Knowing which pane is which
- [ ] **Focus indicator:** a very faint border on the focused pane and slightly dimmer unfocused
      panes. Subtle, no accent-colour frame. [WT uses an accent-colour border.]
- [ ] **Pane names without headers:** a small, low-contrast label in the pane's top-right corner
      (folder name by default, or the title the shell/app sets via OSC 0/2, or a name you type).
      Fades while you type in that pane. No title bars, no chrome.

### Sessions
- [ ] **Session restore + landing page.** On start, a terminal-style menu (like opencode):
      new session / last session / saved sessions. A session stores the layout, pane names and each
      pane's working directory. [WT: `persistedWindowLayout` restores layout and directories, not contents.]
- [ ] Optionally save scrollback text per pane so a restored session shows what was there.

### Look
- [ ] **Keep the acrylic when the window is unfocused.** Today Windows swaps the backdrop for a solid
      colour when inactive (DWMSBT_TRANSIENTWINDOW behavior). Needs a different backdrop method,
      e.g. drawing our own blur, or the accent-policy API.
- [ ] **Ctrl+Plus / Ctrl+Minus / Ctrl+0 change the font size** [WT: same].
- [ ] Cosmetic: highlighted text (inverse/background colours) shows thin dark lines between rows.
      Snap cell rectangles to whole pixels so backgrounds join seamlessly like in WT.

## Where Windows Terminal stands (checked October 2026)

WT already has: split panes (Alt+Shift+Plus/Minus/D), Alt+arrows focus, Alt+Shift+arrows resize,
swap/move panes, zoom, read-only panes, close pane, accent border on focus, search, command palette,
mark mode (keyboard selection), Quake mode, `persistedWindowLayout` session restore (layout, profiles
and working directories, no contents), acrylic, font fallback.

WT does **not** have:
- **Mouse-drag pane resizing.** Issue [#992](https://github.com/microsoft/terminal/issues/992) is open
  since 2019, now on milestone 1.27; PR #16895 is unmerged. dterm has it.
- **Automatic grid.** In WT you build every layout by hand, split by split.
- **Grid ⇄ tabs** for the same sessions.
- **Agent awareness:** nothing tells you which pane is busy, finished, or waiting for you.

## How dterm beats it

1. **Grid first:** auto grid, mouse resize, grid ⇄ tabs, zoom. All built in, no config.
2. **Agent status per pane** (the biggest idea from Terminal-Grid): a soft glow while a pane is busy,
   a calm "done" tint when it finishes, a stronger cue when it rings the bell or asks for input.
   Signals: output activity, the bell, OSC 9/777 notifications, OSC 133 prompt marks.
3. **Sessions that come back** with names and folders, from a quick landing page.
4. **Broadcast input:** type once into all panes (or a chosen set).
5. **Stays tiny:** one native exe (~320 KB today), instant start, no Electron, no settings file needed.

## Other open-source grid terminals

- [Terminal-Grid](https://github.com/LiamDotPro/Terminal-Grid) (Tauri + xterm.js, 0 stars, v0.5.0):
  closest in spirit. 3x3 grid pages, resizable panes, git branch per pane, agent detection with
  "working / done" glow, markdown notes, focus/review mode. Shortcuts: Ctrl+Alt+N new pane,
  Ctrl+Alt+arrows move pane, Alt+arrows focus, Ctrl+Alt+1-9 focus pane n. No session saving.
- [TermGrid](https://github.com/mayberks/termgrid) (Electron + xterm.js, MIT, 0 stars): auto-arranging
  grid for CMD/PowerShell, Ctrl+G spawns a terminal in a picked folder. No manual resizing, no sessions.
- [MulmoTerminal](https://github.com/receptron/mulmoterminal) (browser + tmux, MIT): grid of Claude Code
  sessions that colours the one needing you.
- General terminals with splits: WezTerm (Lua config, no settings UI), Rio, Tabby (Electron).
  Ghostty has no Windows build.
