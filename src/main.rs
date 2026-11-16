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
mod art;
mod cmd;
mod compact;
mod config;
mod context;
mod goal;
mod harness;
mod i18n;
mod jobs;
mod login;
mod mcp;
mod skills;
mod usage;
mod json;
mod tool;
mod tools;
mod voice;

use api::Cfg;
use harness::{Action, Ev};
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
    /// A turn is running (drives the busy hint).
    busy: bool,
    /// Tab was pressed while busy: start recording once the turn ends.
    voice_pending: bool,
    /// Full-screen QR overlay (login).
    qr: Option<Vec<String>>,
    /// Whether speech (ASR/TTS) is available for the current endpoint.
    speech_ok: bool,
    /// Live reasoning lines (folded into one summary line when content starts).
    think_lines: Vec<String>,
    think_active: bool,
    /// Animated splash: current frame and whether the animation is running.
    art_frame: usize,
    art_on: bool,
    /// menuconfig-style configuration screen.
    menu: Option<Menu>,
    /// Selected row in the `/` command palette.
    palette_sel: usize,
    /// True once the QR has been blitted to the framebuffer: further terminal
    /// repaints would paint over it, so draw() becomes a no-op.
    qr_blit: bool,
    /// Context window size of the current model, for the ring widget.
    ctx_window: u64,
    /// `base|model` the cached ctx_window belongs to (avoids a /models probe
    /// on every turn just to keep the ring correct).
    ctx_for: String,
    /// Hide typed characters (API key entry).
    mask: bool,
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

/// Retained transcript lines. The device has 512 MB and a turn can emit
/// hundreds of tool result lines; an unbounded `Vec` was the largest avoidable
/// allocation in a long session. Old lines are dropped from the front, which
/// keeps scrolling-into-the-past bounded but still useful.
const MAX_TRANSCRIPT_LINES: usize = 400;

#[derive(Clone)]
struct MenuItem {
    label: String,
    value: String,
}

