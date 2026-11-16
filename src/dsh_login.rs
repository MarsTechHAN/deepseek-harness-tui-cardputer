//! Real DeepSeek Platform login (DSH token) — the nine-step flow used by the
//! DSH desktop client, ported to pure std + curl.
//!
//! Flow (all plain HTTP):
//!   ① GET  open.weixin.qq.com/connect/qrconnect      → uuid
//!   ② GET  open.weixin.qq.com/connect/qrcode/<uuid>  → 470x470 JPEG
//!   ③ GET  lp.open.weixin.qq.com/connect/l/qrconnect  → wx_errcode / wx_code
//!   ④ GET  platform/auth-api/.../wechat/callback      → 307 Location nonce
//!   ⑤ POST platform/auth-api/.../oauth/get_token      → web session token
//!   ⑥ POST platform/auth-api/.../dsh/auth_init        → authorize_id
//!   ⑦ POST platform/auth-api/.../dsh/auth_approve     → code
//!   ⑧ POST platform/auth-api/.../dsh/auth_exchange    → DSH token
//!
//! The DSH token is sent to the model API as `x-dsh-auth-token` (no Bearer).
//! Base URLs can be overridden for tests:
//! `DST_LOGIN_PLATFORM`, `DST_LOGIN_WECHAT`, `DST_LOGIN_WECHAT_LP`,
//! `DST_LOGIN_API`.

use crate::json::parse;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const WECHAT_APPID: &str = "wx335255e1b73f9e52";
const CLIENT_VERSION: &str = "0.2.2-rc.3";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const TIMEOUT: &str = "40";

fn platform_base() -> String {
    std::env::var("DST_LOGIN_PLATFORM").unwrap_or_else(|_| "https://platform.deepseek.com".into())
}
fn wechat_base() -> String {
    std::env::var("DST_LOGIN_WECHAT").unwrap_or_else(|_| "https://open.weixin.qq.com".into())
}
fn wechat_lp_base() -> String {
    std::env::var("DST_LOGIN_WECHAT_LP").unwrap_or_else(|_| "https://lp.open.weixin.qq.com".into())
}
fn api_base() -> String {
    std::env::var("DST_LOGIN_API").unwrap_or_else(|_| "https://api.deepseek.com".into())
}
fn wechat_callback_url() -> String {
    format!("{}/auth-api/v0/users/oauth/wechat/callback", platform_base())
}

/// Progress events for the UI/page.
pub enum Event {
    Qr(Vec<u8>),
    Phase(String),
    Message(String),
    Step(String),
    WebToken(String),
}

