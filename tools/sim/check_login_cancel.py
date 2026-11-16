#!/usr/bin/env python3
"""Verify that the phone page can cancel an in-flight WeChat login."""
import json
import os
import subprocess
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))


def post(url):
    req = urllib.request.Request(url, data=b"", method="POST")
    return urllib.request.urlopen(req, timeout=5).read()


def main():
    mock = subprocess.Popen(
        [sys.executable, os.path.join(HERE, "mock_login.py"), "--port", "0"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        env=dict(os.environ, MOCK_LOGIN_POLL_DELAY="2"))
    base = mock.stdout.readline().strip().rstrip("/")
    env = dict(os.environ, DST_LOGIN_HOST="127.0.0.1",
               DST_LOGIN_PLATFORM=base, DST_LOGIN_WECHAT=base,
               DST_LOGIN_WECHAT_LP=base, DST_LOGIN_API=base)
    srv = subprocess.Popen([BIN, "--login-server-test", "8935", "en"],
                           stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                           text=True, env=env)
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
        post(url + "/api/login")
        time.sleep(0.3)
        post(url + "/api/cancel")
        state = {}
        for _ in range(40):
            time.sleep(0.2)
            try:
                state = json.loads(urllib.request.urlopen(url + "/api/state", timeout=5).read())
            except Exception:
                break
            if state.get("cancelled") and state.get("phase") == "failed":
                break
        ok = state.get("cancelled") is True and state.get("phase") == "failed"
        print("cancel state:", {k: state.get(k) for k in ("phase", "message", "cancelled")})
        print("OK" if ok else "FAIL")
        return 0 if ok else 1
    finally:
        srv.terminate()
        mock.terminate()
        try:
            srv.wait(timeout=3)
        except Exception:
            srv.kill()
        try:
            mock.wait(timeout=3)
        except Exception:
            mock.kill()


if __name__ == "__main__":
    sys.exit(main())
