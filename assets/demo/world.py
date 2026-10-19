"""Build the demo world: K: (subst) with a few small repos, a claude wrapper, dterm settings, and
the gradient backdrop. `python world.py up` / `python world.py down`."""
import os, subprocess, sys, shutil, textwrap
from rig import WORK, DRIVE, WORLD, APPDATA, FFMPEG

REAL_HOME = os.environ["USERPROFILE"]

def git(cwd, *args):
    subprocess.run(["git", "-c", "user.name=Ada", "-c", "user.email=ada@example.com", "-c", "init.defaultBranch=main", *args],
                   cwd=cwd, check=True, capture_output=True)

def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", newline="\n") as f:
        f.write(textwrap.dedent(text).lstrip())

def repo(name, commits):
    d = os.path.join(WORLD, name)
    os.makedirs(d, exist_ok=True)
    git(d, "init", "-q")
    for msg, files in commits:
        for f, body in files.items():
            write(os.path.join(d, f), body)
        git(d, "add", "-A")
        git(d, "commit", "-q", "-m", msg)
    return d

TESTS = r'''
$tests = @(
  'auth  > issues a session token', 'auth  > rejects an expired token', 'auth  > refreshes before expiry',
  'users > creates a user', 'users > lists users by page', 'users > hides deleted users',
  'billing > charges the default card', 'billing > retries a declined charge', 'billing > sends the receipt',
  'search > ranks exact matches first', 'search > handles empty queries'
)
Write-Host ''
Write-Host ' RUN ' -BackgroundColor Cyan -ForegroundColor Black -NoNewline; Write-Host '  v2.4.0  K:\acme-api' -ForegroundColor DarkGray
Write-Host ''
foreach ($t in $tests) {
  Start-Sleep -Milliseconds (Get-Random -Minimum 120 -Maximum 380)
  Write-Host '  ' -NoNewline; Write-Host ([char]0x2713) -ForegroundColor Green -NoNewline; Write-Host " $t " -NoNewline
  Write-Host "$(Get-Random -Minimum 2 -Maximum 40)ms" -ForegroundColor DarkGray
}
Write-Host ''
Write-Host ' Test Files ' -NoNewline -ForegroundColor DarkGray; Write-Host '4 passed' -ForegroundColor Green
Write-Host '      Tests ' -NoNewline -ForegroundColor DarkGray; Write-Host "$($tests.Count) passed" -ForegroundColor Green
Write-Host '   Duration ' -NoNewline -ForegroundColor DarkGray; Write-Host '2.81s'
'''

BUILD = r'''
$steps = @('compiling tokens', 'bundling client  (412 modules)', 'bundling server  (87 modules)', 'optimizing images', 'writing dist/')
foreach ($s in $steps) { Write-Host '  > ' -ForegroundColor Magenta -NoNewline; Write-Host $s; Start-Sleep -Milliseconds 700 }
Write-Host ''
Write-Host '  dist/index.html        ' -NoNewline; Write-Host '  1.2 kB' -ForegroundColor DarkGray
Write-Host '  dist/assets/app.js     ' -NoNewline; Write-Host '148.6 kB' -ForegroundColor Yellow
Write-Host '  dist/assets/app.css    ' -NoNewline; Write-Host ' 18.3 kB' -ForegroundColor DarkGray
Write-Host ''
Write-Host '  built in 3.6s' -ForegroundColor Green
'''

