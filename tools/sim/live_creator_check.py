#!/usr/bin/env python3
"""live_creator_check.py -- creator mode end-to-end against the real local vLLM.

The scripted scenarios (`run_scenarios.py creator_*`) prove the mechanics with a
mock model; this pass proves that a **real model** can actually use creator mode:
it asks for a battery app, waits for the model to call `app_save`/`app_file`,
then opens `/apps`, shows the annotation preview and checks the package on disk.

It is a *directed* live check: the prompt names the creator tools and says not to
run the app, because the host is a Mac (no /sys battery) and because the point
of this pass is the save/inspect path, not free-form agent behaviour. Everything
else is real: real vLLM endpoint, real streaming, real tool calls and arguments.

    cd tools/sim
    python3 live_creator_check.py                  # uses the DSH settings.yaml endpoint
    python3 live_creator_check.py --out /tmp/x --keep-home

Exit status: 0 when the app package was written and shown in the transcript,
1 otherwise (missing endpoint, model did not save an app, package incomplete).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_live as rl  # endpoint discovery, config writer, contact sheet

DEFAULT_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           "screenshots", "live-creator")
DEFAULT_DOC = os.path.join(rl.ROOT, "docs", "live-creator.png")

PROMPT = ("在设备上做一个查看电池电量的 app：读取 "
          "/sys/class/power_supply/BAT0/capacity 显示电量和充电状态。"
          "用 app_save 和 app_file 把它保存成 app，告诉我 app 名字，不要运行它。")


def write_home(home, base, model, effort, language="zh"):
    os.makedirs(os.path.join(home, ".dst"), exist_ok=True)
    cfg = f"""model: {model}
effort: {effort}
max_tokens: 2048
api:
  base: {base}
  key: live-test-key
tui:
  mode: creator
  language: {language}
sandbox:
  mode: full
context:
  time: false
