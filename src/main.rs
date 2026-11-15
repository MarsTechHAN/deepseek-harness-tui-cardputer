//! dsh-lite — a very small DeepSeek "harness" TUI for the Cardputer Zero.
//!
//! The loop is a port of DSH's `core/agent-loop` + `tools` pipeline (see
//! `harness.rs` and `tool.rs`): a turn owns steps, a step is one request plus
//! its tool batch, and every call traverses
//! tool/call -> pre-execute -> execute -> post-execute -> tool/result with
//! `additionalContexts` injected after the batch.
//!
//! No sandbox, no subagents, no persistence: tools run as the invoking user.
//!
//! Zero external crates: JSON is hand-rolled, HTTP goes through `curl`.

mod api;
mod harness;
mod jobs;
mod json;
mod tool;
mod tools;
mod voice;

use api::Cfg;
use harness::{Action, Ev};
use json::J;
use std::io::{Read, Write};

// ------------------------------------------------------------------ terminal

pub struct Scr {
    rows: usize,
    cols: usize,
    lines: Vec<(String, u8)>, // (text, kind)  kind: 0 normal 1 user 2 tool 3 toolres 4 err 5 dim
    scroll: usize,
    input: Vec<char>,
    cursor: usize,
    note: Option<String>,
    /// Persistent right-hand header text (wifi / battery).
    hdr: String,
    /// Messages typed while a turn was running, oldest first.
    queue: Vec<String>,
    /// Ctrl-C/Ctrl-D asked the app to exit.
    quit: bool,
    /// Submit automatically once speech fills the input.
    auto_send: bool,
    /// Incomplete UTF-8 sequence being assembled from keystrokes.
    utf8_buf: Vec<u8>,
    /// Full-screen panel currently taking over the display.
    artifact: Option<Artifact>,
    /// The opening splash has been scrolled away already.
    splash_done: bool,
    /// True when tmux already draws a status line: skip our own header.
    hide_header: bool,
    /// Live line above the prompt: (1 = thinking, 2 = streaming tail, text).
    live: Option<(u8, String)>,
    on_tick: Option<Box<dyn FnMut(&mut Scr)>>,
}

pub const K_NORM: u8 = 0;
pub const K_USER: u8 = 1;
pub const K_TOOL: u8 = 2;
pub const K_RES: u8 = 3;
pub const K_ERR: u8 = 4;
pub const K_DIM: u8 = 5;
pub const K_ART: u8 = 6;
pub const K_CODE: u8 = 7;
pub const K_HEAD: u8 = 8;

/// 320 px / 6 px per cell. The TUI only ever runs on this panel, so the wrap
/// width is fixed here rather than trusted from the pty.
const PANEL_COLS: usize = 53;

/// A full-screen panel composed by the agent (`artifact` tool).
struct Artifact {
    title: String,
    body: Vec<String>,
    scroll: usize,
}

impl Scr {
    fn new() -> Scr {
        let (r, c) = term_size();
        Scr {
            rows: r,
            cols: c,
            lines: Vec::new(),
            scroll: 0,
            input: Vec::new(),
            cursor: 0,
            note: None,
            hdr: String::new(),
            quit: false,
            auto_send: true,
            utf8_buf: Vec::new(),
            artifact: None,
            splash_done: false,
            hide_header: std::env::var("TMUX").is_ok(),
            queue: Vec::new(),
            live: None,
            on_tick: None,
        }
    }

    fn cw(c: char) -> usize {
        let u = c as u32;
        // crude East-Asian wide detection, enough for CJK
        if (0x1100..=0x115F).contains(&u)
            || (0x2E80..=0xA4CF).contains(&u)
            || (0xAC00..=0xD7A3).contains(&u)
            || (0xF900..=0xFAFF).contains(&u)
            || (0xFE30..=0xFE6F).contains(&u)
            || (0xFF00..=0xFF60).contains(&u)
            || (0xFFE0..=0xFFE6).contains(&u)
            || (0x1F300..=0x1FAFF).contains(&u)
        {
            2
        } else {
            1
        }
    }

    /// Wrap to display cells, preferring spaces over mid-word breaks.
    fn wrap_into(out: &mut Vec<String>, text: &str, cols: usize) {
        for raw in text.split('\n') {
            if raw.is_empty() {
                out.push(String::new());
                continue;
            }
            let chars: Vec<char> = raw.chars().collect();
            let mut i = 0usize;
            while i < chars.len() {
                let mut w = 0usize;
                let mut j = i;
                let mut last_space: Option<usize> = None;
                while j < chars.len() {
                    let cw = Self::cw(chars[j]);
                    if w + cw > cols {
                        break;
                    }
                    w += cw;
                    if chars[j] == ' ' {
                        last_space = Some(j + 1);
                    }
                    j += 1;
                }
                if j >= chars.len() {
                    out.push(chars[i..].iter().collect());
                    break;
                }
                // Break at a space when one is reasonably near the edge.
                let end = match last_space {
                    Some(b) if b > i + cols / 2 => b,
                    _ => j.max(i + 1),
                };
                out.push(chars[i..end].iter().collect::<String>().trim_end().to_string());
                i = end;
                while i < chars.len() && chars[i] == ' ' {
                    i += 1;
                }
            }
        }
    }