pub struct Resp {
    pub status: u16,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
    pub fn json(&self) -> Option<crate::json::J> {
        parse(&self.text())
    }
    /// Platform envelope `{code,msg,data:{biz_code,biz_msg,biz_data}}`.
    pub fn biz(&self) -> Option<crate::json::J> {
        let v = self.json()?;
        if v.get("code")?.as_num()? != 0.0 {
            return None;
        }
        let d = v.get("data")?;
        if d.get("biz_code")?.as_num()? != 0.0 {
            return None;
        }
        d.get("biz_data").cloned()
    }
    pub fn describe(&self) -> String {
        match self.json() {
            Some(v) => {
                let code = v.get("code").and_then(|x| x.as_num()).unwrap_or(-1.0) as i64;
                let msg = v.get("msg").and_then(|x| x.as_str()).unwrap_or("");
                let bc = v
                    .get("data")
                    .and_then(|d| d.get("biz_code"))
                    .and_then(|x| x.as_num())
                    .unwrap_or(-1.0) as i64;
                let bm = v
                    .get("data")
                    .and_then(|d| d.get("biz_msg"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                format!("HTTP {} code={code} msg={msg} biz_code={bc} biz_msg={bm}", self.status)
            }
            None => format!(
                "HTTP {} {}",
                self.status,
                self.text().chars().take(160).collect::<String>()
            ),
        }
    }
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_path(tag: &str) -> std::path::PathBuf {
    let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    crate::config::tmp_dir().join(format!("dshlog-{}-{tag}-{nanos}-{n}", std::process::id()))
}

/// HTTP via curl; redirects are NOT followed (step ④ needs the Location).
pub fn http(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<Resp, String> {
    let hdr = tmp_path("h");
    let out = tmp_path("b");
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", TIMEOUT, "-X", method, "-D"])
        .arg(&hdr)
        .arg("-o")
        .arg(&out);
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    if body.is_some() {
        cmd.arg("--data-binary").arg("@-");
        cmd.stdin(Stdio::piped());
    }
    cmd.arg(url);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("curl spawn: {e}"))?;
    if let Some(b) = body {
        if let Some(mut si) = child.stdin.take() {
            si.write_all(b.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let body_bytes = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    let header_text = std::fs::read_to_string(&hdr).unwrap_or_default();
    let _ = std::fs::remove_file(&hdr);
    if !output.status.success() && header_text.is_empty() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut status = 0u16;
    let mut location = None;
    for line in header_text.lines() {
        if let Some(rest) = line.strip_prefix("HTTP/") {
            if let Some(code) = rest.split_whitespace().nth(1) {
                status = code.parse().unwrap_or(status);
            }
        } else if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("location") {
                location = Some(v.trim().to_string());
            }
        }
    }
    Ok(Resp { status, location, body: body_bytes })
}

// ---------------------------------------------------------------- crypto ---

/// SHA-256 (FIPS 180-4), std-only.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(T[(n & 63) as usize] as char);
        }
    }
    out
}

pub fn random_bytes(len: usize) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open("/dev/urandom").map_err(|e| format!("/dev/urandom: {e}"))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

pub fn pkce_verifier() -> Result<String, String> {
    Ok(base64url(&random_bytes(32)?))
}

pub fn uuid_v4() -> Result<String, String> {
    let mut b = random_bytes(16)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// Persisted device id (0600), generated once.
pub fn device_id() -> String {
    let path = crate::config::home().join("device_id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    let id = uuid_v4().unwrap_or_else(|_| "00000000-0000-4000-8000-000000000000".into());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&path, format!("{id}\n")).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    id
}

pub fn mask(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    if chars.len() <= 8 {
        return "****".into();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

// ------------------------------------------------------------- parsing ----

pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.as_bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    for pair in q.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

pub fn find_uuid(html: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(rel) = html[cursor..].find("uuid") {
        let i = cursor + rel;
        let after = &html[i + 4..];
        let t = after.trim_start();
        if let Some(rest) = t.strip_prefix('=').or_else(|| t.strip_prefix(':')) {
            let cand: String = rest
                .trim_start()
                .trim_start_matches(['"', '\''])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .collect();
            if cand.len() >= 10 {
                return Some(cand);
            }
        }
        cursor = i + 4;
    }
    None
}

pub fn find_wx_errcode(text: &str) -> Option<String> {
    let start = text.find("wx_errcode")?;
    let rest = &text[start + "wx_errcode".len()..];
    let digits: String = rest
        .trim_start()
        .trim_start_matches('=')
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then_some(digits)
}

pub fn find_wx_code(text: &str) -> Option<String> {
    let start = text.find("wx_code")?;
    let rest = &text[start + "wx_code".len()..];
    let t = rest.trim_start().trim_start_matches('=').trim_start();
    let t = t.trim_start_matches(['"', '\'']);
    let code: String = t
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    (!code.is_empty()).then_some(code)
}

// ---------------------------------------------------------------- flow ----

struct Flow {
    verifier: String,
    state: String,
    redirect_uri: String,
}

impl Flow {
    fn new(port: u16) -> Result<Self, String> {
        Ok(Flow {
            verifier: pkce_verifier()?,
            state: base64url(&random_bytes(32)?),
            redirect_uri: format!("http://127.0.0.1:{port}/oauth/callback"),
        })
    }
    fn challenge(&self) -> String {
        base64url(&sha256(self.verifier.as_bytes()))
    }
    fn client_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("content-type", "application/json".into()),
            ("user-agent", UA.into()),
            ("x-client-bundle-id", String::new()),
            ("x-client-platform", "web".into()),
            ("x-client-version", CLIENT_VERSION.into()),
            ("x-client-locale", "zh_CN".into()),
            ("x-client-timezone-offset", "28800".into()),
        ]
    }
}

fn hdrs<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(k, s)| (*k, s.as_str())).collect()
}

fn envelope<'a>(r: &'a Resp, what: &str) -> Result<crate::json::J, String> {
    r.biz().ok_or_else(|| format!("{what} failed: {}", r.describe()))
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// ① QR page → (uuid, jpeg bytes)
fn wechat_qr() -> Result<(String, Vec<u8>), String> {
    let url = format!(
        "{}/connect/qrconnect?appid={WECHAT_APPID}&scope=snsapi_login&redirect_uri={}&state=&login_type=jssdk&self_redirect=false&styletype=&sizetype=&bgcolor=&rst={}&stylelite=1&color_scheme=light",
        wechat_base(),
        percent_encode(&wechat_callback_url()),
        now_millis()
    );
    let page = http("GET", &url, &[("user-agent", UA)], None)?;
    let html = page.text();
    let uuid = find_uuid(&html)
        .ok_or_else(|| format!("qrconnect page has no uuid (HTTP {})", page.status))?;
    let img = http(
        "GET",
        &format!("{}/connect/qrcode/{uuid}", wechat_base()),
        &[("user-agent", UA)],
        None,
    )?;
    if img.body.is_empty() || img.status >= 400 {
        return Err(format!("qrcode image failed: HTTP {}", img.status));
    }
    Ok((uuid, img.body))
}

/// ② long-poll scan result: 408 waiting / 404 scanned / 405 ok / 402 expired / 403 cancel
fn poll_scan(
    uuid: &str,
    emit: &mut dyn FnMut(Event),
) -> Result<String, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut scanned = false;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let url = format!(
            "{}/connect/l/qrconnect?uuid={uuid}&_={}",
            wechat_lp_base(),
            now_millis()
        );
        let r = http(
            "GET",
            &url,
            &[("user-agent", UA), ("referer", &format!("{}/", wechat_base()))],
            None,
        )?;
        let text = r.text();
        match find_wx_errcode(&text).as_deref() {
            Some("405") => {
                let code = find_wx_code(&text).ok_or("405 without wx_code")?;
                emit(Event::Step(format!("② wx_code={}", mask(&code))));
                return Ok(code);
            }
            Some("404") if !scanned => {
                scanned = true;
                emit(Event::Message("scanned, confirm on the phone".into()));
                emit(Event::Step("② scanned, waiting for phone confirmation".into()));
            }
            Some("402") => return Err("QR code expired, please retry".into()),
            Some("403") => return Err("login cancelled on the phone".into()),
            _ => {}
        }
    }
    Err("timed out waiting for the scan".into())
}

