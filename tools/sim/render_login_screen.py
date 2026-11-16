#!/usr/bin/env python3
"""Render a device-accurate WeChat login screen (text left, QR right) for review.

Terminal cells on the Cardputer panel are 6x13 px (53x12 cells), and the QR is
blitted to x=156..320. This composes the same guide lines and the real
framebuffer QR image so overlap/readability can be checked visually.
"""
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))
FIXTURE = os.path.join(HERE, "fixtures", "wechat-qr.jpg")

# 53 columns x 12 rows at 6x13 px, but the QR occupies x=156..320.
CELL_W, CELL_H = 6, 13
TEXT_COLS = (156 // CELL_W) - 1  # 25 -> keep a 1-cell gap before the QR
GUIDE = [
    "WeChat login",
    "-----------------------",
    "1 Scan with WeChat",
    "2 Confirm on phone",
    "3 Device finishes",
    "",
    "page:",
    "http://192.168.1.9:8787",
    "",
    "Esc back",
]


def wrap(text, width):
    out, cur = [], ""
    for ch in text:
        if len(cur) + 1 > width:
            out.append(cur)
            cur = ""
        cur += ch
    if cur:
        out.append(cur)
    return out


def main():
    tmp = tempfile.mkdtemp(prefix="dst-login-screen-")
    ppm = os.path.join(tmp, "panel.ppm")
    subprocess.run([BIN, "--qr-ppm", FIXTURE, ppm], check=True,
                   stdout=subprocess.DEVNULL)
    img = Image.open(ppm).convert("RGB")
    try:
        font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 10)
    except Exception:
        font = ImageFont.load_default()
    draw = ImageDraw.Draw(img)
    lines = []
    for g in GUIDE:
        lines.extend(wrap(g, TEXT_COLS))
    max_x = 0
    for i, line in enumerate(lines[:12]):
        x, y = 1, 1 + i * CELL_H
        draw.text((x, y), line, fill=(230, 237, 243), font=font)
        w = draw.textlength(line, font=font)
        max_x = max(max_x, x + w)
    print(f"text extent: {max_x:.1f}px, QR starts at 156px, cols={TEXT_COLS}")
    if max_x > 152:
        print("FAIL: text overlaps the QR area")
        return 1
    out = os.path.join("/tmp", "wechat_login_screen.png")
    img.resize((960, 510), Image.NEAREST).save(out)
    print("PNG", out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
