#![cfg_attr(not(test), windows_subsystem = "windows")]
//! dterm: Git Bash panes in a tree of splits, drawn with Direct2D/DirectWrite on an acrylic window.
//! Terminal emulation and ConPTY come from alacritty_terminal.
//! Ctrl+Shift+T splits the largest pane, Alt+Shift+Plus / Alt+Shift+Minus split the focused one
//! side by side / top and bottom, Ctrl+Shift+W (or `exit`) closes a pane. Alt+arrows move the focus,
//! Alt+Shift+arrows resize, Ctrl+Alt+arrows swap panes, drag the gaps to resize.
//! Ctrl+V pastes plain text. Select with the mouse or Shift+arrows,
//! copy with Ctrl+C (or right-click), the wheel scrolls back.
//! With several panes each shows a name (typed, else the app's title, else its folder);
//! Ctrl+Shift+R renames the focused one (Enter confirms, Esc cancels, empty resets).
//! Ctrl+Plus / Ctrl+Minus / Ctrl+wheel resize the font, Ctrl+0 resets it.
//! Ctrl+Shift+Z zooms the focused pane to the whole window (again to go back); the other panes keep
//! running. Ctrl+Tab / Ctrl+Shift+Tab move the focus to the next / previous pane.
//! Ctrl+Shift+G (or the button in the top-right corner, shown when the mouse is near the top edge)
//! shows the panes as tabs in a strip instead; click a tab, press Ctrl+Tab or Alt+Left/Right to switch,
//! and use the button at the right end of the strip (or Ctrl+Shift+G) to get the grid back.

use std::{
    borrow::Cow,
    cell::RefCell,
    sync::{Arc, Mutex, OnceLock},
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
    core::{w, BOOL, PCWSTR},
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
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
            Registry::*,
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
const OPACITY: f32 = 0.7; // pane background over the acrylic, like your Windows Terminal profile
const WM_WAKE: u32 = WM_APP; // new terminal output
const WM_EXIT: u32 = WM_APP + 1; // a shell exited; wparam = pane id
const SLOP: f32 = 4.0; // DIPs the mouse may move between the clicks of a double-click

// One Half Dark, as in Windows Terminal. ANSI order: black red green yellow blue magenta cyan white.
const FG: u32 = 0xDCDFE4;
const BG: u32 = 0x282C34;
const GAP_COLOR: u32 = 0x1B1D23;
const SELECTION_ALPHA: f32 = 0.25; // foreground veil over selected cells
const FOCUS_ALPHA: f32 = 0.15; // foreground border of the focused pane
const VEIL_ALPHA: f32 = 0.25; // background veil over unfocused panes
const LABEL_ALPHA: f32 = 0.4; // pane name; brighter on the focused pane
const LABEL_FOCUS_ALPHA: f32 = 0.7;
const TAB_H: f32 = 30.0; // DIPs, height of the tab strip; the view button is this square at its right end
const TAB_MAX: f32 = 180.0; // widest tab; they shrink to share the strip when there are many
const TAB_PAD: f32 = 10.0; // DIPs between a tab's edge and its text
const TAB_SIZE: f32 = 12.0; // DIPs
const TAB_ACTIVE_ALPHA: f32 = 0.9; // name of the active tab; inactive ones use LABEL_ALPHA, hovered LABEL_FOCUS_ALPHA
const UNDERLINE_ALPHA: f32 = 0.6; // underline of the active tab
const REVEAL: f32 = 36.0; // DIPs from the top edge where the view button shows up in the grid
const PALETTE: [u32; 16] = [
    0x282C34, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xDCDFE4,
    0x5A6374, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xDCDFE4,
];

fn rgb(c: u32) -> Rgb {
    Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 }
}

fn dim(c: Rgb) -> Rgb {
    Rgb { r: c.r / 3 * 2, g: c.g / 3 * 2, b: c.b / 3 * 2 }
}