/// ③ callback (no redirect follow) → nonce
fn wechat_callback(wx_code: &str) -> Result<String, String> {
    let url = format!(
        "{}/auth-api/v0/users/oauth/wechat/callback?code={}&state=",
        platform_base(),
        percent_encode(wx_code)
    );
    let r = http("GET", &url, &[("user-agent", UA)], None)?;
    let loc = r
        .location
        .clone()
        .ok_or_else(|| format!("callback without Location: {}", r.describe()))?;
    let absolute = if loc.starts_with("http") {
        loc
    } else {
        format!("{}{}", platform_base(), loc)
    };
    query_param(&absolute, "nonce").ok_or_else(|| "callback Location has no nonce".into())
}

/// ④ nonce → web session token (platform APIs only)
fn get_token(nonce: &str) -> Result<String, String> {
    let body = format!(
        "{{\"nonce\":{},\"provider\":\"WECHAT\"}}",
        crate::json::J::s(nonce).dump()
    );
    let r = http(
        "POST",
        &format!("{}/auth-api/v0/users/oauth/get_token", platform_base()),
        &[
            ("content-type", "application/json"),
            ("user-agent", UA),
            ("x-client-platform", "web"),
        ],
        Some(&body),
    )?;
    let d = envelope(&r, "get_token")?;
    d.get("token")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "get_token returned no token".into())
}

