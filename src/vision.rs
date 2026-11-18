//! Local image attachments for multimodal turns.
//!
//! DeepSeek's Chat Completions API accepts images when `content` is an array of
//! blocks instead of a plain string (JPEG, PNG, GIF and WebP; the format is
//! detected from the file bytes, not the name):
//!
//! ```json
//! {"role":"user","content":[
//!   {"type":"text","text":"what is in this image?"},
//!   {"type":"image_url","image_url":{"url":"data:image/png;base64,<...>"}}]}
//! ```
//!
//! This module owns the *local file* side of that contract: resolve a path,
//! sniff the image type from its magic bytes, enforce a size cap, base64-encode
//! it into a `data:` URL, and produce the small label/placeholder the TUI and
//! the saved transcript use. The network side lives in `harness` (message
//! building) and `api` (curl body).
//!
//! Images are the most expensive thing a turn can carry (up to ~1024 tokens
//! each, and the encoded payload dominates the request body), so only the first
//! request of a turn keeps the raw blocks: after the model has answered, the
//! message is rewritten to a text placeholder (`Attachment::placeholder`). That
//! keeps follow-up steps, session files and checkpoints small.

use std::path::PathBuf;

/// Supported by DeepSeek vision: JPEG, PNG, GIF, WebP (sniffed from content).
pub const MIME_JPEG: &str = "image/jpeg";
pub const MIME_PNG: &str = "image/png";
pub const MIME_GIF: &str = "image/gif";
pub const MIME_WEBP: &str = "image/webp";

/// Refuse files bigger than this. A 8 MB photo is already ~11 MB of base64 in
/// the request body; the API re-scales images to ~1300x1300 anyway, so there is
/// nothing to gain by accepting larger files on a 446 MB device.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Keep a whole turn bounded: at most this many images per message.
pub const MAX_PER_MESSAGE: usize = 2;

/// One image loaded from disk, ready to be embedded in a request.
#[derive(Clone, Debug)]
pub struct Attachment {
    /// Absolute path as shown to the user.
    pub path: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    /// Pixel size when it can be read from the header (0x0 when unknown).
    pub width: u32,
    pub height: u32,
}

impl Attachment {
    /// Load and validate a local image file (`~`, relative and absolute paths).
    pub fn load(path: &str) -> Result<Attachment, String> {
        let raw = path.trim();
        if raw.is_empty() {
            return Err("empty path".into());
        }
        let resolved = resolve(raw);
        let meta = std::fs::metadata(&resolved)
            .map_err(|e| format!("{}: {e}", crate::config::home_shorten(&resolved.display().to_string())))?;
        if !meta.is_file() {
            return Err(format!("{raw} is not a file"));
        }
        if meta.len() as usize > MAX_BYTES {
            return Err(format!(
                "{} is {} MB; the limit is {} MB — resize it or use /ocr",
                raw,
                meta.len() / (1024 * 1024),
                MAX_BYTES / (1024 * 1024)
            ));
        }
        let bytes = std::fs::read(&resolved).map_err(|e| format!("{raw}: {e}"))?;
        let mime = sniff(&bytes).ok_or_else(|| {
            format!("{raw} is not JPEG, PNG, GIF or WebP (detected from file content)")
        })?;
        let (width, height) = dimensions(&bytes, mime);
        Ok(Attachment {
            path: resolved.display().to_string(),
            mime,
            bytes,
            width,
            height,
        })
    }

    /// `data:` URL accepted by the API: `data:image/png;base64,<payload>`.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, b64(&self.bytes))
    }

    /// One-line human label: path, format, size in pixels and bytes.
    pub fn label(&self) -> String {
        let short = crate::config::home_shorten(&self.path);
        let dims = if self.width > 0 && self.height > 0 {
            format!(" · {}x{}", self.width, self.height)
        } else {
            String::new()
        };
        format!(
            "{short} ({}{dims} · {})",
            self.mime.trim_start_matches("image/"),
            human_bytes(self.bytes.len())
        )
    }

    /// What the transcript keeps once the model has seen the image.
    pub fn placeholder(&self) -> String {
        format!("[image attached: {}]", self.label())
    }
}

/// The *user's* home directory (`$HOME`), not dst's state dir. `config::home()`
/// is `~/.dst`, so expanding `~` with it turned `~/photo.png` into
/// `~/.dst/photo.png`.
fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(crate::config::home)
}

fn resolve(path: &str) -> PathBuf {
    let p = if path == "~" {
        user_home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        user_home().join(rest)
    } else {
        PathBuf::from(path)
    };
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
    }
}

fn human_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", (n / 1024).max(1))
    }
}

/// Image type from the first bytes (the API does exactly this, so a copied
/// `.jpg` that is really a PNG still works).
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some(MIME_JPEG);
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some(MIME_PNG);
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some(MIME_GIF);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(MIME_WEBP);
    }
    None
}

fn be16(b: &[u8]) -> u32 {
    u32::from(((b[0] as u16) << 8) | b[1] as u16)
}

