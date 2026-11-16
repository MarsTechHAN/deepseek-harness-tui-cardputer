//! Login helpers: a LAN page (WeChat QR via the real DSH flow + manual API
//! key), plus QR rendering for the device panel.
//!
//! The WeChat flow is the same nine-step protocol as the DSH desktop client
//! (`crate::dsh_login`). The page shows the WeChat QR image fetched by the
//! device, live progress, and a manual API-key form as a fallback.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

pub const DEFAULT_PORT: u16 = 8787;

/// Live state shown both on the phone page and on the device.
#[derive(Default)]
pub struct LoginState {
    pub phase: String,
    pub message: String,
    pub steps: Vec<String>,
    pub qr_version: u64,
    pub qr: Vec<u8>,
    /// Epoch millis when the current QR was received (300s lifetime).
    pub qr_ms: u64,
    pub web_token: Option<String>,
    pub token: Option<String>,
    pub error: Option<String>,
    pub running: bool,
    pub cancelled: bool,
}

pub type Shared = Arc<Mutex<LoginState>>;

pub struct Session {
    pub url: String,
    pub port: u16,
    pub shared: Shared,
    pub rx: Receiver<Submitted>,
    stop: Arc<AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Best-effort LAN address of this device (no external traffic is generated).
pub fn lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) {
        if sock.connect(("1.1.1.1", 80)).is_ok() {
            if let Ok(addr) = sock.local_addr() {
                if let IpAddr::V4(v4) = addr.ip() {
                    if !v4.is_loopback() {
                        return v4.to_string();
                    }
                }
            }
        }
    }
    "127.0.0.1".to_string()
}

/// Start the WeChat flow in a background thread, updating `shared`.
pub fn start_wechat(shared: &Shared, port: u16) {
    {
        let mut st = shared.lock().unwrap();
        if st.running {
            return;
        }
        st.running = true;
        st.phase = "qr".into();
        st.message = "requesting WeChat QR".into();
        st.error = None;
        st.token = None;
        st.cancelled = false;
        st.steps.clear();
    }
    let sh = shared.clone();
    std::thread::spawn(move || {
        let mut emit = |ev: crate::dsh_login::Event| match ev {
            crate::dsh_login::Event::Phase(p) => {
                sh.lock().unwrap().phase = p;
            }
            crate::dsh_login::Event::Message(m) => {
                sh.lock().unwrap().message = m;
            }
            crate::dsh_login::Event::Step(s) => {
                sh.lock().unwrap().steps.push(s);
            }
            crate::dsh_login::Event::WebToken(t) => {
                sh.lock().unwrap().web_token = Some(t);
            }
            crate::dsh_login::Event::Qr(bytes) => {
                let mut st = sh.lock().unwrap();
                st.qr = bytes;
                st.qr_version += 1;
                st.qr_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                st.phase = "waiting".into();
                st.message = "scan the QR with WeChat".into();
            }
        };
        let mut cancelled = || sh.lock().unwrap().cancelled;
        let result = crate::dsh_login::run(port, &mut emit, &mut cancelled);
        let mut st = sh.lock().unwrap();
        st.running = false;
        match result {
            Ok(token) => {
                st.token = Some(token);
                st.phase = "done".into();
                st.message = "logged in".into();
                st.steps.push("⑧ DSH token received".into());
            }
            Err(e) => {
                st.phase = "failed".into();
                st.error = Some(e.clone());
                st.message = e;
            }
        }
    });
}

fn state_json(st: &LoginState) -> String {
    let q = |s: &str| crate::json::J::s(s).dump();
    let opt = |o: &Option<String>| match o {
        Some(v) => q(&crate::dsh_login::mask(v)),
        None => "null".into(),
    };
    let steps = st
        .steps
        .iter()
        .map(|s| q(s))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"phase\":{},\"message\":{},\"qrVersion\":{},\"steps\":[{}],\"token\":{},\"webToken\":{},\"error\":{},\"running\":{},\"cancelled\":{},\"expiresIn\":{}}}",
        q(&st.phase),
        q(&st.message),
        st.qr_version,
        steps,
        opt(&st.token),
        opt(&st.web_token),
        opt(&st.error),
        st.running,
        st.cancelled,
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if st.qr_ms == 0 || now.saturating_sub(st.qr_ms) >= 300_000 {
                0u64
            } else {
                (300_000 - (now - st.qr_ms)) / 1000
            }
        }
    )
}

pub fn page_for_test(lang: &str) -> String { page(lang) }