def up():
    if os.path.isdir(WORLD):
        down()
    if os.path.exists(DRIVE + "\\"):
        sys.exit(f"{DRIVE} is already in use: pick another letter in rig.py")
    os.makedirs(WORLD)
    subprocess.run(["subst", DRIVE, WORLD], check=True)
    repo("acme-api", [
        ("Initial API skeleton", {"package.json": '{ "name": "acme-api", "version": "2.4.0" }\n', "src/server.ts": "export {}\n"}),
        ("Add session tokens", {"src/auth.ts": "export const ttl = 3600\n"}),
        ("Users: paging and soft delete", {"src/users.ts": "export const page = 50\n"}),
        ("Billing: retry declined charges", {"src/billing.ts": "export const retries = 3\n"}),
        ("Search: rank exact matches first", {"src/search.ts": "export const exact = 10\n"}),
        ("Tests for auth, users, billing, search", {"test.ps1": TESTS}),
    ])
    w = repo("web-app", [
        ("Scaffold the web app", {"package.json": '{ "name": "web-app" }\n'}),
        ("Design tokens", {"src/tokens.css": ":root{}\n"}),
        ("Login page", {"src/login.tsx": "export {}\n"}),
    ])
    git(w, "checkout", "-q", "-b", "feat/dark-mode")
    write(os.path.join(w, "src/theme.ts"), "export const dark = true\n"); git(w, "add", "-A"); git(w, "commit", "-q", "-m", "Dark mode toggle")
    write(os.path.join(w, "src/theme.ts"), "export const dark = 'auto'\n"); git(w, "add", "-A"); git(w, "commit", "-q", "-m", "Follow the system theme")
    git(w, "checkout", "-q", "main")
    write(os.path.join(w, "src/nav.tsx"), "export {}\n"); git(w, "add", "-A"); git(w, "commit", "-q", "-m", "Navigation bar")
    git(w, "merge", "-q", "--no-ff", "feat/dark-mode", "-m", "Merge branch 'feat/dark-mode'")
    write(os.path.join(w, "build.ps1"), BUILD); git(w, "add", "-A"); git(w, "commit", "-q", "-m", "Build script")
    repo("infra", [
        ("Terraform state bucket", {"main.tf": "# bucket\n"}),
        ("Staging cluster", {"staging.tf": "# staging\n"}),
    ])
    # claude runs with the real home (its login lives there); dterm itself sees K:\ as home.
    write(os.path.join(WORLD, ".bin", "claude.cmd"), f'''
        @echo off
        set "USERPROFILE={REAL_HOME}"
        set "HOMEDRIVE=" & set "HOMEPATH="
        set "CLAUDE_CONFIG_DIR="
        "{REAL_HOME}\\.local\\bin\\claude.exe" --settings {DRIVE}\\.bin\\demo.json %*
        ''')
    # The demo's status line is just the git branch, not the user's own.
    write(os.path.join(WORLD, ".bin", "demo.json"), '{ "statusLine": { "type": "command", "command": "git branch --show-current" } }\n')
    settings(theme="One Half Dark", opacity=80)
    backdrop()

# Soft color blobs on near black, for the acrylic to blur: (x, y, radius, (r, g, b)).
BLOBS = [(250, 200, 420, (110, 40, 220)), (1650, 250, 450, (220, 40, 120)), (1250, 1050, 480, (0, 150, 140)),
         (300, 1050, 420, (30, 70, 210)), (950, 600, 300, (140, 60, 200))]

def backdrop():
    def channel(c):
        terms = "+".join(f"{b[3][c]}*exp(-(pow(X-{b[0]},2)+pow(Y-{b[1]},2))/{2 * b[2] ** 2})" for b in BLOBS)
        return f"min(255,{(12, 12, 26)[c]}+{terms})"
    vf = f"geq=r='{channel(0)}':g='{channel(1)}':b='{channel(2)}'"
    subprocess.run([FFMPEG, "-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=black:s=1920x1200,format=rgb24",
                    "-vf", vf, "-frames:v", "1", os.path.join(WORK, "backdrop.png")], check=True)

def settings(theme="One Half Dark", opacity=80, shell="Windows PowerShell"):
    write(os.path.join(APPDATA, "dterm", "settings"), f"dterm-settings 1\ntheme {theme}\nopacity {opacity}\nshell {shell}\n")

def down():
    # Only a subst drive of ours is removed, never a network drive with the same letter.
    mapped = subprocess.run(["subst"], capture_output=True, text=True).stdout
    if f"{DRIVE}\\: => {WORLD}".lower() in mapped.lower():
        subprocess.run(["subst", DRIVE, "/d"], capture_output=True)
    # git makes its objects read-only: clear that so they can go.
    shutil.rmtree(WORLD, onerror=lambda f, p, _: (os.chmod(p, 0o666), f(p)))

if __name__ == "__main__":
    {"up": up, "down": down}[sys.argv[1]]()
