//! QR `/login` flow.
//!
//! `dst` starts a tiny HTTP server on the LAN, renders a QR code for its URL in
//! the terminal, and waits for the phone to submit an API key through a plain
//! HTML form. Nothing is sent to any third party: the key travels from the
//! phone to this device over the local network and is written to
//! `~/.dst/api_key` (mode 600).
//!
//! The server is written against `std::net` and needs no crates; the QR image
//! is produced by the optional `qrencode` binary and degrades to a printed URL
//! when it is not installed.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

pub const DEFAULT_PORT: u16 = 8787;

/// Best-effort LAN address of this device (no external traffic is generated).
pub fn lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) {
        // Connecting a UDP socket does not send packets; it only asks the
        // kernel which source address would be used.
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

fn html(ip: &str, port: u16, err: &str) -> String {
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>dst login</title>
<style>
 body{{font-family:system-ui,sans-serif;margin:0;padding:24px;background:#101418;color:#e6edf3}}
 h1{{font-size:20px;margin:0 0 4px}} p{{color:#8b949e;font-size:14px}}
 label{{display:block;margin:18px 0 6px;font-size:13px;color:#8b949e}}
 input,select{{width:100%;padding:12px;font-size:16px;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#e6edf3}}
 button{{margin-top:20px;width:100%;padding:14px;font-size:16px;border:0;border-radius:8px;background:#2f81f7;color:#fff}}
 .e{{color:#f85149;margin-top:12px}} .ok{{color:#3fb950}}
 code{{background:#0d1117;padding:2px 6px;border-radius:4px}}
</style></head><body>
<h1>dst on Cardputer Zero</h1>
<p>{ip}:{port} — paste the API key this device should use.</p>
{error}
<form method="POST" action="/save">
  <label>Provider</label>
  <select name="base">
    <option value="https://api.deepseek.com/v1">DeepSeek official (api.deepseek.com)</option>
    <option value="custom">Other / self-hosted (OpenAI-compatible)</option>
  </select>
  <label>Custom base URL (only if "Other" above)</label>
  <input name="custom_base" placeholder="https://your.endpoint/v1" autocomplete="off">
  <label>API key</label>
  <input name="key" type="password" placeholder="sk-..." autocomplete="off" required>
  <label>Model (optional)</label>
  <input name="model" placeholder="deepseek-flash" autocomplete="off">
  <button type="submit">Save to device</button>
</form>
</body></html>"#,
        ip = ip,
        port = port,
        error = if err.is_empty() {
            String::new()
        } else {
            format!(r#"<p class="e">{err}</p>"#)
        }
    )
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
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
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

pub struct Submitted {
    pub key: String,
    pub base: String,
    pub model: String,
}

/// Start the login server; returns the URL to encode plus a channel that
/// yields the submitted credentials.
pub fn start(port: u16) -> Result<(String, Receiver<Submitted>), String> {
    let ip = lan_ip();
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
        .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    let url = format!("http://{ip}:{port}/");
    let (tx, rx) = channel::<Submitted>();
    let tx = Arc::new(Mutex::new(tx));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            let mut buf = [0u8; 16384];
            let n = match s.read(&mut buf) {
                Ok(n) => n,
                Err(_) => continue,
            };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let mut lines = req.lines();
            let first = lines.next().unwrap_or("");
            let is_post = first.starts_with("POST /save");
            // Read the body (Content-Length is all we need).
            let mut body = String::new();
            if let Some(idx) = req.find("\r\n\r\n") {
                body = req[idx + 4..].to_string();
            }
            let want_len = req
                .lines()
                .find_map(|l| {
                    let l = l.to_ascii_lowercase();
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            while body.len() < want_len {
                let mut more = [0u8; 4096];
                match s.read(&mut more) {
                    Ok(0) => break,
                    Ok(m) => body.push_str(&String::from_utf8_lossy(&more[..m])),
                    Err(_) => break,
                }
            }

            if is_post {
                let key = field(&body, "key").unwrap_or_default();
                let base_choice = field(&body, "base").unwrap_or_default();
                let custom = field(&body, "custom_base").unwrap_or_default();
                let model = field(&body, "model").unwrap_or_default();
                if key.len() < 8 {
                    let page = html(&ip, port, "That key looks too short.");
                    let _ = write!(
                        s,
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        page.len(),
                        page
                    );
                    continue;
                }
                let base = if base_choice == "custom" && !custom.is_empty() {
                    custom
                } else if !base_choice.is_empty() && base_choice != "custom" {
                    base_choice
                } else {
                    "https://api.deepseek.com/v1".to_string()
                };
                let page = format!(
                    "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>dst</title></head><body style=\"font-family:system-ui;background:#101418;color:#3fb950;padding:32px\"><h1>Saved.</h1><p style=\"color:#8b949e\">You can close this page; dst will continue.</p></body></html>"
                );
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    page.len(),
                    page
                );
                if let Ok(g) = tx.lock() {
                    let _ = g.send(Submitted { key, base, model });
                }
                break;
            }

            // GET / -> form (rebuild the msg only for the current query).
            let page = html(&ip, port, "");
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                page.len(),
                page
            );
        }
    });
    Ok((url, rx))
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
