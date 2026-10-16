#![cfg_attr(not(test), windows_subsystem = "windows")]
//! dterm: Git Bash panes in a resizable grid, drawn with Direct2D/DirectWrite on an acrylic window.
//! Terminal emulation and ConPTY come from alacritty_terminal.
//! Ctrl+Shift+T adds a pane, Alt+arrows move the focus, Ctrl+V pastes plain text,
//! drag the gaps to resize, `exit` closes a pane.

use std::{
    borrow::Cow,
    cell::RefCell,
    sync::{Arc, OnceLock},
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
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
            DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalLock, GlobalUnlock},
            Ole::CF_UNICODETEXT,
            Registry::*,
        },
        UI::{Controls::MARGINS, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

const FONT: PCWSTR = w!("Cascadia Mono");
const FONT_SIZE: f32 = 16.0; // DIPs, = 12pt like Windows Terminal
const PAD: f32 = 8.0; // DIPs around the text, like Windows Terminal
const GAP: f32 = 6.0; // DIPs between panes
const MIN: f32 = 0.05; // smallest pane, as a fraction of the window
const OPACITY: f32 = 0.7; // pane background over the acrylic, like your Windows Terminal profile
const WM_WAKE: u32 = WM_APP; // new terminal output
const WM_EXIT: u32 = WM_APP + 1; // a shell exited; wparam = pane id

// One Half Dark, as in Windows Terminal. ANSI order: black red green yellow blue magenta cyan white.
const FG: u32 = 0xDCDFE4;
const BG: u32 = 0x282C34;
const GAP_COLOR: u32 = 0x1B1D23;
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

/// Rows and columns for `n` panes: as square as possible, wider than tall.
fn grid(n: usize) -> (usize, usize) {
    let c = (1..).find(|c| c * c >= n).unwrap();
    (n.div_ceil(c).max(1), c)
}

fn even(n: usize) -> Vec<f32> {
    (0..=n).map(|i| i as f32 / n as f32).collect()
}

/// Offset and length of the cells between boundaries `a` and `b`, leaving gaps at inner boundaries.
fn span(f: &[f32], a: usize, b: usize, len: f32) -> (f32, f32) {
    let x0 = f[a] * len + if a > 0 { GAP / 2.0 } else { 0.0 };
    let x1 = f[b] * len - if b + 1 < f.len() { GAP / 2.0 } else { 0.0 };
    (x0, (x1 - x0).max(0.0))
}

/// Inner boundary under `x`, if any.
fn hit(f: &[f32], x: f32, len: f32) -> Option<usize> {
    (1..f.len() - 1).find(|&i| (x - f[i] * len).abs() <= GAP)
}

fn drag_to(f: &mut [f32], i: usize, x: f32, len: f32) {
    f[i] = (x / len.max(1.0)).max(f[i - 1] + MIN).min(f[i + 1] - MIN);
}

/// Pane next to `from` in direction `(dx, dy)`: the nearest one past its edge, preferring panes
/// straight in line with it.
fn neighbor(rects: &[(f32, f32, f32, f32)], from: usize, (dx, dy): (i32, i32)) -> Option<usize> {
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

/// Terminal columns and lines that fit in a pane of `w` x `h` DIPs.
fn fit(w: f32, h: f32, cell: (f32, f32)) -> (usize, usize) {
    let cols = ((w - 2.0 * PAD) / cell.0).floor().max(2.0);
    let lines = ((h - 2.0 * PAD) / cell.1).floor().max(1.0);
    (cols as usize, lines as usize)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Forwards terminal events from a pane's PTY thread to the window.
#[derive(Clone)]
struct Proxy {
    hwnd: isize,
    id: usize,
    tx: Arc<OnceLock<EventLoopSender>>, // set right after the event loop exists
}

impl EventListener for Proxy {
    fn send_event(&self, e: Event) {
        let post = |msg, wp| unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as _)), msg, WPARAM(wp), LPARAM(0));
        };
        match e {
            Event::Wakeup => post(WM_WAKE, 0),
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
    rect: (f32, f32, f32, f32), // x, y, w, h in DIPs
}

impl Pane {
    fn send(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.tx.send(Msg::Input(bytes.into()));
    }
}

struct App {
    d2d: ID2D1Factory,
    format: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    cell: (f32, f32), // DIPs
    target: Option<(ID2D1HwndRenderTarget, ID2D1SolidColorBrush)>,
    bash: String,
    panes: Vec<Pane>,
    focus: usize, // index into panes
    next_id: usize,
    rows: Vec<f32>, // row boundaries, 0.0 ..= 1.0
    cols: Vec<f32>,
    drag: (Option<usize>, Option<usize>), // (col boundary, row boundary) being dragged
    surrogate: u16,                       // first half of a UTF-16 pair typed via WM_CHAR
    skip_char: bool,                      // WM_KEYDOWN already handled this key
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
    let proxy = Proxy { hwnd: hwnd.0 as isize, id, tx: slot.clone() };
    let term = Arc::new(FairMutex::new(Term::new(Config::default(), &TermSize::new(size.0, size.1), proxy.clone())));
    let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)?;
    let tx = event_loop.channel();
    let _ = slot.set(tx.clone());
    event_loop.spawn();
    Ok(Pane { id, term, tx, size, rect: (0.0, 0.0, 0.0, 0.0) })
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

/// Cursor position in client DIPs.
unsafe fn mouse(hwnd: HWND) -> (f32, f32) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
    let s = scale(hwnd);
    (pt.x as f32 / s, pt.y as f32 / s)
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
        let (n, nc, cell) = (a.panes.len(), a.cols.len() - 1, a.cell);
        for (i, p) in a.panes.iter_mut().enumerate() {
            let c = i % nc;
            let end = if i + 1 == n { nc } else { c + 1 }; // last pane fills its row
            let (x, pw) = span(&a.cols, c, end, w);
            let (y, ph) = span(&a.rows, i / nc, i / nc + 1, h);
            p.rect = (x, y, pw, ph);
            let size = fit(pw, ph, cell);
            if size != p.size {
                p.size = size;
                p.term.lock().resize(TermSize::new(size.0, size.1));
                let _ = p.tx.send(Msg::Resize(window_size(size, cell, s)));
            }
        }
    });
    let _ = InvalidateRect(Some(hwnd), None, false);
}

