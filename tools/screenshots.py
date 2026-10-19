#!/usr/bin/env python3
"""Renders real NLE output as terminal screenshots in docs/screenshots/.

Nothing here is staged: each scene runs its commands in a pseudo-terminal (so programs see a
real TTY and use colour), captures the bytes they print, and draws them in a terminal-style
window with a headless Chromium. Re-run it after a change to see the new output:

    python3 tools/screenshots.py              # all scenes
    python3 tools/screenshots.py inspect run  # only these

Needs: a release build (built on demand), MinGW-w64 (x86_64-w64-mingw32-gcc / g++) for the
example programs, and optionally Go and the x86_64-pc-windows-gnu Rust target for the
Go / Rust scene. Pillow is used to shrink the PNGs.
"""

import html
import os
import pty
import re
import shutil
import struct
import subprocess
import sys
import termios
import fcntl
import glob
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "screenshots"
NLE = ROOT / "target" / "release" / "nle"
DEMO = Path("/home/user/demo") if Path("/home/user").is_dir() else Path.home() / "demo"

SCENES = [
    {
        "name": "01-run",
        "title": "nle sysinfo.exe",
        "width": 920,
        "cmds": ["file sysinfo.exe", "nle sysinfo.exe"],
    },
    {
        "name": "02-inspect",
        "title": "nle inspect",
        "width": 920,
        "cmds": ["nle inspect sysinfo.exe"],
    },
    {
        "name": "03-relink",
        "title": "nle relink",
        "width": 1040,
        "cmds": [
            "nle relink sysinfo.exe -o dist",
            "file dist/sysinfo",
            "ldd dist/sysinfo | grep -E 'libnle|libc.so'",
            "./dist/sysinfo | head -n 8",
        ],
    },
    {
        "name": "04-go-and-rust",
        "title": "Go and Rust programs, no C runtime",
        "width": 1040,
        "cmds": [
            "file go_hello.exe rs_hello.exe",
            "nle go_hello.exe one two; echo \"exit code: $?\"",
            "nle rs_hello.exe one two; echo \"exit code: $?\"",
        ],
    },
    {
        "name": "05-limits",
        "title": "what NLE cannot run",
        "width": 1040,
        "cmds": ["nle inspect window.exe", "nle window.exe"],
    },
    {
        "name": "06-doctor",
        "title": "nle doctor",
        "width": 1040,
        "cmds": ["nle doctor"],
    },
    {
        "name": "07-coverage",
        "title": "nle coverage",
        "width": 760,
        "cmds": ["nle coverage"],
    },
    {
        "name": "08-test-programs",
        "title": "real Windows programs, run natively",
        "width": 1100,
        "cmds": [
            "for t in threads files memory unicode seh dll net process registry; do printf '%-9s' $t; nle t_$t.exe | tail -n 1; done",
        ],
    },
]

# --- terminal colours (GitHub dark) --------------------------------------------------------
FG = "#C9D1D9"
PALETTE = {
    30: "#484F58", 31: "#FF7B72", 32: "#3FB950", 33: "#D29922",
    34: "#58A6FF", 35: "#BC8CFF", 36: "#39C5CF", 37: "#B1BAC4",
    90: "#6E7681", 91: "#FFA198", 92: "#56D364", 93: "#E3B341",
    94: "#79C0FF", 95: "#D2A8FF", 96: "#56D4DD", 97: "#F0F6FC",
}