fn page(lang: &str) -> String {
    const T: &str = r#"{"en": {"title": "dst login", "sub": "Sign in to DeepSeek — or paste an API key. The device saves it locally; nothing is uploaded elsewhere.", "wechat_h2": "WeChat QR login", "wechat_hint": "Scan with WeChat — the device completes the DeepSeek Platform login by itself. The QR expires after about 5 minutes.", "start_btn": "Start WeChat login", "api_h2": "API key", "api_hint": "Use this if you already have a platform API key, or for a self-hosted OpenAI-compatible endpoint.", "label_provider": "Provider", "opt_official": "DeepSeek official (api.deepseek.com)", "opt_custom": "Other / self-hosted (OpenAI-compatible)", "label_custom": "Custom base URL", "label_key": "API key", "label_model": "Model (optional)", "save_btn": "Save to device", "foot1": "Device page · for phones on the same LAN", "foot2": "Token stored locally on the device only (mode 600)", "pill_idle": "idle", "pill_starting": "starting…", "pill_done": "logged in", "pill_failed": "failed", "pill_offline": "offline", "qr_alt": "WeChat login QR", "msg_done": "DSH token saved to the device", "msg_start": "requesting WeChat QR", "msg_scan": "scan the QR with WeChat", "msg_scanned": "scanned, confirm on the phone", "msg_exchanging": "authorized, exchanging token", "msg_expired": "QR expired — refreshing…", "cancel_btn": "Cancel", "msg_cancelled": "login cancelled", "expires_in": "expires in", "expired": "QR expired", "retry_btn": "Retry WeChat login", "msg_failed": "login failed — details below"}, "zh": {"title": "dst 登录", "sub": "登录 DeepSeek，或粘贴 API key。凭据只保存在设备本地，不会上传到别处。", "wechat_h2": "微信扫码登录", "wechat_hint": "用微信扫码，设备会自动完成 DeepSeek 平台登录。二维码约 5 分钟后过期。", "start_btn": "开始微信登录", "api_h2": "API Key", "api_hint": "已有平台 API key，或使用自建 OpenAI 兼容 endpoint 时使用。", "label_provider": "服务商", "opt_official": "DeepSeek 官方（api.deepseek.com）", "opt_custom": "其他 / 自建（OpenAI 兼容）", "label_custom": "自定义 base URL", "label_key": "API key", "label_model": "模型（可选）", "save_btn": "保存到设备", "foot1": "设备页面 · 仅供同一局域网的手机访问", "foot2": "Token 只保存在设备本地（mode 600）", "pill_idle": "未开始", "pill_starting": "正在启动…", "pill_done": "已登录", "pill_failed": "失败", "pill_offline": "未连接", "qr_alt": "微信登录二维码", "msg_done": "DSH token 已保存到设备", "msg_start": "正在申请微信二维码", "msg_scan": "用微信扫一扫二维码", "msg_scanned": "已扫码，请在手机上确认", "msg_exchanging": "已授权，正在换取 token", "msg_expired": "二维码过期 — 正在刷新…", "cancel_btn": "取消", "msg_cancelled": "已取消登录", "expires_in": "剩余", "expired": "二维码已过期", "retry_btn": "重试微信登录", "msg_failed": "登录失败 — 详情见下方"}, "es": {"title": "Acceso dst", "sub": "Inicia sesión en DeepSeek o pega una API key. El dispositivo la guarda localmente; no se sube a ningún sitio.", "wechat_h2": "Acceso con QR de WeChat", "wechat_hint": "Escanea con WeChat: el dispositivo completa el login de DeepSeek Platform. El QR caduca en unos 5 minutos.", "start_btn": "Iniciar login con WeChat", "api_h2": "API key", "api_hint": "Úsalo si ya tienes una API key o para un endpoint propio compatible con OpenAI.", "label_provider": "Proveedor", "opt_official": "DeepSeek oficial (api.deepseek.com)", "opt_custom": "Otro / propio (compatible OpenAI)", "label_custom": "Base URL personalizada", "label_key": "API key", "label_model": "Modelo (opcional)", "save_btn": "Guardar en el dispositivo", "foot1": "Página del dispositivo · solo para móviles de la misma LAN", "foot2": "Token guardado solo en el dispositivo (modo 600)", "pill_idle": "inactivo", "pill_starting": "iniciando…", "pill_done": "sesión iniciada", "pill_failed": "fallido", "pill_offline": "sin conexión", "qr_alt": "QR de login WeChat", "msg_done": "Token DSH guardado en el dispositivo", "msg_start": "solicitando QR de WeChat", "msg_scan": "escanea el QR con WeChat", "msg_scanned": "escaneado, confirma en el móvil", "msg_exchanging": "autorizado, intercambiando token", "msg_expired": "QR caducado — renovando…", "cancel_btn": "Cancelar", "msg_cancelled": "login cancelado", "expires_in": "caduca en", "expired": "QR caducado", "retry_btn": "Reintentar login WeChat", "msg_failed": "login fallido — detalles abajo"}, "ja": {"title": "dst ログイン", "sub": "DeepSeek にログインするか API キーを貼り付けます。認証情報は端末内だけに保存されます。", "wechat_h2": "WeChat QR ログイン", "wechat_hint": "WeChat でスキャンすると、端末が DeepSeek Platform のログインを自動で完了します。QR は約 5 分で失効します。", "start_btn": "WeChat ログインを開始", "api_h2": "API キー", "api_hint": "既存の API キー、または OpenAI 互換の独自 endpoint を使う場合に。", "label_provider": "プロバイダー", "opt_official": "DeepSeek 公式（api.deepseek.com）", "opt_custom": "その他 / 独自（OpenAI 互換）", "label_custom": "カスタム base URL", "label_key": "API キー", "label_model": "モデル（任意）", "save_btn": "端末に保存", "foot1": "端末ページ · 同じ LAN のスマホ用", "foot2": "Token は端末内のみに保存（mode 600）", "pill_idle": "待機中", "pill_starting": "開始中…", "pill_done": "ログイン済み", "pill_failed": "失敗", "pill_offline": "未接続", "qr_alt": "WeChat ログイン QR", "msg_done": "DSH トークンを端末に保存しました", "msg_start": "WeChat QR を取得中", "msg_scan": "WeChat で QR をスキャン", "msg_scanned": "スキャン済み。スマホで確認してください", "msg_exchanging": "認証済み、トークン交換中", "msg_expired": "QR の期限切れ — 更新中…", "cancel_btn": "キャンセル", "msg_cancelled": "ログインをキャンセルしました", "expires_in": "残り", "expired": "QR の期限切れ", "retry_btn": "WeChat ログインを再試行", "msg_failed": "ログイン失敗 — 下記の詳細"}, "de": {"title": "dst Anmeldung", "sub": "Bei DeepSeek anmelden oder einen API-Key einfügen. Das Gerät speichert ihn lokal; nichts wird hochgeladen.", "wechat_h2": "WeChat-QR-Login", "wechat_hint": "Mit WeChat scannen — das Gerät schließt den DeepSeek-Platform-Login selbst ab. Der QR läuft nach ca. 5 Minuten ab.", "start_btn": "WeChat-Login starten", "api_h2": "API-Key", "api_hint": "Für einen vorhandenen API-Key oder einen eigenen OpenAI-kompatiblen Endpoint.", "label_provider": "Anbieter", "opt_official": "DeepSeek official (api.deepseek.com)", "opt_custom": "Andere / eigener (OpenAI-kompatibel)", "label_custom": "Eigene Base-URL", "label_key": "API-Key", "label_model": "Modell (optional)", "save_btn": "Auf Gerät speichern", "foot1": "Geräteseite · nur für Handys im selben LAN", "foot2": "Token nur lokal auf dem Gerät (Modus 600)", "pill_idle": "inaktiv", "pill_starting": "startet…", "pill_done": "eingeloggt", "pill_failed": "fehlgeschlagen", "pill_offline": "offline", "qr_alt": "WeChat-Login-QR", "msg_done": "DSH-Token auf dem Gerät gespeichert", "msg_start": "WeChat-QR wird angefordert", "msg_scan": "QR mit WeChat scannen", "msg_scanned": "gescannt, am Handy bestätigen", "msg_exchanging": "autorisiert, Token wird getauscht", "msg_expired": "QR abgelaufen — wird erneuert…", "cancel_btn": "Abbrechen", "msg_cancelled": "Login abgebrochen", "expires_in": "gültig noch", "expired": "QR abgelaufen", "retry_btn": "WeChat-Login wiederholen", "msg_failed": "Login fehlgeschlagen — Details unten"}, "it": {"title": "Accesso dst", "sub": "Accedi a DeepSeek o incolla una API key. Il dispositivo la salva localmente; nulla viene caricato altrove.", "wechat_h2": "Accesso con QR WeChat", "wechat_hint": "Scansiona con WeChat: il dispositivo completa da solo il login a DeepSeek Platform. Il QR scade dopo circa 5 minuti.", "start_btn": "Avvia login WeChat", "api_h2": "API key", "api_hint": "Usala se hai già una API key o per un endpoint proprio compatibile OpenAI.", "label_provider": "Provider", "opt_official": "DeepSeek official (api.deepseek.com)", "opt_custom": "Altro / proprio (compatibile OpenAI)", "label_custom": "Base URL personalizzata", "label_key": "API key", "label_model": "Modello (opzionale)", "save_btn": "Salva sul dispositivo", "foot1": "Pagina del dispositivo · solo per telefoni nella stessa LAN", "foot2": "Token salvato solo sul dispositivo (mode 600)", "pill_idle": "inattivo", "pill_starting": "avvio…", "pill_done": "accesso effettuato", "pill_failed": "fallito", "pill_offline": "offline", "qr_alt": "QR login WeChat", "msg_done": "Token DSH salvato sul dispositivo", "msg_start": "richiesta QR WeChat", "msg_scan": "scansiona il QR con WeChat", "msg_scanned": "scansionato, conferma sul telefono", "msg_exchanging": "autorizzato, scambio token", "msg_expired": "QR scaduto — aggiornamento…", "cancel_btn": "Annulla", "msg_cancelled": "login annullato", "expires_in": "scade tra", "expired": "QR scaduto", "retry_btn": "Riprova login WeChat", "msg_failed": "login fallito — dettagli sotto"}}"#;
    let l = match lang {
        "zh" | "es" | "ja" | "de" | "it" => lang,
        _ => "en",
    };
    let doc = crate::json::parse(T).unwrap_or_else(|| crate::json::parse("{}").unwrap());
    let tr = |k: &str| -> String {
        doc.get(l).and_then(|v| v.get(k)).and_then(|v| v.as_str())
            .or_else(|| doc.get("en").and_then(|v| v.get(k)).and_then(|v| v.as_str()))
            .unwrap_or(k).to_string()
    };
    let mut out = r#"<!doctype html>
<html lang="@LANG@"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>@TITLE@</title>
<style>
:root{--bg:#0b0f14;--card:#121924;--line:#223044;--text:#e6edf3;--dim:#8b98a9;--accent:#4f9cf9;--ok:#3fb950;--err:#f85149}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font-family:system-ui,-apple-system,"Segoe UI",sans-serif}
.wrap{max-width:840px;margin:0 auto;padding:24px 18px 40px}
h1{font-size:22px;margin:0 0 4px}.sub{color:var(--dim);font-size:14px;margin:0 0 20px;line-height:1.5}
.grid{display:grid;grid-template-columns:1fr 1fr;gap:16px}
@media(max-width:720px){.grid{grid-template-columns:1fr}}
.card{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:18px}
.card h2{font-size:16px;margin:0 0 8px}.card .hint{color:var(--dim);font-size:13px;margin:0 0 12px;line-height:1.5}
.pill{display:inline-block;font-size:12px;padding:4px 10px;border-radius:999px;background:#1d2a3c;color:var(--dim);margin-bottom:10px}
.pill.ok{background:#12361f;color:var(--ok)}.pill.err{background:#3b1517;color:var(--err)}
#qr{width:100%;max-width:280px;aspect-ratio:1/1;object-fit:contain;background:#fff;border-radius:12px;display:block;margin:8px auto 4px}
#qrmsg{text-align:center;color:var(--dim);font-size:13px;min-height:20px;line-height:1.4}
button{width:100%;padding:13px;border:0;border-radius:10px;background:var(--accent);color:#fff;font-size:15px;font-weight:600;margin-top:10px;cursor:pointer}
button.ghost{background:#1a2534;color:var(--text);font-weight:500}
label{display:block;margin:14px 0 6px;font-size:13px;color:var(--dim)}
input,select{width:100%;padding:12px;border-radius:10px;border:1px solid var(--line);background:#0d141d;color:var(--text);font-size:16px}
.steps{margin:10px 0 0;padding-left:18px;color:var(--dim);font-size:12px;line-height:1.7;min-height:20px}
.foot{margin-top:22px;color:var(--dim);font-size:12px;text-align:center;line-height:1.7}
</style></head><body><div class="wrap">
<h1>dst · Cardputer Zero</h1>
<p class="sub">@SUB@</p>
<div class="grid">
 <section class="card">
  <h2>@WECHAT_H2@</h2>
  <p class="hint">@WECHAT_HINT@</p>
  <div id="pill" class="pill">@PILL_IDLE@</div>
  <img id="qr" alt="@QR_ALT@" src="/api/qr" style="display:none">
  <div id="qrmsg"></div>
  <div id="expiry" class="hint" style="text-align:center;min-height:18px"></div>
  <ul id="steps" class="steps"></ul>
  <button id="start">@START_BTN@</button>
  <button id="cancel" class="ghost">@CANCEL_BTN@</button>
 </section>
 <section class="card">
  <h2>@API_H2@</h2>
  <p class="hint">@API_HINT@</p>
  <form method="POST" action="/save">
   <label>@LABEL_PROVIDER@</label>
   <select name="base">
    <option value="https://api.deepseek.com/v1">@OPT_OFFICIAL@</option>
    <option value="custom">@OPT_CUSTOM@</option>
   </select>
   <label>@LABEL_CUSTOM@</label>
   <input name="custom_base" placeholder="https://your.endpoint/v1" autocomplete="off">
   <label>@LABEL_KEY@</label>
   <input name="key" type="password" placeholder="sk-..." autocomplete="off" required>
   <label>@LABEL_MODEL@</label>
   <input name="model" placeholder="deepseek-flash" autocomplete="off">
   <button class="ghost" type="submit">@SAVE_BTN@</button>
  </form>
 </section>
</div>
<p class="foot">@FOOT1@<br>@FOOT2@</p>
</div>
<script>
const $=id=>document.getElementById(id);
const phaseText=p=>({qr:"@MSG_START@",waiting:"@MSG_SCAN@",scanned:"@MSG_SCANNED@",exchanging:"@MSG_EXCHANGING@",done:"@PILL_DONE@",failed:"@PILL_FAILED@"}[p]||p||"@PILL_IDLE@");
const setPill=(t,cls)=>{$('pill').textContent=t;$('pill').className='pill '+(cls||'')};
async function poll(){
 try{
  const r=await fetch('/api/state',{cache:'no-store'});
  const s=await r.json();
  if(s.qrVersion>0){$('qr').style.display='block';$('qr').src='/api/qr?v='+s.qrVersion;}
  if(s.qrVersion>0&&s.expiresIn>0){const m=Math.floor(s.expiresIn/60),sec=s.expiresIn%60;$('expiry').textContent=L('expires_in')+' '+m+':'+String(sec).padStart(2,'0');$('expiry').style.display='block';}
  else if(s.qrVersion>0&&s.expiresIn===0){$('expiry').textContent=L('expired');}
  else {$('expiry').style.display='none';}
  $('qrmsg').textContent=phaseText(s.phase)||s.message||'';
  setPill(phaseText(s.phase),s.phase==='done'?'ok':(s.phase==='failed'?'err':''));
  if(s.steps&&s.steps.length)$('steps').innerHTML=s.steps.map(x=>'<li>'+x.replace(/</g,'&lt;')+'</li>').join('');
  if(s.token){setPill("@PILL_DONE@",'ok');$('qrmsg').textContent="@MSG_DONE@ ("+s.token+")";}
  if(s.phase==='failed'||s.phase==='done'){$('start').textContent=s.phase==='failed'?"@RETRY_BTN@":"@START_BTN@";}
  if(s.cancelled&&s.phase==='failed'){setPill("@PILL_FAILED@",'err');$('qrmsg').textContent="@MSG_CANCELLED@";}
  else if(s.error){
    const expired=/expired|过期|caducado|期限切れ|abgelaufen|scaduto/i.test(s.error);
    setPill(expired?"@MSG_EXPIRED@":"@PILL_FAILED@",'err');
    if(expired){$('qrmsg').textContent="@MSG_EXPIRED@";}
    else{
      const detail=String(s.error).replace(/</g,'&lt;');
      $('qrmsg').innerHTML="@MSG_FAILED@<br><span style='font-size:11px;color:#8b98a9'>"+detail+"</span>";
    }
    if(expired&&!window.__autoRetried){window.__autoRetried=true;setTimeout(()=>fetch('/api/login',{method:'POST'}).catch(()=>{}),900);}
  }
 }catch(e){setPill("@PILL_OFFLINE@",'err')}
}
$('start').onclick=async()=>{setPill("@PILL_STARTING@");await fetch('/api/login',{method:'POST'});poll();};
$('cancel').onclick=async()=>{await fetch('/api/cancel',{method:'POST'}).catch(()=>{});setPill("@PILL_FAILED@",'err');$('qrmsg').textContent="@MSG_CANCELLED@";};
fetch('/api/login',{method:'POST'}).catch(()=>{});
poll();setInterval(poll,1200);
</script></body></html>"#.to_string();
    out = out.replace("@LANG@", l);
    for key in ["TITLE","SUB","WECHAT_H2","WECHAT_HINT","START_BTN","API_H2","API_HINT","LABEL_PROVIDER","OPT_OFFICIAL","OPT_CUSTOM","LABEL_CUSTOM","LABEL_KEY","LABEL_MODEL","SAVE_BTN","FOOT1","FOOT2","PILL_IDLE","PILL_STARTING","PILL_DONE","PILL_FAILED","PILL_OFFLINE","QR_ALT","MSG_DONE","MSG_START","MSG_SCAN","MSG_SCANNED","MSG_EXCHANGING","MSG_EXPIRED","CANCEL_BTN","MSG_CANCELLED","EXPIRES_IN","EXPIRED","RETRY_BTN","MSG_FAILED"] {
        out = out.replace(&format!("@{key}@"), &tr(&key.to_ascii_lowercase()));
    }
    out.replace("@@", "@")
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) { out.push(v); i += 3; }
                else { out.push(b[i]); i += 1; }
            }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn field(body: &str, name: &str) -> Option<String> {
    for pair in body.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if urldecode(k) == name {
            let s = urldecode(v).trim().to_string();
            if !s.is_empty() { return Some(s); }
        }
    }
    None
}

pub struct Submitted {
    pub key: String,
    pub base: String,
    pub model: String,
}

fn reply(stream: &mut std::net::TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = head.clear();
}

/// Start the login server; returns the page URL, live state and the channel
/// that yields manually submitted credentials.
pub fn start(port: u16, lang: &str) -> Result<Session, String> {
    let ip = std::env::var("DST_LOGIN_HOST").unwrap_or_else(|_| lan_ip());
    // Prefer the requested port; fall back to an ephemeral one when another
    // login page is still winding down (Drop stops the previous listener).
    let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)) {
        Ok(l) => l,
        Err(e) => {
            if port == 0 {
                return Err(format!("cannot listen: {e}"));
            }
            TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
                .map_err(|e2| format!("cannot listen on port {port} ({e}) nor ephemeral ({e2})"))?
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_server = stop.clone();
    let url = format!("http://{ip}:{port}/");
    let (tx, rx) = channel::<Submitted>();
    let tx = Arc::new(Mutex::new(tx));
    let shared: Shared = Arc::new(Mutex::new(LoginState::default()));
    let state_for_server = shared.clone();
    let page_lang = lang.to_string();
    let _ = listener.set_nonblocking(true);
    std::thread::spawn(move || {
        loop {
            if stop_for_server.load(Ordering::Relaxed) {
                return;
            }
            let mut s = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    continue;
                }
                Err(_) => continue,
            };
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        req.extend_from_slice(&buf[..n]);
                        if req.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                        if req.len() > 64 * 1024 { break; }
                    }
                    Err(_) => break,
                }
            }
            let text = String::from_utf8_lossy(&req).to_string();
            let mut lines = text.lines();
            let first = lines.next().unwrap_or("");
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("GET");
            let path = parts.next().unwrap_or("/");
            let headers_len = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(text.len());
            let body = text.get(headers_len..).unwrap_or("");

            if method == "GET" && (path == "/" || path.starts_with("/?")) {
                reply(&mut s, "200 OK", "text/html; charset=utf-8", page(&page_lang).as_bytes());
            } else if method == "GET" && path.starts_with("/api/state") {
                let body = { let st = state_for_server.lock().unwrap(); state_json(&st) };
                reply(&mut s, "200 OK", "application/json", body.as_bytes());
            } else if method == "GET" && path.starts_with("/api/qr") {
                let st = state_for_server.lock().unwrap();
                if st.qr.is_empty() { reply(&mut s, "404 Not Found", "text/plain", b"no qr"); }
                else { reply(&mut s, "200 OK", "image/jpeg", &st.qr); }
            } else if method == "POST" && path.starts_with("/api/login") {
                drop(state_for_server.lock());
                start_wechat(&state_for_server, port);
                reply(&mut s, "200 OK", "application/json", b"{\"ok\":true}");
            } else if method == "POST" && path.starts_with("/api/cancel") {
                // Ask the background flow to stop; polling checks this flag.
                state_for_server.lock().unwrap().cancelled = true;
                reply(&mut s, "200 OK", "application/json", b"{\"ok\":true}");
            } else if method == "POST" && path.starts_with("/save") {
                let base = match field(body, "base") {
                    Some(b) if b == "custom" => field(body, "custom_base").unwrap_or_else(|| "https://api.openai.com/v1".into()),
                    Some(b) => b,
                    None => String::new(),
                };
                let key = field(body, "key").unwrap_or_default();
                let model = field(body, "model").unwrap_or_default();
                if key.is_empty() {
                    reply(&mut s, "400 Bad Request", "text/html; charset=utf-8", b"<p>API key required</p>");
                } else {
                    if let Ok(t) = tx.lock() { let _ = t.send(Submitted { key, base, model }); }
                    reply(&mut s, "200 OK", "text/html; charset=utf-8",
                        b"<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'><body style='font-family:system-ui;background:#0b0f14;color:#e6edf3;padding:32px;text-align:center'><h2 style='color:#3fb950'>Saved to device</h2><p>You can close this page.</p></body>");
                }
            } else {
                reply(&mut s, "404 Not Found", "text/plain", b"not found");
            }
        }
    });
    Ok(Session { url, port, shared, rx, stop })
}

