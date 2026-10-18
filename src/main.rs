#![cfg_attr(not(test), windows_subsystem = "windows")]
//! dterm: Git Bash panes in a tree of splits, drawn with Direct2D/DirectWrite on an acrylic window.
//! Terminal emulation and ConPTY come from alacritty_terminal.
//! Shortcuts (the defaults; the settings page changes them): Ctrl+Alt+T splits the largest pane,
//! Ctrl+Alt+Plus / Ctrl+Alt+Minus (or Alt+Shift+Plus / Minus, as in Windows Terminal) split the
//! focused one side by side / top and bottom, Ctrl+Alt+W (or `exit`) closes a pane, Ctrl+Alt+R
//! renames it (Enter confirms, Esc cancels, empty resets), Ctrl+Alt+Z zooms it to the whole window,
//! Ctrl+Alt+G shows the panes as tabs (also the button in the title bar), Ctrl+Alt+S saves the
//! session under a name, Ctrl+Alt+H goes to the start page (the ≡ button next to it) and Ctrl+Alt+Comma opens the settings (theme, opacity, shortcuts).
//! AltGr+key still types its character where the keyboard layout has one.
//! Fixed keys: Alt+arrows move the focus, Alt+Shift+arrows resize, Ctrl+Alt+arrows swap panes, drag
//! the gaps to resize, Ctrl+Tab / Ctrl+Shift+Tab go to the next / previous pane. Select with the
//! mouse or Shift+arrows (Ctrl+Shift+Left / Right by word), copy with Ctrl+C (or right-click),
//! Ctrl+V pastes plain text, the wheel scrolls back, Ctrl+Plus / Minus / 0 / wheel set the font size.
//! Closing the window remembers the session (layout, folders, names, view, font, and the Claude Code
//! conversation of each pane that runs one); with anything saved, dterm starts on a landing page
//! to pick a session or a new one.

use std::{
    borrow::Cow,
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Instant,
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{cell::Flags, color::Colors, test::TermSize, Config, Term, TermMode},
    tty,
    vte::ansi::{Color, CursorShape, Rgb},
};
use windows::{
    core::{s, w, BOOL, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dwm::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT},
        },
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            LibraryLoader::{GetModuleHandleW, GetProcAddress},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
            Registry::*,
            Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
            Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
        },
        UI::{Controls::{MARGINS, WM_MOUSELEAVE}, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

const FONT: PCWSTR = w!("Cascadia Mono");
const FONT_SIZE: f32 = 16.0; // DIPs, = 12pt like Windows Terminal
const PAD: f32 = 8.0; // DIPs around the text, like Windows Terminal
const LABEL_TOP: f32 = 22.0; // top margin of a pane that shows a name label
const LABEL_SIZE: f32 = 11.0; // DIPs
const NAME_MAX: usize = 32; // characters of a typed pane name
const MSYSTEMS: [&str; 6] = ["MINGW64", "MINGW32", "MSYS", "UCRT64", "CLANG64", "CLANGARM64"];
const GAP: f32 = 6.0; // DIPs between panes
const MIN: f32 = 0.05; // smallest child of a split, as a fraction of it
const STEP: f32 = 40.0; // DIPs a divider moves per Alt+Shift+arrow
const OPACITY: f32 = 0.7; // default pane background over the acrylic, like your Windows Terminal profile
const WM_WAKE: u32 = WM_APP; // new terminal output
const WM_EXIT: u32 = WM_APP + 1; // a shell exited; wparam = pane id
const SLOP: f32 = 4.0; // DIPs the mouse may move between the clicks of a double-click

/// A color scheme. ANSI order: black red green yellow blue magenta cyan white, then the bright ones.
struct Theme {
    name: &'static str,
    fg: u32,
    bg: u32,
    gap: u32, // between panes
    ansi: [u32; 16],
}

/// The first is the default: One Half Dark, as in Windows Terminal. The others are Windows
/// Terminal's Campbell and the Catppuccin Mocha and Dracula schemes.
const THEMES: [Theme; 4] = [
    Theme {
        name: "One Half Dark",
        fg: 0xDCDFE4,
        bg: 0x282C34,
        gap: 0x1B1D23,
        ansi: [
            0x282C34, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xDCDFE4,
            0x5A6374, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xDCDFE4,
        ],
    },
    Theme {
        name: "Campbell",
        fg: 0xCCCCCC,
        bg: 0x0C0C0C,
        gap: 0x000000,
        ansi: [
            0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC,
            0x767676, 0xE74856, 0x16C60C, 0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
        ],
    },
    Theme {
        name: "Catppuccin Mocha",
        fg: 0xCDD6F4,
        bg: 0x1E1E2E,
        gap: 0x11111B,
        ansi: [
            0x45475A, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xBAC2DE,
            0x585B70, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xA6ADC8,
        ],
    },
    Theme {
        name: "Dracula",
        fg: 0xF8F8F2,
        bg: 0x282A36,
        gap: 0x191A21,
        ansi: [
            0x21222C, 0xFF5555, 0x50FA7B, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x8BE9FD, 0xF8F8F2,
            0x6272A4, 0xFF6E6E, 0x69FF94, 0xFFFFA5, 0xD6ACFF, 0xFF92DF, 0xA4FFFF, 0xFFFFFF,
        ],
    },
];

/// Index into THEMES of the theme in use; a global so that color lookups need no App.
static THEME: AtomicUsize = AtomicUsize::new(0);

fn theme() -> &'static Theme {
    &THEMES[THEME.load(Ordering::Relaxed).min(THEMES.len() - 1)]
}

fn fg() -> Rgb {
    rgb(theme().fg)
}

fn bg() -> Rgb {
    rgb(theme().bg)
}

const SELECTION_ALPHA: f32 = 0.25; // foreground veil over selected cells
const FOCUS_ALPHA: f32 = 0.15; // foreground border of the focused pane
const VEIL_ALPHA: f32 = 0.25; // background veil over unfocused panes
const LABEL_ALPHA: f32 = 0.4; // pane name; brighter on the focused pane
const LABEL_FOCUS_ALPHA: f32 = 0.7;
const TAB_H: f32 = 30.0; // DIPs, height of the tab strip
const TAB_MAX: f32 = 180.0; // widest tab; they shrink to share the strip when there are many
const TAB_PAD: f32 = 10.0; // DIPs between a tab's edge and its text
const TAB_SIZE: f32 = 12.0; // DIPs
const TAB_ACTIVE_ALPHA: f32 = 0.9; // name of the active tab; inactive ones use LABEL_ALPHA, hovered LABEL_FOCUS_ALPHA
const UNDERLINE_ALPHA: f32 = 0.6; // underline of the active tab
const CAPTION_H: f32 = 32.0; // DIPs, the title bar dterm draws itself; everything else is below it
const CAPTION_BTN: f32 = 46.0; // DIPs, width of its minimize, maximize and close buttons
const RESIZE_BORDER: f32 = 6.0; // DIPs inside the window edge that resize it on Windows 10
const LAND_X: f32 = 48.0; // DIPs, where the landing page's text starts
const LAND_Y: f32 = 56.0;
const LAND_ROWS: f32 = LAND_Y + 130.0; // first choice, below the logo and tagline
const LOGO_SIZE: f32 = 44.0; // DIPs
const PREVIEW: (f32, f32) = (320.0, 200.0); // DIPs, the selected session's layout
const ICON_W: f32 = 28.0; // DIPs, width of a landing row's rename or delete icon
const FOOT_H: f32 = 30.0; // DIPs, the landing page's footer bar
const PROMPT_MAX: usize = 40; // characters of a typed session name
const TOAST_TIMER: usize = 1; // hides the "saved" note
const STATUS_TIMER: usize = 2; // runs while a pane is busy, to notice when it goes quiet
const BUSY_MS: u64 = 1500; // output within this long ago counts as busy
const STATUS_TICK: u32 = 400; // ms between checks while a pane is busy
const STATUS_ALPHA: f32 = 0.75; // tinted name of a background pane that is done or wants you
const BUSY_ALPHA: f32 = 0.55; // ... and of one that is busy

fn rgb(c: u32) -> Rgb {
    Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 }
}

fn dim(c: Rgb) -> Rgb {
    Rgb { r: c.r / 3 * 2, g: c.g / 3 * 2, b: c.b / 3 * 2 }
}

/// xterm 256-color table.
fn indexed(i: u8) -> Rgb {
    match i {
        0..=15 => rgb(theme().ansi[i as usize]),
        16..=231 => {
            let i = i - 16;
            let v = |x: u8| if x == 0 { 0 } else { 55 + x * 40 };
            Rgb { r: v(i / 36), g: v(i / 6 % 6), b: v(i % 6) }
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            Rgb { r: g, g, b: g }
        }
    }
}

/// `None` = the default background, which stays see-through.
fn resolve(c: Color, bold: bool, colors: &Colors) -> Option<Rgb> {
    let i = match c {
        Color::Spec(rgb) => return Some(rgb),
        Color::Indexed(i) => i as usize,
        Color::Named(n) => n as usize,
    };
    let i = if bold && i < 8 { i + 8 } else { i }; // bold text uses the bright colors
    if let Some(set) = colors[i] {
        return Some(set); // changed by the app (OSC 4/10/11)
    }
    match i {
        0..=255 => Some(indexed(i as u8)),
        257 => None,                                      // background
        259..=266 => Some(dim(indexed((i - 259) as u8))), // dim colors
        268 => Some(dim(fg())),                        // dim foreground
        _ => Some(fg()),                               // foreground, cursor, bright foreground
    }
}

/// Escape sequence for a key that produces no character (arrows, F-keys, ...).
fn key_seq(vk: u16, shift: bool, alt: bool, ctrl: bool, app_cursor: bool) -> Option<String> {
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8; // xterm modifier code
    let csi = |f: char| match (m > 1, app_cursor) {
        (true, _) => format!("\x1b[1;{m}{f}"),
        (false, true) => format!("\x1bO{f}"),
        (false, false) => format!("\x1b[{f}"),
    };
    let ss3 = |f: char| if m > 1 { format!("\x1b[1;{m}{f}") } else { format!("\x1bO{f}") };
    let tilde = |n: u8| if m > 1 { format!("\x1b[{n};{m}~") } else { format!("\x1b[{n}~") };
    Some(match vk {
        0x26 => csi('A'), // up
        0x28 => csi('B'), // down
        0x27 => csi('C'), // right
        0x25 => csi('D'), // left
        0x24 => csi('H'), // home
        0x23 => csi('F'), // end
        0x2D => tilde(2), // insert
        0x2E => tilde(3), // delete
        0x21 => tilde(5), // page up
        0x22 => tilde(6), // page down
        0x70 => ss3('P'),
        0x71 => ss3('Q'),
        0x72 => ss3('R'),
        0x73 => ss3('S'),
        0x74 => tilde(15),
        0x75 => tilde(17),
        0x76 => tilde(18),
        0x77 => tilde(19),
        0x78 => tilde(20),
        0x79 => tilde(21),
        0x7A => tilde(23),
        0x7B => tilde(24),
        0x09 if shift => "\x1b[Z".into(), // shift+tab
        _ => return None,
    })
}

/// Clipboard text as the shell should receive it: Enter-style line breaks, and wrapped in
/// bracketed-paste markers when the app asked for them (so pasted lines are not run one by one).
fn paste_text(text: &str, bracketed: bool) -> String {
    let t = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // An ESC inside could fake the end marker and run the rest as typed input.
        format!("\x1b[200~{}\x1b[201~", t.replace('\x1b', ""))
    } else {
        t
    }
}

type Rect = (f32, f32, f32, f32); // x, y, w, h in DIPs

fn contains((x, y, w, h): Rect, (px, py): (f32, f32)) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// The two halves of `r` split at `ratio`, with a GAP between them.
fn halves((x, y, w, h): Rect, side_by_side: bool, ratio: f32) -> (Rect, Rect) {
    let g = GAP / 2.0;
    if side_by_side {
        let cut = w * ratio;
        ((x, y, (cut - g).max(0.0), h), (x + cut + g, y, (w - cut - g).max(0.0), h))
    } else {
        let cut = h * ratio;
        ((x, y, w, (cut - g).max(0.0)), (x, y + cut + g, w, (h - cut - g).max(0.0)))
    }
}

/// How the panes are laid out: a binary tree whose leaves are pane ids.
#[derive(Debug, PartialEq)]
enum Node {
    Leaf(usize),
    /// `a` left of `b` when `side_by_side`, else `a` above `b`; `ratio` is `a`'s share of the extent.
    Split { side_by_side: bool, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    fn has(&self, id: usize) -> bool {
        match self {
            Node::Leaf(i) => *i == id,
            Node::Split { a, b, .. } => a.has(id) || b.has(id),
        }
    }

    /// The same tree with its leaves numbered `next`, `next + 1`, ... in tree order.
    fn numbered(&self, next: &mut usize) -> Node {
        match self {
            Node::Leaf(_) => {
                *next += 1;
                Node::Leaf(*next - 1)
            }
            Node::Split { side_by_side, ratio, a, b } => {
                let a = Box::new(a.numbered(next));
                Node::Split { side_by_side: *side_by_side, ratio: *ratio, a, b: Box::new(b.numbered(next)) }
            }
        }
    }

    /// Pane ids in tree order: left to right, top to bottom (the order of `rects`).
    fn ids(&self) -> Vec<usize> {
        match self {
            Node::Leaf(i) => vec![*i],
            Node::Split { a, b, .. } => {
                let mut v = a.ids();
                v.extend(b.ids());
                v
            }
        }
    }

    /// Turn leaf `id` into a half-and-half split holding it and the new leaf `new`.
    fn split(&mut self, id: usize, new: usize, side_by_side: bool) -> bool {
        match self {
            Node::Leaf(i) if *i == id => {
                let (a, b) = (Box::new(Node::Leaf(id)), Box::new(Node::Leaf(new)));
                *self = Node::Split { side_by_side, ratio: 0.5, a, b };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(id, new, side_by_side) || b.split(id, new, side_by_side),
        }
    }

    /// The tree without leaf `id`: its sibling takes the place of their parent split.
    fn remove(self, id: usize) -> Option<Node> {
        match self {
            Node::Leaf(i) => (i != id).then_some(self),
            Node::Split { side_by_side, ratio, a, b } => match (a.remove(id), b.remove(id)) {
                (Some(a), Some(b)) => Some(Node::Split { side_by_side, ratio, a: Box::new(a), b: Box::new(b) }),
                (a, b) => a.or(b),
            },
        }
    }

    /// The first (top or left) or last pane of this subtree.
    fn edge(&self, first: bool) -> usize {
        match self {
            Node::Leaf(i) => *i,
            Node::Split { a, b, .. } => if first { a.edge(true) } else { b.edge(false) },
        }
    }

    /// The pane of leaf `id`'s sibling that is nearest to it: who gets the room when `id` closes.
    fn sibling(&self, id: usize) -> Option<usize> {
        let Node::Split { a, b, .. } = self else { return None };
        if matches!(**a, Node::Leaf(i) if i == id) {
            Some(b.edge(true))
        } else if matches!(**b, Node::Leaf(i) if i == id) {
            Some(a.edge(false))
        } else {
            a.sibling(id).or_else(|| b.sibling(id))
        }
    }

    /// Exchange the places of leaves `x` and `y`.
    fn swap(&mut self, x: usize, y: usize) {
        match self {
            Node::Leaf(i) if *i == x => *i = y,
            Node::Leaf(i) if *i == y => *i = x,
            Node::Leaf(_) => {}
            Node::Split { a, b, .. } => {
                a.swap(x, y);
                b.swap(x, y);
            }
        }
    }

    /// Where each pane goes when the tree fills `r`.
    fn rects(&self, r: Rect) -> Vec<(usize, Rect)> {
        match self {
            Node::Leaf(i) => vec![(*i, r)],
            Node::Split { side_by_side, ratio, a, b } => {
                let (ra, rb) = halves(r, *side_by_side, *ratio);
                let mut v = a.rects(ra);
                v.extend(b.rects(rb));
                v
            }
        }
    }

    /// Divider under `p` in a tree filling `r`: the path to its split (false = `a`, true = `b` at
    /// each step from the root) and whether that split is side by side. Only the gap counts.
    fn divider_at(&self, r: Rect, p: (f32, f32)) -> Option<(Vec<bool>, bool)> {
        let Node::Split { side_by_side, ratio, a, b } = self else { return None };
        let (ra, rb) = halves(r, *side_by_side, *ratio);
        let (to_b, child, cr) = if contains(ra, p) {
            (false, a, ra)
        } else if contains(rb, p) {
            (true, b, rb)
        } else {
            return contains(r, p).then_some((Vec::new(), *side_by_side));
        };
        let (mut path, sbs) = child.divider_at(cr, p)?;
        path.insert(0, to_b);
        Some((path, sbs))
    }

    /// Put the divider of the split at `path` under `p`, in a tree filling `r`.
    fn drag(&mut self, path: &[bool], r: Rect, p: (f32, f32)) {
        let Node::Split { side_by_side, ratio, a, b } = self else { return };
        match path.split_first() {
            Some((&to_b, rest)) => {
                let (ra, rb) = halves(r, *side_by_side, *ratio);
                if to_b { b.drag(rest, rb, p) } else { a.drag(rest, ra, p) }
            }
            None => {
                let (start, extent, at) = if *side_by_side { (r.0, r.2, p.0) } else { (r.1, r.3, p.1) };
                *ratio = ((at - start) / extent.max(1.0)).clamp(MIN, 1.0 - MIN);
            }
        }
    }

    /// Move the divider of leaf `id`'s nearest enclosing split with this orientation by `delta`
    /// DIPs, in a tree filling `r`. False when there is none.
    fn resize(&mut self, id: usize, r: Rect, side_by_side: bool, delta: f32) -> bool {
        let Node::Split { side_by_side: mine, ratio, a, b } = self else { return false };
        let (ra, rb) = halves(r, *mine, *ratio);
        let done = if a.has(id) {
            a.resize(id, ra, side_by_side, delta)
        } else if b.has(id) {
            b.resize(id, rb, side_by_side, delta)
        } else {
            return false;
        };
        if done {
            return true;
        }
        if *mine != side_by_side {
            return false;
        }
        let extent = if side_by_side { r.2 } else { r.3 };
        *ratio = (*ratio + delta / extent.max(1.0)).clamp(MIN, 1.0 - MIN);
        true
    }
}

/// Pane (index) to split for Ctrl+Shift+T, and whether side by side: the largest one (the focused
/// one on a tie), along its longer side.
fn auto_split(rects: &[Rect], focus: usize) -> (usize, bool) {
    let area = |i: usize| rects.get(i).map_or(0.0, |r| r.2 * r.3);
    let at = (0..rects.len()).fold(focus, |best, i| if area(i) > area(best) + 1.0 { i } else { best });
    (at, rects.get(at).is_none_or(|r| r.2 >= r.3))
}

/// Pane next to `from` in direction `(dx, dy)`: the nearest one past its edge, preferring panes
/// straight in line with it.
fn neighbor(rects: &[Rect], from: usize, (dx, dy): (i32, i32)) -> Option<usize> {
    let (fx, fy, fw, fh) = *rects.get(from)?;
    let (fcx, fcy) = (fx + fw / 2.0, fy + fh / 2.0);
    rects
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .filter_map(|(i, &(x, y, w, h))| {
            let ahead = match (dx, dy) {
                (1, _) => x - (fx + fw),
                (-1, _) => fx - (x + w),
                (_, 1) => y - (fy + fh),
                _ => fy - (y + h),
            };
            let side = if dx != 0 { (y + h / 2.0 - fcy).abs() } else { (x + w / 2.0 - fcx).abs() };
            (ahead >= -1.0).then_some((i, ahead + side))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Where the text of a pane filling `r` goes: its top-left corner and the (width, height) it has.
/// A pane that shows a name label gets a taller top margin. Layout, drawing and mouse mapping
/// all take the text origin from here.
fn inner((x, y, w, h): Rect, labeled: bool) -> ((f32, f32), (f32, f32)) {
    let top = if labeled { LABEL_TOP } else { PAD };
    ((x + PAD, y + top), (w - 2.0 * PAD, h - top - PAD))
}

/// What the window shows: every pane in the split tree, only the focused one filling the window,
/// or the focused one under a strip with a tab per pane. The tree is the same in all three.
#[derive(Clone, Copy, PartialEq, Debug)]
enum View {
    Grid,
    Zoom,
    Tabs,
}

/// View after a request for `to` (Zoom or Tabs): asking for the view you are in goes back to the
/// grid, and Zoom does nothing in Tabs.
fn toggled(view: View, to: View) -> View {
    match (view, to) {
        (View::Tabs, View::Zoom) => View::Tabs,
        (v, t) if v == t => View::Grid,
        (_, t) => t,
    }
}

/// Where each pane goes in a `w` x `h` window: the tree's rects, the focused pane `shown` alone
/// (Zoom), or every pane on the same area under the tab strip, so that switching tabs never
/// resizes a terminal.
fn place(tree: &Node, view: View, shown: usize, (w, h): (f32, f32)) -> Vec<(usize, Rect)> {
    match view {
        View::Grid => tree.rects((0.0, 0.0, w, h)),
        View::Zoom => vec![(shown, (0.0, 0.0, w, h))],
        View::Tabs => tree.ids().into_iter().map(|i| (i, (0.0, TAB_H, w, (h - TAB_H).max(0.0)))).collect(),
    }
}

/// Index `step` places after `at` among `len`, wrapping.
fn cycle(len: usize, at: usize, step: isize) -> usize {
    (at as isize + step).rem_euclid(len.max(1) as isize) as usize
}

/// Width of one tab when `n` of them share `room` DIPs.
fn tab_width(n: usize, room: f32) -> f32 {
    (room / n.max(1) as f32).clamp(0.0, TAB_MAX)
}

/// The tab under `m` in Tabs view: `tabs` tabs share the strip from the left of a `width` DIPs wide window.
fn tab_at(view: View, tabs: usize, (x, y): (f32, f32), width: f32) -> Option<usize> {
    let tw = tab_width(tabs, width);
    (view == View::Tabs && (0.0..TAB_H).contains(&y) && tw > 0.0 && (0.0..tw * tabs as f32).contains(&x)).then(|| (x / tw) as usize)
}

/// What a point `(x, y)` in a `(w, h)` client (pixels) is over when dterm draws its own frame:
/// the resize edges (`border` thick, 0 when maximized), the caption (`caption` high, minus the
/// `(left, right)` wide button strips at its ends, which stay client so dterm draws and clicks them), or the client.
fn frame_hit((x, y): (i32, i32), (w, h): (i32, i32), border: i32, caption: i32, (left, right): (i32, i32)) -> u32 {
    let (l, r, t, b) = (x < border, x >= w - border, y < border, y >= h - border);
    match (l, r, t, b) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (.., true) => HTBOTTOM,
        _ if y < caption && x >= left && x < w - right => HTCAPTION,
        _ => HTCLIENT,
    }
}

/// Which caption button is at `x` in a `width` DIPs wide window: 5 settings at the left end, then
/// 0 start page and 1 grid / tabs (only while a `session` is shown), 2 minimize, 3 maximize and 4 close at the right.
fn caption_button(x: f32, width: f32, session: bool) -> Option<usize> {
    let (left, right) = ((x / CAPTION_BTN).floor(), ((x - (width - 3.0 * CAPTION_BTN)) / CAPTION_BTN).floor());
    if (0.0..3.0).contains(&right) {
        Some(right as usize + 2)
    } else if left == 0.0 {
        Some(5)
    } else {
        (session && left < 3.0 && left > 0.0).then(|| left as usize - 1)
    }
}

/// Left edge of caption button `i` (see `caption_button`).
fn caption_x(i: usize, width: f32) -> f32 {
    match i {
        5 => 0.0,
        0 | 1 => (i + 1) as f32 * CAPTION_BTN,
        _ => width - (5 - i) as f32 * CAPTION_BTN,
    }
}

/// How many caption buttons sit at the left end: settings, plus start page and grid / tabs with a session.
fn left_buttons(session: bool) -> f32 {
    if session { 3.0 } else { 1.0 }
}

/// The tooltip of caption button `i`: its action and the keys that do the same, as set in `keys`.
fn caption_tip(i: usize, view: View, zoomed: bool, keys: &[Option<Chord>]) -> (&'static str, Option<String>) {
    let keys_of = |act: Action| keys[act as usize].map(chord_text);
    match i {
        0 => ("Start page", keys_of(Action::Home)),
        1 if view == View::Tabs => ("Grid view", keys_of(Action::Tabs)),
        1 => ("Tabs view", keys_of(Action::Tabs)),
        2 => ("Minimize", None),
        3 if zoomed => ("Restore", None),
        3 => ("Maximize", None),
        4 => ("Close", Some("Alt+F4".into())),
        _ => ("Settings", keys_of(Action::Settings)),
    }
}

/// Whether panes are shown, so the start page and grid / tabs buttons make sense.
fn session_shown(a: &App) -> bool {
    a.landing.is_none() && a.page.is_none()
}

/// Terminal columns and lines that fit in `(w, h)` DIPs of text area.
fn fit((w, h): (f32, f32), cell: (f32, f32)) -> (usize, usize) {
    ((w / cell.0).floor().max(2.0) as usize, (h / cell.1).floor().max(1.0) as usize)
}

/// Font size after a zoom step: one DIP bigger (+1), smaller (-1), or the default (0). Clamped to 8..=40.
fn zoomed(size: f32, step: i32) -> f32 {
    if step == 0 { FONT_SIZE } else { (size + step as f32).clamp(8.0, 40.0) }
}

/// dterm commands that have a shortcut you can change on the settings page.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    NewPane,
    SplitRight,
    SplitDown,
    Close,
    Rename,
    Save,
    Zoom,
    Tabs,
    Home,
    Settings,
}

/// Each action's id in the settings file, its name on the settings page, and its default keys.
/// The settings page lists them in this order.
const ACTIONS: [(Action, &str, &str, &str); 10] = [
    (Action::NewPane, "new-pane", "New pane", "Ctrl+Alt+T"),
    (Action::SplitRight, "split-right", "Split right", "Ctrl+Alt+Plus"),
    (Action::SplitDown, "split-down", "Split below", "Ctrl+Alt+Minus"),
    (Action::Close, "close-pane", "Close pane", "Ctrl+Alt+W"),
    (Action::Rename, "rename-pane", "Rename pane", "Ctrl+Alt+R"),
    (Action::Save, "save-session", "Save session", "Ctrl+Alt+S"),
    (Action::Zoom, "zoom", "Zoom pane", "Ctrl+Alt+Z"),
    (Action::Tabs, "grid-tabs", "Grid / tabs", "Ctrl+Alt+G"),
    (Action::Home, "start-page", "Start page", "Ctrl+Alt+H"),
    (Action::Settings, "settings", "Settings", "Ctrl+Alt+Comma"),
];

/// A key with modifiers.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Chord {
    vk: u16,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

/// The chord for a key event. The keypad's + and - count as the main ones.
fn chord_of(vk: u16, shift: bool, alt: bool, ctrl: bool) -> Chord {
    let vk = match vk {
        0x6B => 0xBB, // VK_ADD -> VK_OEM_PLUS
        0x6D => 0xBD, // VK_SUBTRACT -> VK_OEM_MINUS
        v => v,
    };
    Chord { vk, ctrl, alt, shift }
}

/// Keys a shortcut can use besides letters, digits and F1-F12. Arrows and Tab are taken by the
/// fixed shortcuts.
const KEY_NAMES: [(u16, &str); 11] = [
    (0xBB, "Plus"), (0xBD, "Minus"), (0xBC, "Comma"), (0xBE, "Period"), (0x24, "Home"), (0x23, "End"),
    (0x21, "PageUp"), (0x22, "PageDown"), (0x2D, "Insert"), (0x2E, "Delete"), (0x20, "Space"),
];

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some((vk as u8 as char).to_string()),
        0x70..=0x7B => Some(format!("F{}", vk - 0x6F)),
        _ => KEY_NAMES.iter().find(|k| k.0 == vk).map(|k| k.1.into()),
    }
}

/// "Ctrl+Alt+T".
fn chord_text(c: Chord) -> String {
    let mods = [(c.ctrl, "Ctrl+"), (c.alt, "Alt+"), (c.shift, "Shift+")];
    let mut s: String = mods.iter().filter(|m| m.0).map(|m| m.1).collect();
    s.push_str(&key_name(c.vk).unwrap_or_default());
    s
}

/// The chord in `chord_text` form. A shortcut needs Ctrl or Alt, so that it never takes typing.
fn parse_chord(text: &str) -> Option<Chord> {
    let (mods, key) = text.trim().rsplit_once('+').unwrap_or(("", text.trim()));
    let vk = (0..=0xFFu16).find(|&v| key_name(v).is_some_and(|n| n.eq_ignore_ascii_case(key)))?;
    let mut c = Chord { vk, ctrl: false, alt: false, shift: false };
    for m in mods.split('+') {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" => c.ctrl = true,
            "alt" => c.alt = true,
            "shift" => c.shift = true,
            _ => return None,
        }
    }
    (c.ctrl || c.alt).then_some(c)
}

