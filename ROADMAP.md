# dterm roadmap

Goal: be a better daily terminal than Windows Terminal for people who run several shells
(and coding agents like Claude Code) side by side. Small native exe, zero config, looks like
Windows Terminal, but built around the grid.

## Feedback round 3 (done in v0.2.0)

- [x] **Ctrl+Shift+S did nothing** on the user's machine (it worked in automated tests). Every
  dterm shortcut moved to Ctrl+Alt, so it is now Ctrl+Alt+S. If that fails too, add a key log
  (WM_KEYDOWN vk + modifiers) to see what arrives; keyboard layout or a global keyboard hook are
  the suspects.
- [x] **Restored sessions lost running programs.** Saving now finds `claude.exe` under each pane's
  shell (process tree), picks that folder's newest conversation in `~/.claude/projects/<folder>`
  written since Claude started, and saves `claude --resume <id>` (or `claude` with no
  conversation yet). Restoring types it into the new shell. Only those two command shapes are
  ever run from a session file. Limits: two Claude panes in one folder may swap conversations;
  other programs (vim, dev servers) don't come back yet.
- [x] **Shortcuts on Ctrl+Alt:** Ctrl+Alt+T, Plus, Minus, W, R, S, Z, G, H, Comma. Alt+Shift+Plus /
  Minus stay as in Windows Terminal; Ctrl+Shift+C / V stay for copy and paste. AltGr+key types
  its character when the layout has one (checked with ToUnicode while right Alt is down); left
  Ctrl + left Alt always give the shortcut.
- [x] **Ctrl+Shift+Left/Right select by word** (blanks and alacritty's double-click stop
  characters end a word); Ctrl+Shift+Up/Down act like Shift+Up/Down.
- [x] **Back to the landing page:** Ctrl+Alt+H and a ≡ button in the title bar, top-left, next to the grid/tabs button (min / max / close sit top-right). The
  session keeps running as the first choice ("This session", Esc goes back); opening another one
  asks once, then saves and closes it.
- [x] **Settings page** (Ctrl+Alt+Comma, keyboard only): theme (One Half Dark, Campbell, Catppuccin
  Mocha, Dracula), opacity, and every shortcut (Enter, press keys; Del = default; keys already
  used move over). Saved to `%APPDATA%\dterm\settings`.
- [x] **Versions and releases:** 0.2.0, shown on the landing page. `release.ps1` builds
  `dist\dterm-vX.Y.Z.exe` and `.zip`; `-Publish` tags and runs `gh release create`.
- [ ] Later: a GitHub Actions workflow that builds the release on tag push; mouse support on the
  settings page (the landing page and tabs already take the mouse); font family and size in settings.

## Requested features

Ordered roughly by how much they hurt today.

### Selection and clipboard
- [x] **Mouse selection.** Drag to select, double-click word, triple-click line.
- [x] **Shift+Left/Right selects** character by character (today it types a capital letter / sends
      an escape sequence). Shift+Up/Down extends by line. Like Windows Terminal's mark mode.
- [x] **Right-click:** copies when there is a selection, pastes when there is none (Windows Terminal behavior).
- [x] **Ctrl+C copies** when there is a selection, otherwise sends interrupt as today.
- [x] **Mouse-wheel scrollback** through history. The wheel goes to apps that enable mouse reporting
      or alternate scroll; clicks and drags are not reported to apps yet.
- [ ] Report clicks / drags to apps that ask for the mouse (vim, tmux, htop).

### Layout and shortcuts
Windows Terminal defaults in brackets, so muscle memory carries over.
- [x] **Alt+Shift+Plus / Alt+Shift+Minus split the focused pane**, exactly like WT:
      Plus = new pane to the right, Minus = new pane below.
      Moving from the auto-grid to real splits means a split tree instead of rows/cols fractions.
- [x] **Alt+Shift+arrows resize the focused pane** by a fixed step [WT: same].
- [x] **Ctrl+Alt+arrows swap** the focused pane with its neighbour [WT: swapPane, unbound by default].
- [x] **Zoom** the focused pane to fill the window, and back to the grid [WT: togglePaneZoom, unbound].
- [x] **Grid ⇄ tabs toggle.** One shortcut (and one small button) switches the same sessions
      between the grid and tab view. WT can't do this: panes and tabs are separate there.
- [x] Ctrl+Alt+W closes the focused pane [WT: Ctrl+Shift+W].
- [x] Keep an add-pane shortcut (now Ctrl+Alt+T, auto grid) and Alt+arrows (move focus, done).

### Knowing which pane is which
- [x] **Focus indicator:** a very faint border on the focused pane and slightly dimmer unfocused
      panes. Subtle, no accent-colour frame. [WT uses an accent-colour border.]
- [x] **Pane names without headers:** a small, low-contrast label in the pane's top-right corner
      (folder name by default, or the title the shell/app sets via OSC 0/2, or a name you type).
      Fades while you type in that pane. No title bars, no chrome.
- [x] **Agent status per pane:** the name of a background pane is tinted blue while it streams
      output, green once it goes quiet, yellow after a bell. Focusing the pane clears it.
- [ ] **Status colours are hard to see:** the tint is too faint on the label. Make it easier to
      notice (stronger colour, a dot or a border cue) without turning it loud. Also: OSC 9/777
      notifications and OSC 133 prompt marks as signals, and count the focused pane of an
      inactive window as a background pane.

### Sessions
- [x] **Session restore + landing page.** On start, a terminal-style menu (like opencode):
      new session / last session / saved sessions. A session stores the layout, pane names and each
      pane's working directory. [WT: `persistedWindowLayout` restores layout and directories, not contents.]
- [ ] Optionally save scrollback text per pane so a restored session shows what was there.

### Look
- [x] **Keep the acrylic when the window is unfocused.** Today Windows swaps the backdrop for a solid
      colour when inactive (DWMSBT_TRANSIENTWINDOW behavior). Needs a different backdrop method,
      e.g. drawing our own blur, or the accent-policy API.
- [x] **Ctrl+Plus / Ctrl+Minus / Ctrl+0 change the font size** [WT: same].
- [x] Cosmetic: highlighted text (inverse/background colours) shows thin dark lines between rows.
      Snap cell rectangles to whole pixels so backgrounds join seamlessly like in WT.
- [x] **Acrylic on Windows 10** (no system backdrops before Windows 11 22H2, where
      `DWMWA_SYSTEMBACKDROP_TYPE` fails with E_INVALIDARG): the accent policy gives the acrylic. Its blur
      also filled the invisible resize borders and the caption, so dterm drops the system frame there
      and draws its own title bar (since then on Windows 11 too, which keeps its own frame on the
      other three sides for rounded corners and the shadow). Acrylic halves the frame rate while dragging, so moves and resizes
      use plain blur. Host backdrop brushes render black on Windows 10, and a window region brings
      back the old basic frame, so neither works.

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
5. **Stays tiny:** one native exe (~470 KB today), instant start, no Electron, no settings file needed.

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