/// Render a QR module matrix into a 320x170 RGB888 framebuffer image.
/// The same geometry is used for the real panel blit and for host-side
/// scanability checks, so what is tested is what the device shows.
pub fn render_matrix_rgb(
    matrix: &[String],
    rect: Option<(usize, usize, usize, usize)>,
) -> Result<(usize, usize, usize, Vec<u8>), String> {
    let n = matrix.len();
    if n == 0 || matrix.iter().any(|r| r.len() != n) {
        return Err("bad matrix".into());
    }
    const FB_W: usize = 320;
    const FB_H: usize = 170;
    let (rx, ry, rw, rh) = rect.unwrap_or((0, 0, FB_W, FB_H));
    let mut scale = (rw / n).min(rh / n).max(1);
    // Keep a 2px quiet zone when possible.
    if scale > 1 && (n + 4) * scale > rw.min(rh) {
        scale -= 1;
    }
    let w = n * scale;
    let h = n * scale;
    let ox = rx + rw.saturating_sub(w) / 2;
    let oy = ry + rh.saturating_sub(h) / 2;
    let mut fb = vec![0u8; FB_W * FB_H * 3];
    for py in ry..(ry + rh).min(FB_H) {
        for px in rx..(rx + rw).min(FB_W) {
            let o = (py * FB_W + px) * 3;
            fb[o] = 0xFF;
            fb[o + 1] = 0xFF;
            fb[o + 2] = 0xFF;
        }
    }
    for (mi, row) in matrix.iter().enumerate() {
        for (mj, ch) in row.bytes().enumerate() {
            if ch != b'1' { continue; }
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = ox + mj * scale + dx;
                    let py = oy + mi * scale + dy;
                    if px >= FB_W || py >= FB_H { continue; }
                    let o = (py * FB_W + px) * 3;
                    fb[o] = 0;
                    fb[o + 1] = 0;
                    fb[o + 2] = 0;
                }
            }
        }
    }
    Ok((w, h, FB_W, fb))
}

