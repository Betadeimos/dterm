"""Drive dterm and record it: input via SendInput, capture via ffmpeg gdigrab.

Every key and click first checks that dterm is the foreground window, and stops otherwise,
so nothing is typed into another app.
"""
import ctypes, ctypes.wintypes as wt, os, subprocess, sys, tempfile, time, shutil

ctypes.windll.shcore.SetProcessDpiAwareness(2)
u32 = ctypes.windll.user32
dwm = ctypes.windll.dwmapi

HERE = os.path.dirname(os.path.abspath(__file__))
WORK = os.path.join(tempfile.gettempdir(), "dterm-demo")
DRIVE = "K:"                              # a free drive letter: short prompts, no user name in paths
WORLD = os.path.join(WORK, "world")       # mapped to K:
APPDATA = os.path.join(WORK, "appdata")   # dterm's %APPDATA% (settings, sessions, PS history)
OUT = os.path.join(WORK, "out")
EXE = os.path.join(HERE, "..", "..", "target", "release", "dterm.exe")
FFMPEG = shutil.which("ffmpeg")
PAD = 36  # px of backdrop around the window in captures

# --- input -------------------------------------------------------------------------------

class KI(ctypes.Structure):
    _fields_ = [("wVk", wt.WORD), ("wScan", wt.WORD), ("dwFlags", wt.DWORD), ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]

class MI(ctypes.Structure):
    _fields_ = [("dx", wt.LONG), ("dy", wt.LONG), ("mouseData", wt.DWORD), ("dwFlags", wt.DWORD), ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]

class _U(ctypes.Union):
    _fields_ = [("ki", KI), ("mi", MI), ("pad", ctypes.c_byte * 32)]

class INPUT(ctypes.Structure):
    _fields_ = [("type", wt.DWORD), ("u", _U)]

EXTENDED = {0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E, 0x24, 0x23, 0x21, 0x22}
VK = {"ctrl": 0x11, "alt": 0x12, "shift": 0x10, "enter": 0x0D, "esc": 0x1B, "tab": 0x09, "back": 0x08,
      "left": 0x25, "up": 0x26, "right": 0x27, "down": 0x28, "del": 0x2E, "f2": 0x71, "f4": 0x73,
      "plus": 0xBB, "minus": 0xBD, "comma": 0xBC, "space": 0x20}

def _send(*inputs):
    arr = (INPUT * len(inputs))(*inputs)
    u32.SendInput(len(inputs), arr, ctypes.sizeof(INPUT))

def _key(vk, up=False, unicode=None):
    i = INPUT(type=1)
    if unicode is not None:
        i.u.ki = KI(0, ord(unicode), 0x0004 | (0x0002 if up else 0), 0, 0)
    else:
        i.u.ki = KI(vk, 0, (0x0002 if up else 0) | (0x0001 if vk in EXTENDED else 0), 0, 0)
    return i