fn le16(b: &[u8]) -> u32 {
    u32::from((b[0] as u16) | ((b[1] as u16) << 8))
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Pixel dimensions from the header (0x0 when the format needs a full decode).
fn dimensions(bytes: &[u8], mime: &str) -> (u32, u32) {
    match mime {
        MIME_PNG if bytes.len() >= 24 => (
            be32(&bytes[16..20]),
            be32(&bytes[20..24]),
        ),
        MIME_GIF if bytes.len() >= 10 => (
            le16(&bytes[6..8]),
            le16(&bytes[8..10]),
        ),
        MIME_JPEG => jpeg_dimensions(bytes).unwrap_or((0, 0)),
        MIME_WEBP => webp_dimensions(bytes).unwrap_or((0, 0)),
        _ => (0, 0),
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xff {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        i += 2;
        // Standalone markers carry no length.
        if marker == 0xd8 || (0xd0..=0xd7).contains(&marker) || marker == 0x01 {
            continue;
        }
        if i + 1 >= bytes.len() {
            return None;
        }
        let len = be16(&bytes[i..i + 2]) as usize;
        if (0xc0..=0xc3).contains(&marker)
            || (0xc5..=0xc7).contains(&marker)
            || (0xc9..=0xcb).contains(&marker)
            || (0xcd..=0xcf).contains(&marker)
        {
            if i + 7 < bytes.len() {
                let h = be16(&bytes[i + 3..i + 5]);
                let w = be16(&bytes[i + 5..i + 7]);
                return Some((w, h));
            }
            return None;
        }
        if len < 2 {
            return None;
        }
        i += len;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 {
        return None;
    }
    let chunk = &bytes[12..16];
    if chunk == b"VP8X" {
        // 24-bit little-endian canvas size minus one.
        let w = 1 + u32::from(bytes[24]) + (u32::from(bytes[25]) << 8) + (u32::from(bytes[26]) << 16);
        let h = 1 + u32::from(bytes[27]) + (u32::from(bytes[28]) << 8) + (u32::from(bytes[29]) << 16);
        return Some((w, h));
    }
    if chunk == b"VP8 " && bytes.len() >= 30 {
        // Lossy: 14-bit width/height after the 3-byte start code.
        let w = u32::from(bytes[26]) | (u32::from(bytes[27] & 0x3f) << 8);
        let h = u32::from(bytes[28]) | (u32::from(bytes[29] & 0x3f) << 8);
        return Some((w, h));
    }
    if chunk == b"VP8L" && bytes.len() >= 25 {
        // Lossless: 14-bit dimensions packed little-endian.
        let b = &bytes[21..25];
        let bits = u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16) | (u32::from(b[3]) << 24);
        let w = 1 + (bits & 0x3fff);
        let h = 1 + ((bits >> 14) & 0x3fff);
        return Some((w, h));
    }
    None
}

/// Standard base64 (RFC 4648, with padding). Written against `std` because the
/// crate has no dependencies; used only for image payloads.
pub fn b64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "dst-vision-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        p
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64(&[0xff, 0x00, 0x10]), "/wAQ");
    }

    #[test]
    fn sniffs_supported_formats_from_content() {
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(MIME_JPEG));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n....."), Some(MIME_PNG));
        assert_eq!(sniff(b"GIF89a......"), Some(MIME_GIF));
        assert_eq!(sniff(b"RIFF\x10\x00\x00\x00WEBPVP8 "), Some(MIME_WEBP));
        assert_eq!(sniff(b"not an image"), None);
    }

    #[test]
    fn png_dimensions_come_from_the_header() {
        // 3x2 PNG header (signature + IHDR width/height).
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&2u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert_eq!(sniff(&png), Some(MIME_PNG));
        assert_eq!(dimensions(&png, MIME_PNG), (3, 2));
    }

    #[test]
    fn load_rejects_unsupported_and_oversized_files() {
        let not_image = tmp("notes.jpg");
        std::fs::write(&not_image, b"this is text, not a jpeg").unwrap();
        let err = Attachment::load(not_image.to_str().unwrap()).unwrap_err();
        assert!(err.contains("not JPEG"), "{err}");

        let big = tmp("big.png");
        std::fs::write(&big, vec![0u8; MAX_BYTES + 1]).unwrap();
        let err = Attachment::load(big.to_str().unwrap()).unwrap_err();
        assert!(err.contains("limit"), "{err}");

        let _ = std::fs::remove_file(not_image);
        let _ = std::fs::remove_file(big);
    }

    #[test]
    fn load_accepts_a_real_image_and_builds_the_blocks() {
        let path = tmp("tiny.png");
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(&path, &png).unwrap();
        let a = Attachment::load(path.to_str().unwrap()).unwrap();
        assert_eq!(a.mime, MIME_PNG);
        assert_eq!((a.width, a.height), (1, 1));
        assert!(a.data_url().starts_with("data:image/png;base64,"));
        assert!(a.label().contains("1x1"));
        assert!(a.label().contains("png"));
        assert!(a.placeholder().starts_with("[image attached: "));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tilde_paths_resolve_to_the_user_home() {
        let home = user_home();
        assert!(resolve("~/x.png").starts_with(&home));
        assert!(resolve("~").starts_with(&home));
        // dst's own state dir is ~/.dst, not ~; the difference mattered.
        assert_ne!(home, crate::config::home());
    }
}