/// PPM (P6) of exactly what would be blitted for this matrix.
pub fn matrix_ppm(matrix: &[String], rect: Option<(usize, usize, usize, usize)>) -> Result<Vec<u8>, String> {
    let (_w, _h, fb_w, rgb) = render_matrix_rgb(matrix, rect)?;
    let mut out = format!("P6\n{fb_w} 170\n255\n").into_bytes();
    out.extend_from_slice(&rgb);
    Ok(out)
}

/// Render a QR module matrix (1 = dark) straight into the framebuffer.
pub fn draw_matrix_fb(matrix: &[String], rect: Option<(usize, usize, usize, usize)>) -> Result<(usize, usize), String> {
    const FB_W: usize = 320;
    const FB_H: usize = 170;
    let (w, h, _fw, rgb) = render_matrix_rgb(matrix, rect)?;
    let mut fb = vec![0u8; FB_W * FB_H * 2];
    for i in 0..(FB_W * FB_H) {
        let v: u16 = ((rgb[i * 3] as u16) << 8) | 0; // RGB565: red channel only (black/white)
        let value: u16 = if v > 0x7F00 { 0xFFFF } else { 0x0000 };
        fb[i * 2] = (value & 0xFF) as u8;
        fb[i * 2 + 1] = (value >> 8) as u8;
    }
    use std::io::{Write as _};
    let mut f = std::fs::OpenOptions::new().write(true).open("/dev/fb_panel")
        .map_err(|e| format!("/dev/fb_panel: {e}"))?;
    // Write row by row from the left edge of the panel.
    for y in 0..FB_H {
        let start = y * FB_W * 2;
        f.write_all(&fb[start..start + FB_W * 2])
            .map_err(|e| format!("fb write: {e}"))?;
    }
    f.flush().ok();
    Ok((w, h))
}