const MIN_OPACITY: f32 = 0.2;

/// What the settings page changes: the theme (index into THEMES), the pane opacity, and each
/// action's keys (`None` = no shortcut), in ACTIONS order.
#[derive(Clone, PartialEq, Debug)]
struct Settings {
    theme: usize,
    opacity: f32,
    keys: [Option<Chord>; ACTIONS.len()],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: 0, opacity: OPACITY, keys: ACTIONS.map(|a| parse_chord(a.3)) }
    }
}

impl Settings {
    fn action(&self, c: Chord) -> Option<Action> {
        self.keys.iter().position(|k| *k == Some(c)).map(|i| ACTIONS[i].0)
    }

    /// Give action `i` the keys `c`, taking them away from any other action that had them.
    fn bind(&mut self, i: usize, c: Option<Chord>) {
        for k in self.keys.iter_mut().filter(|k| c.is_some() && **k == c) {
            *k = None;
        }
        self.keys[i] = c;
    }
}

/// Settings file text: `theme <name>`, `opacity <percent>`, then `key <action id> <keys or none>`.
fn settings_text(s: &Settings) -> String {
    let mut out = format!("dterm-settings 1\ntheme {}\nopacity {}\n", THEMES[s.theme].name, (s.opacity * 100.0).round());
    for (a, k) in ACTIONS.iter().zip(s.keys) {
        out.push_str(&format!("key {} {}\n", a.1, k.map_or("none".into(), chord_text)));
    }
    out
}

/// Settings in `settings_text` form. Lines that are missing or unreadable keep the default,
/// except keys that can't be read, which end up with no shortcut.
fn parse_settings(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines().map(|l| l.trim().trim_start_matches('\u{feff}')) {
        if let Some(name) = line.strip_prefix("theme ") {
            s.theme = THEMES.iter().position(|t| t.name.eq_ignore_ascii_case(name.trim())).unwrap_or(s.theme);
        } else if let Some(v) = line.strip_prefix("opacity ") {
            s.opacity = v.trim().parse::<f32>().map_or(s.opacity, |v| (v / 100.0).clamp(MIN_OPACITY, 1.0));
        } else if let Some((id, keys)) = line.strip_prefix("key ").and_then(|r| r.split_once(' ')) {
            if let Some(i) = ACTIONS.iter().position(|a| a.1 == id) {
                s.keys[i] = parse_chord(keys);
            }
        }
    }
    s
}

/// A pane in a saved session: its folder, its typed name, and the command that brings back the
/// program it was running (see `resume_command`). Any of them may be empty.
#[derive(Debug, PartialEq, Default, Clone)]
struct SavedPane {
    cwd: String,
    name: String,
    run: String,
}

/// A saved session: the split tree (its leaves index `panes`, in tree order), the panes, the
/// view, the font size, the focused pane's index, and when the session was started and last
/// saved (Unix seconds, 0 = unknown).
#[derive(Debug, PartialEq)]
struct Session {
    view: View,
    font: f32,
    focus: usize,
    tree: Node,
    panes: Vec<SavedPane>,
    created: u64,
    closed: u64,
}

/// Whether `run` is a command dterm itself would save: `claude` alone, or resuming one Claude Code
/// conversation by its id. Session files are plain text, so nothing else is ever run from them.
fn resume_command(run: &str) -> bool {
    let id_ok = |id: &str| !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    run == "claude" || run.strip_prefix("claude --resume ").is_some_and(id_ok)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Local (year, month, day, hour, minute) of Unix time `t`.
fn local_time(t: u64) -> Option<(u16, u16, u16, u16, u16)> {
    let ticks = (t + 11_644_473_600) * 10_000_000; // FILETIME counts 100 ns since 1601
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some((local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute))
}

/// `t` for people: "today 15:42", "yesterday 09:10", "16 Oct 14:02", or "16 Oct 2025" in another year.
fn when((y, mo, d, h, mi): (u16, u16, u16, u16, u16), today: (u16, u16, u16), yesterday: (u16, u16, u16)) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS[(mo.clamp(1, 12) - 1) as usize];
    match (y, mo, d) {
        t if t == today => format!("today {h:02}:{mi:02}"),
        t if t == yesterday => format!("yesterday {h:02}:{mi:02}"),
        _ if y == today.0 => format!("{d} {month} {h:02}:{mi:02}"),
        _ => format!("{d} {month} {y}"),
    }
}

/// `when` for Unix time `t` against the local clock; empty when unknown.
fn ago(t: u64) -> String {
    let day = |t: u64| local_time(t).map(|(y, m, d, _, _)| (y, m, d));
    match (t > 0).then(|| local_time(t)).flatten() {
        Some(at) => when(at, day(now()).unwrap_or_default(), day(now().saturating_sub(86_400)).unwrap_or_default()),
        None => String::new(),
    }
}

/// Session file text: a header, then the tree in pre-order (a `split` line is followed by its two
/// children). A pane line is `pane <folder>\t<name>`, then `\t<command>` when it has one.
fn session_text(s: &Session) -> String {
    fn node(n: &Node, panes: &[SavedPane], out: &mut String) {
        match n {
            Node::Leaf(i) => {
                let p = panes.get(*i).cloned().unwrap_or_default();
                let clean = |t: &str| t.replace(['\t', '\r', '\n'], " ");
                out.push_str(&format!("pane {}\t{}", clean(&p.cwd), clean(&p.name)));
                if !p.run.is_empty() {
                    out.push_str(&format!("\t{}", clean(&p.run)));
                }
                out.push('\n');
            }
            Node::Split { side_by_side, ratio, a, b } => {
                out.push_str(&format!("split {} {ratio:.3}\n", if *side_by_side { "side" } else { "stack" }));
                node(a, panes, out);
                node(b, panes, out);
            }
        }
    }
    let view = match s.view {
        View::Grid => "grid",
        View::Zoom => "zoom",
        View::Tabs => "tabs",
    };
    let mut out = format!(
        "dterm-session 1\nview {view}\nfont {}\nfocus {}\ncreated {}\nclosed {}\n",
        s.font, s.focus, s.created, s.closed
    );
    node(&s.tree, &s.panes, &mut out);
    out
}

/// The session in `session_text` form, or `None` when anything is off.
fn parse_session(text: &str) -> Option<Session> {
    fn node<'a>(lines: &mut impl Iterator<Item = &'a str>, panes: &mut Vec<SavedPane>, depth: usize) -> Option<Node> {
        let line = lines.next()?;
        if let Some(rest) = line.strip_prefix("pane ") {
            let mut fields = rest.split('\t').map(|f| f.trim().to_string());
            let mut field = || fields.next().unwrap_or_default();
            panes.push(SavedPane { cwd: field(), name: field(), run: field() });
            return Some(Node::Leaf(panes.len() - 1));
        }
        let mut parts = line.strip_prefix("split ")?.split(' ');
        let side_by_side = match parts.next()? {
            "side" => true,
            "stack" => false,
            _ => return None,
        };
        let ratio: f32 = parts.next()?.parse().ok()?;
        if depth > 64 || !(0.0..=1.0).contains(&ratio) {
            return None;
        }
        let a = Box::new(node(lines, panes, depth + 1)?);
        let b = Box::new(node(lines, panes, depth + 1)?);
        Some(Node::Split { side_by_side, ratio: ratio.clamp(MIN, 1.0 - MIN), a, b })
    }
    let text = text.trim_start_matches('\u{feff}'); // a byte-order mark, as some editors save
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty()).peekable();
    if lines.next()? != "dterm-session 1" {
        return None;
    }
    let view = match lines.next()?.strip_prefix("view ")? {
        "grid" => View::Grid,
        "zoom" => View::Zoom,
        "tabs" => View::Tabs,
        _ => return None,
    };
    let font: f32 = lines.next()?.strip_prefix("font ")?.parse().ok()?;
    let focus: usize = lines.next()?.strip_prefix("focus ")?.parse().ok()?;
    // Optional: files from before the timestamps have neither line.
    let mut stamp = |key: &str| lines.next_if(|l| l.starts_with(key)).and_then(|l| l[key.len()..].parse().ok()).unwrap_or(0);
    let (created, closed) = (stamp("created "), stamp("closed "));
    let mut panes = Vec::new();
    let tree = node(&mut lines, &mut panes, 0)?;
    if lines.next().is_some() {
        return None;
    }
    Some(Session { view, font: font.clamp(8.0, 40.0), focus: focus.min(panes.len() - 1), tree, panes, created, closed })
}

/// File name (without extension) for a session saved as `name`: letters, digits, space, `-`, `_`.
fn session_file_name(name: &str) -> Option<String> {
    let kept: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.into())
}

/// Where sessions live: `%APPDATA%\dterm` (`last.session`, and `sessions\<name>.session`).
fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("dterm"))
}

/// What a landing page choice is: a new session, the last one, a saved one (these can be
/// deleted), or the session that is running behind the page (after Ctrl+Alt+H).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    New,
    Last,
    Saved,
    Live,
}

/// One choice on the landing page. `file` is the session's file, `None` for a new or the running
/// session; `session` is `None` only for a new one. `created` / `closed` are ready to show
/// ("today 15:42"), empty if unknown.
struct Choice {
    label: String,
    detail: String,
    created: String,
    closed: String,
    file: Option<std::path::PathBuf>,
    session: Option<Session>,
    kind: Kind,
}

/// The landing page: its choices, the selected one, whether Del was pressed once on it,
/// whether Enter was pressed once to leave the running session for it, the new name while a
/// saved session is renamed, and a note about the last rename or delete that failed.
struct Landing {
    rows: Vec<Choice>,
    sel: usize,
    armed: bool,
    leaving: bool,
    rename: Option<String>,
    note: Option<&'static str>,
}

impl Landing {
    fn new(rows: Vec<Choice>) -> Self {
        Landing { rows, sel: 0, armed: false, leaving: false, rename: None, note: None }
    }

    /// The choice Esc and the start page key open: the running session, else a new one.
    fn first(&self) -> usize {
        self.rows.iter().position(|r| r.kind == Kind::Live).unwrap_or(0)
    }

    /// Drop the pending questions and notes (the selection moved or something else was done).
    fn calm(&mut self) {
        (self.armed, self.leaving, self.rename, self.note) = (false, false, None, None);
    }

    /// Whether the selected choice is a saved session, the only kind that can be renamed or deleted.
    fn saved(&self) -> bool {
        self.rows.get(self.sel).is_some_and(|r| r.kind == Kind::Saved)
    }

    /// Delete the selected saved session's file and its row. False when it could not be removed.
    fn delete(&mut self) -> bool {
        if !self.saved() {
            return false;
        }
        if let Some(f) = &self.rows[self.sel].file {
            if std::fs::remove_file(f).is_err() && f.exists() {
                (self.armed, self.note) = (false, Some("could not delete the file"));
                return false;
            }
        }
        self.rows.remove(self.sel);
        self.sel = self.sel.min(self.rows.len() - 1);
        self.calm();
        true
    }

    /// Rename the selected saved session's file to `name` (cleaned like a save name) and keep the
    /// saved ones sorted. Fails with a note, leaving everything as it was.
    fn rename_to(&mut self, name: &str) -> Result<(), &'static str> {
        if !self.saved() {
            return Err("only saved sessions can be renamed");
        }
        let new = session_file_name(name).ok_or("a name needs letters or digits")?;
        let row = &mut self.rows[self.sel];
        if new == row.label {
            return Ok(());
        }
        let old = row.file.clone().ok_or("session has no file")?;
        let to = old.with_file_name(format!("{new}.session"));
        // Only a change of letter case may land on the file's own name.
        if to.exists() && new.to_lowercase() != row.label.to_lowercase() {
            return Err("a session with that name exists");
        }
        std::fs::rename(&old, &to).map_err(|_| "could not rename the file")?;
        row.label = new.clone();
        row.file = Some(to);
        let first = self.rows.iter().position(|r| r.kind == Kind::Saved).unwrap_or(0);
        self.rows[first..].sort_by_key(|r| r.label.to_lowercase());
        self.sel = first + self.rows[first..].iter().position(|r| r.label == new).unwrap_or(0);
        Ok(())
    }
}

fn pane_count(n: usize) -> String {
    format!("{n} pane{}", if n == 1 { "" } else { "s" })
}

/// A choice for session `s`, with its times in words (see `ago`).
fn choice(label: String, s: Session, file: Option<std::path::PathBuf>, kind: Kind, stamp: &dyn Fn(u64) -> String) -> Choice {
    let (created, closed) = (stamp(s.created), if kind == Kind::Live { String::new() } else { stamp(s.closed) });
    Choice { label, detail: pane_count(s.panes.len()), created, closed, file, session: Some(s), kind }
}

/// The landing page's choices: new session, the last one, then saved ones by name. Unreadable
/// files are left out. `stamp` turns a Unix time into words (see `ago`).
fn landing_rows(dir: &std::path::Path, stamp: &dyn Fn(u64) -> String) -> Vec<Choice> {
    let read = |f: &std::path::Path| std::fs::read_to_string(f).ok().and_then(|t| parse_session(&t));
    let new = Choice {
        label: "New session".into(),
        detail: String::new(),
        created: String::new(),
        closed: String::new(),
        file: None,
        session: None,
        kind: Kind::New,
    };
    let mut rows = vec![new];
    let last = dir.join("last.session");
    if let Some(s) = read(&last) {
        rows.push(choice("Last session".into(), s, Some(last), Kind::Last, stamp));
    }
    let mut saved: Vec<Choice> = std::fs::read_dir(dir.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.extension().is_some_and(|x| x == "session"))
        .filter_map(|f| {
            let s = read(&f)?;
            let label = f.file_stem()?.to_string_lossy().into_owned();
            Some(choice(label, s, Some(f), Kind::Saved, stamp))
        })
        .collect();
    saved.sort_by_key(|c| c.label.to_lowercase());
    rows.extend(saved);
    rows
}

/// Landing page geometry: the logo and tagline sit above `LAND_ROWS`, where the choices start,
/// one every `line_h` DIPs.
fn landing_row(y: f32, line_h: f32, rows: usize) -> Option<usize> {
    let at = (y - LAND_ROWS) / line_h;
    (at >= 0.0 && (at as usize) < rows).then_some(at as usize)
}

/// Height of one landing page row for cells `cell_h` DIPs tall.
fn land_line(cell_h: f32) -> f32 {
    cell_h + 12.0
}

/// Where the landing page's closed column starts (DIPs right of `LAND_X`), and how wide its list
/// is, with room for the rename and delete icons at the right end. Cells are `cw` DIPs wide.
fn land_metrics(l: &Landing, cw: f32) -> (f32, f32) {
    let chars = |f: fn(&Choice) -> &String| l.rows.iter().map(|r| f(r).chars().count()).max().unwrap_or(0) as f32;
    let typed = l.rename.as_ref().map_or(0, |t| t.chars().count() + 1) as f32;
    let closed = (8.0 + chars(|r| &r.label).max(typed) + chars(|r| &r.detail)) * cw;
    (closed, closed + (chars(|r| &r.closed) + 9.0) * cw + 2.0 * ICON_W)
}

/// The icon under `x` DIPs on a landing row of a list `list_w` wide: `Some(false)` rename,
/// `Some(true)` delete.
fn land_icon(x: f32, list_w: f32) -> Option<bool> {
    let left = LAND_X + list_w - 2.0 * ICON_W;
    ((left..left + 2.0 * ICON_W).contains(&x)).then(|| x >= left + ICON_W)
}

/// The line under the landing page's preview: what a new session is, or when a saved one was
/// started and closed (nothing for sessions saved before dterm recorded times).
fn preview_caption(new: bool, created: &str, closed: &str) -> String {
    match (created, closed) {
        _ if new => "a fresh shell in your home folder".into(),
        ("", "") => String::new(),
        ("", c) => format!("closed {c}"),
        (s, "") => format!("started {s}"),
        (s, c) => format!("started {s}  \u{b7}  closed {c}"),
    }
}

/// A preview name for a saved pane: its typed name, else its folder ("~" for home or unknown).
fn preview_name(p: &SavedPane, home: Option<&str>) -> String {
    if !p.name.is_empty() {
        return p.name.clone();
    }
    let cwd = p.cwd.trim_end_matches(['\\', '/']);
    if cwd.is_empty() || home.is_some_and(|h| h.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(cwd)) {
        return "~".into();
    }
    cwd.rsplit(['\\', '/']).next().unwrap_or(cwd).into()
}

