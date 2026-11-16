#!/usr/bin/env python3
"""Mock of the nine-step DSH/WeChat login chain + a tiny chat endpoint.

The dst login client points its base URLs at this server:
  DST_LOGIN_PLATFORM / DST_LOGIN_WECHAT / DST_LOGIN_WECHAT_LP / DST_LOGIN_API
"""
import argparse
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
QR_JPEG = os.path.join(HERE, "fixtures", "wechat-qr.jpg")
UUID = "TESTUUID1234567890"
WX_CODE = "WXCODE123456"
NONCE = "TESTNONCE123456"
WEB_TOKEN = "WEBTOKEN1234567890abcdef"
AUTH_ID = "AUTHID123456"
EXCHANGE_CODE = "EXCHANGECODE123"
DSH_TOKEN = "dsh" + "x" * 61
STATE = {"state": "s" * 43}
LOG = []
SCAN_COUNT = [0]
EXPIRED_ONCE = [0]
EXPIRED_N = [int(os.environ.get("MOCK_LOGIN_EXPIRED_N", "0"))]


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _json(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _envelope(self, data):
        self._json({"code": 0, "msg": "", "data": {"biz_code": 0, "biz_msg": "", "biz_data": data}})

    def _log(self, body=b""):
        LOG.append({"method": self.command, "path": self.path, "body": body.decode("utf-8", "replace")})
        with open(os.environ.get("MOCK_LOGIN_LOG", "/tmp/mock-login.jsonl"), "a") as f:
            f.write(json.dumps(LOG[-1]) + "\n")

    def _body(self):
        n = int(self.headers.get("content-length", "0") or 0)
        return self.rfile.read(n) if n else b""

    def do_GET(self):
        self._log()
        path = self.path.split("?")[0]
        if path == "/connect/qrconnect":
            body = f'<html><script>var uuid = "{UUID}";</script></html>'.encode()
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif path.startswith("/connect/qrcode/"):
            data = open(QR_JPEG, "rb").read()
            self.send_response(200)
            self.send_header("content-type", "image/jpeg")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        elif path == "/connect/l/qrconnect":
            delay = float(os.environ.get("MOCK_LOGIN_POLL_DELAY", "0"))
            if delay > 0:
                time.sleep(delay)
            errcode = 405
            if os.environ.get("MOCK_LOGIN_EXPIRED") and EXPIRED_ONCE[0] == 0:
                EXPIRED_ONCE[0] = 1
                errcode = 402
            elif EXPIRED_N[0] > 0:
                EXPIRED_N[0] -= 1
                errcode = 402
            elif os.environ.get("MOCK_LOGIN_SCANNED") and SCAN_COUNT[0] == 0:
                SCAN_COUNT[0] = 1
                errcode = 404
            self.send_response(200)
            self.send_header("content-type", "text/javascript")
            body = f"window.wx_errcode={errcode};window.wx_code='{'' if errcode == 404 else WX_CODE}';".encode()
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif path == "/auth-api/v0/users/oauth/wechat/callback":
            self.send_response(307)
            self.send_header("location", f"/authorized?provider=WECHAT&nonce={NONCE}&new=1")
            self.send_header("content-length", "0")
            self.end_headers()
        elif path == "/__log":
            self._json(LOG)
        else:
            self._json({"error": {"message": f"no route {path}"}}, 404)

    def do_POST(self):
        body = self._body()
        self._log(body)
        path = self.path.split("?")[0]
        if path == "/auth-api/v0/users/oauth/get_token":
            self._envelope({"token": WEB_TOKEN})
        elif path == "/auth-api/v0/dsh/auth_init":
            try:
                STATE["state"] = json.loads(body).get("state", STATE["state"])
            except Exception:
                pass
            self._envelope({
                "authorize_url": "https://platform.deepseek.com/dsh/authorize?authorize_id=" + AUTH_ID,
                "authorize_id": AUTH_ID,
                "expires_in": 600,
            })
        elif path == "/auth-api/v0/dsh/auth_approve":
            cb = (
                "http://127.0.0.1:7788/oauth/callback?code="
                + EXCHANGE_CODE
                + "&state="
                + STATE["state"]
            )
            self._envelope({"callback_url": cb})
        elif path == "/auth-api/v0/dsh/auth_exchange":
            self._envelope({"token": DSH_TOKEN})
        elif path == "/chat/completions":
            self._json({"choices": [{"message": {"role": "assistant", "content": "OK"}}]})
        else:
            self._json({"error": {"message": f"no route {path}"}}, 404)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--port-file", default="")
    args = ap.parse_args()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), H)
    port = srv.server_address[1]
    if args.port_file:
        open(args.port_file, "w").write(str(port))
    print(f"http://127.0.0.1:{port}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
