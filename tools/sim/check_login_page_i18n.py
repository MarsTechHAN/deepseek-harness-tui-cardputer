#!/usr/bin/env python3
"""Start the dst login page per language and assert the page is translated."""
import os, subprocess, sys, tempfile, time, urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))
EXPECT = {
    "en": ["Sign in to DeepSeek", "Start WeChat login", "Save to device", "QR expired — refreshing"],
    "zh": ["登录 DeepSeek", "开始微信登录", "保存到设备", "二维码过期 — 正在刷新"],
    "es": ["Inicia sesión en DeepSeek", "Iniciar login con WeChat", "Guardar en el dispositivo", "QR caducado — renovando"],
    "ja": ["DeepSeek にログイン", "WeChat ログインを開始", "端末に保存", "QR の期限切れ — 更新中"],
    "de": ["Bei DeepSeek anmelden", "WeChat-Login starten", "Auf Gerät speichern", "QR abgelaufen — wird erneuert"],
    "it": ["Accedi a DeepSeek", "Avvia login WeChat", "Salva sul dispositivo", "QR scaduto — aggiornamento"],
}

def main():
    ok = True
    for i, (lang, needles) in enumerate(EXPECT.items()):
        port = 8910 + i
        out = tempfile.mktemp()
        env = dict(os.environ, DST_LOGIN_HOST="127.0.0.1")
        p = subprocess.Popen([BIN, "--login-server-test", str(port), lang],
                             stdout=open(out, "w"), stderr=subprocess.DEVNULL,
                             env=env)
        try:
            url = None
            for _ in range(50):
                time.sleep(0.1)
                txt = open(out).read() if os.path.exists(out) else ""
                for line in txt.splitlines():
                    if line.startswith("URL "):
                        url = line[4:].strip().rstrip("/")
                        break
                if url:
                    break
            if not url:
                print(lang, "FAIL no server URL")
                ok = False
                continue
            html = urllib.request.urlopen(url + "/", timeout=5).read().decode("utf-8")
            missing = [n for n in needles if n not in html]
            if missing:
                print(lang, "FAIL missing:", missing)
                ok = False
            else:
                print(lang, "OK", needles[0])
        finally:
            p.terminate()
            try:
                p.wait(timeout=3)
            except Exception:
                p.kill()
    return 0 if ok else 1

if __name__ == "__main__":
    sys.exit(main())