/// Grid point and cell half under `(x, y)` DIPs, for text starting at `origin` in a `size`
/// (columns, lines) terminal scrolled back `offset` lines. Clamped to the grid.
fn locate(
    (x, y): (f32, f32), (ox, oy): (f32, f32), (cw, ch): (f32, f32), (cols, lines): (usize, usize), offset: usize,
) -> (Point, Side) {
    let (fx, fy) = ((x - ox) / cw, (y - oy) / ch);
    let col = (fx.max(0.0) as usize).min(cols - 1);
    let row = (fy.max(0.0) as usize).min(lines - 1);
    let side = if fx >= cols as f32 || (fx >= 0.0 && fx.fract() >= 0.5) { Side::Right } else { Side::Left };
    (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
}

/// Click count (1 to 3) for a press at `(t ms, x, y)`, given the previous `(t, x, y, count)`.
fn click_count((pt, px, py, n): (u32, f32, f32, u32), (t, x, y): (u32, f32, f32), limit: u32) -> u32 {
    if t.wrapping_sub(pt) <= limit && (x - px).abs() <= SLOP && (y - py).abs() <= SLOP {
        n % 3 + 1
    } else {
        1
    }
}

/// Move a selection caret (the boundary left of cell `p`) by `dx` characters and `dy` lines.
/// Wraps across line ends and clamps to lines `top..=bottom`; one line past `bottom` is the very end.
fn caret_move(p: Point, (dx, dy): (i32, i32), cols: usize, top: i32, bottom: i32) -> Point {
    let c = cols as i64;
    let i = (p.line.0 - top) as i64 * c + p.column.0 as i64 + dx as i64 + dy as i64 * c;
    let i = i.clamp(0, (bottom + 1 - top) as i64 * c);
    Point::new(Line(top + (i / c) as i32), Column((i % c) as usize))
}

/// Caret (cell index) after a word step from `at`: right goes past the end of the next word, left
/// to the start of the previous one. Cells are `0..end`; `word(i)` says whether cell `i` is in a word.
fn word_move(at: i64, right: bool, end: i64, word: impl Fn(i64) -> bool) -> i64 {
    let mut i = at;
    if right {
        while i < end && !word(i) {
            i += 1;
        }
        while i < end && word(i) {
            i += 1;
        }
    } else {
        while i > 0 && !word(i - 1) {
            i -= 1;
        }
        while i > 0 && word(i - 1) {
            i -= 1;
        }
    }
    i
}

/// Scrollback lines for a wheel `delta` (120 per notch): 3 per notch, at least 1 per event.
fn wheel_lines(delta: i32) -> i32 {
    match delta * 3 / 120 {
        0 => delta.signum(),
        n => n,
    }
}

/// What a wheel event sends to the app, or `None` when it should scroll the scrollback.
/// `(col, row)` is the 0-based cell under the mouse.
fn wheel_input(mode: TermMode, delta: i32, (col, row): (usize, usize)) -> Option<Vec<u8>> {
    let down = (delta < 0) as u8;
    if mode.intersects(TermMode::MOUSE_MODE) {
        let (c, r) = (col + 1, row + 1);
        let one = if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{};{c};{r}M", 64 + down).into_bytes()
        } else {
            vec![0x1b, b'[', b'M', 32 + 64 + down, (32 + c).min(255) as u8, (32 + r).min(255) as u8]
        };
        Some(one.repeat((delta.unsigned_abs() as usize / 120).max(1)))
    } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let n = wheel_lines(delta).unsigned_abs() as usize;
        key_seq(if down == 0 { 0x26 } else { 0x28 }, false, false, false, mode.contains(TermMode::APP_CURSOR))
            .map(|s| s.repeat(n).into_bytes())
    } else {
        None
    }
}

/// Clipboard form of selected text: UTF-16, CRLF line breaks, NUL-terminated.
fn clip_units(text: &str) -> Vec<u16> {
    wide(&text.replace('\n', "\r\n"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The directory in a Git Bash prompt title (`MSYSTEM:PWD`); `None` for any other title.
fn prompt_cwd(title: &str) -> Option<&str> {
    let (system, dir) = title.split_once(':')?;
    (MSYSTEMS.contains(&system) && (dir.starts_with('/') || dir.starts_with('~'))).then_some(dir)
}

/// Windows form of a Git Bash path: `/c/Users/x` is `C:\Users\x`. `None` outside a drive.
fn win_path(msys: &str) -> Option<String> {
    let rest = msys.strip_prefix('/')?;
    let (drive, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let mut letters = drive.chars();
    let letter = letters.next().filter(char::is_ascii_alphabetic)?;
    letters.next().is_none().then(|| format!("{}:\\{}", letter.to_ascii_uppercase(), tail.replace('/', "\\")))
}

/// Last folder of a directory, or "~" for the home folder. `win` is the Windows form of `msys`, if any.
fn folder_name(msys: &str, win: Option<&str>, home: Option<&str>) -> String {
    let bare = |p: &str| p.trim_end_matches(['/', '\\']).to_string();
    if win.zip(home).is_some_and(|(w, h)| bare(w).eq_ignore_ascii_case(&bare(h))) {
        return "~".into();
    }
    let path = bare(win.unwrap_or(msys));
    path.rsplit(['/', '\\']).next().filter(|f| !f.is_empty()).unwrap_or("/").into()
}

/// What window titles told a pane: filled by its PTY thread, read by the UI thread.
#[derive(Default)]
struct Names {
    app: Option<String>, // title set by the running app
    cwd: Option<String>, // directory Git Bash reported, e.g. `/c/Users/x`
    win: Option<String>, // the same as a Windows path (kept for session restore)
}

impl Names {
    /// A new title (`None` = reset). A Git Bash prompt title only reports the directory and ends
    /// any app title; every other title belongs to the app.
    fn set_title(&mut self, title: Option<&str>) {
        let title = title.map(str::trim).filter(|t| !t.is_empty());
        match title.and_then(prompt_cwd) {
            Some(dir) => {
                self.app = None;
                self.cwd = Some(dir.into());
                self.win = win_path(dir);
            }
            // The console's first title is the shell's own path (…\bash.exe): not a name.
            None if title.is_some_and(|t| t.to_ascii_lowercase().ends_with(".exe")) => self.app = None,
            None => self.app = title.map(Into::into),
        }
    }
}

/// A pane's label: the name typed for it, else the app's title, else its folder.
fn pick_name(custom: Option<&str>, n: &Names, home: Option<&str>) -> Option<String> {
    let folder = || n.cwd.as_deref().map(|c| folder_name(c, n.win.as_deref(), home));
    custom.or(n.app.as_deref()).map(String::from).or_else(folder)
}

/// Window title for the focused pane's name.
fn window_title(name: Option<&str>) -> String {
    name.map_or("dterm".into(), |n| format!("dterm \u{2014} {n}"))
}

/// What a pane that is not focused has been up to. Focusing it clears the state.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Status {
    #[default]
    Idle,
    Busy,      // output is streaming
    Done,      // was busy, then went quiet
    Attention, // it rang the bell
}

/// Written by a pane's PTY thread, read by the window.
#[derive(Default)]
struct Activity {
    output: AtomicU64, // `uptime()` of the last output, plus one; 0 = none yet
    bell: AtomicBool,
}

fn uptime() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// The next status of a pane. `quiet` is the time since its last output, if it ever had any.
fn next_status(now: Status, focused: bool, bell: bool, quiet: Option<u64>) -> Status {
    match (now, focused, bell) {
        (_, true, _) => Status::Idle,
        (_, _, true) | (Status::Attention, _, _) => Status::Attention,
        _ if quiet.is_some_and(|q| q < BUSY_MS) => Status::Busy,
        (Status::Busy | Status::Done, _, _) => Status::Done,
        _ => Status::Idle,
    }
}

/// The color that tints a pane's name for a status: blue busy, green done, yellow waiting.
fn status_tint(s: Status) -> Option<(Rgb, f32)> {
    let t = &theme().ansi;
    match s {
        Status::Idle => None,
        Status::Busy => Some((rgb(t[4]), BUSY_ALPHA)),
        Status::Done => Some((rgb(t[2]), STATUS_ALPHA)),
        Status::Attention => Some((rgb(t[3]), STATUS_ALPHA)),
    }
}

/// Forwards terminal events from a pane's PTY thread to the window.
#[derive(Clone)]
struct Proxy {
    hwnd: isize,
    id: usize,
    tx: Arc<OnceLock<EventLoopSender>>, // set right after the event loop exists
    names: Arc<Mutex<Names>>,
    activity: Arc<Activity>,
}

impl EventListener for Proxy {
    fn send_event(&self, e: Event) {
        let post = |msg, wp| unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as _)), msg, WPARAM(wp), LPARAM(0));
        };
        let title = |t: Option<&str>| {
            if let Ok(mut n) = self.names.lock() {
                n.set_title(t);
            }
            post(WM_WAKE, 0);
        };
        match e {
            Event::Wakeup => {
                self.activity.output.store(uptime() + 1, Ordering::Relaxed);
                post(WM_WAKE, 0);
            }
            Event::Bell => {
                self.activity.bell.store(true, Ordering::Relaxed);
                post(WM_WAKE, 0);
            }
            Event::Title(t) => title(Some(&t)),
            Event::ResetTitle => title(None),
            Event::ChildExit(_) | Event::Exit => post(WM_EXIT, self.id),
            // Replies the terminal owes the app, e.g. cursor position reports.
            Event::PtyWrite(s) => {
                if let Some(tx) = self.tx.get() {
                    let _ = tx.send(Msg::Input(s.into_bytes().into()));
                }
            }
            _ => {}
        }
    }
}

struct Pane {
    id: usize,
    term: Arc<FairMutex<Term<Proxy>>>,
    tx: EventLoopSender,
    size: (usize, usize),        // columns, lines
    rect: Rect,
    origin: (f32, f32),         // top-left of the text, from inner()
    head: Option<Point>,        // free end of a Shift+arrow selection: the boundary left of this cell
    names: Arc<Mutex<Names>>,   // filled from window titles
    custom: Option<String>,     // name the user typed
    pid: Option<u32>,           // the shell's process
    activity: Arc<Activity>,
    status: Status,             // shown while the pane is not focused
}

impl Pane {
    fn send(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.tx.send(Msg::Input(bytes.into()));
    }

    /// Typed or pasted input: drops the selection and snaps the view back to the bottom first.
    fn input(&mut self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.clear_selection();
        self.term.lock().scroll_display(Scroll::Bottom);
        self.send(bytes);
    }

    fn clear_selection(&mut self) {
        self.term.lock().selection = None;
        self.head = None;
    }

    /// The selected text, if any. The selection is cleared either way.
    fn take_selection(&mut self) -> Option<String> {
        let text = self.term.lock().selection_to_string().filter(|t| !t.is_empty());
        self.clear_selection();
        text
    }

    /// Grid point and cell half under `m` (client DIPs); `term` is this pane's locked terminal.
    fn point_at(&self, term: &Term<Proxy>, m: (f32, f32), cell: (f32, f32)) -> (Point, Side) {
        locate(m, self.origin, cell, self.size, term.grid().display_offset())
    }

    /// Re-reads the activity; true if the status changed. A bell on the focused pane is dropped.
    fn refresh_status(&mut self, focused: bool, now: u64) -> bool {
        let bell = !focused && self.activity.bell.load(Ordering::Relaxed);
        if focused {
            self.activity.bell.store(false, Ordering::Relaxed);
        }
        let quiet = self.activity.output.load(Ordering::Relaxed).checked_sub(1).map(|t| now.saturating_sub(t));
        let next = next_status(self.status, focused, bell, quiet);
        std::mem::replace(&mut self.status, next) != next
    }

    /// The pane's label, if it has a name yet.
    fn name(&self, home: Option<&str>) -> Option<String> {
        let names = self.names.lock().ok()?;
        pick_name(self.custom.as_deref(), &names, home)
    }

    /// Shift+arrows: move the selection's free end by a character or a line, starting at the cursor.
    /// With `words`, Left / Right move to the start of the previous / end of the next word.
    // ponytail: the end is not rotated if output scrolls the terminal meanwhile; fine while typing at a prompt.
    fn extend_selection(&mut self, d: (i32, i32), words: bool) {
        let mut t = self.term.lock();
        t.scroll_display(Scroll::Bottom); // the cursor is down there
        let head = match self.head.filter(|_| t.selection.is_some()) {
            Some(h) => h,
            None => {
                let c = t.grid().cursor.point;
                t.selection = Some(Selection::new(SelectionType::Simple, c, Side::Left));
                c
            }
        };
        let (cols, top, bottom) = (t.columns(), t.topmost_line().0, t.bottommost_line().0);
        let head = if words && d.0 != 0 {
            // Cells counted from the top-left of the scrollback; a word is anything but blanks
            // and the characters a double-click stops at.
            let c = cols as i64;
            let point = |i: i64| Point::new(Line(top + (i / c) as i32), Column((i % c) as usize));
            let stops = t.semantic_escape_chars();
            let is_word = |i: i64| !matches!(t.grid()[point(i)].c, ' ' | '\0') && !stops.contains(t.grid()[point(i)].c);
            let at = (head.line.0 - top) as i64 * c + head.column.0 as i64;
            point(word_move(at, d.0 > 0, (bottom + 1 - top) as i64 * c, is_word))
        } else {
            caret_move(head, d, cols, top, bottom)
        };
        self.head = Some(head);
        let (end, side) = if head.line.0 > bottom {
            (Point::new(Line(bottom), Column(cols - 1)), Side::Right) // the end of the last cell
        } else {
            (head, Side::Left)
        };
        if let Some(s) = t.selection.as_mut() {
            s.update(end, side);
        }
    }
}

struct App {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    format: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    label: IDWriteTextFormat, // pane names
    tab: IDWriteTextFormat,   // tab names and the window title
    cap_hot: Option<usize>,   // caption button under the mouse
    cap_down: Option<usize>,  // caption button being pressed
    font_size: f32,           // DIPs
    cell: (f32, f32), // DIPs
    target: Option<(ID2D1HwndRenderTarget, ID2D1SolidColorBrush)>,
    bash: String,
    panes: Vec<Pane>, // storage; the tree refers to them by id
    focus: usize,     // index into panes
    next_id: usize,
    tree: Option<Node>,          // None only before the first pane exists
    drag: Option<Vec<bool>>,     // path to the split whose divider is being dragged
    surrogate: u16,              // first half of a UTF-16 pair typed via WM_CHAR
    skip_char: bool,             // WM_KEYDOWN already handled this key
    select: Option<usize>,       // id of the pane the mouse is selecting in
    click: (u32, f32, f32, u32), // last press: time (ms), x, y (DIPs), click count
    home: Option<String>,        // home folder as a Windows path, shown as "~"
    rename: Option<(usize, String)>, // pane id and the name being typed
    title: String,               // window title as last set
    view: View,
    mouse: Option<(f32, f32)>,   // last mouse position in DIPs; None once it left the window
    tracking: bool,              // WM_MOUSELEAVE is requested
    landing: Option<Landing>,    // the start page, shown before any pane exists
    prompt: Option<String>,      // session name being typed for the save-session shortcut
    toast: Option<String>,       // short note after saving
    created: u64,                // when this session started (Unix seconds), kept across restores
    settings: Settings,
    page: Option<SettingsPage>,  // the settings page, shown over the panes or the landing page
}

/// The settings page: the selected row (theme, opacity, then one per action) and whether it waits
/// for the new keys of the selected action.
struct SettingsPage {
    sel: usize,
    capture: bool,
}

impl App {
    /// Whether panes reserve a label slot: with several panes, or while a lone one is renamed.
    /// A zoomed pane counts as a lone one; in Tabs the names are in the strip.
    fn labeled(&self) -> bool {
        match self.view {
            View::Grid => self.panes.len() > 1 || self.rename.is_some(),
            View::Zoom => self.rename.is_some(),
            View::Tabs => false,
        }
    }

    /// Whether pane `i` is on screen. Only these take part in mouse hit-testing.
    fn is_shown(&self, i: usize) -> bool {
        self.view == View::Grid || i == self.focus
    }

    /// Updates every pane's status; true if any changed. Focusing a pane clears its status.
    fn refresh_status(&mut self) -> bool {
        let (now, focus) = (uptime(), self.focus);
        self.panes.iter_mut().enumerate().fold(false, |c, (i, p)| p.refresh_status(i == focus, now) | c)
    }

    /// Pane ids in tree order, which is also the tab order.
    fn order(&self) -> Vec<usize> {
        self.tree.as_ref().map(Node::ids).unwrap_or_default()
    }

    fn focus_id(&mut self, id: usize) {
        if let Some(i) = self.panes.iter().position(|p| p.id == id) {
            self.focus = i;
        }
    }

    /// What a session file stores about the current window; `None` without panes.
    fn session(&self) -> Option<Session> {
        let tree = self.tree.as_ref()?;
        let order = tree.ids();
        let procs = unsafe { processes() };
        let mut taken = Vec::new(); // Claude Code conversations already given to a pane
        let panes = order
            .iter()
            .map(|id| {
                let p = self.panes.iter().find(|p| p.id == *id);
                let cwd = p.and_then(|p| p.names.lock().ok()?.win.clone()).unwrap_or_default();
                let run = p.and_then(|p| claude_run(&procs, p.pid?, &cwd, &mut taken)).unwrap_or_default();
                SavedPane { cwd, name: p.and_then(|p| p.custom.clone()).unwrap_or_default(), run }
            })
            .collect();
        let focused = self.panes.get(self.focus).map(|p| p.id);
        let focus = order.iter().position(|&i| Some(i) == focused).unwrap_or(0);
        let tree = tree.numbered(&mut 0);
        Some(Session { view: self.view, font: self.font_size, focus, tree, panes, created: self.created, closed: now() })
    }

    /// Move the focus `step` panes along the tree order, wrapping.
    fn focus_step(&mut self, step: isize) {
        let order = self.order();
        let at = self.panes.get(self.focus).and_then(|p| order.iter().position(|&i| i == p.id));
        if let Some(at) = at {
            self.focus_id(order[cycle(order.len(), at, step)]);
        }
    }
}

thread_local!(static APP: RefCell<Option<App>> = const { RefCell::new(None) });

// Never call anything inside `f` that sends a message back to wndproc: it would borrow twice.
fn with<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(a.borrow_mut().as_mut().expect("app")))
}

unsafe fn git_bash() -> String {
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut buf = [0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        let err = RegGetValueW(
            root, w!("SOFTWARE\\GitForWindows"), w!("InstallPath"), RRF_RT_REG_SZ,
            None, Some(buf.as_mut_ptr().cast()), Some(&mut size),
        );
        if err.is_ok() {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
            return format!("{}\\bin\\bash.exe", String::from_utf16_lossy(&buf[..n]));
        }
    }
    "C:\\Program Files\\Git\\bin\\bash.exe".into()
}

/// (pid, parent pid, exe name) of every running process.
unsafe fn processes() -> Vec<(u32, u32, String)> {
    let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return Vec::new() };
    let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut out = Vec::new();
    let mut more = Process32FirstW(snap, &mut e).is_ok();
    while more {
        let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
        more = Process32NextW(snap, &mut e).is_ok();
    }
    let _ = CloseHandle(snap);
    out
}

/// A process named `name` (any case) among the descendants of process `root`.
fn descendant(procs: &[(u32, u32, String)], root: u32, name: &str) -> Option<u32> {
    let mut seen = vec![root]; // also guards against loops from reused pids
    let mut i = 0;
    while let Some(&parent) = seen.get(i) {
        for (pid, ppid, exe) in procs {
            if *ppid == parent && !seen.contains(pid) {
                if exe.eq_ignore_ascii_case(name) {
                    return Some(*pid);
                }
                seen.push(*pid);
            }
        }
        i += 1;
    }
    None
}

/// When process `pid` started, in Unix seconds.
unsafe fn started(pid: u32) -> Option<u64> {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut t = [FILETIME::default(); 4]; // creation, exit, kernel, user
    let ok = GetProcessTimes(h, &mut t[0], &mut t[1], &mut t[2], &mut t[3]).is_ok();
    let _ = CloseHandle(h);
    let ticks = (t[0].dwHighDateTime as u64) << 32 | t[0].dwLowDateTime as u64;
    ok.then(|| (ticks / 10_000_000).saturating_sub(11_644_473_600))
}

/// Claude Code's folder for the conversations started in `cwd`: `.claude\projects\<cwd with every
/// character but ASCII letters and digits turned into ->`.
fn claude_project(cwd: &str) -> Option<std::path::PathBuf> {
    let home = || std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join(".claude"));
    let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).or_else(home)?;
    let name: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    Some(base.join("projects").join(name))
}

/// Ids of the conversations in Claude Code folder `dir` written to since `since` (Unix seconds), newest first.
fn recent_conversations(dir: &std::path::Path, since: u64) -> Vec<String> {
    let mut found: Vec<(u64, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            let modified = e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            (modified >= since).then_some((modified, path.file_stem()?.to_string_lossy().into_owned()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|f| f.1).collect()
}

/// The command that brings back Claude Code if it runs under shell process `shell` in `cwd`: its
/// newest conversation that no other pane has `taken`, or a fresh `claude` when it has none yet.
// ponytail: two Claude panes in one folder may swap conversations; matching by open file handles would pin them.
fn claude_run(procs: &[(u32, u32, String)], shell: u32, cwd: &str, taken: &mut Vec<String>) -> Option<String> {
    let pid = descendant(procs, shell, "claude.exe")?;
    let since = unsafe { started(pid) }.unwrap_or(0);
    let ids = claude_project(cwd).map(|d| recent_conversations(&d, since)).unwrap_or_default();
    Some(match ids.into_iter().find(|id| !taken.contains(id)) {
        Some(id) => {
            let run = format!("claude --resume {id}");
            taken.push(id);
            run
        }
        None => "claude".into(),
    })
}

fn window_size((cols, lines): (usize, usize), cell: (f32, f32), scale: f32) -> WindowSize {
    WindowSize {
        num_lines: lines as u16,
        num_cols: cols as u16,
        cell_width: (cell.0 * scale) as u16,
        cell_height: (cell.1 * scale) as u16,
    }
}

/// A new pane running Git Bash in folder `dir` (the home folder when `None` or gone).
fn spawn(hwnd: HWND, id: usize, bash: &str, cell: (f32, f32), scale: f32, dir: Option<&str>) -> std::io::Result<Pane> {
    let size = (80, 24); // layout() fits it to the pane right away
    let dir = dir.map(std::path::PathBuf::from).filter(|d| d.is_dir());
    let opts = tty::Options {
        shell: Some(tty::Shell::new(bash.into(), vec!["--login".into(), "-i".into()])),
        working_directory: dir.or_else(|| std::env::var_os("USERPROFILE").map(Into::into)),
        drain_on_exit: false,
        env: Default::default(),
        escape_args: true,
    };
    let pty = tty::new(&opts, window_size(size, cell, scale), 0)?;
    let pid = pty.child_watcher().pid().map(|p| p.get());
    let slot = Arc::new(OnceLock::new());
    let names = Arc::new(Mutex::new(Names::default()));
    let activity = Arc::new(Activity::default());
    let proxy = Proxy { hwnd: hwnd.0 as isize, id, tx: slot.clone(), names: names.clone(), activity: activity.clone() };
    let term = Arc::new(FairMutex::new(Term::new(Config::default(), &TermSize::new(size.0, size.1), proxy.clone())));
    let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)?;
    let tx = event_loop.channel();
    let _ = slot.set(tx.clone());
    event_loop.spawn();
    Ok(Pane { id, term, tx, size, rect: (0.0, 0.0, 0.0, 0.0), origin: (PAD, PAD), head: None, names, custom: None, pid, activity, status: Status::Idle })
}

unsafe fn scale(hwnd: HWND) -> f32 {
    GetDpiForWindow(hwnd) as f32 / 96.0
}

unsafe fn client_px(hwnd: HWND) -> (u32, u32) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    (rc.right as u32, rc.bottom as u32)
}