    /// Render model output: headings, bullets, code blocks, emphasis markers
    /// stripped. The screen is 12 rows, so structure matters more than markup.
    pub fn push_md(&mut self, text: &str) {
        let mut in_code = false;
        for raw in text.split('\n') {
            let trimmed = raw.trim_end();
            let t = trimmed.trim_start();
            if t.starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                self.push(&format!("  {trimmed}"), K_CODE);
                continue;
            }
            if t.is_empty() {
                self.push("", K_NORM);
                continue;
            }
            if let Some(h) = t
                .strip_prefix("### ")
                .or_else(|| t.strip_prefix("## "))
                .or_else(|| t.strip_prefix("# "))
            {
                self.push(&strip_inline(h), K_HEAD);
            } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                self.push(&format!("- {}", strip_inline(b)), K_NORM);
            } else if let Some((num, rest)) = numbered(t) {
                self.push(&format!("{num}. {}", strip_inline(rest)), K_NORM);
            } else {
                self.push(&strip_inline(t), K_NORM);
            }
        }
    }

    pub fn push(&mut self, text: &str, kind: u8) {
        let mut wrapped = Vec::new();
        let shown = tidy(text);
        Self::wrap_into(&mut wrapped, &shown, self.cols.min(PANEL_COLS));
        for w in wrapped {
            self.lines.push((w, kind));
        }
        self.scroll = 0;
        self.draw();
    }

    fn draw(&self) {
        if let Some(a) = &self.artifact {
            self.draw_artifact(a);
            return;
        }
        let mut s = String::with_capacity(4096);
        s.push_str("\x1b[?25l");
        s.push_str("\x1b[H");

        // Layout: [header(1)] + transcript + live(1) + input(1).
        let body_rows = self
            .rows
            .saturating_sub(if self.hide_header { 2 } else { 3 })
            .max(1);

        // ---- header: title left, device state right ----
        // Under tmux the status bar already shows wifi/battery, so drawing it
        // again both duplicated it and cost the transcript a row.
        if !self.hide_header {
            let right = fit(
                &self.note.clone().unwrap_or_else(|| self.hdr.clone()),
                self.cols / 2,
            );
            let right_w = dw(&right);
            let left_w = self.cols.saturating_sub(right_w);
            let mut status = fit(" dst", left_w);
            let pad = left_w.saturating_sub(dw(&status));
            status.push_str(&" ".repeat(pad));
            status.push_str(&right);
            s.push_str("\x1b[90m");
            s.push_str(&status);
            s.push_str("\x1b[0m\x1b[K\r\n");
        }

        // ---- transcript window ----
        let total = self.lines.len();
        let end = total.saturating_sub(self.scroll);
        let start = end.saturating_sub(body_rows);
        for i in 0..body_rows {
            let idx = start + i;
            if idx < end {
                let (txt, kind) = &self.lines[idx];
                match *kind {
                    K_USER => s.push_str("\x1b[1;36m"),
                    K_TOOL => s.push_str("\x1b[0;33m"),
                    K_RES => s.push_str("\x1b[90m"),
                    K_ERR => s.push_str("\x1b[0;31m"),
                    K_DIM => s.push_str("\x1b[90m"),
                    K_ART => s.push_str("\x1b[1;36m"),
                    K_CODE => s.push_str("\x1b[90m"),
                    K_HEAD => s.push_str("\x1b[1;37m"),
                    _ => s.push_str("\x1b[0m"),
                }
                s.push_str(txt);
            }
            s.push_str("\x1b[0m\x1b[K\r\n");
        }

        // ---- live line: thinking / streaming tail / queue depth ----
        // One always-visible status line. Recording state used to be drawn
        // only in the header, which is hidden under fbterm/tmux - so pressing
        // the ASR key looked like it did nothing.
        let mut live_line = String::new();
        let mut hot = false;
        if let Some(n) = &self.note {
            live_line.push_str(&strip_ansi(n));
            hot = true;
        }
        if let Some((_, t)) = &self.live {
            if !live_line.is_empty() {
                live_line.push(' ');
            }
            live_line.push_str(t);
        }
        if live_line.is_empty() && !self.queue.is_empty() {
            live_line = format!("{} queued", self.queue.len());
        }
        // Scrolled back: say so, and how to get back.
        if self.scroll > 0 {
            let hint = format!("\u{2191}{} \u{884c} \u{00b7} End", self.scroll);
            if live_line.is_empty() {
                live_line = hint;
            } else {
                live_line = format!("{live_line} | {hint}");
            }
        }
        if !live_line.is_empty() {
            s.push_str(if hot { "\x1b[31m" } else { "\x1b[90m" });
            s.push_str(&fit(&live_line, self.cols));
            s.push_str("\x1b[0m");
        }
        s.push_str("\x1b[K\r\n");

        // ---- input line ----
        s.push_str("\x1b[1;33m> \x1b[0m");
        let avail = self.cols.saturating_sub(2);
        let cur = self.cursor.min(self.input.len());
        // Scroll window fills backwards from the cursor so it stays visible.
        let mut w = 0usize;
        let mut start_i = cur;
        while start_i > 0 {
            let cw = Self::cw(self.input[start_i - 1]);
            if w + cw > avail {
                break;
            }
            w += cw;
            start_i -= 1;
        }
        let shown: String = self.input[start_i..].iter().collect();
        s.push_str(&shown);
        s.push_str("\x1b[K");

        // Cursor column = width of everything BEFORE the cursor in the window.
        let col = 2 + self.input[start_i..cur]
            .iter()
            .map(|c| Self::cw(*c))
            .sum::<usize>();
        let col = col.min(self.cols.saturating_sub(1));
        s.push_str(&format!("\x1b[{};{}H\x1b[?25h", self.rows, col + 1));

        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    fn draw_artifact(&self, a: &Artifact) {
        let mut s = String::with_capacity(4096);
        s.push_str("\x1b[?25l\x1b[H");
        s.push_str("\x1b[1;37m");
        s.push_str(&fit(&format!(" {}", a.title), self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n");
        let body_rows = self.rows.saturating_sub(2).max(1);
        for i in 0..body_rows {
            if let Some(l) = a.body.get(a.scroll + i) {
                s.push_str(l);
            }
            s.push_str("\x1b[0m\x1b[K\r\n");
        }
        let hidden = a.body.len().saturating_sub(a.scroll + body_rows);
        let footer = if hidden > 0 {
            format!(" Esc 返回 | 还有 {} 行", hidden)
        } else {
            " Esc 返回".to_string()
        };
        s.push_str("\x1b[90m");
        s.push_str(&fit(&footer, self.cols));
        s.push_str("\x1b[0m\x1b[K");
        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    /// Assemble a multibyte UTF-8 character from raw input bytes. Without this
    /// every non-ASCII byte was dropped, so Chinese could not be typed or
    /// pasted into the prompt at all.
    fn utf8_push(&mut self, b: u8) {
        if self.utf8_buf.is_empty() && b < 0x80 {
            self.input.insert(self.cursor, b as char);
            self.cursor += 1;
            return;
        }
        self.utf8_buf.push(b);
        let need = match self.utf8_buf[0] {
            0xf0..=0xf4 => 4,
            0xe0..=0xef => 3,
            0xc0..=0xdf => 2,
            _ => 1,
        };
        if self.utf8_buf.len() >= need {
            if let Ok(s) = std::str::from_utf8(&self.utf8_buf) {
                if let Some(ch) = s.chars().next() {
                    self.input.insert(self.cursor, ch);
                    self.cursor += 1;
                }
            }
            self.utf8_buf.clear();
        }
    }

    fn artifact_scroll(&mut self, delta: isize) {
        let page = self.rows.saturating_sub(2).max(1) as isize;
        if let Some(a) = self.artifact.as_mut() {
            let max = (a.body.len() as isize - page).max(0);
            a.scroll = ((a.scroll as isize + delta).clamp(0, max)) as usize;
        }
        self.draw();
    }

    /// One keystroke while a panel is up: scroll or dismiss.
    fn artifact_keystroke(&mut self) -> bool {
        if self.artifact.is_none() {
            return false;
        }
        let page = self.rows.saturating_sub(2).max(1) as isize;
        match self.read_key_timeout() {
            None => false,
            Some(b) => match b {
                0x0d | 0x0a | b'q' | 0x03 => {
                    self.artifact = None;
                    self.draw();
                    true
                }
                0x1b => match self.read_key_timeout() {
                    Some(b'[') | Some(b'O') => {
                        match self.read_key_timeout() {
                            Some(b'A') => self.artifact_scroll(-1),
                            Some(b'B') => self.artifact_scroll(1),
                            Some(b'5') => {
                                let _ = self.read_key_timeout();
                                self.artifact_scroll(-page);
                            }
                            Some(b'6') => {
                                let _ = self.read_key_timeout();
                                self.artifact_scroll(page);
                            }
                            _ => {}
                        }
                        true
                    }
                    _ => {
                        self.artifact = None;
                        self.draw();
                        true
                    }
                },
                _ => false,
            },
        }
    }

    /// Non-blocking key read. `poll_mode(true)` sets VMIN=0/VTIME=0 so this
    /// returns None immediately instead of waiting for a key.
    fn read_key_now(&self) -> Option<u8> {
        let mut b = [0u8; 1];
        let n = std::io::stdin().read(&mut b).unwrap_or(0);
        if n == 1 {
            Some(b[0])
        } else {
            None
        }
    }

    fn read_key(&self) -> u8 {
        let mut b = [0u8; 1];
        let mut si = std::io::stdin();
        loop {
            match si.read(&mut b) {
                Ok(1) => return b[0],
                Ok(_) => continue,
                Err(_) => return 4,
            }
        }
    }

    fn read_key_timeout(&self) -> Option<u8> {
        let _ = std::process::Command::new("stty")
            .args(["-F", "/dev/tty", "min", "0", "time", "3"]).output();
        let mut b = [0u8; 1];
        let n = std::io::stdin().read(&mut b).unwrap_or(0);
        let _ = std::process::Command::new("stty")
            .args(["-F", "/dev/tty", "min", "1", "time", "0"]).output();
        if n == 1 { Some(b[0]) } else { None }
    }

    /// Wait for the key to be genuinely held. Key-repeat starts after 400 ms,
    /// so three TAB bytes (~490 ms) mean "hold"; one byte then silence is a tap.
    fn arm_tab(&mut self) -> bool {
        let t0 = std::time::Instant::now();
        let mut n = 0;
        loop {
            match self.read_key_timeout() {
                Some(0x09) => {
                    n += 1;
                    if n >= 3 {
                        return true;
                    }
                }
                Some(0x03) | Some(0x1b) => return false,
                Some(_) => {}
                None => return false,
            }
            if t0.elapsed().as_millis() > 900 && n < 3 {
                return false;
            }
        }
    }

    /// Long-press Tab => record, release => transcribe with DeepSeek ASR.
    fn vlog(&self, msg: &str) {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/dst-voice.log")
        {
            let _ = writeln!(f, "{msg}");
        }
    }

    fn voice_input(&mut self) {
        // Open the mic BEFORE the hold is confirmed: the codec takes a moment
        // to settle and starting late clipped the first syllable of speech.
        // The pre-roll is discarded by the arming check below.
        let mut rec = match crate::voice::start_recording() {
            Ok(r) => r,
            Err(e) => {
                self.push(&format!("mic: {e}"), K_ERR);
                return;
            }
        };
        // Debounce: a tap stays a normal Tab. Otherwise ~0.5 s of room noise
        // reaches ASR and comes back as an invented sentence.
        if !self.arm_tab() {
            rec.cancel();
            self.vlog("tap (not armed) - no recording");
            return;
        }
        self.vlog("armed");
        let t0 = std::time::Instant::now();
        self.note = Some("\x1b[31m\u{25cf} REC\x1b[90m 0.0s".into());
        self.draw();
        let mut cancelled = false;
        loop {
            match self.read_key_timeout() {
                Some(0x09) => {
                    self.note = Some(format!(
                        "\x1b[31m\u{25cf} REC\x1b[90m {:.1}s",
                        t0.elapsed().as_secs_f32()
                    ));
                    self.draw();
                    if t0.elapsed().as_secs_f32() > 30.0 {
                        break; // hard cap
                    }
                }
                Some(0x03) | Some(0x1b) => {
                    cancelled = true;
                    break;
                }
                Some(_) => {}
                None => break, // key released
            }
        }
        if cancelled {
            rec.cancel();
            self.note = None;
            self.draw();
            return;
        }
        self.note = Some("\x1b[90m\u{2026} transcribing".into());
        self.draw();
        let level = rec.level();
        let secs = t0.elapsed().as_secs_f32();
        let text = match rec.finish() {
            Ok(t) => t,
            Err(e) => {
                self.vlog(&format!("take {secs:.1}s level={level:.4} error={e}"));
                self.note = None;
                self.draw();
                self.push(&format!("asr: {e}"), K_ERR);
                return;
            }
        };
        self.vlog(&format!(
            "take {secs:.1}s level={level:.4} chars={} text={:?}",
            text.chars().count(),
            text.chars().take(40).collect::<String>()
        ));
        self.note = None;
        // Very short take or a one-character result is noise, not speech.
        if t0.elapsed().as_secs_f32() < 0.6 || text.chars().count() < 2 {
            self.draw();
            return;
        }
        for ch in tidy(&text).chars() {
            self.input.insert(self.cursor, ch);
            self.cursor += 1;
        }
        self.draw();
    }

    /// returns Some(line) when the user submits
    fn edit(&mut self) -> Option<String> {
        loop {
            self.draw();
            if self.artifact.is_some() {
                self.artifact_keystroke();
                continue;
            }
            let b = match self.read_key_timeout() {
                Some(b) => b,
                None => {
                    if let Some(mut f) = self.on_tick.take() {
                        f(self);
                        self.on_tick = Some(f);
                    }
                    continue;
                }
            };
            match b {
                0x03 => return None,                       // Ctrl-C
                0x04 => return None,                       // Ctrl-D
                0x0d | 0x0a => {
                    if self.input.is_empty() {
                        continue;
                    }
                    let line: String = self.input.iter().collect();
                    self.input.clear();
                    self.cursor = 0;
                    return Some(line);
                }
                0x7f | 0x08 => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                        self.input.remove(self.cursor);
                    }
                }
                0x15 => {
                    self.input.clear();
                    self.cursor = 0;
                }
                0x1b => {
                    let b1 = self.read_key();
                    if b1 == b'[' || b1 == b'O' {
                        let b2 = self.read_key();
                        match b2 {
                            b'A' => self.scroll = (self.scroll + 5).min(self.lines.len()),
                            b'B' => self.scroll = self.scroll.saturating_sub(5),
                            b'C' => self.cursor = (self.cursor + 1).min(self.input.len()),
                            b'D' => self.cursor = self.cursor.saturating_sub(1),
                            b'H' => self.cursor = 0,
                            b'F' => self.cursor = self.input.len(),
                            b'5' | b'6' => { let _ = self.read_key(); }
                            _ => {}
                        }
                    }
                }
                0x09 => {
                    self.voice_input();
                    if self.auto_send && !self.input.is_empty() {
                        let line: String = self.input.iter().collect();
                        self.input.clear();
                        self.cursor = 0;
                        return Some(line);
                    }
                }
                0x20..=0x7e => {
                    self.input.insert(self.cursor, b as char);
                    self.cursor += 1;
                }
                0x80..=0xff => self.utf8_push(b),
                _ => {}
            }
        }
    }
}

/// Drop SGR/CSI sequences, for text that carries its own colour codes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if let Some(n) = it.next() {
                if n == '[' {
                    for d in it.by_ref() {
                        if d.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// At a 6x13 cell a full-width `，。、：；` glyph is a lone 2-3 px speck, which
/// reads as noise around Chinese text. Substitute the ASCII forms for display:
/// the stored text is untouched, only what is painted changes.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{ff0c}' | '\u{3001}' => out.push(','),
            '\u{3002}' => out.push('.'),
            '\u{ff1a}' => out.push(':'),
            '\u{ff1b}' => out.push(';'),
            '\u{ff01}' => out.push('!'),
            '\u{ff1f}' => out.push('?'),
            '\u{ff08}' => out.push('('),
            '\u{ff09}' => out.push(')'),
            '\u{3010}' => out.push('['),
            '\u{3011}' => out.push(']'),
            '\u{201c}' | '\u{201d}' => out.push('"'),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{300a}' => out.push('<'),
            '\u{300b}' => out.push('>'),
            '\u{ff0d}' => out.push('-'),
            '\u{ff5e}' => out.push('~'),
            '\u{3000}' => out.push(' '),
            '\u{2026}' => out.push_str("..."),
            '\u{00b7}' => out.push('|'),
            _ => out.push(c),
        }
    }
    out
}