/// ⑤ auth_init (PKCE) → authorize_id
fn auth_init(flow: &Flow) -> Result<String, String> {
    let body = format!(
        "{{\"code_challenge\":{},\"code_challenge_method\":\"S256\",\"state\":{},\"redirect_uri\":{},\"locale\":\"zh_CN\",\"login_source\":\"web\"}}",
        crate::json::J::s(&flow.challenge()).dump(),
        crate::json::J::s(&flow.state).dump(),
        crate::json::J::s(&flow.redirect_uri).dump()
    );
    let headers = flow.client_headers();
    let r = http(
        "POST",
        &format!("{}/auth-api/v0/dsh/auth_init", platform_base()),
        &hdrs(&headers),
        Some(&body),
    )?;
    let d = envelope(&r, "auth_init")?;
    d.get("authorize_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "auth_init returned no authorize_id".into())
}

/// ⑥ auth_approve → code (state checked)
fn auth_approve(flow: &Flow, web_token: &str, authorize_id: &str) -> Result<String, String> {
    let body = format!(
        "{{\"authorize_id\":{}}}",
        crate::json::J::s(authorize_id).dump()
    );
    let auth = format!("Bearer {web_token}");
    let referer = format!(
        "{}/dsh/authorize?authorize_id={}",
        platform_base(),
        percent_encode(authorize_id)
    );
    let r = http(
        "POST",
        &format!("{}/auth-api/v0/dsh/auth_approve", platform_base()),
        &[
            ("content-type", "application/json"),
            ("user-agent", UA),
            ("authorization", &auth),
            ("referer", &referer),
            ("x-client-platform", "web"),
        ],
        Some(&body),
    )?;
    let d = envelope(&r, "auth_approve")?;
    let cb = d
        .get("callback_url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "auth_approve returned no callback_url".to_string())?;
    let code = query_param(cb, "code").ok_or_else(|| "callback_url has no code".to_string())?;
    let returned = query_param(cb, "state").unwrap_or_default();
    if returned != flow.state {
        return Err("callback_url state mismatch (discarded)".into());
    }
    Ok(code)
}

/// ⑦ auth_exchange → DSH token
fn auth_exchange(flow: &Flow, code: &str) -> Result<String, String> {
    let body = format!(
        "{{\"code\":{},\"code_verifier\":{},\"redirect_uri\":{},\"device_id\":{},\"device_model\":{},\"os_version\":{}}}",
        crate::json::J::s(code).dump(),
        crate::json::J::s(&flow.verifier).dump(),
        crate::json::J::s(&flow.redirect_uri).dump(),
        crate::json::J::s(&device_id()).dump(),
        crate::json::J::s(&format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)).dump(),
        crate::json::J::s(&format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)).dump()
    );
    let headers = flow.client_headers();
    let r = http(
        "POST",
        &format!("{}/auth-api/v0/dsh/auth_exchange", platform_base()),
        &hdrs(&headers),
        Some(&body),
    )?;
    let d = envelope(&r, "auth_exchange")?;
    d.get("token")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "auth_exchange returned no token".into())
}

/// Run the whole flow. `port` is only used for the loopback redirect URI.
pub fn run(port: u16, emit: &mut dyn FnMut(Event)) -> Result<String, String> {
    let flow = Flow::new(port)?;
    emit(Event::Phase("qr".into()));
    emit(Event::Message("requesting WeChat QR".into()));
    let (uuid, jpeg) = wechat_qr()?;
    emit(Event::Qr(jpeg));
    emit(Event::Step(format!("① QR ready uuid={}", mask(&uuid))));
    emit(Event::Phase("waiting".into()));
    emit(Event::Message("scan the QR with WeChat".into()));

    let wx_code = poll_scan(&uuid, emit)?;
    emit(Event::Phase("exchanging".into()));
    emit(Event::Message("authorized, exchanging token".into()));

    let nonce = wechat_callback(&wx_code)?;
    emit(Event::Step(format!("③ nonce={}", mask(&nonce))));
    let web_token = get_token(&nonce)?;
    emit(Event::WebToken(web_token.clone()));
    emit(Event::Step(format!("④ web token={}", mask(&web_token))));
    let authorize_id = auth_init(&flow)?;
    emit(Event::Step(format!("⑤ authorize_id={}", mask(&authorize_id))));
    let code = auth_approve(&flow, &web_token, &authorize_id)?;
    emit(Event::Step(format!("⑥ code={}", mask(&code))));
    let token = auth_exchange(&flow, &code)?;
    emit(Event::Step(format!("⑦ DSH token={}", mask(&token))));
    emit(Event::Phase("done".into()));
    emit(Event::Message("login complete".into()));
    Ok(token)
}