/// Client size in DIPs, below the caption.
unsafe fn client(hwnd: HWND) -> (f32, f32) {
    let (w, h) = client_px(hwnd);
    let s = scale(hwnd);
    (w as f32 / s, h as f32 / s - CAPTION_H)
}

/// Screen point in client DIPs.
unsafe fn to_client(hwnd: HWND, mut pt: POINT) -> (f32, f32) {
    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
    let s = scale(hwnd);
    (pt.x as f32 / s, pt.y as f32 / s - CAPTION_H)
}

/// Cursor position in client DIPs.
unsafe fn mouse(hwnd: HWND) -> (f32, f32) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    to_client(hwnd, pt)
}

fn color(c: Rgb, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

/// Place every pane, resize terminals whose grid changed, and repaint.
unsafe fn layout(hwnd: HWND) {
    let (w, h) = client(hwnd);
    let (pw, ph) = client_px(hwnd);
    let s = scale(hwnd);
    with(|a| {
        if let Some((rt, _)) = &a.target {
            let _ = rt.Resize(&D2D_SIZE_U { width: pw, height: ph });
        }
        let (cell, labeled) = (a.cell, a.labeled());
        let shown = a.panes.get(a.focus).map_or(0, |p| p.id);
        let placed = a.tree.as_ref().map(|t| place(t, a.view, shown, (w, h))).unwrap_or_default();
        for (id, r) in placed {
            let Some(p) = a.panes.iter_mut().find(|p| p.id == id) else { continue };
            let (origin, room) = inner(r, labeled);
            p.rect = r;
            p.origin = origin;
            let size = fit(room, cell);
            if size != p.size {
                p.size = size;
                p.term.lock().resize(TermSize::new(size.0, size.1));
                let _ = p.tx.send(Msg::Resize(window_size(size, cell, s)));
            }
        }
    });
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Open a shell in a new pane that gets the focus. `Some(side_by_side)` splits the focused pane
/// that way; `None` splits the largest one along its longer side (and starts the first pane).
unsafe fn add_pane(hwnd: HWND, dir: Option<bool>) {
    let s = scale(hwnd);
    let added = with(|a| {
        let id = a.next_id;
        a.next_id += 1;
        let Ok(p) = spawn(hwnd, id, &a.bash, a.cell, s, None) else { return false };
        let (at, side_by_side) = match dir {
            Some(d) => (a.focus, d),
            None => auto_split(&a.panes.iter().map(|p| p.rect).collect::<Vec<_>>(), a.focus),
        };
        let at = a.panes.get(at).map(|p| p.id);
        match (a.tree.as_mut(), at) {
            (Some(t), Some(at)) => {
                t.split(at, id, side_by_side);
            }
            _ => a.tree = Some(Node::Leaf(id)),
        }
        a.panes.push(p);
        a.focus = a.panes.len() - 1;
        true
    });
    if added {
        layout(hwnd);
    }
}

/// Close pane `id`: its sibling takes the room, and the focus moves there if it was on `id`.
/// Closing the last pane closes the window.
unsafe fn remove_pane(hwnd: HWND, id: usize) {
    let left = with(|a| {
        let i = a.panes.iter().position(|p| p.id == id)?;
        let focused = a.panes.get(a.focus).map(|p| p.id);
        let next = a.tree.as_ref().and_then(|t| t.sibling(id));
        a.tree = a.tree.take().and_then(|t| t.remove(id));
        let _ = a.panes.remove(i).tx.send(Msg::Shutdown); // fails if the shell already exited
        a.drag = None;
        if a.rename.as_ref().is_some_and(|r| r.0 == id) {
            a.rename = None;
        }
        let keep = if focused == Some(id) { next } else { focused };
        a.focus = keep
            .and_then(|k| a.panes.iter().position(|p| p.id == k))
            .unwrap_or(i.saturating_sub(1))
            .min(a.panes.len().saturating_sub(1));
        Some(a.panes.len())
    });
    match left {
        // Behind the landing page the running session just ends: its choice goes away.
        Some(0) if with(|a| a.landing.as_mut().map(|l| l.rows.retain(|r| r.kind != Kind::Live)).is_some()) => {
            with(|a| a.landing.as_mut().map(|l| (l.sel, l.leaving) = (0, false)));
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        Some(0) => {
            let _ = DestroyWindow(hwnd);
        }
        Some(_) => layout(hwnd),
        None => {}
    }
}

/// The title bar dterm draws over the top `CAPTION_H` of its client area: the start page and grid /
/// tabs buttons, the title in the middle, and minimize, maximize (or restore) and close drawn like the system ones.
unsafe fn draw_caption(a: &App, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush, width: f32, zoomed: bool) {
    rt.SetTransform(&windows_numerics::Matrix3x2::identity());
    rt.PushAxisAlignedClip(&rect(0.0, 0.0, width, CAPTION_H), D2D1_ANTIALIAS_MODE_ALIASED);
    rt.Clear(Some(&color(bg(), a.settings.opacity)));
    rt.PopAxisAlignedClip();
    brush.SetColor(&color(fg(), 0.8));
    let units: Vec<u16> = a.title.encode_utf16().collect();
    // Centered in the window, clear of the buttons on both sides.
    let slot = rect(3.0 * CAPTION_BTN, 0.0, (width - 6.0 * CAPTION_BTN).max(0.0), CAPTION_H);
    rt.DrawText(&units, &a.tab, &slot, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    for i in [5, 0, 1, 2, 3, 4].into_iter().filter(|&i| i > 1 || session_shown(a)) {
        let x = caption_x(i, width);
        if a.cap_hot == Some(i) {
            // Close turns red like Windows' own; the others get a light wash, a bit stronger while pressed.
            let wash = if a.cap_down == Some(i) { 0.2 } else { 0.12 };
            brush.SetColor(&if i == 4 { color(rgb(0xE81123), 1.0) } else { color(fg(), wash) });
            rt.FillRectangle(&rect(x, 0.0, CAPTION_BTN, CAPTION_H), brush);
        }
        brush.SetColor(&color(fg(), if a.cap_hot == Some(i) { 1.0 } else { 0.8 }));
        // 10 DIP glyphs on half pixels, so 1 DIP lines stay sharp at 100%.
        let (cx, cy) = ((x + CAPTION_BTN / 2.0).floor() + 0.5, (CAPTION_H / 2.0).floor() + 0.5);
        let line = |x0: f32, y0: f32, x1: f32, y1: f32| rt.DrawLine(windows_numerics::Vector2 { X: cx + x0, Y: cy + y0 }, windows_numerics::Vector2 { X: cx + x1, Y: cy + y1 }, brush, 1.0, None);
        let (bx, by) = ((cx - 0.5).round(), (cy - 0.5).round()); // whole-pixel center for filled glyphs
        match i {
            // To the start page: a house.
            0 => {
                rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                // One closed outline: walls, and a peaked roof on top.
                let pts = [(-5.0, 5.5), (-5.0, -1.0), (0.0, -6.0), (5.0, -1.0), (5.0, 5.5), (-5.0, 5.5)];
                pts.windows(2).for_each(|p| line(p[0].0, p[0].1, p[1].0, p[1].1));
                rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            }
            // Settings: a gear, a thick ring with eight chunky teeth around it.
            5 => {
                rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                let center = windows_numerics::Vector2 { X: cx, Y: cy };
                rt.DrawEllipse(&D2D1_ELLIPSE { point: center, radiusX: 3.5, radiusY: 3.5 }, brush, 2.5, None);
                for k in 0..8 {
                    let (s, c) = (k as f32 * std::f32::consts::FRAC_PI_4).sin_cos();
                    let tooth = |r: f32| windows_numerics::Vector2 { X: cx + r * c, Y: cy + r * s };
                    rt.DrawLine(tooth(4.0), tooth(6.5), brush, 2.5, None);
                }
                rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            }
            // Back to the grid: 2 x 2 tiny squares.
            1 if a.view == View::Tabs => {
                for (dx, dy) in [(0.0, 0.0), (6.0, 0.0), (0.0, 6.0), (6.0, 6.0)] {
                    rt.FillRectangle(&rect(bx - 5.0 + dx, by - 5.0 + dy, 4.0, 4.0), brush);
                }
            }
            // To the tabs: a window outline with a tab on top.
            1 => {
                let (x, y) = (bx - 7.0, (by - 5.5).round());
                rt.FillRectangle(&rect(x, y, 6.0, 3.0), brush);
                rt.DrawRectangle(&rect(x + 0.5, y + 3.5, 13.0, 7.0), brush, 1.0, None);
            }
            2 => line(-5.0, 0.0, 5.0, 0.0),
            3 if zoomed => {
                rt.DrawRectangle(&rect(cx - 5.0, cy - 3.0, 8.0, 8.0), brush, 1.0, None);
                line(-3.0, -5.0, 5.0, -5.0);
                line(5.0, -5.0, 5.0, 3.0);
            }
            3 => rt.DrawRectangle(&rect(cx - 5.0, cy - 5.0, 10.0, 10.0), brush, 1.0, None),
            _ => {
                rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                line(-5.0, -5.0, 5.0, 5.0);
                line(-5.0, 5.0, 5.0, -5.0);
                rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            }
        }
    }
    // The tooltip: action name, then its shortcut dimmer, in a small box under the hovered button.
    if let (Some(i), None) = (a.cap_hot, a.cap_down) {
        let (name, keys) = caption_tip(i, a.view, zoomed, &a.settings.keys);
        let width_of = |t: &str| {
            let units: Vec<u16> = t.encode_utf16().collect();
            let mut m = DWRITE_TEXT_METRICS::default();
            a.dw.CreateTextLayout(&units, &a.tab, 10_000.0, CAPTION_H).and_then(|l| l.GetMetrics(&mut m)).map_or(0.0, |_| m.widthIncludingTrailingWhitespace.ceil())
        };
        let gap = 10.0;
        let (nw, kw) = (width_of(name), keys.as_deref().map_or(0.0, |k| width_of(k)));
        let bw = nw + if keys.is_some() { gap + kw } else { 0.0 } + 16.0;
        let bx = (caption_x(i, width) + CAPTION_BTN / 2.0 - bw / 2.0).clamp(4.0, (width - bw - 4.0).max(4.0));
        let r = rect(bx, CAPTION_H + 4.0, bw, 24.0);
        brush.SetColor(&color(bg(), 0.97));
        rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 5.0, radiusY: 5.0 }, brush);
        brush.SetColor(&color(fg(), 0.25));
        rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(bx + 0.5, r.top + 0.5, bw - 1.0, 23.0), radiusX: 5.0, radiusY: 5.0 }, brush, 1.0, None);
        let draw = |t: &str, x: f32, w: f32, alpha: f32| {
            brush.SetColor(&color(fg(), alpha));
            let units: Vec<u16> = t.encode_utf16().collect();
            rt.DrawText(&units, &a.tab, &rect(x, r.top, w, 24.0), brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        };
        draw(name, bx + 8.0, nw, 0.95);
        if let Some(k) = &keys {
            draw(k, bx + 8.0 + nw + gap, kw, 0.5);
        }
    }
}

/// The mouse over dterm's own caption buttons: hover, press, and the click when released over the
/// pressed one. Returns whether the message was theirs.
unsafe fn caption_mouse(hwnd: HWND, msg: u32) -> bool {
    let (m, (w, _)) = (mouse(hwnd), client(hwnd));
    let over = if m.1 < 0.0 { caption_button(m.0, w, with(|a| session_shown(a))) } else { None };
    let (changed, down) = with(|a| (std::mem::replace(&mut a.cap_hot, over) != over, a.cap_down));
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    match msg {
        WM_LBUTTONDOWN if over.is_some() => {
            with(|a| a.cap_down = over);
            SetCapture(hwnd);
        }
        WM_LBUTTONUP if down.is_some() => {
            with(|a| a.cap_down = None);
            let _ = ReleaseCapture();
            let cmd = match over {
                _ if over != down => return true,
                Some(0) => return { go_home(hwnd); true },
                Some(5) => return { run(hwnd, Action::Settings); true },
                Some(1) => return { toggle_view(hwnd, View::Tabs); true },
                Some(2) => SC_MINIMIZE,
                Some(3) if IsZoomed(hwnd).as_bool() => SC_RESTORE,
                Some(3) => SC_MAXIMIZE,
                _ => SC_CLOSE,
            };
            let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
        }
        WM_MOUSEMOVE if over.is_some() && !with(|a| std::mem::replace(&mut a.tracking, true)) => {
            // WM_MOUSELEAVE clears the hover when the mouse moves on to the caption or away.
            let size = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
            let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT { cbSize: size, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 });
        }
        _ => {}
    }
    over.is_some() || down.is_some()
}

/// Set on Windows without system backdrops, where the accent policy below gives the acrylic.
static ACCENT: AtomicBool = AtomicBool::new(false);
const ACCENT_ACRYLIC: u32 = 4; // ACCENT_ENABLE_ACRYLICBLURBEHIND
const ACCENT_BLUR: u32 = 3; // ACCENT_ENABLE_BLURBEHIND

/// Sets the undocumented accent policy (WCA_ACCENT_POLICY = 19). Acrylic halves the frame rate
/// while the window is dragged (30 fps on Windows 10 22H2), so moves and resizes use plain blur.
unsafe fn set_accent(hwnd: HWND, state: u32) -> bool {
    #[repr(C)]
    struct Accent { state: u32, flags: u32, color: u32, anim: u32 }
    #[repr(C)]
    struct Data { attrib: u32, data: *mut Accent, size: usize }
    let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return false };
    let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return false };
    let f: unsafe extern "system" fn(HWND, *mut Data) -> BOOL = std::mem::transmute(f);
    // Flags 0x1E0 draw a thin border on all four sides, as there is no system frame (see WM_NCCALCSIZE).
    // The color (AABBGGRR) is almost clear because we paint the tint; alpha 0 turns the blur off on some builds.
    let mut accent = Accent { state, flags: 0x1E0, color: 0x0100_0000, anim: 0 };
    f(hwnd, &mut Data { attrib: 19, data: &mut accent, size: std::mem::size_of::<Accent>() }).as_bool()
}

unsafe fn paint(hwnd: HWND) {
    let (w, h) = client_px(hwnd);
    let s = scale(hwnd);
    with(|a| {
        if a.target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * s,
                dpiY: 96.0 * s,
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: w, height: h },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let Ok(rt) = a.d2d.CreateHwndRenderTarget(&props, &hprops) else { return };
            // ClearType needs an opaque background; ours is see-through.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            // Aliased shapes tile exactly, so cell backgrounds leave no seams between rows.
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            let Ok(brush) = rt.CreateSolidColorBrush(&color(fg(), 1.0), None) else { return };
            a.target = Some((rt, brush));
        }
        let (rt, brush) = a.target.as_ref().unwrap();
        rt.BeginDraw();
        rt.SetTransform(&windows_numerics::Matrix3x2::translation(0.0, CAPTION_H));
        let zoomed = IsZoomed(hwnd).as_bool();
        if let Some(page) = &a.page {
            rt.Clear(Some(&color(bg(), a.settings.opacity)));
            draw_settings(a, page, rt, brush);
            draw_caption(a, rt, brush, w as f32 / s, zoomed);
            if rt.EndDraw(None, None).is_err() {
                a.target = None;
            }
            return;
        }
        if let Some(l) = &a.landing {
            rt.Clear(Some(&color(bg(), a.settings.opacity)));
            draw_landing(a, l, rt, brush, (w as f32 / s, h as f32 / s - CAPTION_H));
            draw_caption(a, rt, brush, w as f32 / s, zoomed);
            if rt.EndDraw(None, None).is_err() {
                a.target = None;
            }
            return;
        }
        rt.Clear(Some(&color(rgb(theme().gap), 0.9)));
        let (several, labeled) = (a.view == View::Grid && a.panes.len() > 1, a.labeled());
        for (i, p) in a.panes.iter().enumerate().filter(|&(i, _)| a.is_shown(i)) {
            let (x, y, pw, ph) = p.rect;
            let (r, focused) = (rect(x, y, pw, ph), i == a.focus);
            // Clear() inside the clip replaces the pixels, so the pane gets its own see-through tint.
            rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            rt.Clear(Some(&color(bg(), a.settings.opacity)));
            draw_term(rt, brush, &a.format, &a.bold, a.cell, p.origin, focused, &p.term.lock());
            // Several panes: a faint border marks the focused one, a veil dims the others.
            if several && focused {
                brush.SetColor(&color(fg(), FOCUS_ALPHA));
                rt.DrawRectangle(&rect(x + 0.5, y + 0.5, pw - 1.0, ph - 1.0), brush, 1.0, None);
            } else if several {
                brush.SetColor(&color(bg(), VEIL_ALPHA));
                rt.FillRectangle(&r, brush);
            }
            if labeled {
                let (text, alpha) = match a.rename.as_ref().filter(|r| r.0 == p.id) {
                    Some((_, typed)) => (format!("{typed}\u{258f}"), 1.0), // the name being typed, with a caret
                    None => (p.name(a.home.as_deref()).unwrap_or_default(), if focused { LABEL_FOCUS_ALPHA } else { LABEL_ALPHA }),
                };
                let (tint, alpha) = status_tint(p.status).filter(|_| a.rename.is_none()).unwrap_or((fg(), alpha));
                brush.SetColor(&color(tint, alpha));
                let units: Vec<u16> = text.encode_utf16().collect();
                let slot = rect(x + PAD, y, pw - 2.0 * PAD, LABEL_TOP);
                rt.DrawText(&units, &a.label, &slot, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
            rt.PopAxisAlignedClip();
        }
        draw_chrome(a, rt, brush, w as f32 / s);
        // The save-session prompt, or the note after saving: one small line near the bottom.
        let note = a.prompt.as_ref().map(|t| (format!("save session as: {t}\u{258f}"), 0.9)).or(a.toast.clone().map(|t| (t, 0.6)));
        if let Some((text, alpha)) = note {
            let (cw, ch) = (w as f32 / s, h as f32 / s - CAPTION_H);
            let bw = 380f32.min(cw - 16.0);
            let r = rect((cw - bw) / 2.0, ch - 56.0, bw, 30.0);
            brush.SetColor(&color(bg(), 0.95));
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 6.0, radiusY: 6.0 }, brush);
            brush.SetColor(&color(fg(), alpha));
            let units: Vec<u16> = text.encode_utf16().collect();
            rt.DrawText(&units, &a.tab, &r, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
        draw_caption(a, rt, brush, w as f32 / s, zoomed);
        if rt.EndDraw(None, None).is_err() {
            a.target = None; // device lost: recreate on the next paint
        }
    });
}

