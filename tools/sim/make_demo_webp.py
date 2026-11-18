#!/usr/bin/env python3
"""make_demo_webp.py -- build the animated tour shown in the README.

It drives a curated set of simulator scenarios through the real `dst` binary
(scripted mock API, device-fidelity renderer at 640x340), collects the named
panel snapshots, overlays a caption and writes:

    docs/demo.webp               animated tour (whole flow + key features)
    docs/live-walkthrough.png    refreshed 12-panel contact sheet
    docs/live-creator.png        refreshed creator-mode contact sheet
    docs/login-i18n-sheet.png    refreshed six-language login sheet

The images are simulator renders, not photos of the device: same binary, same
53x12 cell renderer and same fonts as the panel, but the model replies are
scripted so the demo is deterministic and needs no API.

    python3 make_demo_webp.py            # ~2 min, writes docs/
    python3 make_demo_webp.py --frames   # only print the frame list
"""

from __future__ import annotations

import json
import os
import sys
import tempfile

import run_scenarios as rs
from run_demo import REPO as ROOT
DOCS = os.path.join(ROOT, "docs")
BIN = os.path.join(ROOT, "target", "debug", "dst")

# Every frame must share one size for the WebP animation: 640x340 panel render
# plus a caption bar (as in the contact sheets).
FRAME_W, FRAME_H = 672, 396

# (scenario, snapshot, caption, duration_ms)
FRAMES = [
    ("tips", "tips", "First run: it lives on the panel, 53x12", 2200),
    ("progress", "pinned", "state_report pins the current stage on top", 2000),
    ("progress", "running", "Tool calls expand their arguments as they run", 2000),
    ("progress", "ctrlo", "CtrlO opens the tool log during a turn", 2000),
    ("ux", "done", "Results land in the transcript as tool cards", 2000),
    ("read_image_tool", "reply", "The agent opens an image itself with read_image", 2200),
    ("image_input", "reply", "Or you attach one with /image: the model sees it", 2200),
    ("paging", "page2", "PgUp/PgDn page the transcript without blanking", 2000),
    ("esc_queue", "queued", "Typing during a turn queues the next message", 2000),
    ("progress", "interrupted", "Esc stops the whole turn, not just one tool", 2000),
    ("voice_interrupt", "recording", "Tab interrupts everything and records at once", 2200),
    ("voice_interrupt", "transcribed", "The take becomes the prompt (send or edit)", 2000),
    ("creator_build", "built", "Creator mode: the skill package is the unit", 2200),
    ("creator_app_flow", "list", "/apps lists runnable skill packages", 2000),
    ("creator_app_flow", "running", "An app owns the whole panel, long-Esc returns", 2400),
    ("onboarding_hw", "asked", "First setup offers a real hardware scan", 2200),
    ("update_hwinfo", "updated", "/update-hwinfo refreshes the hardware skill", 2200),
    ("login_i18n_zh", "menu", "Every screen exists in en/zh/es/ja/de/it", 2000),
    ("ux", "help_menu", "/help is a real command menu with previews", 2000),
    ("config_menu", "menu", "/config edits the same keys as config.yml", 2000),
    ("progress", "next", "Then the conversation simply continues", 1800),
]

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


def _frame(png_path, caption, index, total):
    from PIL import Image, ImageDraw

    im = Image.open(png_path).convert("RGB")
    pad, bar = 16, 40
    w, h = im.width + 2 * pad, im.height + bar + pad
    if (w, h) != (FRAME_W, FRAME_H):
        # Run at --scale 2 for 640x340; resize defensively so every animation
        # frame has identical dimensions.
        im = im.resize((FRAME_W - 2 * pad, FRAME_H - bar - pad), Image.LANCZOS)
        w, h = FRAME_W, FRAME_H
    out = Image.new("RGB", (w, h), (13, 17, 23))
    draw = ImageDraw.Draw(out)
    # panel drop shadow / border
    draw.rectangle([pad - 1, bar - 1, pad + im.width, bar + im.height], fill=(40, 48, 60))
    out.paste(im, (pad, bar))
    title = _font(19)
    small = _font(15)
    counter = f"{index}/{total}"
    counter_w = draw.textlength(counter, font=small)
    room = w - 2 * pad - counter_w - 14
    shown = caption
    while shown and draw.textlength(shown, font=title) > room:
        shown = shown[:-1]
    if shown != caption:
        shown = shown[:-1] + "\u2026"
    draw.text((pad, 9), shown, font=title, fill=(226, 234, 243))
    draw.text((w - pad - counter_w, 12), counter, font=small, fill=(110, 160, 210))
    return out


def _title_card(lines):
    from PIL import Image, ImageDraw

    im = Image.new("RGB", (FRAME_W, FRAME_H), (11, 18, 27))
    draw = ImageDraw.Draw(im)
    # accent line, echoing the splash
    draw.rectangle([48, 104, 48 + 112, 108], fill=(80, 180, 235))
    y = 122
    for text, font, fill in lines:
        draw.text((48, y), text, font=font, fill=fill)
        # crude step by glyph height so title/subtitle spacing stays even
        y += int(font.size * 1.35)
    return im


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
    if "--frames" in argv:
        for i, (scen, snap, caption, ms) in enumerate(FRAMES, 1):
            print(f"{i:02d} {scen:18s} {snap:18s} {ms:5d}ms  {caption}")
        return 0
    if not os.path.exists(BIN):
        raise SystemExit(f"build dst first: cargo build ({BIN} missing)")
    os.makedirs(DOCS, exist_ok=True)
    print("collecting simulator snapshots (real dst binary, scripted API)")
    wanted = [(s, snap) for s, snap, _, _ in FRAMES]
    for _name, (_t, _c, entries) in SHEETS.items():
        wanted += [(s, snap) for s, snap, _ in entries]
    wanted += [(f"login_i18n_{lang}", "menu") for lang in LOGIN_LANGS]
    cache = _collect(wanted)
    print("writing contact sheets")
    build_sheets(cache)
    print("writing animated webp")
    build_webp(cache)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