/// Verify a DSH token with one tiny completion call.
pub fn verify_token(token: &str) -> Result<String, String> {
    let body = "{\"model\":\"deepseek-flash\",\"messages\":[{\"role\":\"user\",\"content\":\"Reply with OK.\"}],\"max_tokens\":8}";
    let r = http(
        "POST",
        &format!("{}/chat/completions", api_base()),
        &[
            ("content-type", "application/json"),
            ("user-agent", "dst/0.1"),
            ("x-dsh-auth-token", token),
        ],
        Some(body),
    )?;
    if r.status >= 400 {
        return Err(format!("verify failed: {}", r.describe()));
    }
    let v = parse(&r.text()).ok_or("verify: bad JSON")?;
    let reply = v
        .get("choices")
        .and_then(|c| c.as_arr())
        .and_then(|a| a.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Ok(reply)
}

// ------------------------------------------------- QR matrix extraction ----

const QR_HELPER: &str = r#"
import sys
from PIL import Image
path = sys.argv[1]
im = Image.open(path).convert("L")
w, h = im.size
px = im.load()
dark = lambda x, y: 0 <= x < w and 0 <= y < h and px[x, y] < 128
xs = []; ys = []
for y in range(0, h, 2):
    for x in range(0, w, 2):
        if dark(x, y):
            xs.append(x); ys.append(y)
if not xs:
    sys.exit(2)
x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
if x1 <= x0 or y1 <= y0:
    sys.exit(3)
# outer top edge of the top-left finder is 7 modules wide
run = 0
x = x0
while dark(x, y0):
    run += 1; x += 1
if run <= 0:
    sys.exit(4)
est = run / 7.0
finder = ["1111111","1000001","1011101","1011101","1011101","1000001","1111111"]
def sample(x0, y0, step):
    n = round((x1 - x0 + 1) / step)
    if (n - 17) % 4 != 0 or n < 21 or n > 177:
        return None
    def cell(i, j):
        cx = x0 + int((j + 0.5) * step)
        cy = y0 + int((i + 0.5) * step)
        return 1 if dark(cx, cy) else 0
    score = 0
    for (oi, oj) in [(0,0),(0,n-7),(n-7,0)]:
        for i in range(7):
            for j in range(7):
                if cell(oi+i, oj+j) == int(finder[i][j]):
                    score += 2 if (i in (0,6) or j in (0,6)) else 1
    total = 3 * (2*24 + 25)
    if score < total * 0.72:
        return None
    rows = []
    for i in range(n):
        rows.append("".join(str(cell(i,j)) for j in range(n)))
    return rows
best = None
for step in range(max(1, int(est*0.75)), int(est*1.35)+3):
    rows = sample(x0, y0, step)
    if rows:
        # prefer the candidate whose finder score we already accepted; first wins
        best = rows
        break
if best is None:
    sys.exit(5)
print("\n".join(best))
"#;

/// Decode a QR JPEG to a module matrix using python3+Pillow (device ships both).
/// Returns rows of "0"/"1" (1 = dark) without the quiet zone.
pub fn qr_matrix_from_jpeg(jpeg: &[u8]) -> Result<Vec<String>, String> {
    let path = tmp_path("qr.jpg");
    std::fs::write(&path, jpeg).map_err(|e| e.to_string())?;
    let out = Command::new("python3")
        .args(["-c", QR_HELPER])
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);
    let out = out.map_err(|e| format!("python3: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "QR matrix decoder failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let rows: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if rows.len() < 21 || rows.iter().any(|r| r.len() != rows.len()) {
        return Err("QR matrix decoder returned an invalid matrix".into());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            base64url(&sha256(b"abc")),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
    }

    #[test]
    fn base64url_padding() {
        assert_eq!(base64url(b"a"), "YQ");
        assert_eq!(base64url(b"ab"), "YWI");
        assert_eq!(base64url(b"abc"), "YWJj");
    }

    #[test]
    fn pkce_challenge_matches_verifier() {
        let v = pkce_verifier().unwrap();
        assert!(v.len() >= 43);
        let c = base64url(&sha256(v.as_bytes()));
        assert!(c.len() >= 43 && c != v);
    }

    #[test]
    fn uuid_shape() {
        let u = uuid_v4().unwrap();
        assert_eq!(u.len(), 36);
        assert_eq!(u.as_bytes()[8], b'-');
        assert_eq!(u.as_bytes()[13], b'-');
    }

    #[test]
    fn parses_wechat_js() {
        assert_eq!(
            find_wx_errcode("window.wx_errcode=405;window.wx_code='abc';").as_deref(),
            Some("405")
        );
        assert_eq!(
            find_wx_code("window.wx_errcode=405;window.wx_code='abc';").as_deref(),
            Some("abc")
        );
        assert_eq!(
            find_uuid(r#"<script>var uuid="011WD7Ie2jLmll2f";</script>"#).as_deref(),
            Some("011WD7Ie2jLmll2f")
        );
        assert_eq!(
            find_uuid(r#"<html><script>var uuid = "TESTUUID1234567890";</script></html>"#).as_deref(),
            Some("TESTUUID1234567890")
        );
        assert_eq!(query_param("http://x/cb?code=c&state=s", "state").as_deref(), Some("s"));
    }
}
