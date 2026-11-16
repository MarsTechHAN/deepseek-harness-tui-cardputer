#!/usr/bin/env python3
"""Verify that the device QR screen is actually scannable.

Renders exactly the framebuffer image dst would blit for the WeChat login QR
(same geometry as the panel: 320x170, QR on the right, text guide on the left),
then decodes it with jsQR. Requires: `npm install --no-save jsqr` in tools/sim.
"""
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))
FIXTURE = os.path.join(HERE, "fixtures", "wechat-qr.jpg")


def main():
    if not os.path.exists(FIXTURE):
        print("FAIL fixture missing:", FIXTURE)
        return 1
    tmp = tempfile.mkdtemp(prefix="dst-qr-")
    ppm = os.path.join(tmp, "panel.ppm")
    r = subprocess.run([BIN, "--qr-ppm", FIXTURE, ppm], capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(ppm):
        print("FAIL render:", r.stdout, r.stderr)
        return 1
    r = subprocess.run(["node", os.path.join(HERE, "qr_scan.js"), ppm], capture_output=True, text=True)
    out = (r.stdout or "").strip()
    print(out or r.stderr.strip())
    return 0 if out.startswith("DECODED ") else 1


if __name__ == "__main__":
    sys.exit(main())
