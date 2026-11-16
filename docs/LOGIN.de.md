# Anmeldung bei dst

dst akzeptiert zwei Zugangsarten, gespeichert in `~/.dst/config.yml` (Modus 600):

| Zugang | Herkunft | Header an die API |
| --- | --- | --- |
| **DSH-Token** (empfohlen privat) | echter DeepSeek-Platform-Login per WeChat-QR (`/login`) | `x-dsh-auth-token: <token>` |
| **API-Key** | vorhandener Key oder eigener Endpoint | `Authorization: Bearer <key>` |

`/config` zeigt die Zeile `API key / token` und als Wert `DSH token`, wenn einer
gespeichert ist. Das Setzen eines API-Keys (`/key`, Config-Dialog, Onboarding)
ersetzt den Token, damit der neue Key nicht verdeckt wird.

![WeChat-Login-Bildschirm am Gerät](login-qr-screen.png)

## Drei Wege zur Anmeldung

`/login` ausführen (oder im Onboarding *DeepSeek official* wählen):

1. **WeChat-QR am Gerät** — dst holt den WeChat-QR für die DeepSeek Platform und
   malt ihn rechts auf das Panel; links stehen eine nummerierte Anleitung und
   ein Countdown `QR valid for m:ss`. Mit WeChat scannen, am Handy bestätigen,
   das Gerät schließt den Token-Tausch selbst ab.
2. **Seite am Handy** — `http://<Geräte-IP>:8787/`. Zeigt denselben QR, Live-
   Fortschritt, Countdown, Cancel/Retry, ein Formular für einen API-Key und ein
   Feld zum Einfügen eines vorhandenen DSH-Tokens. Folgt der UI-Sprache des
   Geräts (en/zh/es/ja/de/it).
3. **Direkt eintippen** — Menuconfig-Stil für DSH-Token oder Base-URL + API-Key
   + Modell.

Die neun Schritte entsprechen dem DSH-Desktop-Client:
`wechat qrconnect → QR-JPEG → Long-Poll → Callback-Nonce → Web-Session-Token →
auth_init (PKCE) → auth_approve → auth_exchange → DSH-Token`.

## Voraussetzungen

- Zugriff auf `open.weixin.qq.com`, `lp.open.weixin.qq.com`,
  `platform.deepseek.com`, `api.deepseek.com`.
- Korrekte Systemzeit (TLS-Prüfung).
- `python3` + Pillow **nur** zum Malen des QR aufs Panel; ohne sie fällt dst auf
  die Handy-Seite zurück, die denselben QR zeigt.
- Port 8787 frei (sonst nutzt dst einen temporären Port).

## Zustände während des Logins

| Zustand | Gerät | Handy-Seite |
| --- | --- | --- |
| wartend | `→ QR valid for 4:50` | lokalisierter Countdown |
| gescannt | `→ scanned — confirm on the phone` | lokalisierter Status |
| bestätigend | `→ authorized — finishing login…` | lokalisierter Status |
| abgelaufen | 1. Mal automatisch neu; 2. Mal `login failed — press r to retry` (`r retry · t type · Esc cancel`) | *QR expired — refreshing…*, einmaliger Retry; Retry-Button bleibt |
| abgebrochen | Esc am Gerät | Cancel auf der Seite |

## Fehlerbehebung

| Symptom | Ursache / Lösung |
| --- | --- |
| QR nicht scannbar | QR kommt von WeChat und wird pixelgenau gemalt; `python3`+Pillow prüfen und Sicht prüfen. Die Seite zeigt denselben QR. |
| `QR expired` zweimal | WLAN/Uhr instabil. Zeit und Netz prüfen, dann `r` (neuer QR), `t` (Token/API-Key tippen) oder Retry. |
| `login failed` mit HTTP/biz-Codes | Die Seite zeigt lokalisierte Zusammenfassung + technisches Detail; später erneut versuchen oder API-Key nutzen. |
| `invalid dsh token` / `auth failed: use /key or /login` | Token abgelaufen: `/login` wiederholen oder mit `/key` auf API-Key wechseln. |
| Handy-Seite nicht erreichbar | Handy und Gerät müssen im selben LAN sein, Port 8787 erreichbar (URL vom Panel nutzen). |
| Kästchen bei Chinesisch/Japanisch | Fehlende CJK-Schrift am Gerät (siehe `docs/UX-AUDIT.md`); die Seite funktioniert weiter. |

Sicherheit: Ein DSH-Token entspricht einer Kontositzung. Es wird nur am Gerät
gespeichert (Modus 600) und von dst nie hochgeladen. Für geteilte Geräte einen
offiziellen API-Key verwenden; dieser Weg ist der private Flow des DSH-Clients
und kann sich ändern.
