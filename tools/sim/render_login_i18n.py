#!/usr/bin/env python3
"""Render the device WeChat-login screen for every UI language and check that
the left text guide never reaches the QR area (x=156px)."""
import os
import re
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))
FIXTURE = os.path.join(HERE, "fixtures", "wechat-qr.jpg")
CELL_W, CELL_H = 6, 13
QR_X = 156
COLS = (QR_X // CELL_W) - 1
LANGS = ["en", "zh", "es", "ja", "de", "it"]
LABELS = {"en": "English", "zh": "中文", "es": "Español", "ja": "日本語", "de": "Deutsch", "it": "Italiano"}


def load_i18n():
    text = open(os.path.join(ROOT, "src", "i18n.rs"), encoding="utf-8").read()
    out = {}
    for lang in ["EN", "ZH", "ES", "JA", "DE", "IT"]:
        start = text.index(f"const {lang}: &[(&str, &str)] = &[")
        end = text.index("\n];", start)
        pairs = {}
        for key, val in re.findall(r'\("([^"]+)", "((?:[^"\\]|\\.)*)"\)', text[start:end]):
            val = re.sub(r"\\u\{([0-9a-fA-F]+)\}", lambda m: chr(int(m.group(1), 16)), val)
            val = val.replace('\\"', '"').replace("\\\\", "\\")
            pairs[key] = val
        out[lang.lower()] = pairs
    return out


def wrap(text, width):
    lines, cur = [], ""
    for ch in text:
        if len(cur) + 1 > width:
            lines.append(cur)
            cur = ""
        cur += ch
    if cur:
        lines.append(cur)
    return lines


def guide_lines(t, status=None):
    title = t.get("login_wechat_title", "WeChat login")
    page = t.get("login_wechat_page", "page: {}").replace("{}", "").rstrip()
    url = "http://192.168.1.9:8787"
    raw = [
        title,
        "-" * 23,
        t.get("login_qr_step1", "1 Scan with WeChat"),
        t.get("login_qr_step2", "2 Confirm on phone"),
        t.get("login_qr_step3", "3 Device finishes"),
    ]
    if status:
        raw.append("\u2192 " + status)
    raw += [
        "",
        page,
        url,
        "",
        t.get("artifact_back", "Esc back"),
    ]
    lines = []
    for item in raw:
        lines.extend(wrap(item, COLS) if item else [""])
    return lines


def main():
    i18n = load_i18n()
    tmp = tempfile.mkdtemp(prefix="dst-login-i18n-")
    ppm = os.path.join(tmp, "panel.ppm")
    subprocess.run([BIN, "--qr-ppm", FIXTURE, ppm], check=True, stdout=subprocess.DEVNULL)
    base = Image.open(ppm).convert("RGB")
    try:
        font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 10)
    except Exception:
        font = ImageFont.load_default()
    outdir = "/tmp/login_i18n"
    os.makedirs(outdir, exist_ok=True)
    failures = 0
    states = {
        "waiting": None,
        "scanned": i18n["en"].get("login_status_scanned", "scanned"),
        "finishing": i18n["en"].get("login_status_finishing", "finishing"),
    }
    for lang in LANGS:
        t = i18n[lang]
        countdown = t.get("login_status_countdown", "QR valid for {}").replace("{}", "4:50")
        for state, en_status in states.items():
            status = countdown if state == "waiting" else t.get(
                "login_status_scanned" if state == "scanned" else "login_status_finishing",
                en_status)
            lines = guide_lines(t, status)
            if len(lines) > 12:
                print(f"{lang}/{state}: too many lines ({len(lines)})")
                failures += 1
            lines = lines[:12]
        # render the waiting (countdown) state for the sheet
        lines = guide_lines(t, countdown)[:12]
        img = base.copy()
        draw = ImageDraw.Draw(img)
        max_x = 0
        for i, line in enumerate(lines):
            draw.text((1, 1 + i * CELL_H), line, fill=(230, 237, 243), font=font)
            max_x = max(max_x, 1 + draw.textlength(line, font=font))
        png = os.path.join(outdir, f"{lang}.png")
        img.resize((960, 510), Image.NEAREST).save(png)
        state_txt = "OK" if max_x <= QR_X - 4 else "OVERLAP"
        if state_txt != "OK":
            failures += 1
        print(f"{lang}: lines={len(lines)} text_max={max_x:.1f}px {state_txt}")
    # contact sheet (3x2)
    imgs = [Image.open(os.path.join(outdir, f"{l}.png")) for l in LANGS]
    w, h = imgs[0].size
    sheet = Image.new("RGB", (w * 2, h * 3), (8, 10, 14))
    for idx, im in enumerate(imgs):
        sheet.paste(im, ((idx % 2) * w, (idx // 2) * h))
    sheet.save("/tmp/login_i18n_sheet.png")
    print("sheet /tmp/login_i18n_sheet.png")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
