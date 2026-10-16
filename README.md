<p align="center"><img src="assets/icon.png" width="128" alt="dterm icon"></p>

<h1 align="center">dterm</h1>

<p align="center">A tiny Windows terminal that puts your shells in a grid.</p>

![dterm with four panes: git log, color test, cargo test and an agent session](assets/screenshot.png)

One native exe (about 430 KB), nothing to configure. Panes run Git Bash and look like Windows
Terminal: acrylic background (kept when the window isn't focused), One Half Dark colors,
Cascadia Mono, font fallback for symbols and emoji. Each pane shows a faint name, and sessions
come back: close dterm and it offers to reopen the same panes in the same folders.

## Shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+Shift+T | Add a pane (splits the largest one, so the grid builds itself) |
| Alt+Shift+Plus / Alt+Shift+Minus | Split the focused pane to the right / below |
| Ctrl+Shift+W, or `exit` | Close the pane |
| Alt+Arrow keys | Move focus to the pane in that direction |
| Alt+Shift+Arrow keys | Resize the focused pane |
| Ctrl+Alt+Arrow keys | Swap the focused pane with its neighbor |
| Drag a gap between panes | Resize panes |
| Ctrl+Shift+Z | Zoom the focused pane to the whole window, and back |
| Ctrl+Shift+G | Switch between grid and tabs (also the button in the top-right corner) |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous pane or tab |
| Ctrl+Shift+R | Rename the focused pane |
| Ctrl+Shift+S | Save the session under a name |
| Mouse drag, double / triple click, Shift+Arrow keys | Select text |
| Ctrl+C (with a selection), right-click | Copy; right-click without a selection pastes |
| Ctrl+V / Ctrl+Shift+V | Paste as plain text |
| Mouse wheel | Scroll back through history |
| Ctrl+Plus / Ctrl+Minus / Ctrl+0, Ctrl+wheel | Font size |

## Sessions

Closing the window saves the session (layout, each pane's folder and name, view, font size) to
`%APPDATA%\dterm\last.session`; Ctrl+Shift+S saves it under a name in `%APPDATA%\dterm\sessions`.
When something is saved, dterm starts on a small landing page: pick the last session, a saved
one (with a preview of its layout and when it was started and closed), or a new one.

## Requirements

- Windows 10 1809 or later (Windows 11 for the acrylic backdrop)
- [Git for Windows](https://gitforwindows.org/) (dterm runs its Git Bash)

## Build

```
cargo build --release
```

The exe is `target\release\dterm.exe`. The icon is linked from the prebuilt `dterm.res`, so the
build needs no resource compiler. To change it, draw a new one with `assets\icon.ps1`, point
`dterm.rc` at it and rebuild `dterm.res` (steps in `build.rs`).

## Status

Early, but in daily use. See [ROADMAP.md](ROADMAP.md) for what's next, like per-pane agent
status (busy / done / waiting for you) and broadcast input.

## License

[Apache-2.0](LICENSE)