def ansi_to_html(data: str) -> str:
    """Turns SGR colour sequences into spans; every other escape sequence is dropped."""
    data = data.replace("\r\n", "\n").replace("\r", "")
    out, state, pos = [], {"b": False, "d": False, "fg": None}, 0

    def open_span():
        style = []
        if state["fg"]:
            style.append(f"color:{state['fg']}")
        if state["b"]:
            style.append("font-weight:700")
        if state["d"]:
            style.append("opacity:.6")
        return f'<span style="{";".join(style)}">' if style else ""

    current = ""
    for m in re.finditer(r"\x1b\[([0-9;?]*)([A-Za-z])", data):
        out.append(current + html.escape(data[pos:m.start()]) + ("</span>" if current else ""))
        current = ""
        pos = m.end()
        if m.group(2) == "m":
            codes = [int(c) for c in m.group(1).split(";") if c.isdigit()] or [0]
            for c in codes:
                if c == 0:
                    state.update(b=False, d=False, fg=None)
                elif c == 1:
                    state["b"] = True
                elif c == 2:
                    state["d"] = True
                elif c == 22:
                    state.update(b=False, d=False)
                elif c in PALETTE:
                    state["fg"] = PALETTE[c]
                elif c == 39:
                    state["fg"] = None
        current = open_span()
    out.append(current + html.escape(data[pos:]) + ("</span>" if current else ""))
    return "".join(out)


# --- running commands ------------------------------------------------------------------------
def run_in_pty(command: str, env: dict, cols: int = 110) -> str:
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(DEMO)
        os.execvpe("bash", ["bash", "-c", command], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, cols, 0, 0))
    chunks = []
    while True:
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        chunks.append(data)
    os.waitpid(pid, 0)
    return b"".join(chunks).decode("utf-8", "replace")


def sh(*args, cwd=None, check=True, **kw):
    return subprocess.run(args, cwd=cwd, check=check, text=True, capture_output=True, **kw)


def prepare():
    if not NLE.exists():
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)
    shutil.rmtree(DEMO, ignore_errors=True)
    DEMO.mkdir(parents=True)
    # example programs
    subprocess.run([str(ROOT / "examples" / "build.sh")], check=True, capture_output=True)
    for exe in (ROOT / "examples" / "bin").glob("*.exe"):
        shutil.copy(exe, DEMO / exe.name)
    # the test programs
    gcc, gxx = "x86_64-w64-mingw32-gcc", "x86_64-w64-mingw32-g++"
    src = ROOT / "tests" / "exe"
    for t in ["t_threads", "t_files", "t_memory", "t_system", "t_unicode", "t_process"]:
        sh(gcc, "-O1", "-o", str(DEMO / f"{t}.exe"), str(src / f"{t}.c"))
    sh(gcc, "-O1", "-o", str(DEMO / "t_net.exe"), str(src / "t_net.c"), "-lws2_32")
    sh(gcc, "-O1", "-o", str(DEMO / "t_registry.exe"), str(src / "t_registry.c"), "-lshlwapi")
    sh(gcc, "-O1", "-shared", "-static-libgcc", "-o", str(DEMO / "helper.dll"), str(src / "helper.c"),
       f"-Wl,--out-implib,{DEMO / 'libhelper.dll.a'}")
    sh(gcc, "-O1", "-o", str(DEMO / "t_dll.exe"), str(src / "t_dll.c"), "-L", str(DEMO), "-lhelper")
    sh(gxx, "-O1", "-fexceptions", "-static", "-o", str(DEMO / "t_seh.exe"), str(src / "t_seh.cpp"))
    # Go and Rust programs
    if shutil.which("go"):
        env = dict(os.environ, GOOS="windows", GOARCH="amd64", CGO_ENABLED="0", GOCACHE=str(DEMO / ".gocache"))
        sh("go", "build", "-o", str(DEMO / "go_hello.exe"), str(ROOT / "tests" / "lang" / "hello.go"), env=env)
    if shutil.which("rustc"):
        sh("rustc", "--target", "x86_64-pc-windows-gnu", "-O", "-o", str(DEMO / "rs_hello.exe"),
           str(ROOT / "tests" / "lang" / "hello.rs"))
    shutil.rmtree(DEMO / ".gocache", ignore_errors=True)


