#!/usr/bin/env python3
"""Screenshot the LAN login page with Chrome headless (host-only convenience).

Chrome headless enforces a minimum layout viewport of 500px, so the screenshot
is taken at 500x1100; the page itself is responsive down to narrow phones.
Usage: python3 screenshot_login_page.py [lang] [out.png]
"""
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))
CHROME = os.environ.get(
    "CHROME_BIN", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
)


def main():
    lang = sys.argv[1] if len(sys.argv) > 1 else "en"
    out = sys.argv[2] if len(sys.argv) > 2 else f"/tmp/login-page-{lang}.png"
    if not os.path.exists(CHROME):
        print("SKIP chrome not found:", CHROME)
        return 0
    env = dict(os.environ, DST_LOGIN_HOST="127.0.0.1")
    srv = subprocess.Popen(
        [BIN, "--login-server-test", "8949", lang],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)
    try:
        url = None
        for _ in range(60):
            line = srv.stdout.readline()
            if line.startswith("URL "):
                url = line[4:].strip().rstrip("/")
                break
        if not url:
            print("FAIL no server URL")
            return 1
        subprocess.run(
            [CHROME, "--headless=new", "--disable-gpu", "--no-sandbox",
             "--proxy-server=direct://", "--proxy-bypass-list=*",
             "--virtual-time-budget=700", f"--window-size=500,1450",
             f"--screenshot={out}", url + "/"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
        if not os.path.exists(out):
            print("FAIL no screenshot")
            return 1
        print("PNG", out)
        return 0
    finally:
        srv.terminate()
        try:
            srv.wait(timeout=3)
        except Exception:
            srv.kill()


if __name__ == "__main__":
    sys.exit(main())