struct Menu {
    title: String,
    items: Vec<MenuItem>,
    sel: usize,
    hint: String,
}

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
            busy: false,
            voice_pending: false,
            speech_ok: true,
            think_lines: Vec::new(),
            think_active: false,
            art_frame: 0,
            art_on: false,
            menu: None,
            palette_sel: 0,
            qr: None,
            qr_blit: false,
            ctx_window: 0,
            ctx_for: String::new(),
            mask: false,
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

    /// Draw frame 0 of the animated splash plus the hint line. Used at startup
    /// and whenever `/clear` resets the screen.
    pub fn seed_splash(&mut self, model: &str) {
        self.lines.clear();
        self.scroll = 0;
        self.splash_done = false;
        self.art_frame = 0;
        // A new screen must not inherit transient state: leaving the thinking
        // panel or the live line behind is what made `/clear` look half-done.
        self.think_lines.clear();
        self.think_active = false;
        self.live = None;
        self.note = None;
        self.artifact = None;
        for l in art_frame_lines(0, self.cols) {
            self.lines.push((l, K_ART));
        }
        self.lines.push((format!("  {}", i18n::tf("hints", &[model])), K_DIM));
        self.art_on = true;
        self.draw();
    }

    /// Advance the splash animation while the user has not started working.
    fn anim_tick(&mut self) {
        if !self.art_on
            || !self.input.is_empty()
            || self.scroll != 0
            || self.lines.len() > art::ART_ROWS + 1
        {
            return;
        }
        self.art_frame = (self.art_frame + 1) % art::FRAMES.len();
        for (i, l) in art_frame_lines(self.art_frame, self.cols).into_iter().enumerate() {
            self.lines[i] = (l, K_ART);
        }
        self.draw();
    }

    pub fn push(&mut self, text: &str, kind: u8) {
        let mut wrapped = Vec::new();
        let shown = tidy(text);
        Self::wrap_into(&mut wrapped, &shown, self.cols.min(PANEL_COLS));
        for w in wrapped {
            self.lines.push((w, kind));
        }
        // Keep the transcript bounded. Scrollback is measured from the end, so
        // dropping the oldest rows does not disturb the current view.
        if self.lines.len() > MAX_TRANSCRIPT_LINES {
            let drop = self.lines.len() - MAX_TRANSCRIPT_LINES;
            self.lines.drain(0..drop);
        }
        self.scroll = 0;
        self.draw();
    }

    /// Palette rows for the current input (empty when not composing a command).
    fn palette_items(&self) -> Vec<(&'static str, String, Option<&'static str>)> {
        let text: String = self.input.iter().collect();
        cmd::palette(&text)
    }

    fn draw(&self) {
        if let Some(m) = &self.menu {
            self.draw_menu(m);
            return;
        }
        if let Some(lines) = &self.qr {
            if self.qr_blit {
                return;
            }
            let mut s = String::with_capacity(4096);
            s.push_str("\x1b[?25l\x1b[H");
            for i in 0..self.rows {
                if let Some(l) = lines.get(i) {
                    s.push_str(l);
                }
                s.push_str("\x1b[0m\x1b[K\r\n");
            }
            let mut out = std::io::stdout();
            let _ = out.write_all(s.as_bytes());
            let _ = out.flush();
            return;
        }
        if let Some(a) = &self.artifact {
            self.draw_artifact(a);
            return;
        }
        let mut s = String::with_capacity(4096);
        s.push_str("\x1b[?25l");
        s.push_str("\x1b[H");

        // Layout: [header(1)] + transcript + palette + live(1) + input(1).
        let pal = self.palette_items();
        let pal_rows = pal.len().min(4);
        let think_rows = if self.think_active {
            self.think_lines.len().min(4)
        } else {
            0
        };
        let body_rows = self
            .rows
            .saturating_sub(if self.hide_header { 2 } else { 3 })
            .max(1)
            .saturating_sub(pal_rows)
            .saturating_sub(think_rows);

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

        // ---- live reasoning (folded once the answer begins) ----
        if think_rows > 0 {
            for line in self.think_lines.iter().skip(self.think_lines.len() - think_rows) {
                s.push_str("\x1b[35m");
                s.push_str(&fit(line, self.cols));
                s.push_str("\x1b[0m\x1b[K\r\n");
            }
        }

        // ---- command palette (only while composing a `/` command) ----
        let pal_start = if self.palette_sel >= pal_rows && pal_rows > 0 {
            self.palette_sel + 1 - pal_rows
        } else {
            0
        };
        for (vis, (name, desc, hint)) in pal.iter().skip(pal_start).take(pal_rows).enumerate() {
            let i = pal_start + vis;
            let selected = i == self.palette_sel;
            let marker = if selected { "\u{25b8}" } else { " " };
            let hint = hint.map(|h| format!(" {h}")).unwrap_or_default();
            let head = format!("{marker} /{name}{hint}");
            let pad = dw(&head);
            let room = self.cols.saturating_sub(pad + 3);
            let d: String = desc.chars().take(room).collect();
            if selected {
                s.push_str("\x1b[1;36m");
            } else {
                s.push_str("\x1b[90m");
            }
            s.push_str(&fit(&format!("{head}   {d}"), self.cols));
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
            live_line = i18n::tf("queued", &[&self.queue.len().to_string()]);
        }
        // An active goal is ambient state: show it when nothing more urgent is
        // happening, so autonomous rounds are not invisible.
        if live_line.is_empty() {
            if let Some(b) = crate::goal::banner() {
                live_line = b;
            }
        }
        // Scrolled back: say so, and how to get back.
        if self.busy && live_line.is_empty() {
            live_line = i18n::t("busy").to_string();
        }
        if self.scroll > 0 {
            let hint = i18n::tf("scrolled", &[&self.scroll.to_string()]);
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

        // ---- input line: prompt + text, with the context ring pinned right ----
        let ring = if self.ctx_window > 0 {
            let pct = crate::usage::context_pct(self.ctx_window);
            let label = if pct > 0.0 && pct < 0.10 {
                format!("{:.1}%", pct * 100.0)
            } else {
                format!("{:.0}%", pct * 100.0)
            };
            format!("{} {}", crate::usage::ring(pct), label)
        } else {
            String::new()
        };
        let ring_w = dw(&ring);
        s.push_str("\x1b[1;33m> \x1b[0m");
        let avail = self.cols.saturating_sub(2 + if ring_w > 0 { ring_w + 1 } else { 0 });
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
        if self.mask {
            s.push_str(&"*".repeat(shown.chars().count()));
        } else {
            s.push_str(&shown);
        }
        s.push_str("\x1b[K");

        // Cursor column = width of everything BEFORE the cursor in the window.
        // Right-aligned ring on the same row as the input.
        if ring_w > 0 {
            let rcol = self.cols.saturating_sub(ring_w) + 1;
            s.push_str(&format!("\x1b[{};{}H\x1b[90m{}\x1b[0m", self.rows, rcol, ring));
        }
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

    fn draw_menu(&self, m: &Menu) {
        let mut s = String::with_capacity(2048);
        // Full-screen panel: clear so no transcript shows through.
        s.push_str("\x1b[?25l\x1b[2J\x1b[H");
        s.push_str("\x1b[1;36m");
        s.push_str(&fit(&format!(" {} ", m.title), self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n");
        let rows = self.rows.saturating_sub(2).max(1);
        // Window the list around the selection (menuconfig behaviour).
        let start = if m.sel >= rows { m.sel + 1 - rows } else { 0 };
        for i in 0..rows {
            let idx = start + i;
            if let Some(item) = m.items.get(idx) {
                let selected = idx == m.sel;
                let mark = if selected { "\u{25b8}" } else { " " };
                let left = format!("{mark} {}", item.label);
                let room = self.cols.saturating_sub(dw(&left) + dw(&item.value) + 1);
                let dots = " ".repeat(room.min(20));
                let line = format!("{left}{dots} {}", item.value);
                if selected {
                    s.push_str("\x1b[7m");
                } else {
                    s.push_str("\x1b[0;37m");
                }
                s.push_str(&fit(&line, self.cols));
                s.push_str("\x1b[0m\x1b[K\r\n");
            } else {
                s.push_str("\x1b[K\r\n");
            }
        }
        s.push_str("\x1b[90m");
        s.push_str(&fit(&m.hint, self.cols));
        s.push_str("\x1b[0m\x1b[K");
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
            .open(config::log_dir().join("voice.log"))
        {
            let _ = writeln!(f, "{msg}");
        }
    }

    fn voice_input(&mut self) {
        if !self.speech_ok {
            self.push(
                "\u{8bed}\u{97f3}\u{4ec5}\u{652f}\u{6301} DeepSeek \u{5b98}\u{65b9} API\u{ff08}\u{7b2c}\u{4e09}\u{65b9} endpoint \u{65e0} ASR/TTS\u{ff09}",
                K_ERR,
            );
            return;
        }
        // Open the mic BEFORE the hold is confirmed: the codec takes a moment
        // to settle and starting late clipped the first syllable of speech.
        // The pre-roll is discarded by the arming check below.
        let rec = match crate::voice::start_recording() {
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
            if self.qr.is_some() {
                match self.read_key_timeout() {
                    Some(0x1b) | Some(0x0d) | Some(0x0a) | Some(0x03) | Some(b'q') => {
                        close_qr(self);
                    }
                    _ => {}
                }
                continue;
            }
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
                    self.anim_tick();
                    continue;
                }
            };
            match b {
                0x03 => return None,                       // Ctrl-C
                0x04 => return None,                       // Ctrl-D
                0x0d | 0x0a => {
                    // Enter on an open palette: run a no-argument command, or
                    // complete a command that expects input.
                    let pal = self.palette_items();
                    if !pal.is_empty() {
                        let i = self.palette_sel.min(pal.len() - 1);
                        let (name, _desc, hint) = pal[i].clone();
                        let typed: String = self.input.iter().collect();
                        let typed = typed.trim_end();
                        // A hint in [] is optional, <..> is required: only the
                        // latter completes with a space instead of running.
                        let needs_arg = hint.map(|h| h.starts_with('<')).unwrap_or(false);
                        let exact = typed == format!("/{name}");
                        if !needs_arg || exact {
                            let cmd = format!("/{name}");
                            self.input.clear();
                            self.cursor = 0;
                            return Some(cmd);
                        }
                        self.input.clear();
                        self.cursor = 0;
                        for ch in format!("/{name} ").chars() {
                            self.input.push(ch);
                        }
                        self.cursor = self.input.len();
                        continue;
                    }
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
                    // A bare Esc is a real key here: it stops a clip that is
                    // still playing after its turn was cancelled. Waiting for
                    // a second byte (the old `read_key`) blocked the editor
                    // until some other key was pressed.
                    let b1 = match self.read_key_timeout() {
                        Some(b) => b,
                        None => {
                            crate::voice::request_stop();
                            continue;
                        }
                    };
                    if b1 == b'\r' {
                        // Alt+Enter / Shift+Enter fallback.
                        if !self.input.is_empty() {
                            let line: String = self.input.iter().collect();
                            self.input.clear();
                            self.cursor = 0;
                            return Some(line);
                        }
                        continue;
                    }
                    if b1 == b'[' || b1 == b'O' {
                        let b2 = self.read_key();
                        if b2.is_ascii_digit() {
                            // Extended key: ESC [ 1 3 ; 2 u is Shift+Enter.
                            let mut seq = String::new();
                            seq.push(b2 as char);
                            loop {
                                let c = self.read_key();
                                seq.push(c as char);
                                if c == b'u' || c == b'~' || c.is_ascii_alphabetic() {
                                    break;
                                }
                                if seq.len() > 8 {
                                    break;
                                }
                            }
                            if seq.starts_with("13;") && !self.input.is_empty() {
                                let line: String = self.input.iter().collect();
                                self.input.clear();
                                self.cursor = 0;
                                return Some(line);
                            }
                            continue;
                        }
                        match b2 {
                            b'A' => {
                                let n = self.palette_items().len();
                                if n > 0 {
                                    self.palette_sel = self.palette_sel.saturating_sub(1);
                                } else {
                                    self.scroll = (self.scroll + 5).min(self.lines.len());
                                }
                            }
                            b'B' => {
                                let n = self.palette_items().len();
                                if n > 0 {
                                    self.palette_sel = (self.palette_sel + 1).min(n - 1);
                                } else {
                                    self.scroll = self.scroll.saturating_sub(5);
                                }
                            }
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
                    self.palette_sel = 0;
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

/// Project the persistent config onto the per-turn API client config.
fn api_cfg(c: &config::Config) -> Cfg {
    Cfg {
        base: c.base.clone(),
        key: c.api_key.clone(),
        model: c.model.clone(),
        max_tokens: c.max_tokens,
        effort: c.effort.clone(),
    }
}

/// Refresh the context-window figure only when the endpoint or model changed.
fn refresh_ctx_window(scr: &mut Scr, cfg: &Cfg) {
    let key = format!("{}|{}", cfg.base, cfg.model);
    if key == scr.ctx_for {
        return;
    }
    scr.ctx_window = crate::api::context_window(cfg).unwrap_or(0);
    scr.ctx_for = key;
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
  told aloud (e.g. \"read it to me\", \"念给我听\"). Never use it for progress,
  confirmations or status: the screen is the channel.
- Keep every spoken message under 150 words (a CJK character counts as one
  word). Anything longer is rejected by the tool, so summarise first.
- The user talks to you by long-pressing Tab; the transcript is submitted
  automatically, so treat messages as spoken and reply conversationally.

STYLE
- Be concise. Answer in the fewest lines that fully answer: no preamble, no
  restating the question, no narration of steps. Aim for <= 4 short lines
  unless the user asks for detail.
- When the user interrupts or cancels, stop immediately; do not retry or repeat
  the interrupted work unless asked.

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
            Ev::Compacting { tokens, messages } => {
                eprintln!("[compacting {messages} messages, {} tokens]", crate::compact::human_tokens(tokens));
            }
            Ev::CompactingDone { ok, note } => eprintln!("[compact {}] {note}", if ok { "ok" } else { "failed" }),
            Ev::Info(n) => eprintln!("[info] {n}"),
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
    harness::run_turn(cfg, &harness::RunOpts::default(), &mut msgs, reg, prompt, &mut ui);
}

// ---------------------------------------------------------------- interactive

/// Front end for the terminal. Draws harness events, and keeps the keyboard
/// live while the model works: typed text is echoed, Enter queues the message
/// for after this turn, Esc cancels.
struct TermUi<'a> {
    scr: &'a mut Scr,
    acc: String,
    /// Incremental reasoning line buffer: reasoning is wrapped as it arrives
    /// instead of re-wrapping the whole accumulated text on every chunk (that
    /// was quadratic and made the panel lag behind the stream).
    think_cur: String,
    think_wrapped: Vec<String>,
    tools: std::collections::HashMap<String, (String, String, std::time::Instant)>,
    spin: usize,
}

impl<'a> TermUi<'a> {
    fn reset_think(&mut self) {
        self.think_cur.clear();
        self.think_wrapped.clear();
        self.scr.think_active = false;
        self.scr.think_lines.clear();
    }

    /// Move the line currently being wrapped into the finished-line buffer.
    fn think_flush(&mut self) {
        let line = self.think_cur.trim_end().to_string();
        self.think_cur.clear();
        if !line.trim().is_empty() {
            self.think_wrapped.push(line);
            if self.think_wrapped.len() > 8 {
                let drop = self.think_wrapped.len() - 8;
                self.think_wrapped.drain(0..drop);
            }
        }
    }

    /// Rebuild the visible tail from finished lines plus the line in progress.
    /// Chunks are *not* line breaks: committing on every delta was what made
    /// the reasoning panel show one short line per stream chunk.
    fn think_refresh(&mut self) {
        let mut shown: Vec<String> = self
            .think_wrapped
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|l| format!("  {l}"))
            .collect();
        if !self.think_cur.trim().is_empty() {
            shown.push(format!("  {}", self.think_cur.trim_end()));
        }
        if shown.len() > 4 {
            let drop = shown.len() - 4;
            shown.drain(0..drop);
        }
        self.scr.think_lines = shown;
        self.scr.think_active = !self.scr.think_lines.is_empty();
    }
}

impl<'a> harness::Ui for TermUi<'a> {
    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::Delta(d) => {
                self.acc.push_str(&d);
                self.scr.live = Some((2, tail_chars(&self.acc, 46)));
                self.scr.draw();
            }
            Ev::ThinkingDelta(t) => {
                // Wrap incrementally: only the new characters are measured.
                let wrap_cols = self.scr.cols.saturating_sub(2);
                for ch in t.chars() {
                    if ch == '\n' {
                        self.think_flush();
                        continue;
                    }
                    let cw = Scr::cw(ch);
                    if !self.think_cur.is_empty() && dw(&self.think_cur) + cw > wrap_cols {
                        self.think_flush();
                    }
                    self.think_cur.push(ch);
                }
                self.think_refresh();
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
                let had_lines = !self.scr.think_lines.is_empty();
                self.reset_think();
                self.scr.live = None;
                if had_lines || chars > 0 {
                    self.scr.push(
                        &format!("\u{25b8} thinking {secs:.1}s | {chars} chars (collapsed)"),
                        K_DIM,
                    );
                }
            }
            Ev::Assistant(t) => {
                self.reset_think();
                self.acc.clear();
                self.scr.live = None;
                // Results are the payload of the turn: render them as
                // structure (headings, bullets, code) rather than raw text.
                self.scr.push_md(&t);
            }
            Ev::ToolCall { id, name, title } => {
                self.reset_think();
                self.scr.live = None;
                self.scr.push(
                    &fit(
                        &format!("* {name} > {title} \u{2026}"),
                        self.scr.cols.saturating_sub(1),
                    ),
                    K_DIM,
                );
                self.tools.insert(id, (name, title, std::time::Instant::now()));
            }
            Ev::ToolResult {
                id,
                content,
                is_error,
                ..
            } => {
                // The pending line was drawn when the call started; this one
                // adds the outcome and a short preview of what came back.
                let (name, title, took) = self
                    .tools
                    .remove(&id)
                    .map(|(n, t, started)| (n, t, started.elapsed().as_secs_f32()))
                    .unwrap_or_else(|| (String::new(), String::new(), 0.0));
                let raw = format!("* {name} > {title} | {took:.1}s {}", if is_error { "x" } else { "ok" });
                let cols = self.scr.cols;
                self.scr.push(&fit(&raw, cols.saturating_sub(1)), K_TOOL);
                let preview = first_lines(&content, if is_error { 3 } else { 1 });
                if !preview.trim().is_empty() {
                    let kind = if is_error { K_ERR } else { K_RES };
                    self.scr.push(
                        &fit(&format!("  \u{21b3} {preview}"), cols.saturating_sub(1)),
                        kind,
                    );
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
            Ev::Compacting { tokens, messages } => {
                self.reset_think();
                self.scr.live = Some((
                    1,
                    format!(
                        "compacting {messages} messages ({} tokens)",
                        crate::compact::human_tokens(tokens)
                    ),
                ));
                self.scr.draw();
            }
            Ev::CompactingDone { ok, note } => {
                self.scr.live = None;
                if !ok {
                    self.scr.push(&format!("compaction failed: {note}"), K_DIM);
                }
            }
            Ev::Info(n) => self.scr.push(&n, K_DIM),
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
                    crate::voice::request_stop();
                    self.scr.quit = true;
                    return Action::Cancel;
                }
                0x09 => {
                    // Push-to-talk during a turn: honoured as soon as the
                    // current work stops, then submitted like a normal take.
                    if !self.scr.voice_pending {
                        self.scr.voice_pending = true;
                        self.scr
                            .push(i18n::t("voice_queued"), K_DIM);
                        changed = true;
                    }
                }
                0x1b => {
                    // Esc cancels the turn, including a `speak` clip that is
                    // still playing: the clip is a side effect of the turn, so
                    // the interrupt has to reach the speaker too.
                    match self.scr.read_key_timeout() {
                        Some(b'\r') => {
                            crate::voice::request_stop();
                            self.force_now();
                            poll_mode(true);
                            return Action::Cancel;
                        }
                        Some(b) if b == b'[' || b == b'O' => {
                            let mut seq = String::new();
                            for _ in 0..8 {
                                match self.scr.read_key_timeout() {
                                    Some(c) => {
                                        seq.push(c as char);
                                        if c == b'u' || c == b'~' || c.is_ascii_alphabetic() {
                                            break;
                                        }
                                    }
                                    None => break,
                                }
                            }
                            if seq.starts_with("13;") {
                                crate::voice::request_stop();
                                self.force_now();
                                poll_mode(true);
                                return Action::Cancel;
                            }
                            poll_mode(true); // read_key_timeout left the tty blocking
                        }
                        _ => {
                            crate::voice::request_stop();
                            return Action::Cancel;
                        }
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

impl<'a> TermUi<'a> {
    /// Queue the currently typed text ahead of everything else, so it runs as
    /// soon as the interrupted turn winds down.
    fn force_now(&mut self) {
        let line: String = self.scr.input.iter().collect();
        let line = line.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.scr.input.clear();
        self.scr.cursor = 0;
        self.scr.queue.insert(0, line.clone());
        self.scr
            .push(&i18n::tf("force_sent", &[&line]), K_TOOL);
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

/// One animation frame, centred in the current terminal width. The art is
/// pre-rendered to block glyphs (see `art.rs`), so this stays cheap.
fn art_frame_lines(frame: usize, cols: usize) -> Vec<String> {
    let f = &art::FRAMES[frame % art::FRAMES.len()];
    let pad = " ".repeat(cols.saturating_sub(art::ART_WIDTH) / 2);
    f.iter().map(|l| format!("{pad}{l}")).collect()
}

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


type ConfigRc = std::rc::Rc<std::cell::RefCell<config::Config>>;
type LoginRx = std::rc::Rc<std::cell::RefCell<Option<std::sync::mpsc::Receiver<login::Submitted>>>>;

/// Start the QR login server and render its code in the transcript.
/// The QR is 13 rows tall; inside tmux the status bar costs one row, so hide it
/// while the code is up and restore it afterwards. The pseudo-terminal also
/// grows from 12 to 13 rows, which `term_size` re-reads.
fn set_tmux_status(on: bool) {
    if std::env::var("TMUX").is_err() {
        return;
    }
    let _ = std::process::Command::new("tmux")
        .args(["set-option", "-g", "status", if on { "on" } else { "off" }])
        .status();
    std::thread::sleep(std::time::Duration::from_millis(150));
}

fn close_qr(scr: &mut Scr) {
    if scr.qr.is_none() {
        return;
    }
    set_tmux_status(true);
    let (r, c) = term_size();
    scr.rows = r;
    scr.cols = c;
    scr.qr = None;
    scr.qr_blit = false;
    {
        use std::io::Write as _;
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[2J\x1b[H");
        let _ = o.flush();
    }
    scr.draw();
}

fn start_login(scr: &mut Scr, login_rx: &LoginRx) {
    match login::start(login::DEFAULT_PORT) {
        Ok((url, rx)) => {
            let (qr, note) = login::qr_lines(&url);
            scr.push(&i18n::tf("login_url", &[&url]), K_NORM);
            if qr.is_empty() {
                scr.push(&i18n::tf("login_no_qr", &[&note]), K_DIM);
                scr.push(i18n::t("login_phone"), K_DIM);
            } else {
                let (r, c) = term_size();
                scr.rows = r;
                scr.cols = c;
                // Two columns: QR on the left (painted as real pixels), guide
                // text on the right, which is also what keeps the code crisp.
                const QR_PX: usize = 168;
                let left_cols = QR_PX / 6 + 2;
                let mut guide: Vec<String> = Vec::new();
                guide.push(format!("扫码登录 DeepSeek"));
                guide.push(String::new());
                guide.push(format!("地址"));
                let url_txt = url.clone();
                let width = c.saturating_sub(left_cols);
                let mut chunk = String::new();
                for ch in url_txt.chars() {
                    if chunk.chars().count() + 1 > width {
                        guide.push(chunk.clone());
                        chunk.clear();
                    }
                    chunk.push(ch);
                }
                if !chunk.is_empty() {
                    guide.push(chunk);
                }
                guide.push(String::new());
                guide.push("1. 手机扫码".to_string());
                guide.push("2. 打开页面并粘贴 key".to_string());
                guide.push("3. 本机自动完成".to_string());
                guide.push(String::new());
                guide.push("Esc 取消".to_string());
                let mut lines: Vec<String> = Vec::new();
                for g in guide.iter().take(scr.rows) {
                    lines.push(format!("{}{}", " ".repeat(left_cols), g));
                }
                scr.qr = Some(lines);
                scr.draw();
                if login::draw_qr_fb(&url, Some((0, 0, QR_PX, 170))).is_ok() {
                    scr.qr_blit = true;
                } else {
                    // Fallback: terminal-drawn QR across the full width.
                    let widest = qr.iter().map(|l| l.chars().count()).max().unwrap_or(0);
                    let pad = c.saturating_sub(widest) / 2;
                    scr.qr = Some(
                        qr.iter()
                            .map(|l| format!("{}{}", " ".repeat(pad), l))
                            .collect(),
                    );
                    scr.draw();
                }
            }
            *login_rx.borrow_mut() = Some(rx);
        }
        Err(e) => scr.push(&format!("无法启动登录服务: {e}"), K_ERR),
    }
}

fn apply_login(c: &mut config::Config, sub: login::Submitted) -> String {
    c.api_key = sub.key;
    if !sub.base.is_empty() {
        c.base = sub.base;
    }
    if !sub.model.is_empty() {
        c.model = sub.model;
    }
    c.configured = true;
    match c.save() {
        Ok(()) => i18n::tf("login_done", &[&config::config_path().display().to_string()]),
        Err(e) => format!("已收到 key，但保存失败: {e}"),
    }
}

/// First-run guide: ask DeepSeek official (QR) or a custom endpoint.
fn onboarding(scr: &mut Scr, config_rc: &ConfigRc, login_rx: &LoginRx) {
    if config_rc.borrow().configured {
        return;
    }
    scr.push(i18n::t("onboard_welcome"), K_NORM);
    scr.push(i18n::t("onboard_choose"), K_NORM);
    scr.push(i18n::t("onboard_opt1"), K_TOOL);
    scr.push(i18n::t("onboard_opt2"), K_TOOL);
    scr.push(i18n::t("onboard_prompt"), K_DIM);
    let choice = match scr.edit() {
        Some(l) => l.trim().to_string(),
        None => return,
    };
    if choice == "2" {
        scr.push(i18n::t("onboard_base"), K_DIM);
        let base = match scr.edit() {
            Some(l) => l.trim().to_string(),
            None => return,
        };
        scr.push(i18n::t("onboard_key"), K_DIM);
        scr.mask = true;
        let key = match scr.edit() {
            Some(l) => l.trim().to_string(),
            None => return,
        };
        scr.mask = false;
        scr.push(i18n::t("onboard_model"), K_DIM);
        let model = match scr.edit() {
            Some(l) => l.trim().to_string(),
            None => return,
        };
        {
            let mut c = config_rc.borrow_mut();
            if !base.is_empty() {
                c.base = base;
            }
            c.api_key = key;
            if !model.is_empty() {
                c.model = model;
            }
            c.configured = !c.api_key.is_empty();
            match c.save() {
                Ok(()) => scr.push(i18n::t("onboard_saved"), K_NORM),
                Err(e) => scr.push(&format!("保存失败: {e}"), K_ERR),
            }
        }
    } else {
        start_login(scr, login_rx);
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs() < 300 {
            let got = {
                let b = login_rx.borrow();
                b.as_ref().and_then(|rx| rx.try_recv().ok())
            };
            if let Some(sub) = got {
                *login_rx.borrow_mut() = None;
                let msg = apply_login(&mut config_rc.borrow_mut(), sub);
                scr.push(&msg, K_NORM);
                return;
            }
            if let Some(k) = scr.read_key_timeout() {
                if k == 0x1b || k == 0x03 {
                    *login_rx.borrow_mut() = None;
                    scr.push(i18n::t("onboard_cancelled"), K_DIM);
                    return;
                }
            }
        }
        *login_rx.borrow_mut() = None;
        scr.push(i18n::t("onboard_timeout"), K_ERR);
    }
}


// --------------------------------------------------------------- sessions

fn new_session_path() -> std::path::PathBuf {
    let stamp = std::process::Command::new("date")
        .arg("+%Y%m%d-%H%M%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "session".into())
        });
    config::sessions_dir().join(format!("{stamp}.json"))
}

fn save_session(msgs: &jobs::Msgs, cfg: &config::Config, path: &std::path::Path) {
    let doc = json::J::o(vec![
        ("model", json::J::s(&cfg.model)),
        ("effort", json::J::s(&cfg.effort)),
        ("messages", json::J::Arr(msgs.borrow().clone())),
    ]);
    let _ = std::fs::write(path, doc.dump());
}

fn resume_session(scr: &mut Scr, msgs: &jobs::Msgs, idx: usize) -> Option<std::path::PathBuf> {
    let sessions = config::list_sessions();
    let (path, label, _) = sessions.get(idx)?.clone();
    let txt = std::fs::read_to_string(&path).ok()?;
    let v = json::parse(&txt)?;
    let arr = v.get("messages")?.as_arr()?.clone();
    *msgs.borrow_mut() = arr.clone();
    scr.lines.clear();
    scr.scroll = 0;
    scr.splash_done = true;
    scr.push(&format!("已恢复会话 {label}"), K_DIM);
    for m in arr.iter().rev() {
        if m.get("role").and_then(|r| r.as_str()) == Some("assistant") {
            if let Some(t) = m.get("content").and_then(|c| c.as_str()) {
                if !t.trim().is_empty() {
                    scr.push_md(t);
                }
            }
            break;
        }
    }
    Some(path)
}

// ---------------------------------------------------------- config wizard

/// prompt inside the menu screens (menu is hidden while typing)
fn menu_prompt(scr: &mut Scr, prompt: &str, mask: bool) -> Option<String> {
    scr.menu = None;
    scr.mask = mask;
    scr.push(prompt, K_DIM);
    scr.draw();
    let r = scr.edit();
    scr.mask = false;
    r.map(|s| s.trim().to_string())
}

/// menuconfig-style configuration: arrows move, Enter edits, Esc leaves.
fn config_menu(scr: &mut Scr, config_rc: &ConfigRc) {
    let mut sel = 0usize;
    loop {
        let c = config_rc.borrow();
        let (model, effort, base, kstate, voice, auto, queued, lang, steps, compact, goalr, caps) = (
            c.model.clone(),
            c.effort.clone(),
            c.base.clone(),
            if c.api_key.is_empty() { "not set".to_string() } else { "configured".to_string() },
            c.tts_voice.clone(),
            c.auto_send_voice,
            c.queued_default,
            c.language.clone(),
            if c.max_steps == 0 { "unlimited".to_string() } else { c.max_steps.to_string() },
            if c.compact_enabled {
                format!("on @ {}", crate::compact::human_tokens(c.compact_threshold))
            } else {
                "off".to_string()
            },
            if c.goal_max_rounds == 0 {
                "unlimited".to_string()
            } else {
                c.goal_max_rounds.to_string()
            },
            c.capabilities(),
        );
        drop(c);
        let items = vec![
            MenuItem { label: i18n::t("menu_model").into(), value: model },
            MenuItem { label: i18n::t("menu_effort").into(), value: effort },
            MenuItem { label: i18n::t("menu_endpoint").into(), value: base },
            MenuItem { label: i18n::t("menu_key").into(), value: kstate },
            MenuItem { label: i18n::t("menu_voice").into(), value: voice },
            MenuItem {
                label: i18n::t("menu_autosend").into(),
                value: if auto { i18n::t("on").into() } else { i18n::t("off").into() },
            },
            MenuItem {
                label: i18n::t("menu_queue").into(),
                value: if queued { i18n::t("queued_mode").into() } else { i18n::t("immediate_mode").into() },
            },
            MenuItem { label: i18n::t("menu_language").into(), value: lang },
            MenuItem { label: i18n::t("menu_steps").into(), value: steps },
            MenuItem { label: i18n::t("menu_compact").into(), value: compact },
            MenuItem { label: i18n::t("menu_goalrounds").into(), value: goalr },
            MenuItem { label: i18n::t("menu_save").into(), value: String::new() },
        ];
        let hint = if caps.web_tools && caps.speech {
            i18n::t("menu_hint").to_string()
        } else {
            i18n::t("menu_hint_gated").to_string()
        };
        scr.menu = Some(Menu { title: i18n::t("menu_title").into(), items: items.clone(), sel, hint });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if k == 0x1b {
            match scr.read_key_timeout() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => {
                    scr.menu = None;
                    scr.draw();
                    return;
                }
            }
            continue;
        }
        if k == 0x03 || k == b'q' {
            scr.menu = None;
            scr.draw();
            return;
        }
        if k != 0x0d {
            continue;
        }
        match sel {
            0 => {
                let cfg = api_cfg(&config_rc.borrow());
                let models = crate::api::list_models(&cfg).unwrap_or_default();
                if models.is_empty() {
                    if let Some(v) = menu_prompt(scr, "Model name:", false) {
                        if !v.is_empty() {
                            config_rc.borrow_mut().model = v;
                            let _ = config_rc.borrow().save();
                        }
                    }
                } else {
                    let mut msel = 0usize;
                    loop {
                        let its: Vec<MenuItem> = models
                            .iter()
                            .map(|(id, name, _)| MenuItem { label: id.clone(), value: name.clone() })
                            .collect();
                        scr.menu = Some(Menu {
                            title: i18n::t("menu_model").into(),
                            items: its,
                            sel: msel,
                            hint: i18n::t("menu_model_hint").into(),
                        });
                        scr.draw();
                        let kk = match scr.read_key_timeout() { Some(k) => k, None => continue };
                        if kk == 0x1b {
                            match scr.read_key_timeout() {
                                Some(b'[') => {
                                    match scr.read_key_timeout() {
                                        Some(b'A') => msel = msel.saturating_sub(1),
                                        Some(b'B') => msel = (msel + 1).min(models.len() - 1),
                                        _ => {}
                                    }
                                }
                                _ => break,
                            }
                            continue;
                        }
                        if kk == 0x0d {
                            config_rc.borrow_mut().model = models[msel].0.clone();
                            let _ = config_rc.borrow().save();
                            break;
                        }
                    }
                }
            }
            1 => {
                let cur = config_rc.borrow().effort.clone();
                let next = match cur.as_str() {
                    "auto" => "low",
                    "low" => "high",
                    "high" => "max",
                    _ => "auto",
                };
                config_rc.borrow_mut().effort = next.to_string();
                let _ = config_rc.borrow().save();
            }
            2 => {
                let mut esel = if crate::config::is_official(&config_rc.borrow().base) { 0 } else { 1 };
                loop {
                    let its = vec![
                        MenuItem { label: i18n::t("menu_official").into(), value: "api.deepseek.com".into() },
                        MenuItem { label: i18n::t("menu_custom").into(), value: config_rc.borrow().base.clone() },
                    ];
                    scr.menu = Some(Menu { title: i18n::t("menu_endpoint").into(), items: its, sel: esel, hint: i18n::t("menu_endpoint_hint").into() });
                    scr.draw();
                    let kk = match scr.read_key_timeout() { Some(k) => k, None => continue };
                    if kk == 0x1b {
                        match scr.read_key_timeout() {
                            Some(b'[') => {
                                match scr.read_key_timeout() {
                                    Some(b'A') => esel = 0,
                                    Some(b'B') => esel = 1,
                                    _ => {}
                                }
                            }
                            _ => break,
                        }
                        continue;
                    }
                    if kk != 0x0d {
                        continue;
                    }
                    if esel == 0 {
                        config_rc.borrow_mut().base = "https://api.deepseek.com/v1".into();
                        let _ = config_rc.borrow().save();
                        break;
                    }
                    // Warn before a third-party endpoint: capabilities shrink.
                    scr.menu = None;
                    scr.draw();
                    scr.push(i18n::t("menu_gate_warn"), K_ERR);
                    scr.push(i18n::t("menu_gate_ask"), K_DIM);
                    let go = match scr.edit() { Some(a) => a.trim().eq_ignore_ascii_case("y"), None => false };
                    if go {
                        if let Some(u) = menu_prompt(scr, "Base URL (https://host/v1):", false) {
                            if u.starts_with("http") {
                                config_rc.borrow_mut().base = u;
                                let _ = config_rc.borrow().save();
                            } else if !u.is_empty() {
                                scr.push("Must be an http(s) URL", K_ERR);
                            }
                        }
                    }
                    break;
                }
            }
            3 => {
                if let Some(k) = menu_prompt(scr, "API key (not echoed):", true) {
                    if !k.is_empty() {
                        let mut c = config_rc.borrow_mut();
                        c.api_key = k;
                        c.configured = true;
                        let _ = c.save();
                    }
                }
            }
            4 => {
                let cur = config_rc.borrow().tts_voice.clone();
                config_rc.borrow_mut().tts_voice = if cur == "stella" { "tide".into() } else { "stella".into() };
                let _ = config_rc.borrow().save();
            }
            5 => {
                let mut c = config_rc.borrow_mut();
                c.auto_send_voice = !c.auto_send_voice;
                let _ = c.save();
            }
            6 => {
                let mut c = config_rc.borrow_mut();
                c.queued_default = !c.queued_default;
                let _ = c.save();
            }
            7 => {
                let cur = config_rc.borrow().language.clone();
                let next = if cur.starts_with("zh") { "en" } else { "zh" };
                config_rc.borrow_mut().language = next.to_string();
                i18n::set(&config_rc.borrow().language);
                let _ = config_rc.borrow().save();
            }
            8 => {
                let mut c = config_rc.borrow_mut();
                c.max_steps = match c.max_steps {
                    0 => 20,
                    20 => 50,
                    50 => 100,
                    _ => 0,
                };
                let _ = c.save();
            }
            9 => {
                let mut c = config_rc.borrow_mut();
                // off -> 512K -> 256K -> 128K -> off
                if !c.compact_enabled {
                    c.compact_enabled = true;
                    c.compact_threshold = 512_000;
                } else if c.compact_threshold > 256_000 {
                    c.compact_threshold = 256_000;
                } else if c.compact_threshold > 128_000 {
                    c.compact_threshold = 128_000;
                } else {
                    c.compact_enabled = false;
                }
                let _ = c.save();
            }
            10 => {
                let mut c = config_rc.borrow_mut();
                c.goal_max_rounds = match c.goal_max_rounds {
                    0 => 3,
                    3 => 10,
                    10 => 25,
                    _ => 0,
                };
                let _ = c.save();
            }
            _ => {
                let _ = config_rc.borrow().save();
                scr.menu = None;
                scr.draw();
                return;
            }
        }
    }
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

    let config_rc: ConfigRc = std::rc::Rc::new(std::cell::RefCell::new(config::Config::load()));
    let login_rx: LoginRx = std::rc::Rc::new(std::cell::RefCell::new(None));
    i18n::set(&config_rc.borrow().language);
    let mut cfg = api_cfg(&config_rc.borrow());
    tools::set_cfg(cfg.clone());
    voice::set_cfg(cfg.clone());
    let shared_cfg = std::rc::Rc::new(std::cell::RefCell::new(cfg.clone()));
    // Skills first (their catalog goes into the prompt), then MCP servers
    // (their tools are discovered over JSON-RPC at startup).
    usage::load();
    let skill_list = skills::load_all();
    tools::set_skills(skill_list.clone());
    mcp::install(mcp::start_all());
    let mut reg = tools::registry(config_rc.borrow().capabilities());
    reg.extend(goal::tool_defs());
    reg.extend(mcp::tool_defs());
    let mut caps_at_start = config_rc.borrow().capabilities();
    let mut system_prompt = format!(
        "{}{}\n\nGOALS\n{}\n",
        SYSTEM_PROMPT,
        skills::catalog_prompt(&skill_list),
        goal::guidance()
    );
    if !caps_at_start.web_tools {
        system_prompt.push_str(
            "\nCAPABILITIES\nThe configured endpoint is a third-party OpenAI-compatible API. search, fetch, ocr and speech tools are not registered because they only exist on DeepSeek's official API. Use bash/read_file/write_file instead, and do not tell the \
user you can search the web or speak.\n",
        );
    }
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
    if argv.len() >= 2 && argv[1] == "--tools" {
        // Diagnostics: the exact tool set and capability decision.
        let caps = config_rc.borrow().capabilities();
        println!("base={}", config_rc.borrow().base);
        println!("official={}", crate::config::is_official(&config_rc.borrow().base));
        println!("web_tools={} speech={}", caps.web_tools, caps.speech);
        let mut names: Vec<&str> = reg.iter().map(|t| t.name).collect();
        names.sort();
        println!("tools={}", names.join(","));
        return;
    }
    if argv.len() >= 4 && argv[1] == "--call" {
        // Diagnostics: execute one tool call exactly as the model would.
        let call = tool::Call {
            id: "cli".to_string(),
            name: argv[2].clone(),
            raw_args: argv[3].clone(),
        };
        let mut host = tool::NullHost;
        let outs = tool::run_batch(&reg, &[call], &mut host);
        for o in outs {
            println!("{}", o.content);
            if o.is_error {
                std::process::exit(2);
            }
        }
        return;
    }
    if argv.len() >= 3 && argv[1] == "--once" {
        run_once(&cfg, &reg, &argv[2]);
        return;
    }

    // The TUI owns the terminal; diagnostics (--tools/--once) run without it.
    if !single_instance() {
        eprintln!("dst: another instance is already running on this terminal");
        std::process::exit(1);
    }

    let mut scr = Scr::new();
    scr.speech_ok = config_rc.borrow().capabilities().speech;
    let mut session_path: Option<std::path::PathBuf> = None;
    raw_mode(true);

    scr.seed_splash(&cfg.model);
    scr.hdr = device_status();
    onboarding(&mut scr, &config_rc, &login_rx);
    cfg = api_cfg(&config_rc.borrow());
    *shared_cfg.borrow_mut() = cfg.clone();
    tools::set_cfg(cfg.clone());
    voice::set_cfg(cfg.clone());
    refresh_ctx_window(&mut scr, &cfg);
    if !config_rc.borrow().configured {
        scr.push(i18n::t("unconfigured"), K_ERR);
    }
    let msgs: jobs::Msgs = std::rc::Rc::new(std::cell::RefCell::new(vec![harness::msg("system", &system_prompt)]));
    {
        let tdefs2 = tdefs.clone();
        let msgs2 = msgs.clone();
        let sc = shared_cfg.clone();
        let crc = config_rc.clone();
        let lrx = login_rx.clone();
        scr.on_tick = Some(Box::new(move |s: &mut Scr| {
            let c = sc.borrow().clone();
            jobs::poll(&c, &tdefs2, &msgs2, s);
            let got = {
                let b = lrx.borrow();
                b.as_ref().and_then(|rx| rx.try_recv().ok())
            };
            if let Some(sub) = got {
                let msg = apply_login(&mut crc.borrow_mut(), sub);
                *lrx.borrow_mut() = None;
                close_qr(s);
                let c = api_cfg(&crc.borrow());
                *sc.borrow_mut() = c.clone();
                tools::set_cfg(c.clone());
                voice::set_cfg(c.clone());
                s.push(&msg, K_NORM);
            }
        }));
    }

    let mut turn_no = 0u32;
    // A pending automatic goal round: (model prompt, one-line transcript label).
    let mut goal_seed: Option<(String, String)> = None;
    // A queued line pulled out of the busy-time queue so the outer loop can
    // dispatch it as a command if it starts with `/` (a queued `/clear` must
    // not be sent to the model as chat).
    let mut pending_line: Option<String> = None;
    loop {
        // A queued command is a human instruction: it must run before the
        // goal driver spends another autonomous round (DSH's driver waits for
        // competing queued prompts instead of starving them).
        let (line, label) = if let Some(q) = pending_line.take() {
            (q, None)
        } else {
            match goal_seed.take() {
                Some((prompt, label)) => (prompt, Some(label)),
                None => match scr.edit() {
                    Some(l) => (l, None),
                    None => break,
                },
            }
        };
        // Slash commands are human commands: they run here and never become a
        // model message (the same contract as DSH's command registry).
        if label.is_none() && line.starts_with('/') {
            let goal_before = goal::current();
            // Bind the borrow before the if-let: a temporary RefMut in the
            // scrutinee would live for the whole body and make every later
            // config access (wizard, login, save) panic.
            let dispatched = {
                let mut guard = config_rc.borrow_mut();
                cmd::dispatch(&line, &mut guard)
            };
            if let Some(out) = dispatched {
                scr.push(&format!("> {line}"), K_USER);
                if out.clear {
                    scr.seed_splash(&config_rc.borrow().model.clone());
                }
                for (text, kind) in out.lines {
                    scr.push(&text, kind);
                }
                if out.quit {
                    break;
                }
                if out.compact {
                    // Manual /compact: DSH's `compactNow` bypasses the pressure
                    // threshold and keeps no verbatim tail.
                    let mut manual = {
                        let c = config_rc.borrow();
                        crate::compact::Settings::from_config(&c)
                    };
                    manual.threshold_tokens = 0;
                    manual.retain_ratio = 0.0;
                    cfg = api_cfg(&config_rc.borrow());
                    refresh_ctx_window(&mut scr, &cfg);
                    let defs = tool::definitions(&reg);
                    let window = scr.ctx_window;
                    let result = {
                        let mut m = msgs.borrow_mut();
                        let mut ui = TermUi {
                            scr: &mut scr,
                            acc: String::new(),
                            think_cur: String::new(),
                            think_wrapped: Vec::new(),
                            tools: std::collections::HashMap::new(),
                            spin: 0,
                        };
                        crate::compact::compact(&cfg, &manual, &mut m, window, &defs, &mut ui)
                    };
                    match result {
                        Ok(r) => scr.push(
                            &format!(
                                "compacted: {} -> {} tokens ({} messages)",
                                crate::compact::human_tokens(r.before_tokens),
                                crate::compact::human_tokens(r.after_tokens),
                                r.dropped_messages
                            ),
                            K_DIM,
                        ),
                        Err(e) => scr.push(&format!("compaction failed: {e}"), K_ERR),
                    }
                }
                if out.login {
                    start_login(&mut scr, &login_rx);
                }
                if out.config_wizard {
                    config_menu(&mut scr, &config_rc);
                }
                if let Some(pick) = out.resume {
                    if let Some(n) = pick {
                        if let Some(p) = resume_session(&mut scr, &msgs, n) {
                            session_path = Some(p);
                        }
                    }
                }
                i18n::set(&config_rc.borrow().language);
                let caps_now = config_rc.borrow().capabilities();
                if caps_now.web_tools != caps_at_start.web_tools || caps_now.speech != caps_at_start.speech {
                    reg = tools::registry(caps_now);
                    reg.extend(goal::tool_defs());
                    reg.extend(mcp::tool_defs());
                    scr.speech_ok = caps_now.speech;
                    caps_at_start = caps_now;
                }
                cfg = api_cfg(&config_rc.borrow());
                *shared_cfg.borrow_mut() = cfg.clone();
                tools::set_cfg(cfg.clone());
                voice::set_cfg(cfg.clone());
                // A command may have paused/cleared the goal while a round was
                // already planned: do not run that round.
                if goal_seed.is_some()
                    && !goal::current().map(|g| g.is_active()).unwrap_or(false)
                {
                    goal_seed = None;
                }
                // `/goal <objective>`, `/goal resume`, `/goal edit` re-arm the
                // driver: start the next automatic round right away.
                if let Some(after) = goal::current() {
                    let changed = match &goal_before {
                        None => true,
                        Some(b) => {
                            b.id != after.id
                                || b.armed != after.armed
                                || b.revision != after.revision
                        }
                    };
                    if changed && after.is_active() {
                        if let Some((before, round)) = goal::start_round() {
                            goal_seed = Some((
                                goal::round_prompt(&before, round),
                                format!("\u{27f3} goal round {round}"),
                            ));
                        }
                    }
                }
                continue;
            }
        }
        let mut next: Option<(String, Option<String>)> = Some((line, label));
        let mut last_report: Option<harness::TurnReport> = None;
        while let Some((submit, label)) = next {
            // The splash is for the idle screen only: once the user starts
            // working, give the whole transcript area to the conversation.
            if !scr.splash_done {
                scr.lines.clear();
                scr.scroll = 0;
                scr.splash_done = true;
                scr.art_on = false;
            }
            match &label {
                Some(l) => scr.push(l, K_TOOL),
                None => scr.push(&format!("> {submit}"), K_USER),
            }
            turn_no += 1;
            cfg = api_cfg(&config_rc.borrow());
            refresh_ctx_window(&mut scr, &cfg);
            let mut opts = harness::RunOpts::from_config(&config_rc.borrow(), scr.ctx_window);
            opts.turn = turn_no;
            if label.is_some() {
                // Goal rounds use their own step budget (0 = unlimited).
                opts.max_steps = config_rc.borrow().goal_max_steps;
            }
            let in_goal_round = label.is_some();
            goal::set_in_round(in_goal_round);
            scr.busy = true;
            poll_mode(true);
            {
                let mut m = msgs.borrow_mut();
                let mut ui = TermUi {
                    scr: &mut scr,
                    acc: String::new(),
                    think_cur: String::new(),
                    think_wrapped: Vec::new(),
                    tools: std::collections::HashMap::new(),
                    spin: 0,
                };
                last_report = Some(harness::run_turn(&cfg, &opts, &mut m, &reg, &submit, &mut ui));
            }
            goal::set_in_round(false);
            poll_mode(false);
            scr.busy = false;
            scr.live = None;
            scr.draw();
            if scr.voice_pending {
                scr.voice_pending = false;
                scr.voice_input();
                if scr.auto_send && !scr.input.is_empty() {
                    let line: String = scr.input.iter().collect();
                    scr.input.clear();
                    scr.cursor = 0;
                    scr.queue.insert(0, line);
                }
            }
            if session_path.is_none() {
                session_path = Some(new_session_path());
            }
            if let Some(p) = &session_path {
                save_session(&msgs, &config_rc.borrow(), p);
            }
            if scr.quit {
                break;
            }
            next = match scr.queue.first() {
                None => None,
                Some(q) if q.starts_with('/') => {
                    pending_line = Some(scr.queue.remove(0));
                    None
                }
                Some(_) => Some((scr.queue.remove(0), None)),
            };
            if pending_line.is_some() {
                break;
            }
        }
        if scr.quit {
            break;
        }
        // ---- goal round driver (DSH `dsh-goal-round-driver`) ----
        // Esc stops the autonomous run: an interrupted round disarms the goal
        // instead of retrying it; `/goal resume` re-arms.
        if last_report.as_ref().map(|r| r.cancelled).unwrap_or(false) {
            if goal::current().map(|g| g.is_active()).unwrap_or(false) {
                goal::set_armed(false);
                scr.push(
                    "goal disarmed after interrupt; /goal resume to continue",
                    K_DIM,
                );
            }
        }
        if scr.queue.is_empty() && pending_line.is_none() {
            if let Some(goal) = goal::current() {
                if goal.is_active() {
                    if let Some((before, round)) = goal::start_round() {
                        goal_seed = Some((
                            goal::round_prompt(&before, round),
                            format!("\u{27f3} goal round {round}"),
                        ));
                    } else if let Some(now) = goal::current() {
                        // `start_round` also returns None at the round cap and
                        // has already disarmed: report the fresh state so the
                        // user knows why the autonomous run stopped.
                        if now.max_rounds > 0 && now.rounds_started >= now.max_rounds {
                            scr.push(
                                &format!(
                                    "goal reached its round limit ({}); /goal resume to continue",
                                    now.max_rounds
                                ),
                                K_DIM,
                            );
                        }
                    }
                }
            }
        }
    }


    if let Some(p) = &session_path {
        save_session(&msgs, &config_rc.borrow(), p);
    }
    raw_mode(false);
    let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[2J\x1b[H");
    println!("bye");
}


fn first_lines(s: &str, n: usize) -> String {
    s.lines()
        .take(n)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