/// Render the QR for `url` as text lines using `qrencode` when available.
/// Returns (qr_lines, note) where note explains a fallback.
pub fn qr_lines(url: &str) -> (Vec<String>, String) {
    let out = std::process::Command::new("qrencode")
        .args(["-t", "UTF8", "-m", "0", "-o", "-"])
        .arg(url)
        .output();
    match out {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let lines: Vec<String> = text
                .lines()
                .map(|l| l.trim_end().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            if lines.is_empty() {
                (Vec::new(), "qrencode produced nothing".into())
            } else {
                (lines, String::new())
            }
        }
        Ok(_) => (Vec::new(), "qrencode failed".into()),
        Err(_) => (
            Vec::new(),
            "qrencode not installed (apt install qrencode)".into(),
        ),
    }
}

/// Render the QR and blit it straight into the framebuffer.
///
/// `qrencode` on this image supports SVG but not PBM, and terminal block glyphs
/// are not pixel-exact on a 6x13 cell (the 1px gaps break the module grid), so
/// the code is rasterised from SVG `<rect>` elements ourselves and written as
/// pixels. `rect` limits the blit area so the guide text drawn by the terminal
/// on the right stays intact.
pub fn draw_qr_fb(url: &str, rect: Option<(usize, usize, usize, usize)>) -> Result<(usize, usize), String> {
    const SCALE: usize = 5;
    let out = std::process::Command::new("qrencode")
        .args(["-t", "SVG", "-m", "4", "-o", "-"])
        .arg(url)
        .output()
        .map_err(|e| format!("qrencode: {e}"))?;
    if !out.status.success() {
        return Err("qrencode failed".into());
    }
    let svg = String::from_utf8_lossy(&out.stdout);

    // viewBox tells us the module count (QR modules + quiet zone).
    let vb = svg
        .split("viewBox=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .ok_or("no viewBox")?;
    let mut dims = vb.split_whitespace().skip(2).map(|v| v.parse::<f64>());
    let vw = dims.next().and_then(|r| r.ok()).ok_or("bad viewBox width")?;
    let vh = dims.next().and_then(|r| r.ok()).ok_or("bad viewBox height")?;
    let modules_w = vw.round() as usize;
    let modules_h = vh.round() as usize;
    if modules_w == 0 || modules_h == 0 || modules_w > 200 {
        return Err("implausible SVG size".into());
    }

    // Rasterise: one rect per dark module in viewBox units.
    let w = modules_w * SCALE;
    let h = modules_h * SCALE;
    let mut bitmap = vec![false; w * h];
    let mut rest: &str = &svg;
    while let Some(i) = rest.find("<rect") {
        rest = &rest[i + 5..];
        let end = match rest.find('>') {
            Some(e) => e,
            None => break,
        };
        let tag = &rest[..end];
        rest = &rest[end..];
        let attr = |name: &str| -> Option<f64> {
            let pat = format!("{name}=\"");
            let i = tag.find(&pat)? + pat.len();
            let j = tag[i..].find('"')? + i;
            tag[i..j].trim().parse::<f64>().ok()
        };
        // qrencode emits a white background rect plus one rect per dark
        // module; only the dark ones are modules.
        let lc = tag.to_ascii_lowercase();
        if lc.contains("ffffff") || lc.contains("fill:#fff") || lc.contains("fill: #fff") {
            continue;
        }
        let (x, y) = match (attr("x"), attr("y")) {
            (Some(x), Some(y)) => (x, y),
            _ => continue,
        };
        let (rw, rh) = (attr("width").unwrap_or(1.0), attr("height").unwrap_or(1.0));
        let x0 = (x * SCALE as f64).round() as usize;
        let y0 = (y * SCALE as f64).round() as usize;
        let rw = (rw * SCALE as f64).round() as usize;
        let rh = (rh * SCALE as f64).round() as usize;
        for py in y0..(y0 + rh).min(h) {
            for px in x0..(x0 + rw).min(w) {
                bitmap[py * w + px] = true;
            }
        }
    }

    const FB_W: usize = 320;
    const FB_H: usize = 170;
    let (rx, ry, rw, rh) = rect.unwrap_or((0, 0, FB_W, FB_H));
    let ox = rx + rw.saturating_sub(w) / 2;
    let oy = ry + rh.saturating_sub(h) / 2;
    let mut fb = vec![0u8; FB_W * FB_H * 2];
    // Standard polarity: white background, black modules (phones do not all
    // handle inverted codes).
    for y in 0..rh.min(FB_H.saturating_sub(ry)) {
        let start = ((ry + y) * FB_W + rx) * 2;
        let end = start + rw.min(FB_W.saturating_sub(rx)) * 2;
        if end <= fb.len() {
            for b in &mut fb[start..end] {
                *b = 0xFF;
            }
        }
    }
    for y in 0..h {
        let py = oy + y;
        if py >= FB_H {
            break;
        }
        for x in 0..w {
            let px = ox + x;
            if px >= FB_W {
                break;
            }
            let v: u16 = if bitmap[y * w + x] { 0x0000 } else { 0xFFFF };
            let o = (py * FB_W + px) * 2;
            fb[o] = (v & 0xFF) as u8;
            fb[o + 1] = (v >> 8) as u8;
        }
    }
    use std::io::{Seek, SeekFrom, Write as _};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/fb_panel")
        .map_err(|e| format!("/dev/fb_panel: {e}"))?;
    for y in 0..rh.min(FB_H.saturating_sub(ry)) {
        let start = ((ry + y) * FB_W + rx) * 2;
        let end = start + rw.min(FB_W.saturating_sub(rx)) * 2;
        if end > fb.len() {
            break;
        }
        f.seek(SeekFrom::Start(start as u64))
            .map_err(|e| format!("fb seek: {e}"))?;
        f.write_all(&fb[start..end])
            .map_err(|e| format!("fb write: {e}"))?;
    }
    f.flush().ok();
    Ok((w, h))
}