/// The landing page, terminal style: the logo, the choices (the selected one bright with a green
/// ❯ and, if saved, rename and delete icons), its layout in a card, and a footer bar with the keys.
/// `(width, height)` is the area below the title bar, in DIPs.
unsafe fn draw_landing(a: &App, l: &Landing, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush, (width, height): (f32, f32)) {
    let (cw, ch) = a.cell;
    let lh = land_line(ch);
    let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: u32, alpha: f32| {
        brush.SetColor(&color(rgb(c), alpha));
        let units: Vec<u16> = s.encode_utf16().collect();
        rt.DrawText(&units, f, &r, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    };
    // Line `y` of terminal text, vertically centered in its row.
    let line = |s: &str, x: f32, y: f32, c: u32, alpha: f32| text(s, &a.format, rect(x, y + (lh - ch) / 2.0, 4000.0, ch), c, alpha);

    // The logo, as on the icon, and a tagline.
    if let Ok(big) = text_format(&a.dw, DWRITE_FONT_WEIGHT_BOLD, LOGO_SIZE) {
        let r = rect(LAND_X, LAND_Y, 4000.0, LOGO_SIZE * 1.4);
        text("D_", &big, r, theme().ansi[2], 1.0);
        text("   dterm", &big, r, theme().fg, 0.95); // monospace: three cells to the right of "D_"
    }
    let tagline = format!("terminals in a grid, sessions that come back  \u{b7}  v{}", env!("CARGO_PKG_VERSION"));
    let live = l.rows.iter().any(|r| r.kind == Kind::Live);
    line(&tagline, LAND_X + 2.0, LAND_Y + LOGO_SIZE * 1.45, theme().fg, 0.4);

    // The choices: label, pane count, when it was last closed.
    let lw = l.rows.iter().map(|r| r.label.chars().count()).max().unwrap_or(0) as f32;
    let lw = lw.max(l.rename.as_ref().map_or(0, |t| t.chars().count() + 1) as f32);
    let (closed, list_w) = land_metrics(l, cw);
    let closed_x = LAND_X + closed;
    // A hairline between the actions (new, running, last) and the saved sessions.
    if let Some(first) = l.rows.iter().position(|r| r.kind == Kind::Saved) {
        brush.SetColor(&color(fg(), 0.1));
        rt.FillRectangle(&rect(LAND_X, LAND_ROWS + first as f32 * lh - 1.0, list_w - 12.0, 1.0), brush);
    }
    let mouse = a.mouse;
    for (i, row) in l.rows.iter().enumerate() {
        let y = LAND_ROWS + i as f32 * lh;
        let on = i == l.sel;
        let (glyph, gcolor) = match row.kind {
            Kind::New => ("+", theme().ansi[2]),
            Kind::Live => ("\u{25cf}", theme().ansi[2]),
            Kind::Last => ("\u{21ba}", theme().ansi[4]),
            Kind::Saved => ("\u{25aa}", theme().ansi[3]),
        };
        line(glyph, LAND_X + 2.0 * cw, y, gcolor, if on { 1.0 } else { 0.6 });
        if on {
            brush.SetColor(&color(fg(), 0.07));
            let bar = D2D1_ROUNDED_RECT { rect: rect(LAND_X - 12.0, y, list_w + 12.0, lh), radiusX: 6.0, radiusY: 6.0 };
            rt.FillRoundedRectangle(&bar, brush);
            line("\u{276f}", LAND_X, y, theme().ansi[2], 1.0);
        }
        let renaming = on.then_some(l.rename.as_ref()).flatten();
        if let Some(typed) = renaming {
            line(&format!("{typed}\u{258f}"), LAND_X + 4.0 * cw, y, theme().ansi[2], 1.0);
            brush.SetColor(&color(rgb(theme().ansi[2]), 0.5));
            rt.FillRectangle(&rect(LAND_X + 4.0 * cw, y + lh - 5.0, (typed.chars().count() as f32 + 1.0) * cw, 1.0), brush);
        } else if on {
            text(&row.label, &a.bold, rect(LAND_X + 4.0 * cw, y + (lh - ch) / 2.0, 4000.0, ch), theme().fg, 1.0);
        } else {
            line(&row.label, LAND_X + 4.0 * cw, y, theme().fg, 0.6);
        }
        line(&row.detail, LAND_X + (6.0 + lw) * cw, y, theme().fg, if on { 0.6 } else { 0.4 });
        if row.kind == Kind::Live {
            line("running", closed_x, y, theme().ansi[2], if on { 0.9 } else { 0.6 });
        } else if !row.closed.is_empty() {
            line(&format!("closed {}", row.closed), closed_x, y, theme().fg, if on { 0.5 } else { 0.3 });
        }
        // Subtle icons on the selected saved session: a pencil to rename, a bin to delete.
        if on && row.kind == Kind::Saved && l.rename.is_none() {
            let (ix, iy) = (LAND_X + list_w - 2.0 * ICON_W, y + lh / 2.0);
            let dot = |c: u32, x: f32, y: f32, w: f32, h: f32, alpha: f32| {
                brush.SetColor(&color(rgb(c), alpha));
                rt.FillRectangle(&rect(x.round(), y.round(), w, h), brush);
            };
            // Glow under the pointer: brighter, and a faint plate behind the icon.
            let over = mouse.and_then(|m| (m.1 >= y && m.1 < y + lh).then(|| land_icon(m.0, list_w)).flatten());
            let plate = |i: f32, c: u32| {
                brush.SetColor(&color(rgb(c), 0.16));
                rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(ix + i * ICON_W + 2.0, y + 3.0, ICON_W - 4.0, lh - 6.0), radiusX: 5.0, radiusY: 5.0 }, brush);
            };
            let pen_a = if over == Some(false) { 1.0 } else { 0.45 };
            if over == Some(false) {
                plate(0.0, theme().ansi[2]);
            }
            if over == Some(true) {
                plate(1.0, theme().ansi[1]);
            }
            let bin = if l.armed || over == Some(true) { theme().ansi[1] } else { theme().fg };
            let alpha = if l.armed || over == Some(true) { 1.0 } else { 0.45 };
            // Pencil: a stepped diagonal with a tip.
            let pc = ix + ICON_W / 2.0;
            for k in 0..6 {
                let o = k as f32 * 2.0 - 5.0;
                dot(theme().fg, pc + o, iy - o - 2.0, 3.0, 3.0, pen_a);
            }
            dot(theme().fg, pc - 6.0, iy + 4.0, 3.0, 1.0, pen_a);
            // Bin: handle, lid, sides, bottom.
            let bc = ix + ICON_W * 1.5;
            dot(bin, bc - 2.0, iy - 7.0, 5.0, 1.0, alpha);
            dot(bin, bc - 6.0, iy - 5.0, 13.0, 2.0, alpha);
            dot(bin, bc - 4.0, iy - 3.0, 1.0, 9.0, alpha);
            dot(bin, bc + 4.0, iy - 3.0, 1.0, 9.0, alpha);
            dot(bin, bc - 4.0, iy + 5.0, 9.0, 1.0, alpha);
        }
    }

    // What the selected choice opens: its layout in miniature, with each pane's name, in a card
    // right of everything on the left (the list and the lines below it).
    let px = LAND_X + list_w.max((tagline.chars().count() + 2) as f32 * cw) + 56.0;
    let Some(row) = l.rows.get(l.sel) else { return };
    if px + PREVIEW.0 + 16.0 <= width - 24.0 {
        let card = rect(px - 16.0, LAND_ROWS - 46.0, PREVIEW.0 + 32.0, PREVIEW.1 + 46.0 + 44.0);
        let card = D2D1_ROUNDED_RECT { rect: card, radiusX: 10.0, radiusY: 10.0 };
        brush.SetColor(&color(fg(), 0.035));
        rt.FillRoundedRectangle(&card, brush);
        brush.SetColor(&color(fg(), 0.09));
        rt.DrawRoundedRectangle(&card, brush, 1.0, None);
        text(&row.label, &a.bold, rect(px, LAND_ROWS - 40.0, PREVIEW.0, ch), theme().fg, 0.9);
        let one = Node::Leaf(0);
        let fresh = [SavedPane::default()];
        let (tree, panes) = row.session.as_ref().map_or((&one, &fresh[..]), |s| (&s.tree, &s.panes[..]));
        for (i, (x, y, w, h)) in tree.rects((px, LAND_ROWS, PREVIEW.0, PREVIEW.1)) {
            let r = rect(x, y, w, h);
            let pane = D2D1_ROUNDED_RECT { rect: r, radiusX: 4.0, radiusY: 4.0 };
            brush.SetColor(&color(bg(), 0.6));
            rt.FillRoundedRectangle(&pane, brush);
            brush.SetColor(&color(fg(), 0.18));
            rt.DrawRoundedRectangle(&pane, brush, 1.0, None);
            let name = if row.session.is_some() { preview_name(&panes[i], a.home.as_deref()) } else { "Git Bash \u{b7} ~".into() };
            text(&name, &a.tab, r, theme().fg, 0.65);
        }
        let caption = preview_caption(row.session.is_none(), &row.created, &row.closed);
        text(&caption,&a.tab, rect(px - 60.0, LAND_ROWS + PREVIEW.1 + 8.0, PREVIEW.0 + 120.0, 24.0), theme().fg, 0.4);
    }

    // The rename, delete or leave question, or what went wrong: wrapped to stay left of the card.
    let y = LAND_ROWS + (l.rows.len() as f32 + 1.0) * lh;
    let (msg, c) = if let Some(n) = l.note {
        (n.to_string(), theme().ansi[1])
    } else if l.armed {
        (format!("press Del again to delete \u{201c}{}\u{201d}", row.label), theme().ansi[1])
    } else if l.rename.is_some() {
        ("Enter renames   Esc cancels".to_string(), theme().fg)
    } else if l.leaving {
        (format!("press Enter again to close this session and open \u{201c}{}\u{201d}", row.label), theme().ansi[3])
    } else {
        (String::new(), theme().fg)
    };
    if !msg.is_empty() {
        let room = if px + PREVIEW.0 + 16.0 <= width - 24.0 { px - 40.0 } else { width - 24.0 } - LAND_X;
        if let Ok(wrap) = text_format(&a.dw, DWRITE_FONT_WEIGHT_NORMAL, FONT_SIZE) {
            let _ = wrap.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            text(&msg, &wrap, rect(LAND_X, y, room.max(cw * 10.0), ch * 3.0), c, 0.9);
        }
    }

    // The footer: the keys that drive dterm, the configurable ones as set on the settings page.
    let mut hints = vec![
        "\u{2191}\u{2193} choose".to_string(),
        "Enter open".into(),
        format!("Esc {}", if live { "back" } else { "new" }),
        "F2 rename".into(),
        "Del delete".into(),
    ];
    for (act, what) in [(Action::Save, "save session"), (Action::Home, "start page"), (Action::Settings, "settings")] {
        hints.extend(a.settings.keys[act as usize].map(|k| format!("{} {what}", chord_text(k))));
    }
    let top = height - FOOT_H;
    brush.SetColor(&color(fg(), 0.04));
    rt.FillRectangle(&rect(0.0, top, width, FOOT_H), brush);
    brush.SetColor(&color(fg(), 0.08));
    rt.FillRectangle(&rect(0.0, top, width, 1.0), brush);
    if let Ok(small) = text_format(&a.dw, DWRITE_FONT_WEIGHT_NORMAL, TAB_SIZE) {
        let _ = small.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        text(&hints.join("   \u{b7}   "), &small, rect(LAND_X, top, (width - LAND_X - 12.0).max(0.0), FOOT_H), theme().fg, 0.35);
    }
}