"""
    path = os.path.join(home, ".dst", "config.yml")
    open(path, "w", encoding="utf-8").write(cfg)
    os.chmod(path, 0o600)
    return home


def build_scenario():
    return {"steps": [
        {"wait_for": {"text": "dst", "timeout_ms": 30000}},
        {"snap": "01_startup"},
        {"send": PROMPT},
        {"key": "Enter"},
        {"wait_for": {"text": "saved app", "timeout_ms": 300000}},
        {"wait_stable": {"stable_ms": 1500, "timeout_ms": 120000}, "snap": "02_built"},
        {"send": "/apps"}, {"key": "Enter"}, {"wait_ms": 900, "snap": "03_apps"},
        {"key": "Right"}, {"wait_ms": 600, "snap": "04_preview"},
        {"key": "Escape"}, {"wait_ms": 300},
        {"key": "CtrlC"},
    ]}


def read_texts(out_dir):
    texts = {}
    path = os.path.join(out_dir, "manifest.json")
    if not os.path.exists(path):
        return texts
    for e in json.load(open(path, encoding="utf-8")).get("snapshots", []):
        name, text_path = e.get("name"), e.get("text")
        if name and text_path and os.path.exists(text_path):
            texts[name] = open(text_path, encoding="utf-8", errors="replace").read()
    return texts


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--bin", default=rl.DEFAULT_BIN)
    ap.add_argument("--base", default="")
    ap.add_argument("--model", default="")
    ap.add_argument("--effort", default="low",
                    help="reasoning effort (low keeps the pass fast on vLLM)")
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--doc", default=DEFAULT_DOC,
                    help="optional contact sheet destination ('' to skip)")
    ap.add_argument("--scale", type=int, default=3)
    ap.add_argument("--timeout", type=int, default=600,
                    help="simulator wall-clock timeout in seconds")
    ap.add_argument("--keep-home", action="store_true")
    args = ap.parse_args(argv)

    if not os.path.exists(args.bin):
        raise SystemExit(f"dst binary not found: {args.bin} (run cargo build)")
    base, model = rl.dsh_endpoint()
    base, model = args.base or base, args.model or model
    if not base:
        raise SystemExit("no endpoint: set DST_LIVE_BASE/DST_LIVE_MODEL or configure ~/.dsh/settings.yaml")
    rl.log(f"endpoint {base}  model {model}  effort {args.effort}")
    try:
        rl.probe_endpoint(base)
    except RuntimeError as exc:
        raise SystemExit(f"live endpoint unreachable: {exc}")

    work = tempfile.mkdtemp(prefix="dst-live-creator-")
    home = write_home(os.path.join(work, "home"), base, model, args.effort)
    cwd = os.path.join(work, "work")
    os.makedirs(cwd, exist_ok=True)
    # The scenario must not sit in dst's cwd: a model that explores its working
    # directory would find the test script (observed live).
    scen_dir = tempfile.mkdtemp(prefix="dst-live-creator-scen-")
    scen = os.path.join(scen_dir, "creator.json")
    json.dump(build_scenario(), open(scen, "w"), ensure_ascii=False, indent=1)

    out = args.out
    if os.path.isdir(out):
        shutil.rmtree(out)
    os.makedirs(out, exist_ok=True)
    env = dict(os.environ, DST_MOCK_SPEECH="1")
    cmd = [sys.executable, rl.SIM, "--bin", os.path.abspath(args.bin),
           "--home", home, "--cwd", cwd, "--out", out,
           "--scenario", scen, "--scale", str(args.scale)]
    t0 = time.time()
    r = subprocess.run(cmd, cwd=rl.ROOT, env=env, capture_output=True,
                       text=True, timeout=args.timeout)
    for line in (r.stdout + r.stderr).splitlines():
        if line.startswith("[sim]"):
            rl.log(line)
    if r.returncode != 0:
        print(r.stderr[-3000:])
        raise SystemExit(f"simulator failed (exit {r.returncode})")

    texts = read_texts(out)
    built = texts.get("02_built", "")
    apps = texts.get("03_apps", "")
    preview = texts.get("04_preview", "")
    apps_dir = os.path.join(home, ".dst", "apps")
    packages = []
    for root, _dirs, files in os.walk(apps_dir):
        if "app.json" in files:
            packages.append(root)
    hard = []
    if "saved app" not in built and "已保存应用" not in built:
        hard.append("the transcript never showed a saved app (saved-app notice)")
    if not packages:
        hard.append("no app package under ~/.dst/apps")
    for pkg in packages:
        for name in ("app.json", "APP.md", "run.sh"):
            if not os.path.exists(os.path.join(pkg, name)):
                hard.append(f"{pkg.replace(home, '~')} is missing {name}")
    if "应用" not in apps and "Apps" not in apps:
        hard.append("/apps did not render its title")
    if not re.search(r"(路径|path):\s*~/\.dst/apps/", preview):
        hard.append("/apps preview did not show the package path")

    if packages:
        rl.log(f"app package: {packages[0].replace(home, '~')}")
    for pkg in packages:
        manifest = os.path.join(pkg, "app.json")
        if os.path.exists(manifest):
            rl.log("manifest: " + open(manifest, encoding="utf-8").read().strip()[:200])
    if args.doc:
        os.makedirs(os.path.dirname(args.doc), exist_ok=True)
        sheet = rl.make_contact_sheet(
            out, ["01_startup", "02_built", "03_apps", "04_preview"],
            os.path.join(out, "contact_sheet.png"),
            f"creator mode live - {model} ({time.time()-t0:.0f}s)")
        if sheet:
            shutil.copyfile(sheet, args.doc)
            rl.log(f"contact sheet -> {args.doc}")
    if args.keep_home:
        rl.log(f"home kept at {home}")

    for problem in hard:
        print("FAIL " + problem)
    shots = len([f for f in os.listdir(out) if f.endswith(".png")])
    rl.log(f"{shots} screenshots in {out} ({time.time()-t0:.0f}s)")
    if hard:
        return 1
    print("OK creator mode end-to-end with the live model")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
