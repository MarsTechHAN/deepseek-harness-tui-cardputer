# Accedere a dst

dst accetta due tipi di credenziali, salvate in `~/.dst/config.yml` (mode 600):

| Credenziale | Come si ottiene | Header verso l'API |
| --- | --- | --- |
| **Token DSH** (consigliato per uso personale) | login reale con QR WeChat su DeepSeek Platform (`/login`) | `x-dsh-auth-token: <token>` |
| **API key** | key già disponibile o endpoint proprio | `Authorization: Bearer <key>` |

`/config` mostra la riga `API key / token` e il valore `DSH token` quando presente.
Impostando una API key (`/key`, il prompt del config o l'onboarding) il token viene
sostituito, così la nuova key non resta nascosta.

![Schermata di login WeChat sul dispositivo](login-qr-screen.png)

## Tre modi per accedere

Esegui `/login` (o scegli *DeepSeek official* nell'onboarding):

1. **QR WeChat sul dispositivo** — dst richiede il QR WeChat per DeepSeek Platform
   e lo disegna a destra del pannello; a sinistra una guida numerata e il conto
   alla rovescia `QR valid for m:ss`. Scansiona con WeChat, conferma sul telefono
   e il dispositivo completa da solo lo scambio del token.
2. **Pagina sul telefono** — `http://<ip-dispositivo>:8787/`. Mostra lo stesso QR,
   progresso live, conto alla rovescia, Cancel/Retry, un modulo per la API key e un
   campo per incollare un token DSH esistente. Segue la lingua UI del dispositivo
   (en/zh/es/ja/de/it).
3. **Digitazione diretta** — menu in stile menuconfig per token DSH oppure
   Base URL + API key + modello.

I nove passi sono gli stessi del client desktop DSH:
`wechat qrconnect → QR JPEG → long-poll → nonce callback → token sessione web →
auth_init (PKCE) → auth_approve → auth_exchange → token DSH`.

## Requisiti

- Accesso a `open.weixin.qq.com`, `lp.open.weixin.qq.com`,
  `platform.deepseek.com`, `api.deepseek.com`.
- Orologio di sistema corretto (validazione TLS).
- `python3` + Pillow **solo** per disegnare il QR sul pannello; senza, dst usa la
  pagina del telefono che mostra lo stesso QR.
- Porta 8787 libera (altrimenti dst usa una porta effimera).

## Stati durante il login

| Stato | Dispositivo | Pagina telefono |
| --- | --- | --- |
| in attesa | `→ QR valid for 4:50` | conto alla rovescia localizzato |
| scansionato | `→ scanned — confirm on the phone` | stato localizzato |
| conferma | `→ authorized — finishing login…` | stato localizzato |
| scaduto | 1ª volta aggiorna da solo; 2ª: `login failed — press r to retry` (`r retry · t type · Esc cancel`) | *QR expired — refreshing…*, un retry automatico; resta Retry |
| annullato | Esc sul dispositivo | Cancel sulla pagina |

## Risoluzione problemi

| Sintomo | Causa / soluzione |
| --- | --- |
| QR non scansionabile | Il QR è generato da WeChat e disegnato pixel per pixel; verifica `python3`+Pillow e che lo schermo non sia coperto. La pagina mostra lo stesso QR. |
| `QR expired` due volte | Wi-Fi/orologio instabili. Controlla ora e rete, poi `r` (nuovo QR), `t` (digita token/API key) o Retry. |
| `login failed` con codici HTTP/biz | La pagina mostra un riepilogo localizzato e il dettaglio tecnico; riprova più tardi o usa una API key. |
| `invalid dsh token` / `auth failed: use /key or /login` | Token scaduto: ripeti `/login` o passa a una API key con `/key`. |
| Pagina telefono non raggiungibile | Telefono e dispositivo devono essere sulla stessa LAN e la porta 8787 raggiungibile (usa l'URL mostrato sul pannello). |
| Riquadri per cinese/giapponese | Manca un font CJK sul dispositivo (vedi `docs/UX-AUDIT.md`); la pagina funziona comunque. |

Sicurezza: un token DSH equivale a una sessione account. È salvato solo sul
dispositivo (mode 600) e dst non lo carica mai altrove. Su dispositivi condivisi
usa una API key ufficiale; questo flusso è privato del client DSH e può cambiare.
