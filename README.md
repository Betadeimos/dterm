<p align="center"><img src="assets/icon.png" width="128" alt="dterm icon"></p>

<h1 align="center">dterm</h1>

<p align="center"><b>A native Windows terminal that puts all your shells, and your coding agents, in one grid.</b><br>
One exe under 500 KB. No Electron. No config.</p>

![dterm demo: the start page, a saved session, adding panes, zoom, tabs, then themes and opacity in settings](assets/demo.gif)

## Why dterm

- **The grid builds itself.** `Ctrl+Alt+T` splits the largest pane. Press it four times, get a 2x2.
- **Drag to resize.** Grab any gap between panes and pull. (Windows Terminal has had this
  [open since 2019](https://github.com/microsoft/terminal/issues/992).)
- **Grid or Tabs, same sessions.** `Ctrl+Alt+G` flips between them without touching a shell.
- **Claude Code comes back.** Close dterm, reopen it: every pane returns to its folder, and panes that
  ran [Claude Code](https://claude.com/claude-code) resume their newest conversation (`claude --resume <id>`).
- **See which pane needs you.** A small dot before a pane's name (and its tab) is blue while it streams output, green once it has been quiet for 4.5 s, yellow after a bell. Focusing the pane clears it.
- **Looks like Windows Terminal.** Acrylic that stays on when unfocused, One Half Dark, Cascadia Mono,
  emoji and symbol fallback, 4 themes.

Native Rust, Direct2D and DirectWrite, ConPTY. Panes run Git Bash.

## Install

Download `dterm-vX.Y.Z.exe` (or the zip) from [Releases](https://github.com/Betadeimos/dterm/releases)
and run it. No installer: put it anywhere and pin it to the taskbar.

**Requires** Windows 10 1809+ and [Git for Windows](https://gitforwindows.org/). Windows 11 22H2+ uses the
system acrylic; Windows 10 gets dterm's own.

## Build from source

You need [Rust](https://rustup.rs/) (MSVC toolchain) and Visual Studio Build Tools with
"Desktop development with C++".

```
cargo build --release     # target\release\dterm.exe
cargo test                # unit tests
```

`.\release.ps1` builds `dist\dterm-vX.Y.Z.exe` and a zip; `-Publish` also tags and creates the GitHub
release (needs the GitHub CLI). Want to help? Read [CONTRIBUTING.md](CONTRIBUTING.md).

## Shortcuts

dterm's own shortcuts use Ctrl+Alt, so Ctrl+Shift combinations reach your shell and apps.
Rows marked * can be changed on the settings page. AltGr counts as Ctrl+Alt on many keyboards: when
AltGr+key types a character on your layout, that character wins, and left Ctrl + left Alt give the shortcut.

### Panes and layout

| Keys | Action |
| --- | --- |
| `Ctrl+Alt+T` * | Add a pane (splits the largest one) |
| `Ctrl+Alt+Plus` / `Ctrl+Alt+Minus` * | Split the focused pane right / below (also `Alt+Shift+Plus` / `Minus`) |
| `Ctrl+Alt+W` *, or `exit` | Close the pane |
| `Ctrl+Alt+R` * | Rename the focused pane |
| `Ctrl+Alt+Z` * | Zoom the focused pane to the whole window, and back |
| `Ctrl+Alt+G` * | Switch between Grid and Tabs (or the button, top-left) |
| `Alt+Arrows` | Move focus (in tabs: Left / Right pick the previous / next tab) |
| `Alt+Shift+Arrows` | Resize the focused pane (grid) |
| `Ctrl+Alt+Arrows` | Swap the focused pane with its neighbor (grid) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous pane or tab |
| Drag a gap | Resize panes |

### Sessions and settings

| Keys | Action |
| --- | --- |
| `Ctrl+Alt+S` * | Save the session (over its file, or ask for a name the first time) |
| `Ctrl+Alt+H` * | Start page (also the ≡ button); the session keeps running |
| `Ctrl+Alt+Comma` * | Settings: theme, opacity, every shortcut |

### Text

| Keys | Action |
| --- | --- |
| Mouse drag, double / triple click | Select text |
| `Shift+Arrows`, `Ctrl+Shift+Left` / `Right` | Select by character / word |
| `Ctrl+C` (with a selection), right-click | Copy; right-click with no selection pastes |
| `Ctrl+V` / `Ctrl+Shift+V` | Paste as plain text |
| `Esc` | Drop the selection |
| Mouse wheel | Scroll history (apps that ask for the mouse get the wheel) |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0`, `Ctrl+wheel` | Font size |

## Sessions

Closing the window, or going to the start page, saves the layout, each pane's folder and name, the view and the
font size to `%APPDATA%\dterm\last.session`. `Ctrl+Alt+S` saves it under a name in
`%APPDATA%\dterm\sessions`, and from then on saves over that file. Closing the last pane with `exit` ends dterm
without saving.

When something is saved, dterm opens on a start page: last session, a saved one (with a layout preview), or a
new one. Arrows or mouse choose, Enter opens, `F2` renames, `Del` (twice) deletes.

Settings live in `%APPDATA%\dterm\settings`, a small text file you can also edit by hand.

## Status and roadmap

Early, but in daily use. See [ROADMAP.md](ROADMAP.md): more status signals (OSC 9/777, OSC 133), broadcast input, mouse
reporting for vim and tmux.

## License

[Apache-2.0](LICENSE)