class Dterm:
    def __init__(self):
        self.proc = None
        self.hwnd = None
        self.rec = None

    # --- process and window ---
    def launch(self, x=None, y=None, w=None, h=None):
        """Start dterm at (x, y), w x h; with no arguments, where the last launch put it."""
        if x is not None:
            self.place = (x, y, w, h)
        x, y, w, h = self.place
        # Not the markers of the Claude Code session running this script: the demo's claude is a fresh one.
        session = ("AI_AGENT", "CLAUDECODE", "CLAUDE_PID", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT",
                   "CLAUDE_CODE_SESSION_ATTENDED", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN")
        env = {k: v for k, v in os.environ.items() if k.upper() not in session}
        env["APPDATA"] = APPDATA
        env["USERPROFILE"] = DRIVE + "\\"
        env["HOMEDRIVE"], env["HOMEPATH"] = DRIVE, "\\"
        env["CLAUDE_CONFIG_DIR"] = os.path.join(os.environ["USERPROFILE"], ".claude")  # dterm finds conversations there
        env["PATH"] = DRIVE + "\\.bin;" + env["PATH"]
        u32.SetCursorPos(x + w // 2, y + h // 2)  # dterm opens on the monitor under the mouse
        self.proc = subprocess.Popen([EXE], env=env, cwd=DRIVE + "\\")
        for _ in range(100):
            time.sleep(0.1)
            self.hwnd = self._find()
            if self.hwnd:
                break
        assert self.hwnd, "dterm window not found"
        time.sleep(0.4)
        ok = u32.SetWindowPos(wt.HWND(self.hwnd), wt.HWND(-1), x, y, w, h, 0x0040)  # HWND_TOPMOST, SWP_SHOWWINDOW: above the backdrop
        if not ok:
            print("SetWindowPos failed", ctypes.GetLastError(), flush=True)
        self.front()
        time.sleep(0.8)
        print("dterm at", self.rect(), flush=True)

    def _find(self):
        found = []
        pid = self.proc.pid
        @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
        def cb(h, _):
            p = wt.DWORD()
            u32.GetWindowThreadProcessId(h, ctypes.byref(p))
            cls = ctypes.create_unicode_buffer(64)
            u32.GetClassNameW(h, cls, 64)
            if p.value == pid and cls.value == "dterm" and u32.IsWindowVisible(h):
                found.append(h)
            return True
        u32.EnumWindows(cb, 0)
        return found[0] if found else None

    def front(self):
        # An Alt tap lets this process move the foreground (SetForegroundWindow's rules).
        _send(_key(0x12), _key(0x12, True))
        u32.SetForegroundWindow(self.hwnd)
        time.sleep(0.2)

    def guard(self):
        if u32.GetForegroundWindow() != self.hwnd:
            self.front()
            if u32.GetForegroundWindow() != self.hwnd:
                self.close()
                sys.exit("dterm lost the focus: stopped so nothing is typed elsewhere")

    def rect(self):
        r = wt.RECT()
        dwm.DwmGetWindowAttribute(self.hwnd, 9, ctypes.byref(r), ctypes.sizeof(r))  # DWMWA_EXTENDED_FRAME_BOUNDS
        return r.left, r.top, r.right - r.left, r.bottom - r.top

    def close(self, graceful=False):
        if self.proc and self.proc.poll() is None:
            if graceful and self.hwnd:
                u32.PostMessageW(self.hwnd, 0x0010, 0, 0)  # WM_CLOSE: saves the session
                try:
                    self.proc.wait(5)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
            else:
                self.proc.kill()
        self.proc = None

    # --- keys ---
    def keys(self, *names, wait=0.35):
        """A chord: keys("ctrl", "alt", "t")."""
        self.guard()
        vks = [VK[n] if n in VK else ord(n.upper()) for n in names]
        _send(*[_key(v) for v in vks])
        time.sleep(0.03)
        _send(*[_key(v, True) for v in reversed(vks)])
        time.sleep(wait)

    def type(self, text, cps=22, enter=True, wait=0.4):
        for ch in text:
            self.guard()
            r = u32.VkKeyScanW(ord(ch))
            vk, shift = r & 0xFF, (r >> 8) & 1
            if r == -1 or (r >> 8) & 6:  # not on the keyboard, or needs Ctrl/Alt
                _send(_key(0, unicode=ch), _key(0, True, unicode=ch))
            else:
                seq = ([_key(0x10)] if shift else []) + [_key(vk), _key(vk, True)] + ([_key(0x10, True)] if shift else [])
                _send(*seq)
            time.sleep(1.0 / cps)
        if enter:
            time.sleep(0.15)
            self.keys("enter", wait=wait)

    # --- mouse (client DIPs, which are pixels at 100 % scaling) ---
    def _abs(self, cx, cy):
        pt = wt.POINT(int(cx), int(cy))
        u32.ClientToScreen(self.hwnd, ctypes.byref(pt))
        return pt.x, pt.y

    def move(self, cx, cy, steps=1, dur=0.0):
        x1, y1 = self._abs(cx, cy)
        p = wt.POINT()
        u32.GetCursorPos(ctypes.byref(p))
        for i in range(1, steps + 1):
            t = i / steps
            t = t * t * (3 - 2 * t)  # ease
            u32.SetCursorPos(int(p.x + (x1 - p.x) * t), int(p.y + (y1 - p.y) * t))
            time.sleep(dur / steps)

    def _btn(self, flag):
        i = INPUT(type=0)
        i.u.mi = MI(0, 0, 0, flag, 0, 0)
        _send(i)

    def click(self, cx, cy, wait=0.3, right=False):
        self.guard()
        self.move(cx, cy, steps=20, dur=0.35)
        self._btn(0x0008 if right else 0x0002)
        time.sleep(0.05)
        self._btn(0x0010 if right else 0x0004)
        time.sleep(wait)

    def drag(self, a, b, dur=0.9):
        self.guard()
        self.move(*a, steps=20, dur=0.3)
        time.sleep(0.15)
        self._btn(0x0002)
        time.sleep(0.1)
        self.move(*b, steps=40, dur=dur)
        time.sleep(0.1)
        self._btn(0x0004)
        time.sleep(0.3)

    def client(self):
        r = wt.RECT()
        u32.GetClientRect(self.hwnd, ctypes.byref(r))
        return r.right, r.bottom

    # --- capture ---
    def park(self):
        """The mouse waits outside the capture (right edge of the desktop) until a scene moves it."""
        u32.SetCursorPos(u32.GetSystemMetrics(78) - 40, 600)

    def record(self, name, fps=24, park=True):
        if park:
            self.park()
        x, y, w, h = self.rect()
        x, y, w, h = x - PAD, y - PAD, w + 2 * PAD, h + 2 * PAD
        w, h = w - w % 2, h - h % 2
        os.makedirs(OUT, exist_ok=True)
        self.raw = os.path.join(OUT, name + ".mkv")
        self.rec = subprocess.Popen(
            [FFMPEG, "-y", "-loglevel", "error", "-f", "gdigrab", "-framerate", str(fps), "-draw_mouse", "1",
             "-offset_x", str(x), "-offset_y", str(y), "-video_size", f"{w}x{h}", "-i", "desktop",
             "-c:v", "libx264rgb", "-crf", "0", "-preset", "ultrafast", self.raw],
            stdin=subprocess.PIPE)
        time.sleep(0.6)

    def stop(self):
        time.sleep(0.4)
        self.rec.communicate(b"q", timeout=30)
        self.rec = None
        return self.raw

    def shot(self, name):
        x, y, w, h = self.rect()
        os.makedirs(OUT, exist_ok=True)
        path = os.path.join(OUT, name + ".png")
        subprocess.run([FFMPEG, "-y", "-loglevel", "error", "-f", "gdigrab", "-draw_mouse", "0", "-offset_x", str(x - PAD),
                        "-offset_y", str(y - PAD), "-video_size", f"{w + 2 * PAD}x{h + 2 * PAD}", "-i", "desktop",
                        "-frames:v", "1", path], check=True)
        return path


def gif(raw, dest, width=960, fps=12, colors=160):
    """MKV to a looping GIF with one palette for the whole clip."""
    vf = (f"fps={fps},scale={width}:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors={colors}:stats_mode=full[p];"
          f"[b][p]paletteuse=dither=sierra2_4a")
    subprocess.run([FFMPEG, "-y", "-loglevel", "error", "-i", raw, "-vf", vf, "-loop", "0", dest], check=True)
    return dest