/// `1. text` -> ("1", "text")
fn numbered(t: &str) -> Option<(String, &str)> {
    let mut digits = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() && digits.len() < 3 {
            digits.push(c);
        } else if c == '.' && !digits.is_empty() {
            let rest = &t[digits.len() + 1..];
            if let Some(r) = rest.strip_prefix(' ') {
                return Some((digits, r));
            }
            return None;
        } else {
            return None;
        }
    }
    None
}

/// Drop inline markup the 6x13 font cannot render: **bold**, `code`,
/// [text](url) and stray emphasis markers.
fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' => { /* drop emphasis markers */ }
            '[' => {
                // [text](url) -> text (url)
                let mut label = String::new();
                let mut depth = 0;
                let mut closed = false;
                for d in chars.by_ref() {
                    if d == '[' {
                        depth += 1;
                    }
                    if d == ']' {
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                        depth -= 1;
                    }
                    label.push(d);
                }
                if closed && chars.peek() == Some(&'(') {
                    chars.next();
                    let mut url = String::new();
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                        url.push(d);
                    }
                    out.push_str(&label);
                    out.push_str(" (");
                    out.push_str(&url);
                    out.push(')');
                } else {
                    out.push('[');
                    out.push_str(&label);
                    if closed {
                        out.push(']');
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Display width in terminal cells (CJK counts as two).
fn dw(s: &str) -> usize {
    s.chars().map(Scr::cw).sum()
}

fn fit(s: &str, cols: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = Scr::cw(ch);
        if w + cw > cols {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

fn term_size() -> (usize, usize) {
    // Probe the pty this process actually owns first (plain `stty size` reads
    // stdin), then /dev/tty. Whatever comes back is clamped to the panel:
    // 320x170 / 6x13 = 53 columns x 13 rows. Sizing to a larger default is
    // what made long lines run past the edge and soft-wrap.
    let probes: [&[&str]; 2] = [&["size"], &["-F", "/dev/tty", "size"]];
    for probe in probes {
        if let Ok(o) = std::process::Command::new("stty").args(probe).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            let mut it = s.split_whitespace();
            if let (Some(r), Some(c)) = (it.next(), it.next()) {
                if let (Ok(rows), Ok(cols)) = (r.parse::<usize>(), c.parse::<usize>()) {
                    if rows > 0 && cols > 0 {
                        return (rows.min(13), cols.min(53));
                    }
                }
            }
        }
    }
    (12, 53)
}

fn raw_mode(on: bool) {
    let arg = if on { "raw" } else { "-raw" };
    let _ = std::process::Command::new("stty")
        .args(["-F", "/dev/tty", arg, "-echo"])
        .output();
    if !on {
        let _ = std::process::Command::new("stty")
            .args(["-F", "/dev/tty", "sane"])
            .output();
    }
}

// ------------------------------------------------------------------ config

fn read_key_file() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/hanxiao".into());
    let p = format!("{home}/.config/dsh-lite/api_key");
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_cfg() -> Cfg {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/hanxiao".into());
    let mut base = "https://api.deepseek.com/v1".to_string();
    let mut model = "deepseek-flash".to_string();
    let mut max_tokens = 4096u32;
    let p = format!("{home}/.config/dsh-lite/config.json");
    if let Ok(s) = std::fs::read_to_string(p) {
        if let Some(v) = json::parse(&s) {
            if let Some(b) = v.get("base_url").and_then(|x| x.as_str()) {
                base = b.to_string();
            }
            if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
                model = m.to_string();
            }
            if let Some(m) = v.get("max_tokens").and_then(|x| x.as_num()) {
                max_tokens = m as u32;
            }
        }
    }
    Cfg {
        base,
        key: read_key_file().unwrap_or_default(),
        model,
        max_tokens,
    }
}

const SYSTEM_PROMPT: &str = "\
You are dst, a DeepSeek harness running on a handheld Cardputer Zero.

DEVICE
- Raspberry Pi CM0: 4x Cortex-A53 at ~1 GHz, 512 MB RAM, microSD, battery.
- The CPU is slow and RAM is small: prefer ONE command that gathers several
  facts over many round trips. No builds, no large downloads.
- Tool calls use standard function calling: you emit tool_calls, the harness
  returns one tool message per call carrying its tool_call_id.

SCREEN - this shapes every reply
- 53 columns x 13 rows. The chat log shows about 9 lines; one row is the status
  line and one is the input line.
- A reply must be readable without scrolling: aim for <= 8 lines, each under
  53 characters. Short lines beat long paragraphs.
- When an answer genuinely needs more room (a table, code, a diagram, a long
  comparison, a checklist), call `artifact`: it replaces the whole screen with
  a panel you compose, and the user dismisses it with Esc. Put the finished
  result in it, not a progress note. Do not use it for a one-line answer.
- No emoji, no wide tables, no horizontal rules: the font is 6x13 cells.

VOICE
- `speak` is text-to-speech. Use it ONLY when the user explicitly asks to be
- `speak` is text-to-speech. Use it ONLY when the user explicitly asks to be
  told aloud (e.g. \"read it to me\", \"念给我听\"). Never use it for progress,
  confirmations or status: the screen is the channel.
- The user talks to you by long-pressing Tab; the transcript is submitted
  automatically, so treat messages as spoken and reply conversationally.

RULES
- Never claim a file changed or a command ran unless a tool result shows it.
- ALWAYS reply in the language the user writes in. A Chinese question\n  gets a Chinese answer, including inside artifacts.
- Show the result rather than describing the process.";

/// `--once`: the same driver, printing events instead of drawing them.
struct OnceUi;

impl harness::Ui for OnceUi {
    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::Delta(d) => {
                print!("{d}");
                let _ = std::io::stdout().flush();
            }
            Ev::ThinkingDone { secs, chars } => {
                eprintln!("[thinking {secs:.1}s, {chars} chars]");
            }
            Ev::Assistant(_) => println!(),
            Ev::ToolCall { name, title, .. } => println!("[call] {name}: {title}"),
            Ev::ToolResult {
                name,
                content,
                is_error,
                ..
            } => {
                let tag = if is_error { "error" } else { "result" };
                println!("[{tag}:{name}] {}", first_lines(&content, 6));
            }
            Ev::Context(c) => println!("[context] {c}"),
            Ev::Note(n) => eprintln!("[note] {n}"),
            _ => {}
        }
    }
    fn pump(&mut self) -> Action {
        Action::Continue
    }
}

fn run_once(cfg: &Cfg, reg: &[tool::ToolDef], prompt: &str) {
    let mut msgs = vec![harness::msg("system", SYSTEM_PROMPT)];
    let mut ui = OnceUi;
    harness::run_turn(cfg, &mut msgs, reg, prompt, &mut ui);
}

// ---------------------------------------------------------------- interactive

/// Front end for the terminal. Draws harness events, and keeps the keyboard
/// live while the model works: typed text is echoed, Enter queues the message
/// for after this turn, Esc cancels.
struct TermUi<'a> {
    scr: &'a mut Scr,
    acc: String,
    tools: std::collections::HashMap<String, (String, String, std::time::Instant)>,
    spin: usize,
}