/// The view button, and in Tabs view the strip with a tab per pane, over the panes.
unsafe fn draw_chrome(a: &App, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush, width: f32) {
    if a.view != View::Tabs {
        return;
    }
    let hit = a.mouse.and_then(|m| tab_at(a.view, a.panes.len(), m, width));
    {
        // The strip has the same tint as the panes, so the tabs sit right on the acrylic.
        rt.PushAxisAlignedClip(&rect(0.0, 0.0, width, TAB_H), D2D1_ANTIALIAS_MODE_ALIASED);
        rt.Clear(Some(&color(bg(), a.settings.opacity)));
        rt.PopAxisAlignedClip();
        let order = a.order();
        let tw = tab_width(order.len(), width);
        let focused = a.panes.get(a.focus).map(|p| p.id);
        for (n, p) in order.iter().filter_map(|id| a.panes.iter().find(|p| p.id == *id)).enumerate() {
            let active = focused == Some(p.id);
            let (text, alpha) = match a.rename.as_ref().filter(|r| r.0 == p.id) {
                Some((_, typed)) => (format!("{typed}\u{258f}"), 1.0), // the name being typed, with a caret
                None => {
                    let alpha = match (active, hit == Some(n)) {
                        (true, _) => TAB_ACTIVE_ALPHA,
                        (_, true) => LABEL_FOCUS_ALPHA,
                        _ => LABEL_ALPHA,
                    };
                    (p.name(a.home.as_deref()).unwrap_or_default(), alpha)
                }
            };
            let (x, room) = (n as f32 * tw + TAB_PAD, (tw - 2.0 * TAB_PAD).max(0.0));
            let (tint, alpha) = status_tint(p.status).filter(|_| a.rename.is_none()).unwrap_or((fg(), alpha));
            brush.SetColor(&color(tint, alpha));
            let units: Vec<u16> = text.encode_utf16().collect();
            rt.DrawText(&units, &a.tab, &rect(x, 0.0, room, TAB_H), brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if active {
                brush.SetColor(&color(fg(), UNDERLINE_ALPHA));
                rt.FillRectangle(&rect(x, TAB_H - 2.0, room, 2.0), brush);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_term(
    rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush,
    format: &IDWriteTextFormat, bold_format: &IDWriteTextFormat,
    (cw, ch): (f32, f32), (ox, oy): (f32, f32), focused: bool, term: &Term<Proxy>,
) {
    let content = term.renderable_content();
    let cursor = content.cursor;
    let offset = content.display_offset as i32;
    let selection = content.selection;
    // Unfocused panes show a hollow cursor, like Windows Terminal.
    let shape = match cursor.shape {
        CursorShape::Block if !focused => CursorShape::HollowBlock,
        s => s,
    };

    // Text in runs of plain ASCII with one style; anything else is drawn alone in its own cell,
    // so fallback-font glyphs with odd widths never shift the rest of the line.
    let mut run: Vec<u16> = Vec::new();
    let mut run_at = (0.0f32, 0.0f32);
    let mut run_style = (Rgb::default(), false);
    let flush = |run: &mut Vec<u16>, at: (f32, f32), style: (Rgb, bool)| {
        if run.is_empty() {
            return;
        }
        brush.SetColor(&color(style.0, 1.0));
        let f = if style.1 { bold_format } else { format };
        // Color glyphs only for real emoji (outside the BMP); symbols like ⏺ ✳ stay plain text, as in Windows Terminal.
        let opts = if run.iter().any(|u| (0xD800..0xDC00).contains(u)) {
            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT
        } else {
            D2D1_DRAW_TEXT_OPTIONS_NONE
        };
        rt.DrawText(run, f, &rect(at.0, at.1, cw * (run.len() as f32 + 1.0), ch), brush, opts, DWRITE_MEASURING_MODE_NATURAL);
        run.clear();
    };

    let base = (fg(), bg()); // the theme's; `fg` and `bg` below are the cell's
    for cell in content.display_iter {
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = flags.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, bold, content.colors).unwrap_or(base.1);
        let mut bg = resolve(cell.bg, false, content.colors);
        if flags.contains(Flags::INVERSE) {
            (fg, bg) = (bg.unwrap_or(base.1), Some(fg));
        }
        if flags.contains(Flags::DIM) {
            fg = dim(fg);
        }
        let x = ox + cell.point.column.0 as f32 * cw;
        let y = oy + (cell.point.line.0 + offset) as f32 * ch;
        let width = if flags.contains(Flags::WIDE_CHAR) { 2.0 * cw } else { cw };
        let is_cursor = cell.point == cursor.point && shape != CursorShape::Hidden;
        let selected = !(is_cursor && shape == CursorShape::Block)
            && selection.is_some_and(|s| s.contains_cell(&cell, cursor.point, shape));
        if is_cursor && shape == CursorShape::Block {
            (fg, bg) = (bg.unwrap_or(base.1), Some(fg));
        }

        if let Some(b) = bg {
            flush(&mut run, run_at, run_style);
            brush.SetColor(&color(b, 1.0));
            rt.FillRectangle(&rect(x, y, width, ch), brush);
        }
        // Selection: a light veil over the cell, like Windows Terminal; the text keeps its color.
        if selected {
            flush(&mut run, run_at, run_style);
            brush.SetColor(&color(base.0, SELECTION_ALPHA));
            rt.FillRectangle(&rect(x, y, width, ch), brush);
        }
        if is_cursor {
            brush.SetColor(&color(base.0, 1.0));
            match shape {
                CursorShape::Beam => rt.FillRectangle(&rect(x, y, 2.0, ch), brush),
                CursorShape::Underline => rt.FillRectangle(&rect(x, y + ch - 2.0, width, 2.0), brush),
                CursorShape::HollowBlock => rt.DrawRectangle(&rect(x + 0.5, y + 0.5, width - 1.0, ch - 1.0), brush, 1.0, None),
                _ => {}
            }
        }

        let c = cell.c;
        if c == ' ' || flags.contains(Flags::HIDDEN) {
            flush(&mut run, run_at, run_style);
            continue;
        }
        let plain = c.is_ascii() && cell.zerowidth().is_none();
        let joins = plain && !run.is_empty() && run_style == (fg, bold) && run_at.1 == y
            && (run_at.0 + run.len() as f32 * cw - x).abs() < 0.5;
        if !joins {
            flush(&mut run, run_at, run_style);
            run_at = (x, y);
            run_style = (fg, bold);
        }
        let mut buf = [0u16; 2];
        run.extend_from_slice(c.encode_utf16(&mut buf));
        for z in cell.zerowidth().unwrap_or(&[]) {
            run.extend_from_slice(z.encode_utf16(&mut buf));
        }
        if !plain {
            flush(&mut run, run_at, run_style);
        }
    }
    flush(&mut run, run_at, run_style);
}

unsafe fn clipboard_text(hwnd: HWND) -> Option<String> {
    OpenClipboard(Some(hwnd)).ok()?;
    let text = (|| {
        let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
        if p.is_null() {
            return None;
        }
        let n = (0..).take_while(|&i| *p.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        let _ = GlobalUnlock(HGLOBAL(h.0));
        Some(s)
    })();
    let _ = CloseClipboard();
    text
}

unsafe fn set_clipboard_text(hwnd: HWND, text: &str) {
    if OpenClipboard(Some(hwnd)).is_err() {
        return;
    }
    let _ = EmptyClipboard();
    let units = clip_units(text);
    if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
            let _ = GlobalUnlock(h);
        }
        // The clipboard owns the memory once SetClipboardData succeeds.
        if p.is_null() || SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
        }
    }
    let _ = CloseClipboard();
}

/// Copy the focused pane's selection to the clipboard and clear it. False when nothing is selected.
unsafe fn copy(hwnd: HWND) -> bool {
    let text = with(|a| a.panes.get_mut(a.focus).and_then(Pane::take_selection));
    let _ = InvalidateRect(Some(hwnd), None, false);
    let Some(text) = text else { return false };
    set_clipboard_text(hwnd, &text);
    true
}

unsafe fn paste(hwnd: HWND) {
    let Some(text) = clipboard_text(hwnd) else { return };
    with(|a| {
        if let Some(p) = a.panes.get_mut(a.focus) {
            let bracketed = p.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
            p.input(paste_text(&text, bracketed).into_bytes());
        }
    });
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Start typing a name for the focused pane in its label slot.
unsafe fn begin_rename(hwnd: HWND) {
    with(|a| a.rename = a.panes.get(a.focus).map(|p| (p.id, p.custom.clone().unwrap_or_default())));
    layout(hwnd); // a lone pane gets its label slot now
}

/// Stop typing a name. `keep` stores it as the pane's name; empty removes the name.
unsafe fn end_rename(hwnd: HWND, keep: bool) {
    with(|a| {
        let Some((id, typed)) = a.rename.take() else { return };
        if let (true, Some(p)) = (keep, a.panes.iter_mut().find(|p| p.id == id)) {
            let typed = typed.trim();
            p.custom = (!typed.is_empty()).then(|| typed.into());
        }
    });
    layout(hwnd);
}

/// Change the font size by a step (see `zoomed`); every terminal refits.
unsafe fn zoom(hwnd: HWND, step: i32) {
    let size = with(|a| zoomed(a.font_size, step));
    set_font(hwnd, size);
}

unsafe fn set_font(hwnd: HWND, size: f32) {
    let changed = with(|a| {
        if size == a.font_size {
            return false;
        }
        let fonts = (|| {
            let format = text_format(&a.dw, DWRITE_FONT_WEIGHT_NORMAL, size).ok()?;
            let bold = text_format(&a.dw, DWRITE_FONT_WEIGHT_BOLD, size).ok()?;
            Some((cell_size(&a.dw, &format).ok()?, format, bold))
        })();
        let Some((cell, format, bold)) = fonts else { return false };
        (a.font_size, a.cell, a.format, a.bold) = (size, cell, format, bold);
        true
    });
    if changed {
        layout(hwnd);
    }
}

/// Write the current session to `file`. False when there is nothing to save or writing failed.
fn save_session(file: &std::path::Path) -> bool {
    let Some(s) = with(|a| a.session()) else { return false };
    file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(file, session_text(&s)).is_ok()
}

/// Closing the window remembers the session for the landing page's "Last session".
fn save_last() {
    if let Some(dir) = data_dir() {
        save_session(&dir.join("last.session"));
    }
}

/// Open a saved session in the empty window: its panes start in their folders, and Claude Code
/// comes back in the panes that ran it.
unsafe fn open_session(hwnd: HWND, s: Session) -> bool {
    set_font(hwnd, s.font);
    let scale = scale(hwnd);
    let opened = with(|a| {
        // Fresh ids: exit notices from panes of a closed session may still be on their way.
        let base = a.next_id;
        let mut panes = Vec::new();
        for (i, saved) in s.panes.iter().enumerate() {
            let dir = Some(saved.cwd.as_str()).filter(|c| !c.is_empty());
            let Ok(mut p) = spawn(hwnd, base + i, &a.bash, a.cell, scale, dir) else {
                panes.iter().for_each(|p: &Pane| drop(p.tx.send(Msg::Shutdown)));
                return false;
            };
            p.custom = (!saved.name.is_empty()).then(|| saved.name.clone());
            if resume_command(&saved.run) {
                p.send(format!("{}\r", saved.run).into_bytes()); // typed ahead: the shell runs it once it is up
            }
            panes.push(p);
        }
        // The leaves are numbered in tree order, the same order as the ids just given out.
        let tree = s.tree.numbered(&mut base.clone());
        (a.panes, a.tree, a.next_id, a.view) = (panes, Some(tree), base + s.panes.len(), s.view);
        a.focus = s.focus;
        a.created = if s.created > 0 { s.created } else { now() };
        true
    });
    if opened {
        layout(hwnd);
    }
    opened
}

/// Close every pane without closing the window (for the landing page to open another session).
fn close_all(a: &mut App) {
    for p in a.panes.drain(..) {
        let _ = p.tx.send(Msg::Shutdown);
    }
    (a.tree, a.focus, a.view, a.drag, a.select, a.rename, a.prompt) = (None, 0, View::Grid, None, None, None, None);
}

/// Leave the landing page through choice `i`: back to the running session, a saved session, or a
/// new one (also when the file can't be opened any more). With a session running behind the page,
/// opening another one closes it, so the first Enter or click only asks.
unsafe fn open_choice(hwnd: HWND, i: usize) {
    let (live, kind) = with(|a| {
        let l = a.landing.as_ref();
        (l.is_some_and(|l| l.rows.iter().any(|r| r.kind == Kind::Live)), l.and_then(|l| l.rows.get(i)).map(|r| r.kind))
    });
    if kind == Some(Kind::Live) {
        with(|a| a.landing = None);
        layout(hwnd);
        return;
    }
    if live {
        let ask = with(|a| {
            let Some(l) = a.landing.as_mut() else { return true };
            let ask = !(l.leaving && l.sel == i);
            (l.sel, l.leaving, l.armed) = (i, ask, false);
            ask
        });
        if ask {
            let _ = InvalidateRect(Some(hwnd), None, false);
            return;
        }
        save_last();
        with(close_all);
    }
    let session = with(|a| {
        a.created = now(); // a new session starts now; a restored one takes its own time
        a.landing.take().and_then(|l| l.rows.into_iter().nth(i)).and_then(|c| c.session)
    });
    if !session.is_some_and(|s| open_session(hwnd, s)) {
        add_pane(hwnd, None);
    }
    if with(|a| a.panes.is_empty()) {
        fail("Could not start Git Bash");
        let _ = DestroyWindow(hwnd);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Ctrl+Alt+H or the ≡ button: the landing page, with the running session as its first choice.
/// The panes keep running behind it; the session is also saved as the last one.
unsafe fn go_home(hwnd: HWND) {
    save_last();
    let rows = data_dir().map(|d| landing_rows(&d, &ago)).unwrap_or_default();
    with(|a| {
        let Some(s) = a.session() else { return };
        // The last session is this one now, so it shows up only as the running one.
        let mut rows: Vec<Choice> = rows.into_iter().filter(|r| r.kind != Kind::Last).collect();
        rows.insert(1, choice("This session".into(), s, None, Kind::Live, &ago));
        let mut l = Landing::new(rows);
        l.sel = 1; // the running session
        a.landing = Some(l);
        (a.drag, a.select, a.rename, a.prompt) = (None, None, None, None);
    });
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Start renaming the selected saved session on the landing page, with its name to edit.
fn start_rename(a: &mut App) {
    if let Some(l) = a.landing.as_mut().filter(|l| l.saved()) {
        l.calm();
        l.rename = Some(l.rows[l.sel].label.clone());
    }
}

/// Delete the selected saved session: the first call asks, the second removes the file.
fn press_delete(a: &mut App) {
    let Some(l) = a.landing.as_mut().filter(|l| l.saved()) else { return };
    let ask = !std::mem::replace(&mut l.armed, true);
    (l.leaving, l.rename, l.note) = (false, None, None);
    if !ask {
        l.delete();
    }
}

/// Keys on the landing page: ↑↓ choose, Enter opens, Esc opens the first choice (the running
/// session, else a new one), F2 renames and Del (twice) deletes a saved one. While a name is
/// typed, Enter keeps it, Esc drops it, and the other keys edit it.
unsafe fn landing_key(hwnd: HWND, vk: u16) {
    if with(|a| a.landing.as_ref().is_some_and(|l| l.rename.is_some())) {
        with(|a| {
            let Some(l) = a.landing.as_mut() else { return };
            match vk {
                0x0D | 0x1B => {
                    a.skip_char = true;
                    let typed = l.rename.take().unwrap_or_default();
                    l.note = if vk == 0x0D { l.rename_to(&typed).err() } else { None };
                }
                0x08 => {
                    l.rename.as_mut().map(|t| t.pop());
                }
                _ => {}
            }
        });
        let _ = InvalidateRect(Some(hwnd), None, false);
        return;
    }
    match vk {
        0x26 | 0x28 => with(|a| {
            if let Some(l) = a.landing.as_mut() {
                l.sel = cycle(l.rows.len(), l.sel, if vk == 0x26 { -1 } else { 1 });
                l.calm();
            }
        }),
        0x71 => with(start_rename),
        0x0D | 0x1B => {
            with(|a| a.skip_char = true); // the Enter / Esc character is not for the new shell
            let i = with(|a| a.landing.as_ref().map_or(0, |l| if vk == 0x0D { l.sel } else { l.first() }));
            open_choice(hwnd, i);
        }
        0x2E => with(press_delete),
        _ => {}
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Write the settings to `%APPDATA%\dterm\settings` and show them.
unsafe fn apply_settings(hwnd: HWND) {
    let s = with(|a| a.settings.clone());
    THEME.store(s.theme, Ordering::Relaxed);
    if let Some(d) = data_dir() {
        let _ = std::fs::create_dir_all(&d).and_then(|_| std::fs::write(d.join("settings"), settings_text(&s)));
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Keys on the settings page: ↑↓ choose, ←→ change the theme or opacity, Enter waits for new keys
/// for a shortcut (Esc cancels), Del puts back its default, Esc closes the page.
unsafe fn settings_key(hwnd: HWND, vk: u16, chord: Chord) {
    drop_char(hwnd);
    let rows = 2 + ACTIONS.len();
    let changed = with(|a| {
        let Some(page) = a.page.as_mut() else { return false };
        let s = &mut a.settings;
        let action = page.sel.checked_sub(2);
        if page.capture {
            page.capture = false;
            match (vk, action) {
                (0x1B, _) => {}
                (_, Some(i)) if key_name(chord.vk).is_some() && (chord.ctrl || chord.alt) => s.bind(i, Some(chord)),
                _ => page.capture = true, // a modifier on its own, or keys that can't be a shortcut: keep waiting
            }
            return !page.capture;
        }
        let step = match vk {
            0x25 => -1,
            0x27 | 0x0D => 1,
            _ => 0,
        };
        match (vk, page.sel) {
            (0x26 | 0x28, _) => page.sel = cycle(rows, page.sel, if vk == 0x26 { -1 } else { 1 }),
            (0x1B, _) => a.page = None,
            (_, 0) if step != 0 => s.theme = cycle(THEMES.len(), s.theme, step),
            (_, 1) if step != 0 => s.opacity = (s.opacity + 0.05 * step as f32).clamp(MIN_OPACITY, 1.0),
            (0x0D, _) => page.capture = true,
            (0x2E | 0x08, i) if i >= 2 => s.bind(i - 2, parse_chord(ACTIONS[i - 2].3)),
            _ => return false,
        }
        true
    });
    if changed {
        apply_settings(hwnd);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// The settings page, in the style of the landing page: a title, then a row per setting with its
/// value (the selected one bright with a green ❯), the keys, and the shortcuts that can't change.
unsafe fn draw_settings(a: &App, page: &SettingsPage, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush) {
    let (cw, ch) = a.cell;
    let lh = land_line(ch);
    let line = |s: &str, x: f32, y: f32, c: u32, alpha: f32| {
        brush.SetColor(&color(rgb(c), alpha));
        let units: Vec<u16> = s.encode_utf16().collect();
        let r = rect(x, y + (lh - ch) / 2.0, 4000.0, ch);
        rt.DrawText(&units, &a.format, &r, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    };
    let t = theme();
    line("settings", LAND_X, LAND_Y, t.ansi[2], 1.0);
    let s = &a.settings;
    let mut rows = vec![
        ("Theme".to_string(), format!("\u{2039} {} \u{203a}", t.name)),
        ("Opacity".to_string(), format!("\u{2039} {}% \u{203a}", (s.opacity * 100.0).round())),
    ];
    for (act, k) in ACTIONS.iter().zip(s.keys) {
        rows.push((act.2.to_string(), k.map_or("none".into(), chord_text)));
    }
    let top = LAND_Y + 2.0 * lh;
    let value_x = LAND_X + 18.0 * cw;
    for (i, (label, value)) in rows.iter().enumerate() {
        // A gap between the look and the shortcuts.
        let y = top + i as f32 * lh + if i >= 2 { lh / 2.0 } else { 0.0 };
        let on = i == page.sel;
        let value = if on && page.capture { "press the new keys\u{2026}".to_string() } else { value.clone() };
        if on {
            brush.SetColor(&color(fg(), 0.07));
            let bar = D2D1_ROUNDED_RECT { rect: rect(LAND_X - 12.0, y, 46.0 * cw, lh), radiusX: 6.0, radiusY: 6.0 };
            rt.FillRoundedRectangle(&bar, brush);
            line("\u{276f}", LAND_X, y, t.ansi[2], 1.0);
        }
        line(label, LAND_X + 2.0 * cw, y, t.fg, if on { 1.0 } else { 0.6 });
        line(&value, value_x, y, if on && page.capture { t.ansi[3] } else { t.fg }, if on { 0.9 } else { 0.45 });
    }
    let y = top + (rows.len() as f32 + 1.5) * lh;
    let keys = if page.capture {
        "press a key with Ctrl or Alt   Esc cancel"
    } else if page.sel < 2 {
        "\u{2191}\u{2193} choose   \u{2190}\u{2192} change   Esc back"
    } else {
        "\u{2191}\u{2193} choose   Enter new keys   Del default   Esc back"
    };
    line(keys, LAND_X, y, t.fg, 0.4);
    line("fixed: Alt+arrows focus   Alt+Shift+arrows resize   Ctrl+Alt+arrows swap   Ctrl+Tab next pane", LAND_X, y + lh, t.fg, 0.25);
    line("Shift+arrows select (Ctrl: by word)   Ctrl+C / Ctrl+V copy, paste   Ctrl+Plus / Minus / 0 font", LAND_X, y + 2.0 * lh, t.fg, 0.25);
}

/// Enter in the save-session prompt: save under the typed name and say so for a moment.
unsafe fn end_prompt(hwnd: HWND, keep: bool) {
    let typed = with(|a| a.prompt.take());
    if let (true, Some(name)) = (keep, typed.as_deref().and_then(session_file_name)) {
        let saved = data_dir().is_some_and(|d| save_session(&d.join("sessions").join(format!("{name}.session"))));
        with(|a| a.toast = Some(if saved { format!("saved \u{201c}{name}\u{201d}") } else { "could not save".into() }));
        SetTimer(Some(hwnd), TOAST_TIMER, 1500, None);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// The zoom and grid / tabs shortcuts, or the view button: switch to `to` or back to the grid (see `toggled`).
unsafe fn toggle_view(hwnd: HWND, to: View) {
    let changed = with(|a| {
        let view = toggled(a.view, to);
        a.drag = None;
        std::mem::replace(&mut a.view, view) != view
    });
    if changed {
        layout(hwnd);
    }
}

/// Commands that need the whole grid (focus moves, splits, closing) first end a Zoom. True when
/// the grid is showing now; false in Tabs, where grid-only commands have nothing to act on.
unsafe fn leave_zoom(hwnd: HWND) -> bool {
    let view = with(|a| a.view);
    if view == View::Zoom {
        with(|a| a.view = View::Grid);
        layout(hwnd);
    }
    view != View::Tabs
}

/// Window title: the focused pane's name.
unsafe fn update_title(hwnd: HWND) {
    let title = with(|a| {
        let t = window_title(a.panes.get(a.focus).and_then(|p| p.name(a.home.as_deref())).as_deref());
        (t != a.title).then(|| {
            a.title = t.clone();
            t
        })
    });
    if let Some(t) = title {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide(&t).as_ptr()));
        let _ = InvalidateRect(Some(hwnd), None, false); // the caption shows it
    }
}

/// Drop the WM_CHAR or WM_SYSCHAR that TranslateMessage queued for the key being handled, if the key made one.
unsafe fn drop_char(hwnd: HWND) {
    for m in [WM_CHAR, WM_SYSCHAR] {
        let _ = PeekMessageW(&mut MSG::default(), Some(hwnd), m, m, PM_REMOVE);
    }
}

/// Whether AltGr (right Alt) is held and key `vk` types a character with it on this keyboard
/// layout, like AltGr+è = [ on an Italian keyboard. Then the character wins over a Ctrl+Alt
/// shortcut (AltGr counts as Ctrl+Alt); left Ctrl + left Alt still give the shortcut.
unsafe fn altgr_char(vk: u16, scan: u32) -> bool {
    let mut state = [0u8; 256];
    if GetKeyState(VK_RMENU.0 as i32) >= 0 || GetKeyboardState(&mut state).is_err() {
        return false;
    }
    let mut out = [0u16; 8];
    // Flag 4: leave the keyboard state alone, so a dead key still works for the real keystroke.
    let n = ToUnicode(vk as u32, scan, Some(&state), &mut out, 4);
    n != 0 && out[0] >= 0x20
}

/// Run a dterm command (see ACTIONS).
unsafe fn run(hwnd: HWND, act: Action) {
    drop_char(hwnd);
    match act {
        Action::NewPane | Action::SplitRight | Action::SplitDown => {
            leave_zoom(hwnd);
            let dir = match act {
                Action::SplitRight => Some(true),
                Action::SplitDown => Some(false),
                _ => None,
            };
            add_pane(hwnd, dir);
        }
        Action::Close => {
            if let Some(id) = with(|a| a.panes.get(a.focus).map(|p| p.id)) {
                leave_zoom(hwnd);
                remove_pane(hwnd, id);
            }
        }
        Action::Rename => begin_rename(hwnd),
        Action::Save => {
            with(|a| (a.prompt, a.toast) = (Some(String::new()), None));
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        Action::Zoom => toggle_view(hwnd, View::Zoom),
        Action::Tabs => toggle_view(hwnd, View::Tabs),
        Action::Home => go_home(hwnd),
        Action::Settings => {
            with(|a| a.page = Some(SettingsPage { sel: 0, capture: false }));
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

unsafe fn modifiers() -> (bool, bool, bool) {
    let down = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
    (down(VK_SHIFT), down(VK_MENU), down(VK_CONTROL))
}

/// Returns true when the key was handled here. `scan` is the key's scan code.
unsafe fn on_key(hwnd: HWND, vk: u16, scan: u32) -> bool {
    let (shift, alt, ctrl) = modifiers();
    let chord = chord_of(vk, shift, alt, ctrl);
    // The settings page takes every key; Alt chords (Alt+F4) stay with the system unless it waits for new keys.
    if with(|a| a.page.is_some()) {
        let mine = !alt || ctrl || with(|a| a.page.as_ref().is_some_and(|p| p.capture));
        if mine {
            settings_key(hwnd, vk, chord);
        }
        return mine;
    }
    // A dterm shortcut, unless a name is being typed or AltGr makes a character with this key.
    let typing = with(|a| a.prompt.is_some() || a.rename.is_some() || a.landing.as_ref().is_some_and(|l| l.rename.is_some()));
    let action = with(|a| a.settings.action(chord)).filter(|_| !typing && !altgr_char(vk, scan));
    // The landing page takes every key; Alt chords (Alt+F4) stay with the system. Of the
    // shortcuts, settings and the start page (back to the running session) work there.
    if with(|a| a.landing.is_some()) {
        match action {
            Some(Action::Settings) => run(hwnd, Action::Settings),
            Some(Action::Home) if with(|a| a.landing.as_ref().is_some_and(|l| l.rows.iter().any(|r| r.kind == Kind::Live))) => {
                drop_char(hwnd);
                open_choice(hwnd, with(|a| a.landing.as_ref().map_or(0, |l| l.first())));
            }
            _ if !alt => landing_key(hwnd, vk),
            _ => return false,
        }
        return true;
    }
    if let Some(act) = action {
        run(hwnd, act);
        return true;
    }
    // Typing a session name: like renaming, keys edit it.
    if with(|a| a.prompt.is_some()) {
        if !alt {
            match vk {
                0x0D | 0x1B => {
                    with(|a| a.skip_char = true);
                    end_prompt(hwnd, vk == 0x0D);
                }
                0x08 => {
                    with(|a| {
                        if let Some(t) = a.prompt.as_mut() {
                            t.pop();
                        }
                    });
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                _ => {}
            }
        }
        return !alt;
    }
    // While a name is typed, keys edit it and never reach the shell; Alt chords stay with the system.
    if with(|a| a.rename.is_some()) {
        if !alt {
            match vk {
                0x0D | 0x1B => {
                    with(|a| a.skip_char = true); // the Enter / Esc character is not for the shell either
                    end_rename(hwnd, vk == 0x0D);
                }
                0x08 => {
                    with(|a| {
                        if let Some(r) = a.rename.as_mut() {
                            r.1.pop();
                        }
                    });
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                _ => {}
            }
        }
        return !alt;
    }
    // Ctrl+Tab / Ctrl+Shift+Tab: next / previous pane (tab). Whatever character they make is dropped.
    if ctrl && !alt && vk == 0x09 {
        drop_char(hwnd);
        leave_zoom(hwnd);
        with(|a| a.focus_step(if shift { -1 } else { 1 }));
        let _ = InvalidateRect(Some(hwnd), None, false);
        return true;
    }
    // Alt+arrows move the focus between panes, like Windows Terminal.
    let dir = match vk {
        0x25 => Some((-1, 0)),
        0x27 => Some((1, 0)),
        0x26 => Some((0, -1)),
        0x28 => Some((0, 1)),
        _ => None,
    };
    // In Tabs, Alt+Left / Right go to the previous / next tab and Alt+Up / Down do nothing.
    if let (Some(d), true, false, false) = (dir, alt, ctrl, shift) {
        if with(|a| a.view == View::Tabs) {
            with(|a| a.focus_step(d.0 as isize));
        } else {
            leave_zoom(hwnd);
            with(|a| {
                let rects: Vec<_> = a.panes.iter().map(|p| p.rect).collect();
                if let Some(i) = neighbor(&rects, a.focus, d) {
                    a.focus = i;
                }
            });
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
        return true;
    }
    // Shift+arrows select text instead of reaching the shell; with Ctrl, Left / Right go by words.
    if let (Some(d), true, false) = (dir, shift, alt) {
        with(|a| {
            if let Some(p) = a.panes.get_mut(a.focus) {
                p.extend_selection(d, ctrl);
            }
        });
        let _ = InvalidateRect(Some(hwnd), None, false);
        return true;
    }
    // Alt+Shift+arrows move the nearest divider of that direction around the focused pane. Like
    // swapping below, they end a Zoom first and do nothing in Tabs.
    if let (Some(d), true, true, false) = (dir, alt, shift, ctrl) {
        if leave_zoom(hwnd) {
            let (w, h) = client(hwnd);
            with(|a| {
                let id = a.panes.get(a.focus).map(|p| p.id);
                if let (Some(id), Some(t)) = (id, a.tree.as_mut()) {
                    t.resize(id, (0.0, 0.0, w, h), d.0 != 0, STEP * (d.0 + d.1) as f32);
                }
            });
            layout(hwnd);
        }
        return true;
    }
    // Ctrl+Alt+arrows swap the focused pane with its neighbor; the focus goes along.
    if let (Some(d), true, true, false) = (dir, alt, ctrl, shift) {
        if leave_zoom(hwnd) {
            with(|a| {
                let rects: Vec<_> = a.panes.iter().map(|p| p.rect).collect();
                if let (Some(i), Some(t)) = (neighbor(&rects, a.focus, d), a.tree.as_mut()) {
                    t.swap(a.panes[a.focus].id, a.panes[i].id);
                }
            });
            layout(hwnd);
        }
        return true;
    }
    // Alt+Shift+Plus / Minus split the focused pane side by side / top and bottom. The WM_SYSCHAR must not reach the shell.
    if alt && shift && !ctrl {
        let split = match vk {
            0xBB | 0x6B => Some(true),  // VK_OEM_PLUS, VK_ADD
            0xBD | 0x6D => Some(false), // VK_OEM_MINUS, VK_SUBTRACT
            _ => None,
        };
        if split.is_some() {
            drop_char(hwnd);
            leave_zoom(hwnd);
            add_pane(hwnd, split);
            return true;
        }
    }
    // Ctrl+V and Ctrl+Shift+V paste; their ^V must not reach the shell.
    if ctrl && !alt && vk == 'V' as u16 {
        drop_char(hwnd);
        paste(hwnd);
        return true;
    }
    // Ctrl+Plus / Minus / 0 change the font size. Whatever character they make (Ctrl+Minus gives 0x1F)
    // is dropped, not left for the next key.
    if ctrl && !alt {
        let step = match vk {
            0xBB | 0x6B => Some(1),  // VK_OEM_PLUS, VK_ADD
            0xBD | 0x6D => Some(-1), // VK_OEM_MINUS, VK_SUBTRACT
            0x30 | 0x60 => Some(0),  // '0', VK_NUMPAD0
            _ => None,
        };
        if let Some(step) = step {
            drop_char(hwnd);
            zoom(hwnd, step);
            return true;
        }
    }
    // Ctrl+C copies a selection; without one it is ^C for the shell, but Ctrl+Shift+C never is.
    if ctrl && !alt && vk == 'C' as u16 && (copy(hwnd) || shift) {
        with(|a| a.skip_char = true);
        return true;
    }
    // Esc drops a selection instead of reaching the shell.
    if vk == 0x1B
        && with(|a| {
            let had = a.panes.get_mut(a.focus).is_some_and(|p| p.take_selection().is_some());
            a.skip_char = had;
            had
        })
    {
        let _ = InvalidateRect(Some(hwnd), None, false);
        return true;
    }
    let sent = with(|a| {
        let Some(p) = a.panes.get_mut(a.focus) else { return false };
        let app_cursor = p.term.lock().mode().contains(TermMode::APP_CURSOR);
        let Some(seq) = key_seq(vk, shift, alt, ctrl, app_cursor) else { return false };
        p.input(seq.into_bytes());
        a.skip_char = vk == 0x09; // shift+tab also arrives as a '\t' WM_CHAR
        true
    });
    if sent {
        let _ = InvalidateRect(Some(hwnd), None, false); // the selection is gone
    }
    sent
}

unsafe fn on_char(unit: u16, alt: bool) {
    with(|a| {
        if std::mem::take(&mut a.skip_char) {
            return;
        }
        if (0xD800..0xDC00).contains(&unit) {
            a.surrogate = unit;
            return;
        }
        let units = if (0xDC00..0xE000).contains(&unit) { vec![std::mem::take(&mut a.surrogate), unit] } else { vec![unit] };
        let mut s: String = char::decode_utf16(units).filter_map(|r| r.ok()).collect();
        if let Some(l) = a.landing.as_mut() {
            if let Some(typed) = l.rename.as_mut() {
                if !alt && !s.chars().any(char::is_control) && typed.chars().count() < PROMPT_MAX {
                    typed.push_str(&s);
                }
            }
            return;
        }
        if let Some(typed) = a.prompt.as_mut() {
            if !alt && !s.chars().any(char::is_control) && typed.chars().count() < PROMPT_MAX {
                typed.push_str(&s);
            }
            return;
        }
        // While a name is typed, text goes there; control keys are handled in on_key.
        if let Some((_, typed)) = a.rename.as_mut() {
            if !alt && !s.chars().any(char::is_control) && typed.chars().count() < NAME_MAX {
                typed.push_str(&s);
            }
            return;
        }
        // Backspace sends DEL and Ctrl+Backspace sends BS, like Windows Terminal.
        s = match s.as_str() {
            "\x08" => "\x7f".into(),
            "\x7f" => "\x08".into(),
            _ => s,
        };
        if alt {
            s.insert(0, '\x1b');
        }
        if let Some(p) = a.panes.get_mut(a.focus) {
            p.input(s.into_bytes());
        }
    });
}

/// The visible pane under `m`.
fn pane_at(a: &App, m: (f32, f32)) -> Option<usize> {
    a.panes.iter().enumerate().position(|(i, p)| a.is_shown(i) && contains(p.rect, m))
}

/// Divider under the mouse (the gap between two panes): path to its split, and whether the
/// split is side by side. Only the grid has dividers.
unsafe fn divider_at(hwnd: HWND) -> Option<(Vec<bool>, bool)> {
    let (m, (w, h)) = (mouse(hwnd), client(hwnd));
    with(|a| a.tree.as_ref().filter(|_| a.view == View::Grid)?.divider_at((0.0, 0.0, w, h), m))
}

/// The tab under the mouse, and whether the mouse is over the tab strip.
unsafe fn chrome_at(hwnd: HWND) -> (Option<usize>, bool) {
    let (m, (w, _)) = (mouse(hwnd), client(hwnd));
    with(|a| (tab_at(a.view, a.panes.len(), m, w), a.view == View::Tabs && m.1 < TAB_H))
}

/// Wheel over the pane under `m`: report it to the app, send it arrows, or scroll the scrollback.
unsafe fn wheel(hwnd: HWND, m: (f32, f32), delta: i32) {
    with(|a| {
        let Some(i) = pane_at(a, m) else { return };
        let p = &a.panes[i];
        let mut term = p.term.lock();
        let offset = term.grid().display_offset() as i32;
        let (pt, _) = p.point_at(&term, m, a.cell);
        match wheel_input(*term.mode(), delta, (pt.column.0, (pt.line.0 + offset) as usize)) {
            Some(bytes) => p.send(bytes),
            None => term.scroll_display(Scroll::Delta(wheel_lines(delta))),
        }
    });
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Refreshes the pane statuses and runs the timer only while one is busy.
unsafe fn sync_status(hwnd: HWND) {
    let (changed, busy) = with(|a| (a.refresh_status(), a.panes.iter().any(|p| p.status == Status::Busy)));
    if busy {
        SetTimer(Some(hwnd), STATUS_TIMER, STATUS_TICK, None);
    } else {
        let _ = KillTimer(Some(hwnd), STATUS_TIMER);
    }
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_WAKE => {
            sync_status(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_EXIT => {
            remove_pane(hwnd, wp.0);
            return LRESULT(0);
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            with(|a| a.refresh_status()); // a focus change clears its pane
            paint(hwnd);
            let _ = EndPaint(hwnd, &ps);
            update_title(hwnd); // focus or names may have changed
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        // Always draw the frame as active: Windows otherwise swaps the acrylic for a flat color
        // whenever another window has the focus.
        WM_NCACTIVATE => return DefWindowProcW(hwnd, msg, WPARAM(1), lp),
        WM_SIZE => layout(hwnd),
        // dterm draws its own caption, so the system one goes. Windows 11 keeps the rest of its
        // frame (rounded corners, shadow, resize borders), like Windows Terminal. Windows 10 drops
        // it all: its invisible resize borders would show the acrylic as a halo.
        WM_NCCALCSIZE if wp.0 != 0 => {
            let p = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
            let (zoomed, top) = (IsZoomed(hwnd).as_bool(), p.rgrc[0].top);
            let dpi = GetDpiForWindow(hwnd);
            let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            let (fx, fy) = (GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad, GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad);
            let r = &mut p.rgrc[0];
            if !ACCENT.load(Ordering::Relaxed) {
                DefWindowProcW(hwnd, msg, wp, lp);
                // A maximized window hangs its frame off the screen; keep the content on it.
                p.rgrc[0].top = if zoomed { top + fy } else { top };
            } else if zoomed {
                (r.left, r.top, r.right, r.bottom) = (r.left + fx, r.top + fy, r.right - fx, r.bottom - fy);
            }
            return LRESULT(0);
        }
        WM_NCHITTEST => {
            // Windows 11's own side and bottom borders resize as usual; the top edge and the caption are ours.
            if !ACCENT.load(Ordering::Relaxed) {
                let hit = DefWindowProcW(hwnd, msg, wp, lp);
                if hit.0 != HTCLIENT as isize {
                    return hit;
                }
            }
            let mut pt = POINT { x: lp.0 as u16 as i16 as i32, y: (lp.0 >> 16) as u16 as i16 as i32 };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
            let ((w, h), s) = (client_px(hwnd), scale(hwnd));
            let border = if IsZoomed(hwnd).as_bool() { 0 } else { (RESIZE_BORDER * s) as i32 };
            let left = left_buttons(with(|a| session_shown(a))) * CAPTION_BTN * s;
            let buttons = (left as i32, (3.0 * CAPTION_BTN * s) as i32);
            let hit = frame_hit((pt.x, pt.y), (w as i32, h as i32), border, (CAPTION_H * s) as i32, buttons);
            return LRESULT(hit as isize);
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP if caption_mouse(hwnd, msg) => return LRESULT(0),
        WM_ENTERSIZEMOVE if ACCENT.load(Ordering::Relaxed) => _ = set_accent(hwnd, ACCENT_BLUR),
        WM_EXITSIZEMOVE if ACCENT.load(Ordering::Relaxed) => {
            // Going straight from blur back to acrylic keeps the blur; switching off in between doesn't.
            set_accent(hwnd, 0);
            set_accent(hwnd, ACCENT_ACRYLIC);
        }
        // Closing the window (not the last pane exiting) remembers the session.
        WM_CLOSE => save_last(),
        WM_ENDSESSION if wp.0 != 0 => save_last(),
        WM_TIMER if wp.0 == STATUS_TIMER => {
            sync_status(hwnd);
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == TOAST_TIMER => {
            let _ = KillTimer(Some(hwnd), TOAST_TIMER);
            with(|a| a.toast = None);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        // The settings page takes no mouse input, and none reaches the panes behind it.
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MOUSEWHEEL if with(|a| a.page.is_some()) => return LRESULT(0),
        // The landing page: hovering selects a choice, a click opens it.
        WM_MOUSEMOVE | WM_LBUTTONDOWN if with(|a| a.landing.is_some()) => {
            let m = mouse(hwnd);
            let renaming = with(|a| a.landing.as_ref().is_some_and(|l| l.rename.is_some()));
            // Only over the list itself: the page around it does nothing.
            let row = with(|a| {
                let l = a.landing.as_ref()?;
                let list_w = land_metrics(l, a.cell.0).1;
                if !(LAND_X - 12.0..=LAND_X + list_w).contains(&m.0) {
                    return None;
                }
                landing_row(m.1, land_line(a.cell.1), l.rows.len())
            });
            if msg == WM_MOUSEMOVE {
                with(|a| a.mouse = Some(m)); // for the icons' glow
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            if renaming {
                // A click anywhere while a name is typed keeps it, like Enter.
                if msg == WM_LBUTTONDOWN {
                    landing_key(hwnd, 0x0D);
                    with(|a| a.skip_char = false); // a click has no Enter character to skip
                }
                return LRESULT(0);
            }
            if let Some(i) = row {
                let icon = with(|a| {
                    let l = a.landing.as_ref()?;
                    land_icon(m.0, land_metrics(l, a.cell.0).1).filter(|_| l.rows[i].kind == Kind::Saved && l.sel == i)
                });
                if msg != WM_LBUTTONDOWN {
                    if with(|a| a.landing.as_mut().is_some_and(|l| std::mem::replace(&mut l.sel, i) != i)) {
                        with(|a| a.landing.as_mut().map(|l| l.calm()));
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                } else if let Some(delete) = icon {
                    with(if delete { press_delete } else { start_rename });
                    let _ = InvalidateRect(Some(hwnd), None, false);
                } else {
                    open_choice(hwnd, i);
                }
            }
            return LRESULT(0);
        }
        WM_RBUTTONDOWN | WM_MOUSEWHEEL if with(|a| a.landing.is_some()) => return LRESULT(0),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT && (mouse(hwnd).1 < 0.0 || with(|a| a.landing.is_some() || a.page.is_some())) => {
            let _ = SetCursor(LoadCursorW(None, IDC_ARROW).ok());
            return LRESULT(1);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if on_key(hwnd, wp.0 as u16, (lp.0 as u32 >> 16) & 0xFF) {
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            on_char(wp.0 as u16, false);
            let _ = InvalidateRect(Some(hwnd), None, false); // the selection is gone
            return LRESULT(0);
        }
        WM_SYSCHAR if wp.0 != ' ' as usize => {
            on_char(wp.0 as u16, true); // Alt+key sends ESC + key
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        // A lone Alt tap would enter menu mode and swallow the next key; Alt+Space still opens the system menu.
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_KEYMENU && lp.0 != ' ' as isize => return LRESULT(0),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let id = match (chrome_at(hwnd), divider_at(hwnd)) {
                ((Some(_), _), _) => IDC_HAND,
                ((_, true), _) => IDC_ARROW,
                (_, Some((_, true))) => IDC_SIZEWE,
                (_, Some((_, false))) => IDC_SIZENS,
                _ => IDC_IBEAM,
            };
            let _ = SetCursor(LoadCursorW(None, id).ok());
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => {
            if let (Some(i), _) = chrome_at(hwnd) {
                // A tab: no selection starts under it.
                with(|a| {
                    if let Some(&id) = a.order().get(i) {
                        a.focus_id(id);
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
            } else if let Some((path, _)) = divider_at(hwnd) {
                with(|a| a.drag = Some(path));
                SetCapture(hwnd);
            } else {
                // Click inside a pane: it gets the keyboard and a new selection (word on a double
                // click, line on a triple click) that follows the mouse until the button is released.
                let (m, t, limit) = (mouse(hwnd), GetMessageTime() as u32, GetDoubleClickTime());
                let started = with(|a| {
                    let i = pane_at(a, m)?;
                    a.focus = i;
                    a.panes.iter_mut().for_each(Pane::clear_selection);
                    a.click = (t, m.0, m.1, click_count(a.click, (t, m.0, m.1), limit));
                    let ty = match a.click.3 {
                        1 => SelectionType::Simple,
                        2 => SelectionType::Semantic,
                        _ => SelectionType::Lines,
                    };
                    let p = &a.panes[i];
                    let mut term = p.term.lock();
                    let (point, side) = p.point_at(&term, m, a.cell);
                    term.selection = Some(Selection::new(ty, point, side));
                    drop(term);
                    a.select = Some(p.id);
                    Some(())
                });
                if started.is_some() {
                    SetCapture(hwnd);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_MOUSEMOVE => {
            let ((x, y), (w, h)) = (mouse(hwnd), client(hwnd));
            let (moved, selecting, hover, track) = with(|a| {
                let before = a.mouse.and_then(|m| tab_at(a.view, a.panes.len(), m, w));
                a.mouse = Some((x, y));
                let hover = tab_at(a.view, a.panes.len(), (x, y), w) != before;
                let track = !std::mem::replace(&mut a.tracking, true);
                let mut dragged = false;
                if let (Some(path), Some(t)) = (&a.drag, &mut a.tree) {
                    t.drag(path, (0.0, 0.0, w, h), (x, y));
                    dragged = true;
                }
                let mut changed = false;
                if let Some(p) = a.select.and_then(|id| a.panes.iter().find(|p| p.id == id)) {
                    let mut term = p.term.lock();
                    let (point, side) = p.point_at(&term, (x, y), a.cell);
                    if let Some(s) = term.selection.as_mut() {
                        let before = s.clone();
                        s.update(point, side);
                        changed = *s != before;
                    }
                }
                (dragged, changed, hover, track)
            });
            if moved {
                layout(hwnd);
            } else if selecting || hover {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            if track {
                // Ask for WM_MOUSELEAVE, so the view button hides again when the mouse leaves.
                let size = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
                let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT { cbSize: size, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 });
            }
        }
        WM_MOUSELEAVE => {
            with(|a| {
                a.mouse = None;
                a.cap_hot = None;
                a.tracking = false;
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_LBUTTONUP => {
            // A click without dragging leaves no selection behind.
            with(|a| {
                let p = a.select.take().and_then(|id| a.panes.iter().find(|p| p.id == id));
                if let Some(mut term) = p.map(|p| p.term.lock()) {
                    if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                        term.selection = None;
                    }
                }
            });
            let _ = ReleaseCapture();
        }
        WM_RBUTTONDOWN => {
            // Right-click copies a selection, or pastes when there is none.
            let m = mouse(hwnd);
            if with(|a| pane_at(a, m).map(|i| a.focus = i).is_some()) && !copy(hwnd) {
                paste(hwnd);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            // lparam is in screen coordinates; the high word of wparam is the signed wheel delta.
            let delta = (wp.0 >> 16) as u16 as i16 as i32;
            let pt = POINT { x: lp.0 as u16 as i16 as i32, y: (lp.0 >> 16) as u16 as i16 as i32 };
            if modifiers().2 {
                zoom(hwnd, delta.signum()); // Ctrl+wheel: font size
            } else {
                wheel(hwnd, to_client(hwnd, pt), delta);
            }
            return LRESULT(0);
        }
        WM_CAPTURECHANGED => with(|a| {
            a.cap_down = None;
            a.drag = None;
            a.select = None;
        }),
        WM_DESTROY => PostQuitMessage(0),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn text_format(dw: &IDWriteFactory, weight: DWRITE_FONT_WEIGHT, size: f32) -> windows::core::Result<IDWriteTextFormat> {
    let f = dw.CreateTextFormat(FONT, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
    f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    Ok(f)
}

/// Small format for pane and tab names: aligned as asked, centered in the slot, cut with "…" when too long.
unsafe fn label_format(dw: &IDWriteFactory, size: f32, align: DWRITE_TEXT_ALIGNMENT) -> windows::core::Result<IDWriteTextFormat> {
    let f = text_format(dw, DWRITE_FONT_WEIGHT_NORMAL, size)?;
    f.SetTextAlignment(align)?;
    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, ..Default::default() };
    f.SetTrimming(&trimming, &dw.CreateEllipsisTrimmingSign(&f)?)?;
    Ok(f)
}

/// Size of one character cell in DIPs.
unsafe fn cell_size(dw: &IDWriteFactory, format: &IDWriteTextFormat) -> windows::core::Result<(f32, f32)> {
    let probe: Vec<u16> = "M".repeat(100).encode_utf16().collect();
    let layout = dw.CreateTextLayout(&probe, format, 10_000.0, 1_000.0)?;
    let mut m = DWRITE_TEXT_METRICS::default();
    layout.GetMetrics(&mut m)?;
    Ok((m.widthIncludingTrailingWhitespace / 100.0, m.height.ceil()))
}

unsafe fn fail(text: &str) {
    MessageBoxW(None, PCWSTR(wide(text).as_ptr()), w!("dterm"), MB_ICONERROR);
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let bash = git_bash();
        if !std::path::Path::new(&bash).exists() {
            return fail(&format!("Git Bash not found at {bash}"));
        }
        tty::setup_env(); // TERM=xterm-256color, COLORTERM=truecolor

        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).expect("Direct2D");
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DirectWrite");
        let format = text_format(&dw, DWRITE_FONT_WEIGHT_NORMAL, FONT_SIZE).expect("font");
        let bold = text_format(&dw, DWRITE_FONT_WEIGHT_BOLD, FONT_SIZE).expect("font");
        let label = label_format(&dw, LABEL_SIZE, DWRITE_TEXT_ALIGNMENT_TRAILING).expect("font");
        let tab = label_format(&dw, TAB_SIZE, DWRITE_TEXT_ALIGNMENT_CENTER).expect("font");
        let cell = cell_size(&dw, &format).expect("font metrics");
        let settings = data_dir().and_then(|d| std::fs::read_to_string(d.join("settings")).ok()).map_or_else(Settings::default, |t| parse_settings(&t));
        THEME.store(settings.theme, Ordering::Relaxed);

        let hinst = GetModuleHandleW(None).expect("module");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("dterm"),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(1 as _)).unwrap_or_default(), // from dterm.res
            ..Default::default()
        };
        RegisterClassW(&wc);
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                d2d, dw, format, bold, label, tab, cap_hot: None, cap_down: None, font_size: FONT_SIZE, cell, target: None, bash, panes: Vec::new(),
                focus: 0, next_id: 0, tree: None, drag: None, surrogate: 0, skip_char: false,
                select: None, click: (0, f32::MIN, f32::MIN, 0),
                home: std::env::var("USERPROFILE").ok(), rename: None, title: "dterm".into(),
                view: View::Grid, mouse: None, tracking: false, landing: None, prompt: None, toast: None,
                created: now(), settings, page: None,
            })
        });
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(), w!("dterm"), w!("dterm"), WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
            None, None, Some(hinst.into()), None,
        )
        .expect("window");

        // Dark title bar and acrylic behind the whole window; we paint the tint ourselves.
        let dark = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let backdrop = DWMSBT_TRANSIENTWINDOW;
        if DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as _, 4).is_ok() {
            let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 });
        } else {
            // Windows 10 and Windows 11 before 22H2 have no system backdrops. The extended frame
            // would hide the accent's acrylic behind black, so it is left out.
            ACCENT.store(set_accent(hwnd, ACCENT_ACRYLIC), Ordering::Relaxed);
            let flags = SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, flags); // WM_NCCALCSIZE drops the system frame
        }

        // With a last or saved session, start on the landing page; otherwise straight in a new one.
        let rows = data_dir().map(|d| landing_rows(&d, &ago)).unwrap_or_default();
        if rows.len() > 1 {
            with(|a| a.landing = Some(Landing::new(rows)));
        } else {
            add_pane(hwnd, None);
            if with(|a| a.panes.is_empty()) {
                return fail("Could not start Git Bash");
            }
        }
        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT); // honors a shortcut's "Run: Maximized"
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[test]
fn check() {
    use alacritty_terminal::vte::ansi::NamedColor;
    let colors = Colors::default();
    assert_eq!(indexed(16), Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(indexed(231), Rgb { r: 255, g: 255, b: 255 });
    assert_eq!(indexed(232), Rgb { r: 8, g: 8, b: 8 });
    assert_eq!(resolve(Color::Indexed(1), false, &colors), Some(rgb(0xE06C75)));
    assert_eq!(resolve(Color::Named(NamedColor::Background), false, &colors), None);
    assert_eq!(resolve(Color::Named(NamedColor::Black), true, &colors), Some(rgb(0x5A6374)));
    assert_eq!(key_seq(0x26, false, false, false, false).as_deref(), Some("\x1b[A"));
    assert_eq!(key_seq(0x26, false, false, false, true).as_deref(), Some("\x1bOA"));
    assert_eq!(key_seq(0x25, false, false, true, false).as_deref(), Some("\x1b[1;5D"));
    assert_eq!(key_seq(0x2E, false, false, false, false).as_deref(), Some("\x1b[3~"));
    assert_eq!(key_seq(0x09, true, false, false, false).as_deref(), Some("\x1b[Z"));
    assert_eq!(key_seq(0x09, false, false, false, false), None);
    assert_eq!(key_seq(0x41, false, false, false, false), None);
    assert_eq!(paste_text("a\r\nb\nc", false), "a\rb\rc");
    assert_eq!(paste_text("x\x1b[201~y", true), "\x1b[200~x[201~y\x1b[201~");
    // Pane status: busy while output is fresh, done once it stops, waiting after a bell; focus clears it.
    use Status::*;
    assert_eq!(next_status(Idle, false, false, Some(100)), Busy);
    assert_eq!(next_status(Busy, false, false, Some(BUSY_MS)), Done);
    assert_eq!(next_status(Done, false, false, Some(BUSY_MS + 1)), Done);
    assert_eq!(next_status(Done, false, false, Some(10)), Busy);
    assert_eq!(next_status(Idle, false, false, None), Idle);
    assert_eq!(next_status(Idle, false, false, Some(BUSY_MS + 1)), Idle); // old output is not news
    assert_eq!(next_status(Busy, false, true, Some(10)), Attention);
    assert_eq!(next_status(Attention, false, false, Some(10)), Attention); // sticks through output
    assert_eq!(next_status(Attention, true, false, Some(10)), Idle);
    assert_eq!(next_status(Done, true, true, None), Idle);
    // Text area: PAD all round, or a taller top margin where the name label sits.
    let pane = (10.0, 20.0, 1000.0, 600.0);
    assert_eq!(inner(pane, false), ((18.0, 28.0), (984.0, 584.0)));
    assert_eq!(inner(pane, true), ((18.0, 42.0), (984.0, 570.0)));
    assert_eq!(fit(inner(pane, false).1, (10.0, 20.0)), (98, 29));
    assert_eq!(fit(inner(pane, true).1, (10.0, 20.0)), (98, 28));
    assert_eq!(fit((0.0, 0.0), (10.0, 20.0)), (2, 1));
    // Font size: steps of one DIP, 0 resets, clamped to 8..=40.
    assert_eq!((zoomed(16.0, 1), zoomed(16.0, -1), zoomed(30.0, 0)), (17.0, 15.0, 16.0));
    assert_eq!((zoomed(40.0, 1), zoomed(8.0, -1)), (40.0, 8.0));
    // Pane names: Git Bash prompt titles report the directory, other titles are the app's.
    assert_eq!(prompt_cwd("MINGW64:/c/Users/me"), Some("/c/Users/me"));
    assert_eq!(prompt_cwd("UCRT64:~"), Some("~"));
    assert_eq!((prompt_cwd("Claude Code"), prompt_cwd("vim:/etc"), prompt_cwd("MINGW64:build")), (None, None, None));
    assert_eq!(win_path("/c/Users/me/my proj").as_deref(), Some(r"C:\Users\me\my proj"));
    assert_eq!((win_path("/d").as_deref(), win_path("/c/").as_deref()), (Some(r"D:\"), Some(r"C:\")));
    assert_eq!((win_path("/usr/bin"), win_path("~"), win_path("/")), (None, None, None));
    let home = Some(r"C:\Users\me");
    assert_eq!(folder_name("/usr/bin", None, home), "bin");
    assert_eq!(folder_name("/", None, home), "/");
    assert_eq!(folder_name("/c", Some(r"C:\"), home), "C:");
    assert_eq!(folder_name("/C/USERS/ME/", Some(r"C:\USERS\ME\"), home), "~");
    let mut n = Names::default();
    assert_eq!(pick_name(None, &n, home), None);
    n.set_title(Some("MINGW64:/c/Users/me/proj"));
    assert_eq!((n.cwd.as_deref(), n.win.as_deref()), (Some("/c/Users/me/proj"), Some(r"C:\Users\me\proj")));
    assert_eq!(pick_name(None, &n, home).as_deref(), Some("proj"));
    n.set_title(Some("\u{2733} Claude Code")); // the app's title beats the folder; a typed name beats both
    assert_eq!(pick_name(None, &n, home).as_deref(), Some("\u{2733} Claude Code"));
    assert_eq!(pick_name(Some("mine"), &n, home).as_deref(), Some("mine"));
    assert_eq!(n.cwd.as_deref(), Some("/c/Users/me/proj")); // an app title keeps the directory
    n.set_title(Some("MINGW64:/c/Users/me")); // back at the prompt: the app title is gone
    assert_eq!(pick_name(None, &n, home).as_deref(), Some("~"));
    n.set_title(Some("vim"));
    n.set_title(None);
    n.set_title(Some("  "));
    assert_eq!((n.app.as_deref(), pick_name(None, &n, home).as_deref()), (None, Some("~")));
    assert_eq!((window_title(None).as_str(), window_title(Some("proj")).as_str()), ("dterm", "dterm \u{2014} proj"));
    // Split tree in a 1000 x 600 window: pane 0 | (pane 1 over pane 2).
    let win = (0.0, 0.0, 1000.0, 600.0);
    let mk = || {
        let mut t = Node::Leaf(0);
        assert!(t.split(0, 1, true) && t.split(1, 2, false) && !t.split(9, 3, true));
        t
    };
    let ids = |t: Option<Node>| t.map(|t| t.rects(win).iter().map(|r| r.0).collect::<Vec<_>>());
    let t = mk();
    assert_eq!(
        t.rects(win),
        [(0, (0.0, 0.0, 497.0, 600.0)), (1, (503.0, 0.0, 497.0, 297.0)), (2, (503.0, 303.0, 497.0, 297.0))]
    );
    // Dividers are the gaps only; the path leads from the root (false = a, true = b).
    assert_eq!(t.divider_at(win, (500.0, 100.0)), Some((vec![], true)));
    assert_eq!(t.divider_at(win, (700.0, 300.0)), Some((vec![true], false)));
    assert_eq!(t.divider_at(win, (200.0, 100.0)), None);
    assert_eq!(t.divider_at(win, (700.0, 100.0)), None);
    assert_eq!(Node::Leaf(0).divider_at(win, (500.0, 100.0)), None);
    // Dragging moves one split's ratio, clamped.
    let mut t = mk();
    t.drag(&[true], win, (0.0, 450.0));
    assert_eq!(t.rects(win)[1].1, (503.0, 0.0, 497.0, 447.0));
    t.drag(&[], win, (250.0, 0.0));
    assert_eq!(t.rects(win)[0].1, (0.0, 0.0, 247.0, 600.0));
    t.drag(&[], win, (5000.0, 0.0));
    assert!((t.rects(win)[0].1 .2 - 947.0).abs() < 0.01);
    t.drag(&[], win, (-5000.0, 0.0));
    assert!((t.rects(win)[0].1 .2 - 47.0).abs() < 0.01);
    t.drag(&[true, true], win, (0.0, 0.0)); // path ends at a leaf: nothing happens
    // Alt+Shift+arrows: the nearest enclosing split with that orientation moves by `delta` DIPs.
    let mut t = mk();
    assert!(t.resize(2, win, false, -40.0)); // pane 2's own stacked split
    assert!((t.rects(win)[1].1 .3 - 257.0).abs() < 0.01);
    assert!(t.resize(1, win, true, 40.0)); // pane 1 has no side-by-side split below the root
    assert!((t.rects(win)[0].1 .2 - 537.0).abs() < 0.01);
    assert!(t.resize(2, win, true, 5000.0));
    assert!((t.rects(win)[0].1 .2 - 947.0).abs() < 0.01);
    assert!(!t.resize(0, win, false, 40.0)); // no stacked split above pane 0
    assert!(!t.resize(9, win, true, 40.0));
    assert!(!Node::Leaf(0).resize(0, win, true, 40.0));
    // Closing: the sibling's nearest pane is next in line; the sibling takes the room.
    let t = mk();
    assert_eq!((t.sibling(0), t.sibling(1), t.sibling(2), t.sibling(9)), (Some(1), Some(2), Some(1), None));
    assert_eq!(ids(mk().remove(2)), Some(vec![0, 1]));
    assert_eq!(ids(mk().remove(0)), Some(vec![1, 2]));
    assert_eq!(mk().remove(0).unwrap().rects(win), [(1, (0.0, 0.0, 1000.0, 297.0)), (2, (0.0, 303.0, 1000.0, 297.0))]);
    assert_eq!(ids(mk().remove(9)), Some(vec![0, 1, 2]));
    assert!(Node::Leaf(5).remove(5).is_none());
    assert_eq!(Node::Leaf(0).sibling(0), None);
    // Swapping exchanges the leaves.
    let mut t = mk();
    t.swap(0, 2);
    assert_eq!(ids(Some(t)), Some(vec![2, 1, 0]));
    // Views: tree order, toggling, where panes go, tab strip hit-testing.
    assert_eq!(mk().ids(), [0, 1, 2]);
    assert_eq!((cycle(3, 2, 1), cycle(3, 0, -1), cycle(3, 1, 0), cycle(1, 0, 1)), (0, 2, 1, 0));
    let (g, z, tb) = (View::Grid, View::Zoom, View::Tabs);
    assert_eq!((toggled(g, z), toggled(z, z), toggled(tb, z)), (z, g, tb));
    assert_eq!((toggled(g, tb), toggled(z, tb), toggled(tb, tb)), (tb, tb, g));
    let t = mk();
    assert_eq!(place(&t, g, 1, (1000.0, 600.0)), t.rects(win));
    assert_eq!(place(&t, z, 1, (1000.0, 600.0)), [(1, win)]);
    let content = (0.0, TAB_H, 1000.0, 600.0 - TAB_H); // the same area for every tab
    assert_eq!(place(&t, tb, 1, (1000.0, 600.0)), [(0, content), (1, content), (2, content)]);
    assert_eq!((tab_width(3, 970.0), tab_width(10, 970.0), tab_width(2, -5.0)), (TAB_MAX, 97.0, 0.0));
    // 3 tabs of 180 DIPs from the left; nothing else in the strip, and no tabs in the grid.
    let hit = |view, x, y| tab_at(view, 3, (x, y), 1000.0);
    assert_eq!((hit(tb, 10.0, 10.0), hit(tb, 370.0, 29.0), hit(tb, 539.0, 0.0)), (Some(0), Some(2), Some(2)));
    assert_eq!((hit(tb, 541.0, 10.0), hit(tb, 10.0, 30.0), hit(tb, -1.0, 10.0), hit(tb, 990.0, 10.0)), (None, None, None, None));
    assert_eq!((hit(g, 10.0, 10.0), hit(z, 10.0, 10.0)), (None, None));
    // Ctrl+Shift+T: the largest pane (the focused one on a tie), along its longer side.
    let p = [(0.0, 0.0, 497.0, 600.0), (503.0, 0.0, 497.0, 297.0), (503.0, 303.0, 497.0, 297.0)];
    assert_eq!((auto_split(&p, 1), auto_split(&p, 2)), ((0, false), (0, false)));
    assert_eq!(auto_split(&p[1..], 1), (1, true));
    assert_eq!(auto_split(&p[..1], 0), (0, false));
    assert_eq!(auto_split(&[(0.0, 0.0, 1000.0, 600.0)], 0), (0, true));
    assert_eq!(auto_split(&[], 0), (0, true));
    // 3 panes: top-left, top-right, bottom spanning both.
    let r = [(0.0, 0.0, 497.0, 297.0), (503.0, 0.0, 497.0, 297.0), (0.0, 303.0, 1000.0, 297.0)];
    assert_eq!(neighbor(&r, 0, (1, 0)), Some(1));
    assert_eq!(neighbor(&r, 1, (-1, 0)), Some(0));
    assert_eq!(neighbor(&r, 0, (0, 1)), Some(2));
    assert_eq!(neighbor(&r, 1, (0, 1)), Some(2));
    assert_eq!(neighbor(&r, 2, (0, -1)), Some(0));
    assert_eq!(neighbor(&r, 1, (1, 0)), None);
    assert_eq!(neighbor(&r, 2, (0, 1)), None);
    // 2x3 grid: moving right from the top-left stays in the top row.
    let g = [(0.0, 0.0, 330.0, 297.0), (336.0, 0.0, 330.0, 297.0), (672.0, 0.0, 328.0, 297.0),
             (0.0, 303.0, 330.0, 297.0), (336.0, 303.0, 330.0, 297.0)];
    assert_eq!(neighbor(&g, 0, (1, 0)), Some(1));
    assert_eq!(neighbor(&g, 4, (0, -1)), Some(1));
    assert_eq!(neighbor(&[], 0, (1, 0)), None);
    // Pixel to grid: text at (8, 8), 10 x 20 cells, 80 x 24 grid, scrolled back 3 lines.
    let at = |x, y| locate((x, y), (8.0, 8.0), (10.0, 20.0), (80, 24), 3);
    assert_eq!(at(33.0, 53.0), (Point::new(Line(-1), Column(2)), Side::Right));
    assert_eq!(at(32.0, 53.0), (Point::new(Line(-1), Column(2)), Side::Left));
    assert_eq!(at(0.0, 0.0), (Point::new(Line(-3), Column(0)), Side::Left));
    assert_eq!(at(5000.0, 5000.0), (Point::new(Line(20), Column(79)), Side::Right));
    // Click counting: 1 -> 2 -> 3 -> 1; too slow or too far starts over.
    assert_eq!(click_count((0, f32::MIN, f32::MIN, 0), (1000, 50.0, 50.0), 500), 1);
    assert_eq!(click_count((1000, 50.0, 50.0, 1), (1200, 51.0, 52.0), 500), 2);
    assert_eq!(click_count((1000, 50.0, 50.0, 2), (1200, 50.0, 50.0), 500), 3);
    assert_eq!(click_count((1000, 50.0, 50.0, 3), (1200, 50.0, 50.0), 500), 1);
    assert_eq!(click_count((1000, 50.0, 50.0, 1), (1600, 50.0, 50.0), 500), 1);
    assert_eq!(click_count((1000, 50.0, 50.0, 1), (1200, 60.0, 50.0), 500), 1);
    assert_eq!(click_count((u32::MAX - 100, 50.0, 50.0, 1), (100, 50.0, 50.0), 500), 2); // timer wrapped
    // Selection caret: 10 columns, lines -5..=23, line 24 column 0 is the very end.
    let p = |l, c| Point::new(Line(l), Column(c));
    assert_eq!(caret_move(p(0, 3), (-1, 0), 10, -5, 23), p(0, 2));
    assert_eq!(caret_move(p(0, 0), (-1, 0), 10, -5, 23), p(-1, 9));
    assert_eq!(caret_move(p(0, 9), (1, 0), 10, -5, 23), p(1, 0));
    assert_eq!(caret_move(p(0, 3), (0, -1), 10, -5, 23), p(-1, 3));
    assert_eq!(caret_move(p(0, 3), (0, 1), 10, -5, 23), p(1, 3));
    assert_eq!(caret_move(p(-5, 0), (-1, 0), 10, -5, 23), p(-5, 0));
    assert_eq!(caret_move(p(23, 4), (0, 1), 10, -5, 23), p(24, 0));
    assert_eq!(caret_move(p(24, 0), (1, 0), 10, -5, 23), p(24, 0));
    assert_eq!(caret_move(p(24, 0), (-1, 0), 10, -5, 23), p(23, 9));
    // Wheel: lines and what the app receives.
    assert_eq!((wheel_lines(120), wheel_lines(-240), wheel_lines(30), wheel_lines(-30)), (3, -6, 1, -1));
    let (sgr, legacy) = (TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE, TermMode::MOUSE_DRAG);
    assert_eq!(wheel_input(sgr, 120, (4, 9)).as_deref(), Some(&b"\x1b[<64;5;10M"[..]));
    assert_eq!(wheel_input(sgr, -240, (4, 9)).as_deref(), Some(&b"\x1b[<65;5;10M\x1b[<65;5;10M"[..]));
    assert_eq!(wheel_input(legacy, 120, (0, 0)).as_deref(), Some(&[0x1b, b'[', b'M', 96, 33, 33][..]));
    assert_eq!(wheel_input(legacy, -120, (300, 1)).as_deref(), Some(&[0x1b, b'[', b'M', 97, 255, 34][..]));
    let alt = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
    assert_eq!(wheel_input(alt, 120, (0, 0)).as_deref(), Some(&b"\x1b[A\x1b[A\x1b[A"[..]));
    assert_eq!(wheel_input(alt | TermMode::APP_CURSOR, -120, (0, 0)).as_deref(), Some(&b"\x1bOB\x1bOB\x1bOB"[..]));
    assert_eq!(wheel_input(alt | sgr, 120, (0, 0)).as_deref(), Some(&b"\x1b[<64;1;1M"[..])); // mouse mode wins
    assert_eq!(wheel_input(TermMode::ALT_SCREEN, 120, (0, 0)), None);
    assert_eq!(wheel_input(TermMode::ALTERNATE_SCROLL, 120, (0, 0)), None);
    assert_eq!(wheel_input(TermMode::NONE, 120, (0, 0)), None);
    assert_eq!(clip_units("a\nb"), [97, 13, 10, 98, 0]);
    // Sessions: pane 0 | (pane 1 over pane 2), round trip through the file format.
    let tree = Node::Split {
        side_by_side: true,
        ratio: 0.25,
        a: Box::new(Node::Leaf(0)),
        b: Box::new(Node::Split { side_by_side: false, ratio: 0.5, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) }),
    };
    let pane = |cwd: &str, name: &str, run: &str| SavedPane { cwd: cwd.into(), name: name.into(), run: run.into() };
    let panes = vec![pane("C:\\a b", "build", ""), pane("", "", "claude --resume 0a1b-2c"), pane("D:\\x", "", "")];
    let s = Session { view: View::Tabs, font: 18.0, focus: 2, tree, panes, created: 1_700_000_000, closed: 1_700_003_600 };
    let text = session_text(&s);
    assert!(text.starts_with(
        "dterm-session 1\nview tabs\nfont 18\nfocus 2\ncreated 1700000000\nclosed 1700003600\nsplit side 0.250\npane C:\\a b\tbuild\n"
    ));
    assert!(text.contains("pane \t\tclaude --resume 0a1b-2c\n")); // the command only when there is one
    assert_eq!(parse_session(&text), Some(s));
    assert!(parse_session(&text.replace("\n", "\r\n")).is_some()); // Notepad line breaks
    // Leaves get renumbered in tree order, whatever their ids were.
    let odd = Node::Split { side_by_side: true, ratio: 0.5, a: Box::new(Node::Leaf(7)), b: Box::new(Node::Leaf(3)) };
    assert_eq!(odd.numbered(&mut 0).ids(), vec![0, 1]);
    // Anything malformed is refused, never a panic.
    let head = "dterm-session 1\nview grid\nfont 16\nfocus 0\n";
    for bad in ["", "dterm-session 2\n", head, &format!("{head}split side 0.5\npane a\t\n"), &format!("{head}split up 0.5\n"),
        &format!("{head}split side x\npane a\t\npane b\t\n"), &format!("{head}pane a\t\npane b\t\n"), "view grid\nfont 16\n"] {
        assert!(parse_session(bad).is_none(), "{bad:?}");
    }
    let one = parse_session(&format!("{head}pane \t\n")).unwrap();
    assert_eq!((one.tree, one.panes), (Node::Leaf(0), vec![SavedPane::default()]));
    assert_eq!(parse_session("dterm-session 1\nview grid\nfont 99\nfocus 9\npane a\t\n").map(|s| (s.font, s.focus)), Some((40.0, 0)));
    // Saved-session file names keep letters, digits, space, - and _.
    assert_eq!(session_file_name("  my: work/agents?  "), Some("my workagents".into()));
    assert_eq!(session_file_name("../.."), None);
    assert_eq!(session_file_name("a_b-c 2"), Some("a_b-c 2".into()));
    // Landing page rows: choices start two lines below LAND_Y.
    assert_eq!(landing_row(LAND_ROWS + 1.0, 20.0, 3), Some(0));
    assert_eq!(landing_row(LAND_ROWS + 2.0 * 20.0 + 1.0, 20.0, 3), Some(2));
    assert_eq!(landing_row(LAND_ROWS + 3.0 * 20.0 + 1.0, 20.0, 3), None);
    assert_eq!(landing_row(LAND_ROWS - 1.0, 20.0, 3), None);
    // Landing page icons sit at the right end of the list.
    assert_eq!(land_icon(LAND_X + 100.0 - 2.0 * ICON_W + 1.0, 100.0), Some(false));
    assert_eq!(land_icon(LAND_X + 100.0 - ICON_W + 1.0, 100.0), Some(true));
    assert_eq!(land_icon(LAND_X + 100.0 - 2.0 * ICON_W - 1.0, 100.0), None);
    assert_eq!(land_icon(LAND_X + 100.0 + 1.0, 100.0), None);
    // Renaming and deleting saved sessions touches only their files.
    let dir = std::env::temp_dir().join(format!("dterm-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let saved = |name: &str| {
        let f = dir.join(format!("{name}.session"));
        std::fs::write(&f, session_text(&parse_session("dterm-session 1\nview grid\nfont 16\nfocus 0\npane a\t\n").unwrap())).unwrap();
        choice(name.into(), parse_session(&std::fs::read_to_string(&f).unwrap()).unwrap(), Some(f), Kind::Saved, &ago)
    };
    let mut rows = vec![landing_rows(&dir, &ago).remove(0)];
    rows.extend(["alpha", "beta", "gamma"].map(saved));
    let mut l = Landing::new(rows);
    l.sel = 0;
    assert!(!l.saved() && l.rename_to("x").is_err() && !l.delete()); // "New session" is neither
    l.sel = 2; // beta
    assert_eq!(l.rename_to("alpha"), Err("a session with that name exists"));
    assert_eq!(l.rename_to("?:"), Err("a name needs letters or digits"));
    assert_eq!(l.rename_to("zeta"), Ok(()));
    assert!(dir.join("zeta.session").exists() && !dir.join("beta.session").exists());
    assert_eq!((l.rows[l.sel].label.as_str(), l.sel), ("zeta", 3)); // sorted after gamma, still selected
    assert_eq!(l.rename_to("ZETA"), Ok(())); // only the letter case changes
    assert!(l.delete() && !dir.join("ZETA.session").exists() && !dir.join("zeta.session").exists());
    assert_eq!((l.rows.len(), l.sel), (3, 2)); // selection stays on the last row
    let _ = std::fs::remove_dir_all(&dir);
    // Files from before the timestamps still open, with unknown times.
    let old = parse_session("dterm-session 1\nview grid\nfont 16\nfocus 0\npane C:\\x\t\n").unwrap();
    assert_eq!((old.created, old.closed), (0, 0));
    assert!(parse_session("\u{feff}dterm-session 1\nview grid\nfont 16\nfocus 0\npane a\t\n").is_some()); // BOM
    // Times in words.
    let (today, yesterday) = ((2026, 10, 16), (2026, 10, 15));
    assert_eq!(when((2026, 10, 16, 9, 5), today, yesterday), "today 09:05");
    assert_eq!(when((2026, 10, 15, 23, 59), today, yesterday), "yesterday 23:59");
    assert_eq!(when((2026, 3, 1, 14, 2), today, yesterday), "1 Mar 14:02");
    assert_eq!(when((2025, 12, 31, 14, 2), today, yesterday), "31 Dec 2025");
    assert_eq!(ago(0), "");
    // Preview names: the typed name, else the folder, "~" for home or unknown.
    let home = Some("C:\\Users\\x");
    assert_eq!(preview_name(&pane("C:\\Users\\x\\dterm", "", ""), home), "dterm");
    assert_eq!(preview_name(&pane("C:\\Users\\X\\", "", ""), home), "~");
    assert_eq!(preview_name(&SavedPane::default(), home), "~");
    assert_eq!(preview_name(&pane("C:\\a", "build", ""), home), "build");
    assert_eq!(preview_caption(true, "", ""), "a fresh shell in your home folder");
    assert_eq!(preview_caption(false, "", ""), ""); // old file without times
    assert_eq!(preview_caption(false, "today 09:00", "today 10:00"), "started today 09:00  \u{b7}  closed today 10:00");
    // The console's first title (the shell's path) is not an app name.
    let mut n = Names::default();
    n.set_title(Some("C:\\Program Files\\Git\\bin\\bash.exe"));
    assert_eq!(n.app, None);
    n.set_title(Some("Claude Code"));
    assert_eq!(n.app.as_deref(), Some("Claude Code"));
    assert_eq!(pane_count(1), "1 pane");
    // Only commands dterm saves itself are run from a session file.
    assert!(resume_command("claude") && resume_command("claude --resume 3f2a-9c"));
    assert!(!resume_command("claude --resume ") && !resume_command("claude --resume x; rm -rf ~") && !resume_command("vim"));
    // Shortcuts: names, parsing, the keypad's + and -, and Ctrl or Alt required.
    let ca = |vk| Chord { vk, ctrl: true, alt: true, shift: false };
    assert_eq!(parse_chord("Ctrl+Alt+T"), Some(ca(0x54)));
    assert_eq!(parse_chord(" ctrl+alt+plus "), Some(ca(0xBB)));
    assert_eq!(parse_chord("Alt+Shift+F5"), Some(Chord { vk: 0x74, ctrl: false, alt: true, shift: true }));
    assert_eq!((parse_chord("Shift+T"), parse_chord("T"), parse_chord("Ctrl+Up"), parse_chord("Meta+T")), (None, None, None, None));
    assert_eq!((chord_text(ca(0xBC)), chord_text(ca(0x37))), ("Ctrl+Alt+Comma".into(), "Ctrl+Alt+7".into()));
    assert_eq!((chord_of(0x6B, false, true, true), chord_of(0x6D, false, true, true)), (ca(0xBB), ca(0xBD)));
    for a in ACTIONS {
        assert_eq!(parse_chord(a.3).map(chord_text).as_deref(), Some(a.3)); // every default reads back
    }
    // Settings: defaults, binding moves keys, the file round trip, bad lines keep defaults.
    let mut st = Settings::default();
    assert_eq!(st.action(ca(0x54)), Some(Action::NewPane));
    assert_eq!(st.action(ca(0xBD)), Some(Action::SplitDown));
    assert_eq!(st.action(Chord { vk: 0x54, ctrl: true, alt: false, shift: true }), None); // Ctrl+Shift+T is the shell's now
    st.bind(Action::Zoom as usize, Some(ca(0x54)));
    assert_eq!((st.action(ca(0x54)), st.keys[Action::NewPane as usize]), (Some(Action::Zoom), None));
    st.theme = 2;
    st.opacity = 0.85;
    assert_eq!(parse_settings(&settings_text(&st)), st);
    assert!(settings_text(&st).contains("key new-pane none\n"));
    let odd = parse_settings("\u{feff}theme nope\nopacity 5\nkey zoom Shift+Q\nkey nope Ctrl+Alt+Q\njunk\n");
    assert_eq!((odd.theme, odd.opacity, odd.keys[Action::Zoom as usize]), (0, MIN_OPACITY, None));
    assert_eq!(parse_settings(""), Settings::default());
    // Word steps over "ab  cd" (cells 0..6): right to the end of a word, left to its start.
    let words = |i: i64| !matches!(i, 2 | 3);
    assert_eq!((word_move(0, true, 6, words), word_move(2, true, 6, words), word_move(6, true, 6, words)), (2, 6, 6));
    assert_eq!((word_move(6, false, 6, words), word_move(4, false, 6, words), word_move(1, false, 6, words)), (4, 0, 0));
    // Processes: Claude Code under Git Bash's launcher and its bash; loops from reused pids end.
    let procs = [(10, 1, "bash.exe".into()), (11, 10, "bash.exe".into()), (12, 11, "claude.exe".into()), (20, 1, "Claude.exe".into())];
    assert_eq!((descendant(&procs, 10, "claude.exe"), descendant(&procs, 11, "CLAUDE.EXE"), descendant(&procs, 12, "claude.exe")), (Some(12), Some(12), None));
    assert_eq!(descendant(&[(5, 6, "a".into()), (6, 5, "b".into())], 5, "c"), None);
    let dir = claude_project("N:\\Davide Chincoli\\12_CODING\\dterm").unwrap();
    assert_eq!(dir.file_name().unwrap(), "N--Davide-Chincoli-12-CODING-dterm");
    // Own frame on Windows 10: edges resize, the caption drags except over its buttons.
    let hit = |x, y, border| frame_hit((x, y), (800, 600), border, 32, (138, 138));
    assert_eq!((hit(0, 0, 6), hit(799, 0, 6), hit(0, 599, 6), hit(799, 599, 6)), (HTTOPLEFT, HTTOPRIGHT, HTBOTTOMLEFT, HTBOTTOMRIGHT));
    assert_eq!((hit(2, 300, 6), hit(797, 300, 6), hit(400, 2, 6), hit(400, 597, 6)), (HTLEFT, HTRIGHT, HTTOP, HTBOTTOM));
    assert_eq!((hit(400, 20, 6), hit(700, 20, 6), hit(400, 40, 6), hit(400, 2, 0), hit(0, 300, 0)), (HTCAPTION, HTCLIENT, HTCLIENT, HTCAPTION, HTCLIENT));
    assert_eq!((hit(50, 20, 6), hit(137, 20, 6), hit(138, 20, 6)), (HTCLIENT, HTCLIENT, HTCAPTION));
    let at = |x, session| caption_button(x, 800.0, session);
    assert_eq!((at(799.0, true), at(753.0, true), at(754.0, true), at(662.0, true), at(661.0, true), at(800.0, true)), (Some(4), Some(3), Some(4), Some(2), None, None));
    assert_eq!((at(0.0, true), at(45.0, true), at(46.0, true), at(92.0, true), at(138.0, true)), (Some(5), Some(5), Some(0), Some(1), None));
    assert_eq!((at(10.0, false), at(60.0, false)), (Some(5), None));
    assert_eq!((caption_x(5, 800.0), caption_x(0, 800.0), caption_x(1, 800.0), caption_x(2, 800.0), caption_x(4, 800.0)), (0.0, 46.0, 92.0, 662.0, 754.0));
    // Tooltips read the current shortcuts.
    let mut keys = Settings::default().keys;
    assert_eq!(caption_tip(5, View::Grid, false, &keys), ("Settings", Some("Ctrl+Alt+Comma".into())));
    keys[Action::Tabs as usize] = parse_chord("Ctrl+Alt+Y");
    assert_eq!(caption_tip(1, View::Grid, false, &keys), ("Tabs view", Some("Ctrl+Alt+Y".into())));
    keys[Action::Home as usize] = None;
    assert_eq!(caption_tip(0, View::Grid, false, &keys), ("Start page", None));
    assert_eq!(caption_tip(3, View::Grid, true, &keys), ("Restore", None));
}

#[test]
#[ignore] // starts Git Bash: cargo test -- --ignored
fn typeahead_reaches_bash() {
    let bash = unsafe { git_bash() };
    let p = spawn(HWND(std::ptr::null_mut()), 0, &bash, (8.0, 16.0), 1.0, None).expect("bash");
    p.send(b"echo dterm_ok_$((40+2))\r".to_vec());
    let found = (0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let t = p.term.lock();
        (0..t.screen_lines() as i32).any(|l| {
            let row = &t.grid()[Line(l)];
            (0..t.columns()).map(|c| row[Column(c)].c).collect::<String>().contains("dterm_ok_42")
        })
    });
    let _ = p.tx.send(Msg::Shutdown);
    assert!(found, "the typed-ahead command never ran");
}