/// xterm 256-color table.
fn indexed(i: u8) -> Rgb {
    match i {
        0..=15 => rgb(PALETTE[i as usize]),
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
        268 => Some(dim(rgb(FG))),                        // dim foreground
        _ => Some(rgb(FG)),                               // foreground, cursor, bright foreground
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

/// Part of the window chrome under the mouse.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Tab(usize),
    Button, // switches between the grid and the tabs
}

/// What `m` is over: a tab (in Tabs view) or the view button, the `TAB_H` square in the top-right
/// corner of a `width` DIPs wide window.
fn chrome_hit(view: View, tabs: usize, (x, y): (f32, f32), width: f32) -> Option<Hit> {
    if !(0.0..TAB_H).contains(&y) {
        return None;
    }
    if (width - TAB_H..width).contains(&x) {
        return Some(Hit::Button);
    }
    let tw = tab_width(tabs, width - TAB_H);
    (view == View::Tabs && tw > 0.0 && (0.0..tw * tabs as f32).contains(&x)).then(|| Hit::Tab((x / tw) as usize))
}

/// What the mouse at `m` (`None` = outside the window) means for the chrome: whether it is near
/// the top edge, which reveals the view button in the grid, and what it is over.
fn hover_state(view: View, tabs: usize, m: Option<(f32, f32)>, width: f32) -> (bool, Option<Hit>) {
    m.map_or((false, None), |m| (m.1 < REVEAL, chrome_hit(view, tabs, m, width)))
}

/// Terminal columns and lines that fit in `(w, h)` DIPs of text area.
fn fit((w, h): (f32, f32), cell: (f32, f32)) -> (usize, usize) {
    ((w / cell.0).floor().max(2.0) as usize, (h / cell.1).floor().max(1.0) as usize)
}

/// Font size after a zoom step: one DIP bigger (+1), smaller (-1), or the default (0). Clamped to 8..=40.
fn zoomed(size: f32, step: i32) -> f32 {
    if step == 0 { FONT_SIZE } else { (size + step as f32).clamp(8.0, 40.0) }
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

/// Forwards terminal events from a pane's PTY thread to the window.
#[derive(Clone)]
struct Proxy {
    hwnd: isize,
    id: usize,
    tx: Arc<OnceLock<EventLoopSender>>, // set right after the event loop exists
    names: Arc<Mutex<Names>>,
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
            Event::Wakeup => post(WM_WAKE, 0),
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

    /// The pane's label, if it has a name yet.
    fn name(&self, home: Option<&str>) -> Option<String> {
        let names = self.names.lock().ok()?;
        pick_name(self.custom.as_deref(), &names, home)
    }

    /// Shift+arrows: move the selection's free end by a character or a line, starting at the cursor.
    // ponytail: the end is not rotated if output scrolls the terminal meanwhile; fine while typing at a prompt.
    fn extend_selection(&mut self, d: (i32, i32)) {
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
        let head = caret_move(head, d, cols, top, bottom);
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
    tab: IDWriteTextFormat,   // tab names
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

    /// Pane ids in tree order, which is also the tab order.
    fn order(&self) -> Vec<usize> {
        self.tree.as_ref().map(Node::ids).unwrap_or_default()
    }

    fn focus_id(&mut self, id: usize) {
        if let Some(i) = self.panes.iter().position(|p| p.id == id) {
            self.focus = i;
        }
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

fn window_size((cols, lines): (usize, usize), cell: (f32, f32), scale: f32) -> WindowSize {
    WindowSize {
        num_lines: lines as u16,
        num_cols: cols as u16,
        cell_width: (cell.0 * scale) as u16,
        cell_height: (cell.1 * scale) as u16,
    }
}

fn spawn(hwnd: HWND, id: usize, bash: &str, cell: (f32, f32), scale: f32) -> std::io::Result<Pane> {
    let size = (80, 24); // layout() fits it to the pane right away
    let opts = tty::Options {
        shell: Some(tty::Shell::new(bash.into(), vec!["--login".into(), "-i".into()])),
        working_directory: std::env::var_os("USERPROFILE").map(Into::into),
        drain_on_exit: false,
        env: Default::default(),
        escape_args: true,
    };
    let pty = tty::new(&opts, window_size(size, cell, scale), 0)?;
    let slot = Arc::new(OnceLock::new());
    let names = Arc::new(Mutex::new(Names::default()));
    let proxy = Proxy { hwnd: hwnd.0 as isize, id, tx: slot.clone(), names: names.clone() };
    let term = Arc::new(FairMutex::new(Term::new(Config::default(), &TermSize::new(size.0, size.1), proxy.clone())));
    let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)?;
    let tx = event_loop.channel();
    let _ = slot.set(tx.clone());
    event_loop.spawn();
    Ok(Pane { id, term, tx, size, rect: (0.0, 0.0, 0.0, 0.0), origin: (PAD, PAD), head: None, names, custom: None })
}

unsafe fn scale(hwnd: HWND) -> f32 {
    GetDpiForWindow(hwnd) as f32 / 96.0
}

unsafe fn client_px(hwnd: HWND) -> (u32, u32) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    (rc.right as u32, rc.bottom as u32)
}

/// Client size in DIPs.
unsafe fn client(hwnd: HWND) -> (f32, f32) {
    let (w, h) = client_px(hwnd);
    let s = scale(hwnd);
    (w as f32 / s, h as f32 / s)
}

/// Screen point in client DIPs.
unsafe fn to_client(hwnd: HWND, mut pt: POINT) -> (f32, f32) {
    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
    let s = scale(hwnd);
    (pt.x as f32 / s, pt.y as f32 / s)
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
        let Ok(p) = spawn(hwnd, id, &a.bash, a.cell, s) else { return false };
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
        Some(0) => {
            let _ = DestroyWindow(hwnd);
        }
        Some(_) => layout(hwnd),
        None => {}
    }
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
            let Ok(brush) = rt.CreateSolidColorBrush(&color(rgb(FG), 1.0), None) else { return };
            a.target = Some((rt, brush));
        }
        let (rt, brush) = a.target.as_ref().unwrap();
        rt.BeginDraw();
        rt.Clear(Some(&color(rgb(GAP_COLOR), 0.9)));
        let (several, labeled) = (a.view == View::Grid && a.panes.len() > 1, a.labeled());
        for (i, p) in a.panes.iter().enumerate().filter(|&(i, _)| a.is_shown(i)) {
            let (x, y, pw, ph) = p.rect;
            let (r, focused) = (rect(x, y, pw, ph), i == a.focus);
            // Clear() inside the clip replaces the pixels, so the pane gets its own see-through tint.
            rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            rt.Clear(Some(&color(rgb(BG), OPACITY)));
            draw_term(rt, brush, &a.format, &a.bold, a.cell, p.origin, focused, &p.term.lock());
            // Several panes: a faint border marks the focused one, a veil dims the others.
            if several && focused {
                brush.SetColor(&color(rgb(FG), FOCUS_ALPHA));
                rt.DrawRectangle(&rect(x + 0.5, y + 0.5, pw - 1.0, ph - 1.0), brush, 1.0, None);
            } else if several {
                brush.SetColor(&color(rgb(BG), VEIL_ALPHA));
                rt.FillRectangle(&r, brush);
            }
            if labeled {
                let (text, alpha) = match a.rename.as_ref().filter(|r| r.0 == p.id) {
                    Some((_, typed)) => (format!("{typed}\u{258f}"), 1.0), // the name being typed, with a caret
                    None => (p.name(a.home.as_deref()).unwrap_or_default(), if focused { LABEL_FOCUS_ALPHA } else { LABEL_ALPHA }),
                };
                brush.SetColor(&color(rgb(FG), alpha));
                let units: Vec<u16> = text.encode_utf16().collect();
                let slot = rect(x + PAD, y, pw - 2.0 * PAD, LABEL_TOP);
                rt.DrawText(&units, &a.label, &slot, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
            rt.PopAxisAlignedClip();
        }
        draw_chrome(a, rt, brush, w as f32 / s);
        if rt.EndDraw(None, None).is_err() {
            a.target = None; // device lost: recreate on the next paint
        }
    });
}

/// The view button, and in Tabs view the strip with a tab per pane, over the panes.
unsafe fn draw_chrome(a: &App, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush, width: f32) {
    let tabs = a.view == View::Tabs;
    let (near, hit) = hover_state(a.view, a.panes.len(), a.mouse, width);
    if tabs {
        // The strip has the same tint as the panes, so the tabs sit right on the acrylic.
        rt.PushAxisAlignedClip(&rect(0.0, 0.0, width, TAB_H), D2D1_ANTIALIAS_MODE_ALIASED);
        rt.Clear(Some(&color(rgb(BG), OPACITY)));
        rt.PopAxisAlignedClip();
        let order = a.order();
        let tw = tab_width(order.len(), width - TAB_H);
        let focused = a.panes.get(a.focus).map(|p| p.id);
        for (n, p) in order.iter().filter_map(|id| a.panes.iter().find(|p| p.id == *id)).enumerate() {
            let active = focused == Some(p.id);
            let (text, alpha) = match a.rename.as_ref().filter(|r| r.0 == p.id) {
                Some((_, typed)) => (format!("{typed}\u{258f}"), 1.0), // the name being typed, with a caret
                None => {
                    let alpha = match (active, hit == Some(Hit::Tab(n))) {
                        (true, _) => TAB_ACTIVE_ALPHA,
                        (_, true) => LABEL_FOCUS_ALPHA,
                        _ => LABEL_ALPHA,
                    };
                    (p.name(a.home.as_deref()).unwrap_or_default(), alpha)
                }
            };
            let (x, room) = (n as f32 * tw + TAB_PAD, (tw - 2.0 * TAB_PAD).max(0.0));
            brush.SetColor(&color(rgb(FG), alpha));
            let units: Vec<u16> = text.encode_utf16().collect();
            rt.DrawText(&units, &a.tab, &rect(x, 0.0, room, TAB_H), brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if active {
                brush.SetColor(&color(rgb(FG), UNDERLINE_ALPHA));
                rt.FillRectangle(&rect(x, TAB_H - 2.0, room, 2.0), brush);
            }
        }
    } else if near {
        // A backing keeps the button legible over a pane's name or first line.
        brush.SetColor(&color(rgb(BG), 0.9));
        rt.FillRectangle(&rect(width - TAB_H, 0.0, TAB_H, TAB_H), brush);
    }
    if tabs || near {
        let hot = hit == Some(Hit::Button);
        brush.SetColor(&color(rgb(FG), if hot { LABEL_FOCUS_ALPHA } else { LABEL_ALPHA }));
        let (cx, cy) = (width - TAB_H / 2.0, TAB_H / 2.0);
        if tabs {
            // Back to the grid: 2 x 2 tiny squares.
            let (x, y) = ((cx - 5.0).round(), (cy - 5.0).round());
            for (dx, dy) in [(0.0, 0.0), (6.0, 0.0), (0.0, 6.0), (6.0, 6.0)] {
                rt.FillRectangle(&rect(x + dx, y + dy, 4.0, 4.0), brush);
            }
        } else {
            // To the tabs: a window outline with a tab on top.
            let (x, y) = ((cx - 7.0).round(), (cy - 5.5).round());
            rt.FillRectangle(&rect(x, y, 6.0, 3.0), brush);
            rt.DrawRectangle(&rect(x + 0.5, y + 3.5, 13.0, 7.0), brush, 1.0, None);
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

    for cell in content.display_iter {
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = flags.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, bold, content.colors).unwrap_or(rgb(BG));
        let mut bg = resolve(cell.bg, false, content.colors);
        if flags.contains(Flags::INVERSE) {
            (fg, bg) = (bg.unwrap_or(rgb(BG)), Some(fg));
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
            (fg, bg) = (bg.unwrap_or(rgb(BG)), Some(fg));
        }

        if let Some(b) = bg {
            flush(&mut run, run_at, run_style);
            brush.SetColor(&color(b, 1.0));
            rt.FillRectangle(&rect(x, y, width, ch), brush);
        }
        // Selection: a light veil over the cell, like Windows Terminal; the text keeps its color.
        if selected {
            flush(&mut run, run_at, run_style);
            brush.SetColor(&color(rgb(FG), SELECTION_ALPHA));
            rt.FillRectangle(&rect(x, y, width, ch), brush);
        }
        if is_cursor {
            brush.SetColor(&color(rgb(FG), 1.0));
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
    let changed = with(|a| {
        let size = zoomed(a.font_size, step);
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

/// Ctrl+Shift+Z / Ctrl+Shift+G / the view button: switch to `to` or back to the grid (see `toggled`).
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
    }
}

/// Drop the WM_CHAR that TranslateMessage queued for the key being handled, if the key made one.
unsafe fn drop_char(hwnd: HWND) {
    let _ = PeekMessageW(&mut MSG::default(), Some(hwnd), WM_CHAR, WM_CHAR, PM_REMOVE);
}

unsafe fn modifiers() -> (bool, bool, bool) {
    let down = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
    (down(VK_SHIFT), down(VK_MENU), down(VK_CONTROL))
}

/// Returns true when the key was handled here.
unsafe fn on_key(hwnd: HWND, vk: u16) -> bool {
    let (shift, alt, ctrl) = modifiers();
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
    // Shift+arrows select text instead of reaching the shell.
    if let (Some(d), true, false, false) = (dir, shift, alt, ctrl) {
        with(|a| {
            if let Some(p) = a.panes.get_mut(a.focus) {
                p.extend_selection(d);
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
            with(|a| a.skip_char = true);
            leave_zoom(hwnd);
            add_pane(hwnd, split);
            return true;
        }
    }
    // dterm shortcuts; their Ctrl+letter WM_CHAR must not reach the shell.
    if ctrl && !alt && (vk == 'V' as u16 || (shift && "TWRZG".contains(vk as u8 as char))) {
        with(|a| a.skip_char = true);
        if vk == 'V' as u16 {
            paste(hwnd);
        } else if vk == 'T' as u16 {
            leave_zoom(hwnd);
            add_pane(hwnd, None);
        } else if vk == 'R' as u16 {
            begin_rename(hwnd);
        } else if vk == 'Z' as u16 {
            toggle_view(hwnd, View::Zoom);
        } else if vk == 'G' as u16 {
            toggle_view(hwnd, View::Tabs);
        } else if let Some(id) = with(|a| a.panes.get(a.focus).map(|p| p.id)) {
            leave_zoom(hwnd);
            remove_pane(hwnd, id);
        }
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

/// The window chrome under the mouse, and whether the mouse is over the tab strip.
unsafe fn chrome_at(hwnd: HWND) -> (Option<Hit>, bool) {
    let (m, (w, _)) = (mouse(hwnd), client(hwnd));
    with(|a| (chrome_hit(a.view, a.panes.len(), m, w), a.view == View::Tabs && m.1 < TAB_H))
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_WAKE => {
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
            paint(hwnd);
            let _ = EndPaint(hwnd, &ps);
            update_title(hwnd); // focus or names may have changed
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_SIZE => layout(hwnd),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if on_key(hwnd, wp.0 as u16) {
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
            if let (Some(hit), _) = chrome_at(hwnd) {
                // A tab or the view button: no selection starts under them.
                match hit {
                    Hit::Tab(i) => with(|a| {
                        if let Some(&id) = a.order().get(i) {
                            a.focus_id(id);
                        }
                    }),
                    Hit::Button => toggle_view(hwnd, View::Tabs),
                }
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
                let before = hover_state(a.view, a.panes.len(), a.mouse, w);
                a.mouse = Some((x, y));
                let hover = hover_state(a.view, a.panes.len(), a.mouse, w) != before;
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
                d2d, dw, format, bold, label, tab, font_size: FONT_SIZE, cell, target: None, bash, panes: Vec::new(),
                focus: 0, next_id: 0, tree: None, drag: None, surrogate: 0, skip_char: false,
                select: None, click: (0, f32::MIN, f32::MIN, 0),
                home: std::env::var("USERPROFILE").ok(), rename: None, title: "dterm".into(),
                view: View::Grid, mouse: None, tracking: false,
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
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as _, 4);
        let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 });

        add_pane(hwnd, None);
        if with(|a| a.panes.is_empty()) {
            return fail("Could not start Git Bash");
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
    // 3 tabs of 180 DIPs from the left; the button is the strip's last TAB_H.
    let hit = |view, x, y| chrome_hit(view, 3, (x, y), 1000.0);
    assert_eq!((hit(tb, 10.0, 10.0), hit(tb, 370.0, 29.0), hit(tb, 539.0, 0.0)), (Some(Hit::Tab(0)), Some(Hit::Tab(2)), Some(Hit::Tab(2))));
    assert_eq!((hit(tb, 541.0, 10.0), hit(tb, 10.0, 30.0), hit(tb, -1.0, 10.0)), (None, None, None));
    assert_eq!((hit(tb, 970.0, 10.0), hit(g, 970.0, 10.0), hit(z, 999.0, 0.0)), (Some(Hit::Button), Some(Hit::Button), Some(Hit::Button)));
    assert_eq!((hit(g, 969.0, 10.0), hit(g, 10.0, 10.0), hit(g, 970.0, 30.0)), (None, None, None));
    // The grid's button shows only near the top edge.
    let near = |y| hover_state(g, 3, Some((990.0, y)), 1000.0);
    assert_eq!((near(10.0), near(35.0), near(36.0)), ((true, Some(Hit::Button)), (true, None), (false, None)));
    assert_eq!(hover_state(g, 3, None, 1000.0), (false, None));
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
}
