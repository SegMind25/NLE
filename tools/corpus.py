#!/usr/bin/env python3
"""A corpus of real Windows programs to measure NLE with, and a ranking of what it still lacks.

The programs are the official release builds of well-known open-source tools. They are downloaded into
target/corpus/<name>/ (never into the repository), checked with `nle inspect`, and optionally run:

    python3 -I tools/corpus.py                 # download what is missing and report the verdicts
    python3 -I tools/corpus.py --run           # also run each program's smoke test through nle
    python3 -I tools/corpus.py --only rg fd    # only these
    python3 -I tools/corpus.py --missing       # the unimplemented functions, most wanted first

The programs are untrusted downloads and `--run` executes them (natively, with your user's rights, and never through
Wine: every run passes --backend native); use a throw-away environment.
"""

import argparse
import collections
import json
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NLE = ROOT / "target" / "release" / "nle"
CORPUS = ROOT / "target" / "corpus"

# name, url, archive kind (exe, zip, tgz), the program inside, smoke-test arguments, text the output must contain.
# `gui` programs are run on the headless display for a moment instead.
PROGRAMS = [
    dict(name="ripgrep", url="https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-pc-windows-msvc.zip", kind="zip", exe="ripgrep-14.1.1-x86_64-pc-windows-msvc/rg.exe", args=["--version"], expect="ripgrep 14.1.1"),
    dict(name="fd", url="https://github.com/sharkdp/fd/releases/download/v10.2.0/fd-v10.2.0-x86_64-pc-windows-msvc.zip", kind="zip", exe="fd-v10.2.0-x86_64-pc-windows-msvc/fd.exe", args=["--version"], expect="fd 10.2.0"),
    dict(name="bat", url="https://github.com/sharkdp/bat/releases/download/v0.24.0/bat-v0.24.0-x86_64-pc-windows-msvc.zip", kind="zip", exe="bat-v0.24.0-x86_64-pc-windows-msvc/bat.exe", args=["--version"], expect="bat 0.24.0"),
    dict(name="hyperfine", url="https://github.com/sharkdp/hyperfine/releases/download/v1.19.0/hyperfine-v1.19.0-x86_64-pc-windows-msvc.zip", kind="zip", exe="hyperfine-v1.19.0-x86_64-pc-windows-msvc/hyperfine.exe", args=["--version"], expect="hyperfine 1.19.0"),
    dict(name="jq", url="https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-windows-amd64.exe", kind="exe", exe="jq.exe", args=["--version"], expect="jq-1.7.1"),
    dict(name="7zr", url="https://github.com/ip7z/7zip/releases/download/24.09/7zr.exe", kind="exe", exe="7zr.exe", args=["i"], expect="7-Zip"),
    dict(name="fzf", url="https://github.com/junegunn/fzf/releases/download/v0.55.0/fzf-0.55.0-windows_amd64.zip", kind="zip", exe="fzf.exe", args=["--version"], expect="0.55.0"),
    dict(name="notepad++", url="https://github.com/notepad-plus-plus/notepad-plus-plus/releases/download/v8.7.9/npp.8.7.9.portable.x64.zip", kind="zip", exe="notepad++.exe", args=[], expect=None, gui=True),
]


def fetch(p: dict) -> Path:
    d = CORPUS / p["name"]
    exe = d / p["exe"]
    if exe.exists():
        return exe
    d.mkdir(parents=True, exist_ok=True)
    blob = d / "download"
    print(f"downloading {p['name']} ...", flush=True)
    req = urllib.request.Request(p["url"], headers={"User-Agent": "nle-corpus"})
    with urllib.request.urlopen(req, timeout=120) as r, open(blob, "wb") as f:
        shutil.copyfileobj(r, f)
    if p["kind"] == "zip":
        with zipfile.ZipFile(blob) as z:
            z.extractall(d)
    elif p["kind"] == "tgz":
        with tarfile.open(blob) as t:
            t.extractall(d, filter="data")
    else:
        shutil.move(blob, exe)
        return exe
    blob.unlink(missing_ok=True)
    return exe


def inspect(exe: Path) -> dict:
    out = subprocess.run([str(NLE), "inspect", "--json", str(exe)], capture_output=True, text=True)
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return {"error": (out.stderr or out.stdout).strip()[:200]}


def smoke(p: dict, exe: Path) -> str:
    env = dict(os.environ, NLE_PREFIX=str(CORPUS / ".prefix"))
    try:
        if p.get("gui"):
            shot = CORPUS / p["name"] / "shot.png"
            shot.unlink(missing_ok=True)
            env.update(NLE_GUI="headless", NLE_GUI_SCRIPT="wait 3000; snapshot %s; close" % shot)
            r = subprocess.run([str(NLE), "run", "--backend", "native", str(exe)] + p["args"], capture_output=True, text=True, timeout=60, env=env, cwd=exe.parent)
            # a window is only a success when the program also came through the script's close without a fatal error
            ok = shot.exists() and r.returncode == 0 and "nle: fatal" not in r.stderr and "unhandled exception" not in r.stderr
            return "ok (window drawn, closed cleanly)" if ok else f"FAIL exit {r.returncode}: {(r.stderr or r.stdout).strip()[-160:]}"
        r = subprocess.run([str(NLE), "run", "--backend", "native", str(exe)] + p["args"], capture_output=True, text=True, timeout=60, env=env, cwd=exe.parent)
        text = r.stdout + r.stderr
        if p["expect"] and p["expect"] in text:
            return "ok"
        return f"FAIL exit {r.returncode}: {text.strip()[-160:]}"
    except subprocess.TimeoutExpired:
        return "FAIL timeout"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", action="store_true", help="run the smoke tests")
    ap.add_argument("--missing", action="store_true", help="rank the unimplemented functions")
    ap.add_argument("--only", nargs="*", help="names of the programs to use")
    a = ap.parse_args()
    if not NLE.exists():
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)
    wanted = [p for p in PROGRAMS if not a.only or p["name"] in a.only]
    wanted_missing = collections.Counter()
    rows = []
    for p in wanted:
        try:
            exe = fetch(p)
        except Exception as e:  # a download that fails should not stop the others
            rows.append((p["name"], "download failed", "", str(e)[:60]))
            continue
        rep = inspect(exe)
        if "error" in rep:
            rows.append((p["name"], "unreadable", "", rep["error"]))
            continue
        total = sum(l["resolved"] + l["unresolved"] for l in rep.get("libraries", {}).values())
        missing = rep.get("unresolved", [])
        for name in missing:
            wanted_missing[name.lower()] += 1
        result = smoke(p, exe) if a.run else ""
        rows.append((p["name"], rep.get("verdict", "?"), f"{len(missing)}/{total} missing", result))
    w = max(len(r[0]) for r in rows) if rows else 8
    for r in rows:
        print(f"{r[0]:<{w}}  {r[1]:<12} {r[2]:<18} {r[3]}")
    if a.missing:
        print("\nmost wanted missing functions (programs needing them):")
        for name, n in wanted_missing.most_common(60):
            print(f"  {n:>3}  {name}")


if __name__ == "__main__":
    main()