impl<'a> harness::Ui for TermUi<'a> {
    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::Delta(d) => {
                self.acc.push_str(&d);
                self.scr.live = Some((2, tail_chars(&self.acc, 46)));
                self.scr.draw();
            }
            Ev::Thinking { secs, chars } => {
                const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
                let f = FRAMES[self.spin % FRAMES.len()];
                self.spin += 1;
                let text = if chars > 0 {
                    format!("{f} thinking {secs:.1}s")
                } else {
                    format!("{f} working {secs:.1}s")
                };
                self.scr.live = Some((1, text));
                self.scr.draw();
            }
            Ev::ThinkingDone { secs, chars } => {
                self.scr.live = None;
                self.scr.push(
                    &format!("\u{25b8} thinking {secs:.1}s | {chars} chars (collapsed)"),
                    K_DIM,
                );
            }
            Ev::Assistant(t) => {
                self.acc.clear();
                self.scr.live = None;
                // Results are the payload of the turn: render them as
                // structure (headings, bullets, code) rather than raw text.
                self.scr.push_md(&t);
            }
            Ev::ToolCall { id, name, title } => {
                self.scr.live = None;
                self.tools.insert(id, (name, title, std::time::Instant::now()));
            }
            Ev::ToolResult {
                id,
                content,
                is_error,
                ..
            } => {
                // Calls are all announced before any of them settles, so the
                // line is built from the id-keyed record, not a single slot.
                let (name, title, took) = self
                    .tools
                    .remove(&id)
                    .map(|(n, t, started)| (n, t, started.elapsed().as_secs_f32()))
                    .unwrap_or_else(|| (String::new(), String::new(), 0.0));
                // Always one row: the tool line must never wrap, whatever the
                // command was, or it steals space from the answer.
                let cols = self.scr.cols;
                // Minimal: which tool, how long. The command text is noise on a 53-wide
                // screen and the answer already says what was found.
                let line = format!("* {name} > {title} | {took:.1}s");
                self.scr.push(&fit(&line, cols.saturating_sub(1)), K_TOOL);
                if is_error {
                    self.scr
                        .push(&format!("  \u{21b3} {}", first_lines(&content, 4)), K_ERR);
                }
            }
            Ev::Artifact { title, body } => {
                self.acc.clear();
                self.scr.live = None;
                let cols = self.scr.cols;
                let mut lines: Vec<String> = Vec::new();
                for l in body.lines() {
                    let mut w: Vec<String> = Vec::new();
                    Scr::wrap_into(&mut w, l, cols);
                    lines.extend(w);
                }
                self.scr.artifact = Some(Artifact {
                    title: tidy(&title),
                    body: lines,
                    scroll: 0,
                });
                self.scr.draw();
            }
            Ev::Context(c) => self.scr.push(&format!("\u{ff0b} {c}"), K_DIM),
            Ev::Note(n) => self.scr.push(&n, K_ERR),
            _ => {}
        }
    }

    fn pump(&mut self) -> Action {
        if self.scr.artifact.is_some() {
            self.scr.artifact_keystroke();
            poll_mode(true); // read_key_timeout left the tty blocking
            return Action::Continue;
        }
        let mut changed = false;
        loop {
            let k = match self.scr.read_key_now() {
                Some(k) => k,
                None => break,
            };
            match k {
                0x03 | 0x04 => {
                    self.scr.quit = true;
                    return Action::Cancel;
                }
                0x1b => {
                    // A bare Esc cancels; Esc + '[' / 'O' is an arrow key.
                    match self.scr.read_key_timeout() {
                        Some(b) if b == b'[' || b == b'O' => {
                            let _ = self.scr.read_key_timeout();
                            poll_mode(true); // read_key_timeout left the tty blocking
                        }
                        _ => return Action::Cancel,
                    }
                }
                0x0d | 0x0a => {
                    let line: String = self.scr.input.iter().collect();
                    if !line.trim().is_empty() {
                        self.scr.input.clear();
                        self.scr.cursor = 0;
                        self.scr.queue.push(line.clone());
                        let n = self.scr.queue.len();
                        self.scr.push(&format!("\u{23ce} queued ({n}): {line}"), K_DIM);
                    }
                    changed = true;
                }
                0x7f | 0x08 => {
                    if self.scr.cursor > 0 {
                        self.scr.cursor -= 1;
                        self.scr.input.remove(self.scr.cursor);
                        changed = true;
                    }
                }
                0x15 => {
                    self.scr.input.clear();
                    self.scr.cursor = 0;
                    changed = true;
                }
                0x20..=0x7e => {
                    self.scr.input.insert(self.scr.cursor, k as char);
                    self.scr.cursor += 1;
                    changed = true;
                }
                0x80..=0xff => {
                    self.scr.utf8_push(k);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.scr.draw();
        }
        Action::Continue
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    let v: Vec<char> = s.chars().collect();
    let start = v.len().saturating_sub(n);
    v[start..].iter().collect()
}

/// Switch the tty to non-blocking key reads for the duration of a turn.
fn poll_mode(on: bool) {
    let _ = std::process::Command::new("stty")
        .args([
            "-F",
            "/dev/tty",
            "min",
            if on { "0" } else { "1" },
            "time",
            "0",
        ])
        .output();
}

// ------------------------------------------------------------------ splash

/// Opening splash: the DeepSeek pixel whale from dsh-TUI `whaleFrames.ts`
/// (40x25 sprite, standard frame), tone-mapped to block glyphs so it reads on
/// an 8-colour framebuffer terminal. 34x8 cells, fits above the prompt.
const SPLASH: &[&str] = &[
    "      ████████████      ██▓▓▓▓▓▓▓█",
    "    ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███",
    "   ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█",
    "   █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓██",
    "   █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓██",
    "   ██▓░           ▓█▓▓▓▓██▓██",
    "     ███░░░      ░░██▓▓▓▓███",
    "        █████████████████████",
];


/// Wifi + battery for the header. Read directly so a tmux-formatted status
/// script cannot leak markup like ``#[fg=green]`` into the line.
fn device_status() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(o) = std::process::Command::new("sh")
        .arg("-c")
        .arg("ip -4 -o addr show scope global 2>/dev/null | awk '{print $2, $4}' | head -1")
        .output()
    {
        let t = String::from_utf8_lossy(&o.stdout);
        let t = t.trim();
        if !t.is_empty() {
            parts.push(t.replace("/24", "").replace("/16", ""));
        }
    }
    if let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") {
        for e in rd.flatten() {
            let d = e.path();
            if let Ok(v) = std::fs::read_to_string(d.join("voltage_now")) {
                if let Ok(uv) = v.trim().parse::<f64>() {
                    let cur = std::fs::read_to_string(d.join("current_now"))
                        .ok()
                        .and_then(|c| c.trim().parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let mut s = format!("{:.2}V", uv / 1e6);
                    if cur != 0.0 {
                        s.push_str(&format!(" {:+.2}A", cur / 1e6));
                    }
                    parts.push(s);
                    break;
                }
            }
        }
    }
    parts.join(" | ")
}

/// Only one `dst` may own a terminal. Two instances both reading /dev/tty is
/// what makes keys appear dead or go to the wrong window.
fn single_instance() -> bool {
    let dir = "/tmp/dst.lock";
    let pidfile = format!("{dir}/pid");
    if let Ok(s) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            let proc_dir = format!("/proc/{pid}");
            if std::path::Path::new(&proc_dir).exists() {
                let comm = std::fs::read_to_string(format!("{proc_dir}/comm")).unwrap_or_default();
                if comm.trim() == "dst" {
                    return false;
                }
            }
        }
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&pidfile, std::process::id().to_string());
    true
}

