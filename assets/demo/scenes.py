"""The README's GIFs and screenshot. Each scene starts dterm fresh on a gradient backdrop, plays
keys and clicks, and writes %TEMP%\\dterm-demo\\out\\<scene>.gif (hero: hero.png).

    cargo build --release
    python world.py up            # K: with small demo repos (needs ffmpeg and git)
    python scenes.py warm         # once: answers Claude Code's trust question for K:\\acme-api
    python scenes.py grid resize agents status tabs broadcast worktree settings hero
    python world.py down

It takes over the keyboard and mouse while it runs; it stops if dterm loses the focus. `agents`
and `hero` send one short prompt each to Claude Code.
"""
import os, shutil, subprocess, sys, time
from rig import Dterm, gif, HERE, APPDATA, OUT
import world

CAPTION = 32  # dterm's title bar, DIPs

def fresh(theme="One Half Dark", opacity=80):
    """No sessions: dterm opens straight into one pane."""
    shutil.rmtree(os.path.join(APPDATA, "dterm"), ignore_errors=True)
    world.settings(theme=theme, opacity=opacity)

def clear(d):
    d.type("cls", wait=0.3)

def four(d, run=True):
    """2x2: acme-api (top left), web-app (top right), infra (bottom right), acme-api (bottom left, focused)."""
    d.type("cd acme-api"); clear(d)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd web-app"); clear(d)
    if run:
        d.type("git log --oneline --graph --all", wait=0.6)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd infra"); clear(d)
    if run:
        d.type("git status", wait=0.6)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd acme-api"); clear(d)

def finish(d, name, **kw):
    raw = d.stop()
    print(gif(raw, os.path.join(OUT, name + ".gif"), **kw), flush=True)

# --- scenes ------------------------------------------------------------------------------

def grid(d):
    """The grid builds itself: Ctrl+Alt+T four times, then move around with Alt+arrows."""
    d.record("grid")
    time.sleep(0.6)
    d.type("cd acme-api", cps=16)
    d.keys("ctrl", "alt", "t", wait=0.8)
    d.type("cd web-app", cps=16)
    d.type("git log --oneline --graph", cps=16, wait=0.6)
    d.keys("ctrl", "alt", "t", wait=0.8)
    d.type("cd infra", cps=16)
    d.keys("ctrl", "alt", "t", wait=0.8)
    d.type("cd acme-api", cps=16)
    d.type(r".\test.ps1", cps=16, wait=1.2)
    for k in ("up", "left", "down", "right"):
        d.keys("alt", k, wait=0.6)
    time.sleep(2.5)
    finish(d, "grid")

def resize(d):
    """Drag the gaps between panes."""
    four(d)
    w, h = d.client()
    mid_x, mid_y = w / 2, CAPTION + (h - CAPTION) / 2
    d.move(w * 0.8, h * 0.8)
    d.record("resize")
    time.sleep(0.6)
    d.drag((mid_x, mid_y - 120), (mid_x - 260, mid_y - 120))
    d.drag((mid_x - 260, mid_y - 120), (mid_x + 180, mid_y - 120))
    d.drag((mid_x + 180 + 200, mid_y), (mid_x + 180 + 200, mid_y - 170))
    d.drag((mid_x - 200, mid_y), (mid_x - 200, mid_y + 150))
    d.move(w * 0.8, h * 0.8, steps=20, dur=0.4)
    time.sleep(1.5)
    finish(d, "resize")

def tabs(d):
    """Grid, Tabs and Zoom over the same panes."""
    four(d)
    d.type(r".\test.ps1", wait=0.2)
    d.record("tabs")
    time.sleep(1.0)
    d.keys("ctrl", "alt", "g", wait=1.2)
    for _ in range(3):
        d.keys("ctrl", "tab", wait=1.0)
    d.keys("ctrl", "alt", "g", wait=1.2)
    d.keys("alt", "right", wait=0.3)
    d.keys("alt", "up", wait=0.5)  # the git graph
    d.keys("ctrl", "alt", "z", wait=1.5)
    d.keys("ctrl", "alt", "z", wait=1.5)
    finish(d, "tabs")

def warm(d):
    """Not recorded: run claude once in acme-api so its folder trust question is answered."""
    d.type("cd acme-api")
    d.type("claude", wait=6)
    d.shot("warm1")
    d.keys("down", wait=0.5)
    d.keys("enter", wait=5)
    d.shot("warm2")

def agents(d):
    """Close dterm with Claude Code running, reopen it: the session comes back, Claude resumes."""
    d.type("cd acme-api"); clear(d)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd web-app"); clear(d)
    d.keys("alt", "left", wait=0.4)
    d.record("agents")
    time.sleep(0.5)
    d.type("claude", cps=14, wait=5)
    d.type("Summarize the last 3 commits, one short line each.", cps=30, wait=14)
    d.keys("alt", "right", wait=0.4)
    d.type(r".\build.ps1", cps=16, wait=4.5)
    # Close with the window's X button.
    w, _ = d.client()
    d.click(w - 23, 16, wait=1.6)
    d.launch()
    d.park()
    time.sleep(1.5)
    d.keys("down", wait=1.2)  # the start page: last session
    d.keys("enter", wait=8)
    finish(d, "agents")