fn regrid(a: &mut App) {
    let (r, c) = grid(a.panes.len());
    a.rows = even(r);
    a.cols = even(c);
}

unsafe fn add_pane(hwnd: HWND) {
    let s = scale(hwnd);
    let added = with(|a| {
        let id = a.next_id;
        a.next_id += 1;
        let Ok(p) = spawn(hwnd, id, &a.bash, a.cell, s) else { return false };
        a.panes.push(p);
        a.focus = a.panes.len() - 1;
        regrid(a);
        true
    });
    if added {
        layout(hwnd);
    }
}

unsafe fn remove_pane(hwnd: HWND, id: usize) {
    let left = with(|a| {
        let Some(i) = a.panes.iter().position(|p| p.id == id) else { return None };
        a.panes.remove(i);
        if a.focus >= i && a.focus > 0 {
            a.focus -= 1;
        }
        if !a.panes.is_empty() {
            regrid(a);
        }
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
            let Ok(brush) = rt.CreateSolidColorBrush(&color(rgb(FG), 1.0), None) else { return };
            a.target = Some((rt, brush));
        }
        let (rt, brush) = a.target.as_ref().unwrap();
        rt.BeginDraw();
        rt.Clear(Some(&color(rgb(GAP_COLOR), 0.9)));
        for (i, p) in a.panes.iter().enumerate() {
            let (x, y, pw, ph) = p.rect;
            let r = rect(x, y, pw, ph);
            // Clear() inside the clip replaces the pixels, so the pane gets its own see-through tint.
            rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            rt.Clear(Some(&color(rgb(BG), OPACITY)));
            draw_term(rt, brush, &a.format, &a.bold, a.cell, (x + PAD, y + PAD), i == a.focus, &p.term.lock());
            rt.PopAxisAlignedClip();
        }
        if rt.EndDraw(None, None).is_err() {
            a.target = None; // device lost: recreate on the next paint
        }
    });
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
        if is_cursor && shape == CursorShape::Block {
            (fg, bg) = (bg.unwrap_or(rgb(BG)), Some(fg));
        }

        if let Some(b) = bg {
            flush(&mut run, run_at, run_style);
            brush.SetColor(&color(b, 1.0));
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

unsafe fn paste(hwnd: HWND) {
    let Some(text) = clipboard_text(hwnd) else { return };
    with(|a| {
        if let Some(p) = a.panes.get(a.focus) {
            let bracketed = p.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
            p.send(paste_text(&text, bracketed).into_bytes());
        }
    });
}

unsafe fn modifiers() -> (bool, bool, bool) {
    let down = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
    (down(VK_SHIFT), down(VK_MENU), down(VK_CONTROL))
}

/// Returns true when the key was handled here.
unsafe fn on_key(hwnd: HWND, vk: u16) -> bool {
    let (shift, alt, ctrl) = modifiers();
    // Alt+arrows move the focus between panes, like Windows Terminal.
    let dir = match vk {
        0x25 => Some((-1, 0)),
        0x27 => Some((1, 0)),
        0x26 => Some((0, -1)),
        0x28 => Some((0, 1)),
        _ => None,
    };
    if let (Some(d), true, false, false) = (dir, alt, ctrl, shift) {
        with(|a| {
            let rects: Vec<_> = a.panes.iter().map(|p| p.rect).collect();
            if let Some(i) = neighbor(&rects, a.focus, d) {
                a.focus = i;
            }
        });
        let _ = InvalidateRect(Some(hwnd), None, false);
        return true;
    }
    // dterm shortcuts; their Ctrl+letter WM_CHAR must not reach the shell.
    if ctrl && !alt && (vk == 'V' as u16 || (shift && vk == 'T' as u16)) {
        with(|a| a.skip_char = true);
        if vk == 'V' as u16 {
            paste(hwnd);
        } else {
            add_pane(hwnd);
        }
        return true;
    }
    with(|a| {
        let Some(p) = a.panes.get(a.focus) else { return false };
        let app_cursor = p.term.lock().mode().contains(TermMode::APP_CURSOR);
        let Some(seq) = key_seq(vk, shift, alt, ctrl, app_cursor) else { return false };
        p.send(seq.into_bytes());
        a.skip_char = vk == 0x09; // shift+tab also arrives as a '\t' WM_CHAR
        true
    })
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
        // Backspace sends DEL and Ctrl+Backspace sends BS, like Windows Terminal.
        s = match s.as_str() {
            "\x08" => "\x7f".into(),
            "\x7f" => "\x08".into(),
            _ => s,
        };
        if alt {
            s.insert(0, '\x1b');
        }
        if let Some(p) = a.panes.get(a.focus) {
            p.send(s.into_bytes());
        }
    });
}

