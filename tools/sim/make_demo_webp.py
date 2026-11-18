#!/usr/bin/env python3
"""make_demo_webp.py -- record the animated tour shown in the README.

`docs/demo.webp` is a screen recording, not a slideshow: screen_sim.py runs the
real `dst` binary with a scripted (local, deterministic) model and records one
rendered frame every ~110 ms, including the typing, the streaming answer, the
pinned progress row, the CtrlO panel and the side-question panel. Frames are
deduplicated, timed from the real wall clock and encoded as an animated WebP.

Also refreshes the contact sheets:

    docs/demo.webp               screen recording of a real session
    docs/live-walkthrough.png    12-panel contact sheet
    docs/live-creator.png        creator-mode contact sheet
    docs/login-i18n-sheet.png    six-language login sheet

    python3 make_demo_webp.py               # record + sheets (~4 min)
    python3 make_demo_webp.py --record-only # only docs/demo.webp (~1 min)
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile

import run_scenarios as rs
from run_demo import REPO as ROOT
from run_demo import SIM, start_mock, stop_mock
DOCS = os.path.join(ROOT, "docs")
BIN = os.path.join(ROOT, "target", "debug", "dst")

# The recording scenario: a real dst session, typed like a (fast) human.
TOUR_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scenarios", "demo_tour")
RECORD_INTERVAL_MS = 110
# WebP animation limits: a frame should never linger longer than this.
MAX_FRAME_MS = 700
MIN_FRAME_MS = 50


# Contact sheets: (doc path, title, cols, [(scenario, snapshot, label)])
SHEETS = {
    "live-walkthrough.png": (
        "dst on the Cardputer Zero -- device-fidelity simulator walkthrough",
        3,
        [
            ("tips", "tips", "01 startup"),
            ("progress", "pinned", "02 progress row"),
            ("progress", "running", "03 tool expanded"),
            ("ux", "done", "04 tool done"),
            ("paging", "page2", "05 scrollback"),
            ("esc_queue", "queued", "06 queued message"),
            ("voice_interrupt", "recording", "07 recording"),
            ("ux", "help_menu", "08 help"),
            ("config_menu", "menu", "09 config"),
            ("update_hwinfo", "updated", "10 hardware scan"),
            ("login_i18n_zh", "menu", "11 login (zh)"),
            ("creator_app_flow", "running", "12 an app running"),
        ],
    ),
    "live-creator.png": (
        "creator mode -- skills are the unit, meta.json makes them runnable",
        2,
        [
            ("creator_build", "built", "01 skill package written"),
            ("creator_build", "apps_preview_body", "02 SKILL.md in /apps"),
            ("creator_app_flow", "list", "03 /apps launcher"),
            ("creator_app_flow", "running", "04 app owning the panel"),
        ],
    ),
}

LOGIN_LANGS = ["en", "zh", "es", "ja", "de", "it"]


def _font(size):
    from PIL import ImageFont

    for path in (
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    ):
        try:
            return ImageFont.truetype(path, size)
        except Exception:
            continue
    return ImageFont.load_default()


def _png_for(work, snap):
    """Find a snapshot PNG inside a scenario work dir (multi-phase scenarios
    write one manifest.json per phase, so search them all)."""
    for dirpath, _dirs, files in os.walk(work):
        if "manifest.json" not in files:
            continue
        try:
            doc = json.load(open(os.path.join(dirpath, "manifest.json"), encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        for entry in doc.get("snapshots", []):
            if entry.get("name") == snap and entry.get("png") and os.path.exists(entry["png"]):
                return entry["png"]
    return None


def _collect(wanted):
    """Run each needed scenario once and map (scenario, snapshot) -> png path."""
    os.environ["DST_SCENARIO_PNG"] = "1"
    os.environ["DST_SIM_SCALE"] = "2"  # 640x340 panel renders
    os.environ["DST_KEEP_WORK"] = "1"  # run_scenario deletes the work dir otherwise
    cache, works = {}, {}
    for scen in sorted({s for s, _ in wanted}):
        if scen in works:
            continue
        print(f"  running {scen} ...", flush=True)
        _screens, _reqs, work = rs.run_scenario(scen, BIN)
        works[scen] = work
    for scen, snap in wanted:
        if (scen, snap) in cache:
            continue
        png = _png_for(works[scen], snap)
        if png is None or not os.path.exists(png):
            raise SystemExit(f"missing snapshot {scen}/{snap}")
        cache[(scen, snap)] = png
    return cache


def _dedupe_frames(frames):
    """Drop frames identical to their predecessor and derive real durations.

    Returns [(PIL image, duration_ms)]. The recording ticks at a fixed rate, so
    the time between two kept frames is exactly how long the first one was on
    the panel — that keeps typing/streaming motion while idle pauses stay cheap.
    """
    from PIL import Image

    def blank(image):
        """True for the pre-first-paint frame (nothing but the cursor)."""
        hist = image.convert("L").histogram()
        lit = sum(hist[48:])
        return lit < image.width * image.height * 0.002

    kept = []
    prev_hash = None
    started = False
    for frame in frames:
        with open(frame["path"], "rb") as fh:
            raw = fh.read()
        digest = hashlib.sha1(raw).hexdigest()
        if digest == prev_hash:
            continue
        prev_hash = digest
        image = Image.open(io.BytesIO(raw)).convert("RGB")
        if not started:
            # A recording that opens on an empty frame looks like a broken
            # slideshow: start at the first real paint (splash frame 1).
            if blank(image):
                continue
            started = True
        kept.append((frame["t_ms"], image))
    if not kept:
        raise SystemExit("recording produced no frames")
    images, durations = [], []
    for i, (_t, image) in enumerate(kept):
        if i + 1 < len(kept):
            ms = kept[i + 1][0] - kept[i][0]
        else:
            ms = 600
        durations.append(max(MIN_FRAME_MS, min(MAX_FRAME_MS, int(ms))))
        images.append(image)
    return images, durations


def build_recording():
    """Record a real dst session and write docs/demo.webp."""
    from PIL import Image

    work = tempfile.mkdtemp(prefix="dst-demo-")
    home = os.path.join(work, "home")
    frames_dir = os.path.join(work, "frames")
    out = os.path.join(work, "out")
    os.makedirs(os.path.join(home, ".dst"), exist_ok=True)
    os.makedirs(frames_dir, exist_ok=True)
    os.makedirs(out, exist_ok=True)
    overlay = os.path.join(TOUR_DIR, "home")
    if os.path.isdir(overlay):
        shutil.copytree(overlay, home, dirs_exist_ok=True)
    script = os.path.join(TOUR_DIR, "script.json")
    scenario = os.path.join(TOUR_DIR, "scenario.json")
    log_path = os.path.join(work, "requests.jsonl")
    env = dict(os.environ)
    mock, url, _state = start_mock(0, script, log_path, env)
    try:
        cfg = open(os.path.join(TOUR_DIR, "config.yml"), encoding="utf-8").read()
        cfg = cfg.replace("  key: test", f"  base: {url}\n  key: test")
        cfg_path = os.path.join(home, ".dst", "config.yml")
        with open(cfg_path, "w", encoding="utf-8") as fh:
            fh.write(cfg)
        os.chmod(cfg_path, 0o600)
        cmd = [
            sys.executable, SIM, "--bin", BIN, "--home", home, "--cwd", work,
            "--out", out, "--scenario", scenario, "--scale", "2",
            "--record-dir", frames_dir,
            "--record-interval-ms", str(RECORD_INTERVAL_MS),
        ]
        print("  recording demo_tour with the real dst binary ...", flush=True)
        proc = subprocess.run(cmd, cwd=os.path.dirname(os.path.abspath(__file__)))
        if proc.returncode != 0:
            raise SystemExit(f"screen_sim failed while recording (exit {proc.returncode})")
        with open(os.path.join(frames_dir, "frames.json"), encoding="utf-8") as fh:
            recorded = json.load(fh)
    finally:
        stop_mock(mock)
    frames, durations = _dedupe_frames(recorded["frames"])
    total_ms = sum(durations)
    path = os.path.join(DOCS, "demo.webp")
    frames[0].save(
        path,
        "WEBP",
        save_all=True,
        append_images=frames[1:],
        duration=durations,
        loop=0,
        quality=72,
        method=4,
    )
    kb = os.path.getsize(path) / 1024
    print(f"  wrote {os.path.relpath(path, ROOT)} "
          f"({len(frames)} frames, {kb:.0f} KB, {total_ms / 1000.0:.1f}s)")
    return work


def _sheet(path, title, cols, entries, cache):
    from PIL import Image, ImageDraw, ImageFont

    font = "/System/Library/Fonts/Menlo.ttc"
    try:
        title_font = ImageFont.truetype(font, 24)
        label_font = ImageFont.truetype(font, 17)
    except Exception:
        title_font = label_font = ImageFont.load_default()
    pad, label_h, title_h = 14, 26, 44
    imgs = [(label, Image.open(cache[key]).convert("RGB")) for key, label in entries]
    cell_w = max(im.width for _, im in imgs) + 2 * pad
    cell_h = max(im.height for _, im in imgs) + label_h + 2 * pad
    rows = (len(imgs) + cols - 1) // cols
    W = cols * cell_w + 2 * pad
    H = title_h + rows * cell_h + pad
    sheet = Image.new("RGB", (W, H), (12, 15, 20))
    draw = ImageDraw.Draw(sheet)
    draw.text((pad, 10), title, font=title_font, fill=(230, 237, 243))
    for i, (label, im) in enumerate(imgs):
        r, c = divmod(i, cols)
        x = pad + c * cell_w
        y = title_h + r * cell_h
        draw.text((x + pad + 4, y + 2), label, font=label_font, fill=(120, 200, 255))
        sheet.paste(im, (x + pad, y + label_h))
        draw.rectangle(
            [x + pad - 1, y + label_h - 1, x + pad + im.width, y + label_h + im.height],
            outline=(60, 70, 85),
        )
    sheet.save(path, "PNG")
    print(f"  wrote {os.path.relpath(path, ROOT)} ({sheet.width}x{sheet.height})")


def build_sheets(cache):
    for name, (title, cols, entries) in SHEETS.items():
        wanted = [(s, snap) for s, snap, _ in entries]
        for key in wanted:
            if key not in cache:
                raise SystemExit(f"sheet needs {key}")
        _sheet(os.path.join(DOCS, name), title, cols,
               [((s, snap), label) for s, snap, label in entries], cache)

    # six-language login sheet from the login_i18n_* scenarios
    entries = []
    for lang in LOGIN_LANGS:
        key = (f"login_i18n_{lang}", "menu")
        if key not in cache:
            raise SystemExit(f"login sheet needs {key}")
        entries.append((key, lang))
    _sheet(os.path.join(DOCS, "login-i18n-sheet.png"),
           "WeChat login on the panel -- every UI language (simulator render)",
           2, entries, cache)


def build_webp(cache):
    from PIL import Image

    frames, durations = [], []
    intro = _title_card([
        ("dst", _font(44), (236, 244, 252)),
        ("a DeepSeek harness on the Cardputer Zero", _font(22), (150, 205, 235)),
        ("Rust std only - one binary - 53x12 cells", _font(22), (140, 150, 165)),
        ("zero crates - no runtime - curl + /proc only", _font(17), (110, 120, 135)),
    ])
    frames.append(intro)
    durations.append(2800)
    total = len(FRAMES)
    for i, (scen, snap, caption, ms) in enumerate(FRAMES, 1):
        img = _frame(cache[(scen, snap)], caption, i, total)
        frames.append(img)
        durations.append(ms)
    outro = _title_card([
        ("the panel is the product", _font(22), (226, 234, 243)),
        ("built for 320x170 px", _font(17), (150, 205, 235)),
        ("long-esc exits any app - Tab to talk - PgUp/PgDn", _font(17), (140, 150, 165)),
    ])
    frames.append(outro)
    durations.append(2600)
    path = os.path.join(DOCS, "demo.webp")
    frames[0].save(
        path,
        "WEBP",
        save_all=True,
        append_images=frames[1:],
        duration=durations,
        loop=0,
        quality=80,
        method=4,
    )
    kb = os.path.getsize(path) / 1024
    print(f"  wrote {os.path.relpath(path, ROOT)} ({len(frames)} frames, {kb:.0f} KB)")


def main(argv):
    if not os.path.exists(BIN):
        raise SystemExit(f"build dst first: cargo build ({BIN} missing)")
    os.makedirs(DOCS, exist_ok=True)
    if "--storyboard" in argv:
        with open(os.path.join(TOUR_DIR, "scenario.json"), encoding="utf-8") as fh:
            for i, step in enumerate(json.load(fh)["steps"], 1):
                print(f"{i:02d} {json.dumps(step, ensure_ascii=False)}")
        return 0
    if "--sheets-only" not in argv:
        print("recording the animated tour (real dst binary, scripted API)")
        build_recording()
    if "--record-only" in argv:
        return 0
    print("collecting contact-sheet snapshots")
    wanted = []
    for _name, (_t, _c, entries) in SHEETS.items():
        wanted += [(s, snap) for s, snap, _ in entries]
    wanted += [(f"login_i18n_{lang}", "menu") for lang in LOGIN_LANGS]
    cache = _collect(wanted)
    print("writing contact sheets")
    build_sheets(cache)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
