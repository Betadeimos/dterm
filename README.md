# dterm

A tiny Windows terminal that puts your shells in a grid.

One native exe (about 320 KB), nothing to configure. Panes run Git Bash, start in your home
folder, and look like Windows Terminal: acrylic background, One Half Dark colors, Cascadia Mono,
font fallback for symbols and emoji.

## Shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+Shift+T | Add a pane (the grid rearranges itself) |
| Alt+Arrow keys | Move focus to the pane in that direction |
| Ctrl+V / Ctrl+Shift+V | Paste as plain text |
| Drag a gap between panes | Resize panes |
| Click a pane | Type into it |
| `exit` | Close the pane (closing the last one closes the window) |

## Requirements

- Windows 10 1809 or later (Windows 11 for the acrylic backdrop)
- [Git for Windows](https://gitforwindows.org/) (dterm runs its Git Bash)

## Build

```
cargo build --release
```

The exe is `target\release\dterm.exe`. The icon comes from the prebuilt `dterm.res`; to change it,
see `icon.ps1` and `build.rs`.

## Status

Early, but in daily use. See [ROADMAP.md](ROADMAP.md) for what's next: text selection and
copy, scrollback, splits like Windows Terminal, grid/tabs toggle, session restore, and per-pane
agent status.

## License

[Apache-2.0](LICENSE)