fn pane_at(a: &App, (x, y): (f32, f32)) -> Option<usize> {
    a.panes.iter().position(|p| {
        let (px, py, pw, ph) = p.rect;
        x >= px && x < px + pw && y >= py && y < py + ph
    })
}

/// Divider under the mouse: (col boundary, row boundary). Only in the gaps: the last row's
/// wide pane covers the column boundaries of the rows above it.
unsafe fn divider_at(hwnd: HWND) -> (Option<usize>, Option<usize>) {
    let ((x, y), (w, h)) = (mouse(hwnd), client(hwnd));
    with(|a| if pane_at(a, (x, y)).is_some() { (None, None) } else { (hit(&a.cols, x, w), hit(&a.rows, y, h)) })
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
            return LRESULT(0);
        }
        WM_SYSCHAR if wp.0 != ' ' as usize => {
            on_char(wp.0 as u16, true); // Alt+key sends ESC + key
            return LRESULT(0);
        }
        // A lone Alt tap would enter menu mode and swallow the next key; Alt+Space still opens the system menu.
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_KEYMENU && lp.0 != ' ' as isize => return LRESULT(0),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let id = match divider_at(hwnd) {
                (Some(_), Some(_)) => IDC_SIZEALL,
                (Some(_), None) => IDC_SIZEWE,
                (None, Some(_)) => IDC_SIZENS,
                _ => IDC_IBEAM,
            };
            let _ = SetCursor(LoadCursorW(None, id).ok());
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => {
            let d = divider_at(hwnd);
            if d != (None, None) {
                with(|a| a.drag = d);
                SetCapture(hwnd);
            } else {
                // Click inside a pane: it gets the keyboard.
                let m = mouse(hwnd);
                with(|a| {
                    if let Some(i) = pane_at(a, m) {
                        a.focus = i;
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_MOUSEMOVE => {
            let ((x, y), (w, h)) = (mouse(hwnd), client(hwnd));
            let moved = with(|a| {
                if let Some(i) = a.drag.0 {
                    drag_to(&mut a.cols, i, x, w);
                }
                if let Some(i) = a.drag.1 {
                    drag_to(&mut a.rows, i, y, h);
                }
                a.drag != (None, None)
            });
            if moved {
                layout(hwnd);
            }
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
        }
        WM_CAPTURECHANGED => with(|a| a.drag = (None, None)),
        WM_DESTROY => PostQuitMessage(0),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn text_format(dw: &IDWriteFactory, weight: DWRITE_FONT_WEIGHT) -> windows::core::Result<IDWriteTextFormat> {
    let f = dw.CreateTextFormat(FONT, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, FONT_SIZE, w!("en-us"))?;
    f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
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
        let format = text_format(&dw, DWRITE_FONT_WEIGHT_NORMAL).expect("font");
        let bold = text_format(&dw, DWRITE_FONT_WEIGHT_BOLD).expect("font");
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
                d2d, format, bold, cell, target: None, bash, panes: Vec::new(), focus: 0, next_id: 0,
                rows: even(1), cols: even(1), drag: (None, None), surrogate: 0, skip_char: false,
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

        add_pane(hwnd);
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
    assert_eq!(fit(1000.0, 600.0, (10.0, 20.0)), (98, 29));
    assert_eq!(grid(1), (1, 1));
    assert_eq!(grid(3), (2, 2));
    assert_eq!(grid(5), (2, 3));
    let mut f = even(2);
    assert_eq!(span(&f, 0, 1, 1000.0), (0.0, 497.0));
    assert_eq!(span(&f, 1, 2, 1000.0), (503.0, 497.0));
    assert_eq!(span(&f, 0, 2, 1000.0), (0.0, 1000.0));
    assert_eq!(hit(&f, 502.0, 1000.0), Some(1));
    assert_eq!(hit(&f, 200.0, 1000.0), None);
    drag_to(&mut f, 1, 990.0, 1000.0);
    assert!((f[1] - 0.95).abs() < 1e-6);
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
}