# --- drawing -----------------------------------------------------------------------------------
PAGE = """<!doctype html><meta charset="utf-8">
<style>
  html, body {{ margin: 0; background: transparent; }}
  #shot {{ box-sizing: border-box; width: {width}px; padding: 34px;
    background: radial-gradient(700px 360px at 8% 0%, rgba(56,189,248,.22), transparent 62%),
                radial-gradient(600px 360px at 100% 100%, rgba(52,211,153,.16), transparent 62%), #0A1020; }}
  .win {{ border-radius: 12px; overflow: hidden; background: #0D1117; border: 1px solid #30363D;
    box-shadow: 0 24px 60px rgba(0,0,0,.55); }}
  .bar {{ position: relative; height: 38px; background: #161B22; border-bottom: 1px solid #30363D; }}
  .dot {{ position: absolute; top: 13px; width: 12px; height: 12px; border-radius: 50%; }}
  .title {{ position: absolute; inset: 0; text-align: center; color: #8B949E;
    font: 14px/38px "DejaVu Sans", "Liberation Sans", Arial, sans-serif; }}
  pre {{ margin: 0; padding: 18px 22px 20px; color: {fg}; white-space: pre-wrap; overflow-wrap: anywhere;
    font: 16px/1.5 "DejaVu Sans Mono", "Liberation Mono", monospace; }}
  .p {{ color: #3FB950; font-weight: 700; }}
  .c {{ color: #F0F6FC; font-weight: 700; }}
</style>
<div id="shot"><div class="win">
  <div class="bar"><span class="dot" style="left:16px;background:#FF5F57"></span><span class="dot" style="left:36px;background:#FEBC2E"></span><span class="dot" style="left:56px;background:#28C840"></span><div class="title">{title}</div></div>
<pre>{body}</pre>
</div></div>
<script>addEventListener('load', function () {{ document.documentElement.setAttribute('data-h', document.getElementById('shot').offsetHeight); }});</script>
"""


def chrome():
    for pattern in ("/opt/pw-browsers/chromium_headless_shell-*/chrome-linux/headless_shell",):
        found = sorted(glob.glob(pattern))
        if found:
            return found[-1]
    for name in ("chromium", "chromium-browser", "google-chrome"):
        if shutil.which(name):
            return shutil.which(name)
    sys.exit("no Chromium found; set up a headless Chromium first")


def render(scene: dict, body_html: str, tmp: Path):
    page = tmp / f"{scene['name']}.html"
    page.write_text(PAGE.format(width=scene["width"], title=html.escape(scene["title"]), body=body_html, fg=FG))
    base = [chrome(), "--headless", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", f"file://{page}"]
    dom = subprocess.run(base[:-1] + ["--window-size=%d,1200" % scene["width"], "--dump-dom", base[-1]],
                         capture_output=True, text=True, check=True).stdout
    height = int(re.search(r'data-h="(\d+)"', dom).group(1))
    png = OUT / f"{scene['name']}.png"
    subprocess.run(base[:-1] + ["--force-device-scale-factor=2", f"--window-size={scene['width']},{height}",
                                f"--screenshot={png}", base[-1]], check=True, capture_output=True)
    try:
        from PIL import Image
        # Lossless, but with the alpha channel dropped and the encoder at full effort: the flat
        # terminal colours compress well, and a palette would blur the traffic-light dots.
        Image.open(png).convert("RGB").save(png, optimize=True, compress_level=9)
    except ImportError:
        pass
    print(f"wrote {png.relative_to(ROOT)}  ({png.stat().st_size // 1024} KiB)")


def main():
    wanted = sys.argv[1:]
    scenes = [s for s in SCENES if not wanted or any(w in s["name"] for w in wanted)]
    OUT.mkdir(parents=True, exist_ok=True)
    prepare()
    env = dict(
        os.environ,
        PATH=f"{NLE.parent}:{os.environ['PATH']}",
        TERM="xterm-256color",
        HOME=str(DEMO.parent),
        USER="user",
        NLE_PREFIX=str(DEMO / ".prefix"),
    )
    env.pop("NO_COLOR", None)
    tmp = ROOT / "target" / "screenshots-tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    for scene in scenes:
        parts = []
        for cmd in scene["cmds"]:
            parts.append(f'<span class="p">$</span> <span class="c">{html.escape(cmd)}</span>\n')
            parts.append(ansi_to_html(run_in_pty(cmd, env)))
        body = "".join(parts).rstrip("\n")
        render(scene, body, tmp)
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