fn main() {
    // A panic must not leave the panel full of half-drawn glyphs and escape
    // sequences: restore the terminal first, then report.
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write as _;
        let _ = std::process::Command::new("stty")
            .args(["-F", "/dev/tty", "sane", "min", "1", "time", "0"])
            .output();
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[0m\x1b[?25h\x1b[2J\x1b[H");
        let _ = o.flush();
        eprintln!("dst: internal error: {info}");
    }));

    if !single_instance() {
        eprintln!("dst: another instance is already running on this terminal");
        std::process::exit(1);
    }

    let cfg = read_cfg();
    if cfg.key.is_empty() {
        eprintln!("dsh-lite: no API key at ~/.config/dsh-lite/api_key");
        std::process::exit(1);
    }

    tools::set_cfg(cfg.clone());
    voice::set_cfg(cfg.clone());
    let reg = tools::registry();
    let tdefs = tool::definitions(&reg);

    let argv: Vec<String> = std::env::args().collect();
    if argv.len() >= 2 && argv[1] == "--voice-selftest" {
        voice::set_cfg(cfg.clone());
        let secs = argv.get(2).and_then(|s| s.parse().ok()).unwrap_or(5u32);
        match voice::mic_probe(secs) {
            Ok((rms, text)) => println!("rms={rms:.4}\ntranscript={text:?}"),
            Err(e) => println!("error: {e}"),
        }
        return;
    }
    if argv.len() >= 3 && argv[1] == "--once" {
        run_once(&cfg, &reg, &argv[2]);
        return;
    }

    let mut scr = Scr::new();
    raw_mode(true);

    for line in SPLASH {
        scr.push(line, K_ART);
    }
    scr.push("  dst \u{b7} a DeepSeek harness on a Cardputer Zero", K_DIM);
    scr.push(&format!("  {} | Tab \u{957f}\u{6309}\u{5f55}\u{97f3} | /quit", cfg.model), K_DIM);
    scr.hdr = device_status();
    let msgs: jobs::Msgs = std::rc::Rc::new(std::cell::RefCell::new(vec![harness::msg("system", SYSTEM_PROMPT)]));
    {
        let cfg2 = cfg.clone();
        let tdefs2 = tdefs.clone();
        let msgs2 = msgs.clone();
        scr.on_tick = Some(Box::new(move |s: &mut Scr| jobs::poll(&cfg2, &tdefs2, &msgs2, s)));
    }

    loop {
        let line = match scr.edit() {
            Some(l) => l,
            None => break,
        };
        if line == "/quit" || line == "/exit" {
            break;
        }
        let mut next = Some(line);
        while let Some(submit) = next {
            // The splash is for the idle screen only: once the user starts
            // working, give the whole transcript area to the conversation.
            if !scr.splash_done {
                scr.lines.clear();
                scr.scroll = 0;
                scr.splash_done = true;
            }
            scr.push(&format!("> {submit}"), K_USER);
            poll_mode(true);
            {
                let mut m = msgs.borrow_mut();
                let mut ui = TermUi {
                    scr: &mut scr,
                    acc: String::new(),
                    tools: std::collections::HashMap::new(),
                    spin: 0,
                };
                harness::run_turn(&cfg, &mut m, &reg, &submit, &mut ui);
            }
            poll_mode(false);
            scr.live = None;
            scr.draw();
            if scr.quit {
                break;
            }
            next = if scr.queue.is_empty() {
                None
            } else {
                Some(scr.queue.remove(0))
            };
        }
        if scr.quit {
            break;
        }
    }

    raw_mode(false);
    let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[2J\x1b[H");
    println!("bye");
}

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out.replace('\n', " ")
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines()
        .take(n)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