def status(d):
    """Dots on background panes: blue busy, green done, yellow waiting."""
    four(d, run=False)  # focus: bottom left
    d.record("status")
    time.sleep(0.6)
    d.keys("alt", "up", wait=0.3)     # acme-api
    d.type(r".\test.ps1", cps=18, wait=0.2)
    d.keys("alt", "right", wait=0.3)  # web-app
    d.type(r".\build.ps1", cps=18, wait=0.2)
    d.keys("alt", "down", wait=0.3)   # infra
    d.type('Start-Sleep 3; Write-Host "plan: 3 to add. Apply?`a"', cps=30, wait=0.2)
    d.keys("alt", "left", wait=0.3)   # the idle pane
    time.sleep(7)
    d.keys("alt", "right", wait=1.2)  # looking at infra clears its dot
    d.keys("alt", "up", wait=1.5)
    finish(d, "status")

def broadcast(d):
    """Type once into every pane."""
    d.type("cd acme-api"); clear(d)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd web-app"); clear(d)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd infra"); clear(d)
    d.record("broadcast")
    time.sleep(0.8)
    d.keys("ctrl", "alt", "b", wait=1.0)
    d.type("git status -sb", cps=14, wait=1.0)
    d.type("git log --oneline -3", cps=14, wait=1.5)
    d.keys("esc", wait=1.8)
    finish(d, "broadcast")

def worktree(d):
    """Ctrl+Alt+N: a git worktree on a new branch, in a new pane."""
    d.type("cd acme-api"); clear(d)
    d.type("git log --oneline -4", wait=0.4)
    d.record("worktree")
    time.sleep(0.8)
    d.keys("ctrl", "alt", "n", wait=0.8)
    d.type("feat/login", cps=12, enter=False, wait=0.6)
    d.keys("enter", wait=2.0)
    d.type("git branch", cps=16, wait=0.6)
    d.type("pwd", cps=16, wait=2.0)
    finish(d, "worktree")

def settings(d):
    """Themes, opacity, shell and shortcuts on the settings page."""
    four(d)
    d.record("settings")
    time.sleep(0.6)
    d.keys("ctrl", "alt", "comma", wait=1.0)
    for _ in range(4):
        d.keys("right", wait=0.9)
    d.keys("right", wait=0.6)  # back to One Half Dark
    d.keys("down", wait=0.4)
    for k in ("left", "left", "left", "right", "right", "right"):
        d.keys(k, wait=0.35)
    d.keys("down", wait=0.4)
    for _ in range(4):
        d.keys("right", wait=0.8)
    d.keys("down", wait=0.4)
    d.keys("down", wait=0.4)
    d.keys("down", wait=0.6)
    d.keys("esc", wait=1.5)
    finish(d, "settings")

def hero(d):
    """The README screenshot: Claude Code, a git graph, tests and a build, side by side."""
    # Claude Code on the left; a git graph, tests and a build stacked on the right.
    d.type("cd acme-api"); clear(d)
    d.type("claude", wait=5)
    d.type("Turn the last 4 commit messages into a short changelog. Only run git log.", wait=1)
    d.keys("ctrl", "alt", "t", wait=1.0)
    d.type("cd web-app"); clear(d)
    d.type("git log --oneline --graph --all", wait=0.6)
    d.keys("ctrl", "alt", "minus", wait=1.0)
    d.type("cd acme-api"); clear(d)
    d.type(r".\test.ps1", wait=0.3)
    d.keys("ctrl", "alt", "minus", wait=1.0)
    d.type("cd web-app"); clear(d)
    d.type(r".\build.ps1", wait=0.3)
    d.keys("ctrl", "alt", "s", wait=0.6)
    d.type("acme", wait=0.6)
    d.keys("alt", "left", wait=0.3)
    time.sleep(30)
    d.move(-50, -50)
    d.shot("hero")

SCENES = {f.__name__: f for f in (grid, resize, tabs, warm, agents, status, broadcast, worktree, settings, hero)}
SIZES = {"hero": (260, 110, 1460, 900)}

if __name__ == "__main__":
    backdrop = subprocess.Popen([sys.executable, os.path.join(HERE, "backdrop.py")])
    time.sleep(1.5)
    try:
        for name in sys.argv[1:]:
            fresh()
            d = Dterm()
            d.launch(*SIZES.get(name, (380, 220, 1160, 720)))
            try:
                SCENES[name](d)
            finally:
                if d.rec:
                    d.stop()
                d.close()
                time.sleep(0.5)
    finally:
        backdrop.kill()
