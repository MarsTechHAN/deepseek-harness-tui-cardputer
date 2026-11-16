#!/usr/bin/env python3
"""Post a DSH token to the login page and verify the device receives it."""
import os
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))


def main():
    env = dict(os.environ, DST_LOGIN_HOST="127.0.0.1")
    srv = subprocess.Popen([BIN, "--login-server-test", "8950", "en"],
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
        req = urllib.request.Request(
            url + "/save-token",
            data=b"token=dshtesttoken1234567890abcdef",
            headers={"content-type": "application/x-www-form-urlencoded"},
            method="POST")
        urllib.request.urlopen(req, timeout=5).read()
        for _ in range(20):
            line = srv.stdout.readline().strip()
            if line.startswith("TOKENPASTE "):
                print("OK", line)
                return 0
        print("FAIL token was not delivered")
        return 1
    finally:
        srv.terminate()
        try:
            srv.wait(timeout=3)
        except Exception:
            srv.kill()


if __name__ == "__main__":
    sys.exit(main())
