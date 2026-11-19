//! Fixed-size, page-by-page full-screen artifact layout and renderer.
//!
//! Designed for the Cardputer Zero 53x12 panel (1 header row, fixed `body_rows`
//! page canvas, 1 footer row). Pages never scroll line-by-line: content is
//! laid out into discrete fixed-height pages and flipped page-by-page.

use crate::json::J;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Default,
    Info,
    Success,
    Warn,
    Error,
    Code,
}

impl Theme {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "info" | "cyan" => Theme::Info,
            "ok" | "success" | "green" => Theme::Success,
            "warn" | "warning" | "yellow" => Theme::Warn,
            "err" | "error" | "alert" | "red" => Theme::Error,
            "code" | "mono" => Theme::Code,
            _ => Theme::Default,
        }
    }

    pub fn header_sgr(self) -> &'static str {
        match self {
            Theme::Default => "\x1b[1;37m",
            Theme::Info => "\x1b[1;36m",
            Theme::Success => "\x1b[1;32m",
            Theme::Warn => "\x1b[1;33m",
            Theme::Error => "\x1b[1;31m",
            Theme::Code => "\x1b[1;36m",
        }
    }

    pub fn accent_sgr(self) -> &'static str {
        match self {
            Theme::Default | Theme::Info | Theme::Code => "\x1b[36m",
            Theme::Success => "\x1b[32m",
            Theme::Warn => "\x1b[33m",
            Theme::Error => "\x1b[31m",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "center" | "centre" | "mid" => Align::Center,
            "right" | "end" => Align::Right,
            _ => Align::Left,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLayout {
    Auto,
    Center,
    Kv,
    Columns,
}

impl PageLayout {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "center" | "centre" | "hero" => PageLayout::Center,
            "kv" | "stats" | "metrics" => PageLayout::Kv,
            "columns" | "split" | "two-col" => PageLayout::Columns,
            _ => PageLayout::Auto,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    /// Optional per-page subtitle shown in the header bar.
    pub title: Option<String>,
    /// ANSI-styled lines (len <= page_rows, each visible width <= page_cols).
    pub lines: Vec<String>,
    /// Plain-text (ANSI-free) lines corresponding to `lines`.
    pub plain: Vec<String>,
}

/// A full-screen, fixed-size paginated panel.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub title: String,
    pub subtitle: Option<String>,
    pub theme: Theme,
    pub footer: Option<String>,
    pub pages: Vec<Page>,
    pub page: usize,
    #[allow(dead_code)]
    pub page_rows: usize,
    #[allow(dead_code)]
    pub page_cols: usize,
    /// Transient footer notice (e.g. "saved to ~/.dst/tmp/...").
    pub notice: Option<String>,
    /// Enter submits (used when reviewing a prompt app) instead of only closing.
    pub submit: bool,
    /// Clean ANSI-free text exported by `s`.
    pub save_text: String,
}

impl Artifact {
    pub fn total_pages(&self) -> usize {
        self.pages.len().max(1)
    }

    pub fn current_page(&self) -> &Page {
        let idx = self.page.min(self.pages.len().saturating_sub(1));
        &self.pages[idx]
    }

    pub fn next_page(&mut self) -> bool {
        let last = self.pages.len().saturating_sub(1);
        if self.page < last {
            self.page += 1;
            self.notice = None;
            true
        } else {
            false
        }
    }

    pub fn prev_page(&mut self) -> bool {
        if self.page > 0 {
            self.page -= 1;
            self.notice = None;
            true
        } else {
            false
        }
    }

    pub fn first_page(&mut self) -> bool {
        if self.page != 0 {
            self.page = 0;
            self.notice = None;
            true
        } else {
            false
        }
    }

    pub fn last_page(&mut self) -> bool {
        let last = self.pages.len().saturating_sub(1);
        if self.page != last {
            self.page = last;
            self.notice = None;
            true
        } else {
            false
        }
    }

    pub fn goto_page(&mut self, idx: usize) -> bool {
        let target = idx.min(self.pages.len().saturating_sub(1));
        if target != self.page {
            self.page = target;
            self.notice = None;
            true
        } else {
            false
        }
    }

    /// Build a plain-text paginated artifact (tool logs, job logs, prompt preview).
    pub fn from_plain(title: impl Into<String>, text: &str, page_rows: usize, page_cols: usize) -> Self {
        let page_rows = page_rows.max(1);
        let page_cols = page_cols.max(10);
        let title = clean_title(&title.into());
        let mut wrapped = Vec::new();
        for raw in text.lines() {
            let parts = wrap_plain(raw, page_cols);
            if parts.is_empty() {
                wrapped.push(String::new());
            } else {
                wrapped.extend(parts);
            }
        }
        if wrapped.is_empty() {
            wrapped.push(String::new());
        }
        let pages = paginate_lines(None, wrapped.clone(), wrapped, page_rows);
        Artifact {
            title,
            subtitle: None,
            theme: Theme::Default,
            footer: None,
            pages,
            page: 0,
            page_rows,
            page_cols,
            notice: None,
            submit: false,
            save_text: text.to_string(),
        }
    }

    /// Build a plain-text paginated artifact from pre-split lines.
    pub fn from_lines_plain(
        title: impl Into<String>,
        lines: &[String],
        page_rows: usize,
        page_cols: usize,
    ) -> Self {
        let joined = lines.join("\n");
        Self::from_plain(title, &joined, page_rows, page_cols)
    }

    /// Build a rich Markdown paginated artifact (/btw answers, skills, docs).
    pub fn from_markdown(
        title: impl Into<String>,
        md: &str,
        page_rows: usize,
        page_cols: usize,
    ) -> Self {
        let spec = ArtifactSpec {
            title: clean_title(&title.into()),
            subtitle: None,
            theme: Theme::Default,
            layout: PageLayout::Auto,
            footer: None,
            body: Some(md.to_string()),
            pages: Vec::new(),
            blocks: Vec::new(),
        };
        spec.render(page_rows, page_cols).artifact
    }

    /// Build an artifact from a validated `artifact` tool JSON payload.
    pub fn from_tool_json(v: &J, page_rows: usize, page_cols: usize) -> Result<RenderResult, String> {
        let spec = ArtifactSpec::from_json(v)?;
        Ok(spec.render(page_rows, page_cols))
    }
}

/// Structured page input inside `pages: [...]`.
#[derive(Clone, Debug)]
pub struct PageSpec {
    pub title: Option<String>,
    pub layout: PageLayout,
    pub body: Option<String>,
    pub blocks: Vec<J>,
}

/// Parsed specification from an `artifact` tool call.
#[derive(Clone, Debug)]
pub struct ArtifactSpec {
    pub title: String,
    pub subtitle: Option<String>,
    pub theme: Theme,
    pub layout: PageLayout,
    pub footer: Option<String>,
    pub body: Option<String>,
    pub pages: Vec<PageSpec>,
    pub blocks: Vec<J>,
}

pub struct RenderResult {
    pub artifact: Artifact,
    /// Number of extra pages produced when an explicit page exceeded `page_rows`.
    pub overflowed_pages: usize,
}

impl ArtifactSpec {
    pub fn from_json(args: &J) -> Result<Self, String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .map(clean_title)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing 'title'".to_string())?;
        let subtitle = args
            .get("subtitle")
            .and_then(|v| v.as_str())
            .map(clean_title)
            .filter(|s| !s.is_empty());
        let theme = args
            .get("theme")
            .or_else(|| args.get("kind"))
            .and_then(|v| v.as_str())
            .map(Theme::parse)
            .unwrap_or(Theme::Default);
        let layout = args
            .get("layout")
            .and_then(|v| v.as_str())
            .map(PageLayout::parse)
            .unwrap_or(PageLayout::Auto);
        let footer = args
            .get("footer")
            .and_then(|v| v.as_str())
            .map(clean_title)
            .filter(|s| !s.is_empty());
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());
        let blocks = args
            .get("blocks")
            .and_then(|v| v.as_arr())
            .cloned()
            .unwrap_or_default();

        let mut pages = Vec::new();
        if let Some(arr) = args.get("pages").and_then(|v| v.as_arr()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    if !s.trim().is_empty() {
                        pages.push(PageSpec {
                            title: None,
                            layout,
                            body: Some(s.to_string()),
                            blocks: Vec::new(),
                        });
                    }
                } else if matches!(item, J::Obj(_)) {
                    let pt = item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(clean_title)
                        .filter(|s| !s.is_empty());
                    let pl = item
                        .get("layout")
                        .and_then(|v| v.as_str())
                        .map(PageLayout::parse)
                        .unwrap_or(layout);
                    let pb = item
                        .get("body")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.trim().is_empty());
                    let pblocks = item
                        .get("blocks")
                        .and_then(|v| v.as_arr())
                        .cloned()
                        .unwrap_or_default();
                    if pb.is_some() || !pblocks.is_empty() || pt.is_some() {
                        pages.push(PageSpec {
                            title: pt,
                            layout: pl,
                            body: pb,
                            blocks: pblocks,
                        });
                    }
                }
            }
        }

        if body.is_none() && blocks.is_empty() && pages.is_empty() {
            return Err("artifact needs non-empty 'body', 'pages', or 'blocks'".to_string());
        }

        Ok(ArtifactSpec {
            title,
            subtitle,
            theme,
            layout,
            footer,
            body,
            pages,
            blocks,
        })
    }

    pub fn render(&self, page_rows: usize, page_cols: usize) -> RenderResult {
        let page_rows = page_rows.max(1);
        let page_cols = page_cols.max(12);
        let mut all_pages: Vec<Page> = Vec::new();
        let mut overflowed_pages = 0usize;

        if !self.pages.is_empty() {
            for ps in &self.pages {
                let mut rendered = Vec::new();
                if let Some(b) = &ps.body {
                    rendered.extend(render_body_for_layout(b, ps.layout, page_cols, self.theme));
                }
                if !ps.blocks.is_empty() {
                    rendered.extend(render_blocks(&ps.blocks, page_cols, self.theme));
                }
                if rendered.is_empty() {
                    rendered.push(String::new());
                }
                let styled = apply_page_layout(rendered, ps.layout, page_rows, page_cols);
                let plain: Vec<String> = styled.iter().map(|l| strip_ansi(l)).collect();
                let chunks = paginate_lines(ps.title.clone(), styled, plain, page_rows);
                if chunks.len() > 1 {
                    overflowed_pages += chunks.len() - 1;
                }
                all_pages.extend(chunks);
            }
        } else {
            // Single `body` (which may contain `===` page breaks) and/or `blocks`.
            if let Some(b) = &self.body {
                let sections = split_explicit_pages(b);
                let explicit = sections.len() > 1;
                for (sec_title, sec_body) in sections {
                    let rendered =
                        render_body_for_layout(&sec_body, self.layout, page_cols, self.theme);
                    let styled = apply_page_layout(rendered, self.layout, page_rows, page_cols);
                    let plain: Vec<String> = styled.iter().map(|l| strip_ansi(l)).collect();
                    let chunks = paginate_lines(sec_title, styled, plain, page_rows);
                    if explicit && chunks.len() > 1 {
                        overflowed_pages += chunks.len() - 1;
                    }
                    all_pages.extend(chunks);
                }
            }
            if !self.blocks.is_empty() {
                let rendered = render_blocks(&self.blocks, page_cols, self.theme);
                let styled = apply_page_layout(rendered, self.layout, page_rows, page_cols);
                let plain: Vec<String> = styled.iter().map(|l| strip_ansi(l)).collect();
                all_pages.extend(paginate_lines(None, styled, plain, page_rows));
            }
        }

        if all_pages.is_empty() {
            all_pages.push(Page {
                title: None,
                lines: vec![String::new()],
                plain: vec![String::new()],
            });
        }

        // Build clean ANSI-free save text across all pages.
        let mut save_parts = Vec::new();
        for (idx, p) in all_pages.iter().enumerate() {
            if all_pages.len() > 1 {
                if let Some(pt) = &p.title {
                    save_parts.push(format!("=== [{}/{}] {} ===", idx + 1, all_pages.len(), pt));
                } else if idx > 0 {
                    save_parts.push(format!("=== [{}/{}] ===", idx + 1, all_pages.len()));
                }
            }
            save_parts.push(p.plain.join("\n"));
        }
        let save_text = save_parts.join("\n");

        RenderResult {
            artifact: Artifact {
                title: self.title.clone(),
                subtitle: self.subtitle.clone(),
                theme: self.theme,
                footer: self.footer.clone(),
                pages: all_pages,
                page: 0,
                page_rows,
                page_cols,
                notice: None,
                submit: false,
                save_text,
            },
            overflowed_pages,
        }
    }
}

// ------------------------------------------------------------------ width / ANSI helpers

/// Display width of a single Unicode scalar on a 6x13 bitmap grid.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
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

/// Strip ANSI CSI (`\x1b[...m`) escape sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            it.next();
            for nc in it.by_ref() {
                if ('@'..='~').contains(&nc) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Display width ignoring ANSI escape sequences.
pub fn visible_width(s: &str) -> usize {
    strip_ansi(s).chars().map(char_width).sum()
}

/// Clip an ANSI-styled string to `max_cells` display cells without breaking escapes.
pub fn clip_ansi(s: &str, max_cells: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    let mut had_esc = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            had_esc = true;
            out.push(c);
            out.push(it.next().unwrap());
            for nc in it.by_ref() {
                out.push(nc);
                if ('@'..='~').contains(&nc) {
                    break;
                }
            }
            continue;
        }
        let cw = char_width(c);
        if w + cw > max_cells {
            break;
        }
        out.push(c);
        w += cw;
    }
    if had_esc && !out.ends_with("\x1b[0m") {
        out.push_str("\x1b[0m");
    }
    out
}

/// Truncate plain text to `max_cells` with `…` if needed.
fn ellipsize_plain(s: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    let total: usize = s.chars().map(char_width).sum();
    if total <= max_cells {
        return s.to_string();
    }
    if max_cells == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > max_cells - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn pad_ansi(s: &str, width: usize, align: Align) -> String {
    let clipped = clip_ansi(s, width);
    let vis = visible_width(&clipped);
    let rem = width.saturating_sub(vis);
    match align {
        Align::Left => format!("{clipped}{}", " ".repeat(rem)),
        Align::Right => format!("{}{clipped}", " ".repeat(rem)),
        Align::Center => {
            let left = rem / 2;
            let right = rem - left;
            format!("{}{clipped}{}", " ".repeat(left), " ".repeat(right))
        }
    }
}

fn clean_title(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wrap_plain(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = Vec::new();
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
                let cw = char_width(chars[j]);
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
    out
}

// ------------------------------------------------------------------ pagination

fn split_explicit_pages(body: &str) -> Vec<(Option<String>, String)> {
    let mut sections: Vec<(Option<String>, Vec<String>)> = Vec::new();
    let mut cur_title: Option<String> = None;
    let mut cur_lines: Vec<String> = Vec::new();
    let mut in_code = false;

    for raw in body.lines() {
        let t = raw.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            cur_lines.push(raw.to_string());
            continue;
        }
        if !in_code {
            if t == "===" || t == "---page---" || t == "\x0c" {
                if !cur_lines.is_empty() || cur_title.is_some() {
                    sections.push((cur_title.take(), std::mem::take(&mut cur_lines)));
                }
                continue;
            }
            if let Some(rest) = t.strip_prefix("=== ") {
                let rest = rest.trim().trim_end_matches(" ===").trim();
                if !cur_lines.is_empty() || cur_title.is_some() {
                    sections.push((cur_title.take(), std::mem::take(&mut cur_lines)));
                }
                cur_title = if rest.is_empty() {
                    None
                } else {
                    Some(clean_title(rest))
                };
                continue;
            }
        }
        cur_lines.push(raw.to_string());
    }
    if !cur_lines.is_empty() || cur_title.is_some() || sections.is_empty() {
        sections.push((cur_title, cur_lines));
    }

    sections
        .into_iter()
        .map(|(t, ls)| (t, ls.join("\n")))
        .collect()
}

fn paginate_lines(
    title: Option<String>,
    styled: Vec<String>,
    plain: Vec<String>,
    page_rows: usize,
) -> Vec<Page> {
    let page_rows = page_rows.max(1);
    if styled.is_empty() {
        return vec![Page {
            title,
            lines: vec![String::new()],
            plain: vec![String::new()],
        }];
    }
    let mut pages = Vec::new();
    let mut i = 0usize;
    while i < styled.len() {
        // Trim leading blank lines on continuation pages so every page starts cleanly.
        if i > 0 {
            while i < styled.len() && plain.get(i).map(|s| s.trim().is_empty()).unwrap_or(false) {
                i += 1;
            }
            if i >= styled.len() {
                break;
            }
        }
        let end = (i + page_rows).min(styled.len());
        pages.push(Page {
            title: title.clone(),
            lines: styled[i..end].to_vec(),
            plain: plain[i..end].to_vec(),
        });
        i = end;
    }
    if pages.is_empty() {
        pages.push(Page {
            title,
            lines: vec![String::new()],
            plain: vec![String::new()],
        });
    }
    pages
}

fn apply_page_layout(
    lines: Vec<String>,
    layout: PageLayout,
    page_rows: usize,
    page_cols: usize,
) -> Vec<String> {
    match layout {
        PageLayout::Center => {
            let centered: Vec<String> = lines
                .into_iter()
                .map(|l| pad_ansi(&l, page_cols, Align::Center))
                .collect();
            if centered.len() < page_rows {
                let top = (page_rows - centered.len()) / 2;
                let mut out = vec![String::new(); top];
                out.extend(centered);
                out
            } else {
                centered
            }
        }
        _ => lines,
    }
}

// ------------------------------------------------------------------ inline styling

#[derive(Clone)]
struct Span {
    text: String,
    sgr: &'static str,
}

fn parse_inline(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut cur = String::new();

    let flush = |cur: &mut String, spans: &mut Vec<Span>| {
        if !cur.is_empty() {
            spans.push(Span {
                text: std::mem::take(cur),
                sgr: "",
            });
        }
    };

    while i < chars.len() {
        // Status badges: [OK], [PASS], [DONE], [WARN], [TODO], [ERR], [FAIL], [INFO]
        if chars[i] == '[' {
            let rest: String = chars[i..].iter().collect();
            let badge: Option<(&str, &'static str)> = [
                ("[OK]", "\x1b[1;32m"),
                ("[PASS]", "\x1b[1;32m"),
                ("[DONE]", "\x1b[1;32m"),
                ("[WARN]", "\x1b[1;33m"),
                ("[TODO]", "\x1b[1;33m"),
                ("[ERR]", "\x1b[1;31m"),
                ("[FAIL]", "\x1b[1;31m"),
                ("[INFO]", "\x1b[1;36m"),
            ]
            .into_iter()
            .find(|(tag, _)| rest.starts_with(tag));
            if let Some((tag, sgr)) = badge {
                flush(&mut cur, &mut spans);
                spans.push(Span {
                    text: tag.to_string(),
                    sgr,
                });
                i += tag.chars().count();
                continue;
            }
        }
        // Inline code: `...`
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                if !inner.is_empty() {
                    flush(&mut cur, &mut spans);
                    spans.push(Span {
                        text: inner,
                        sgr: "\x1b[36m",
                    });
                    i += end + 2;
                    continue;
                }
            }
        }
        // Bold: **...**
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            let mut j = i + 2;
            let mut found = None;
            while j + 1 < chars.len() {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[i + 2..end].iter().collect();
                if !inner.is_empty() {
                    flush(&mut cur, &mut spans);
                    spans.push(Span {
                        text: inner,
                        sgr: "\x1b[1;37m",
                    });
                    i = end + 2;
                    continue;
                }
            }
        }
        // Emphasis: *...* (single asterisk, not preceded/followed by space)
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] != ' ' {
            if let Some(rel) = chars[i + 1..].iter().position(|&c| c == '*') {
                let end = i + 1 + rel;
                if end > i + 1 && chars[end - 1] != ' ' {
                    let inner: String = chars[i + 1..end].iter().collect();
                    flush(&mut cur, &mut spans);
                    spans.push(Span {
                        text: inner,
                        sgr: "\x1b[33m",
                    });
                    i = end + 1;
                    continue;
                }
            }
        }
        cur.push(chars[i]);
        i += 1;
    }
    flush(&mut cur, &mut spans);
    spans
}

/// Word-wrap styled inline text with distinct first-line and continuation prefixes.
fn wrap_inline(
    text: &str,
    first_prefix: &str,
    cont_prefix: &str,
    cols: usize,
    base_sgr: &str,
) -> Vec<String> {
    let spans = parse_inline(text);
    // Flatten into (char, sgr) units so wrapping respects CJK widths and words.
    let mut cells: Vec<(char, &'static str)> = Vec::new();
    for sp in spans {
        for c in sp.text.chars() {
            cells.push((c, sp.sgr));
        }
    }
    if cells.is_empty() {
        return vec![clip_ansi(first_prefix, cols)];
    }

    let first_pw = visible_width(first_prefix);
    let cont_pw = visible_width(cont_prefix);
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line_no = 0usize;

    while i < cells.len() {
        let (pref, pw) = if line_no == 0 {
            (first_prefix, first_pw)
        } else {
            (cont_prefix, cont_pw)
        };
        let room = cols.saturating_sub(pw).max(1);
        let mut w = 0usize;
        let mut j = i;
        let mut last_space: Option<usize> = None;
        while j < cells.len() {
            let cw = char_width(cells[j].0);
            if w + cw > room {
                break;
            }
            w += cw;
            if cells[j].0 == ' ' {
                last_space = Some(j + 1);
            }
            j += 1;
        }
        let end = if j >= cells.len() {
            j
        } else {
            match last_space {
                Some(b) if b > i + room / 2 => b,
                _ => j.max(i + 1),
            }
        };
        // Trim trailing spaces on wrapped break.
        let mut trim_end = end;
        while trim_end > i && cells[trim_end - 1].0 == ' ' && end < cells.len() {
            trim_end -= 1;
        }

        let mut line = String::from(pref);
        let mut active_sgr = "";
        if !base_sgr.is_empty() {
            line.push_str(base_sgr);
        }
        for &(ch, sgr) in &cells[i..trim_end] {
            if sgr != active_sgr {
                if !active_sgr.is_empty() {
                    line.push_str("\x1b[0m");
                    if !base_sgr.is_empty() {
                        line.push_str(base_sgr);
                    }
                }
                if !sgr.is_empty() {
                    line.push_str(sgr);
                }
                active_sgr = sgr;
            }
            line.push(ch);
        }
        if !active_sgr.is_empty() || !base_sgr.is_empty() {
            line.push_str("\x1b[0m");
        }
        out.push(line);

        i = end;
        while i < cells.len() && cells[i].0 == ' ' {
            i += 1;
        }
        line_no += 1;
    }
    out
}

// ------------------------------------------------------------------ markdown & layout renderers

fn render_body_for_layout(body: &str, layout: PageLayout, cols: usize, theme: Theme) -> Vec<String> {
    match layout {
        PageLayout::Columns => {
            if let Some((left, right)) = split_columns_text(body) {
                return render_two_columns(&left, &right, 0.5, cols, theme);
            }
            render_markdown(body, cols, theme)
        }
        PageLayout::Kv => {
            let mut items = Vec::new();
            let mut fallback = false;
            for raw in body.lines() {
                let t = raw.trim();
                if t.is_empty() {
                    continue;
                }
                if let Some((k, v)) = t.split_once("::").or_else(|| t.split_once(':')) {
                    items.push((k.trim().to_string(), v.trim().to_string(), None));
                } else {
                    fallback = true;
                    break;
                }
            }
            if !fallback && !items.is_empty() {
                return render_kv_items(&items, cols, theme);
            }
            render_markdown(body, cols, theme)
        }
        _ => render_markdown(body, cols, theme),
    }
}

fn split_columns_text(body: &str) -> Option<(String, String)> {
    if let Some((l, r)) = body.split_once("\n||\n") {
        return Some((l.to_string(), r.to_string()));
    }
    if let Some((l, r)) = body.split_once("\n||") {
        return Some((l.to_string(), r.trim_start_matches('\n').to_string()));
    }
    None
}

fn numbered_prefix(t: &str) -> Option<(&str, &str)> {
    let dot = t.find(". ")?;
    if dot == 0 || dot > 3 {
        return None;
    }
    let num = &t[..dot];
    if num.chars().all(|c| c.is_ascii_digit()) {
        Some((num, &t[dot + 2..]))
    } else {
        None
    }
}

fn is_table_line(t: &str) -> bool {
    t.contains('|') && t.matches('|').count() >= 2
}

fn split_table_row(line: &str) -> Vec<String> {
    let mut s = line.trim();
    if let Some(r) = s.strip_prefix('|') {
        s = r;
    }
    if let Some(r) = s.strip_suffix('|') {
        s = r;
    }
    s.split('|').map(|c| c.trim().to_string()).collect()
}

fn parse_table_sep(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for c in cells {
        let t = c.trim();
        if t.len() < 2 {
            return None;
        }
        let left_colon = t.starts_with(':');
        let right_colon = t.ends_with(':');
        let core = t.trim_matches(':');
        if core.is_empty() || !core.chars().all(|ch| ch == '-') {
            return None;
        }
        aligns.push(match (left_colon, right_colon) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    Some(aligns)
}

fn parse_inline_bar(t: &str) -> Option<(String, f64, f64, String)> {
    // Syntax: `[bar:78%] Label` or `[bar:42/100] Label` or `[bar:78%]`
    let rest = t.strip_prefix("[bar:")?;
    let close = rest.find(']')?;
    let spec = rest[..close].trim();
    let label = rest[close + 1..].trim().to_string();
    if let Some(pct_s) = spec.strip_suffix('%') {
        let v: f64 = pct_s.trim().parse().ok()?;
        return Some((label, v, 100.0, "%".to_string()));
    }
    if let Some((v_s, m_s)) = spec.split_once('/') {
        let v: f64 = v_s.trim().parse().ok()?;
        let m: f64 = m_s.trim().parse().ok()?;
        return Some((label, v, m.max(1.0), String::new()));
    }
    let v: f64 = spec.parse().ok()?;
    Some((label, v, 100.0, "%".to_string()))
}

fn render_markdown(md: &str, cols: usize, theme: Theme) -> Vec<String> {
    let raw_lines: Vec<&str> = md.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < raw_lines.len() {
        let raw = raw_lines[i];
        let trimmed = raw.trim_end();
        let t = trimmed.trim_start();

        // Fenced code block
        if let Some(lang_raw) = t.strip_prefix("```") {
            let lang = lang_raw.trim();
            i += 1;
            let mut code_lines = Vec::new();
            while i < raw_lines.len() {
                if raw_lines[i].trim_start().starts_with("```") {
                    i += 1;
                    break;
                }
                code_lines.push(raw_lines[i]);
                i += 1;
            }
            out.extend(render_code_block(lang, &code_lines.join("\n"), cols));
            continue;
        }

        // Blank line
        if t.is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }

        // Horizontal divider
        if (t.len() >= 3 && t.chars().all(|c| c == '-' || c == '─'))
            || (t.len() >= 3 && t.chars().all(|c| c == '*'))
        {
            out.push(format!("\x1b[90m{}\x1b[0m", "─".repeat(cols)));
            i += 1;
            continue;
        }

        // Markdown table (2+ consecutive pipe rows)
        if is_table_line(t) && i + 1 < raw_lines.len() && is_table_line(raw_lines[i + 1].trim()) {
            let mut rows = Vec::new();
            while i < raw_lines.len() && is_table_line(raw_lines[i].trim()) {
                rows.push(split_table_row(raw_lines[i]));
                i += 1;
            }
            let (headers, aligns, data_rows) = if rows.len() >= 2 {
                if let Some(al) = parse_table_sep(&rows[1]) {
                    (rows[0].clone(), al, rows[2..].to_vec())
                } else {
                    (rows[0].clone(), Vec::new(), rows[1..].to_vec())
                }
            } else {
                (rows[0].clone(), Vec::new(), Vec::new())
            };
            out.extend(render_table(&headers, &data_rows, &aligns, cols, theme));
            continue;
        }

        // Inline progress bar: [bar:78%] Label
        if let Some((label, val, max, unit)) = parse_inline_bar(t) {
            out.push(render_bar_line(&label, val, max, &unit, None, cols));
            i += 1;
            continue;
        }

        // Headings
        if let Some(h) = t.strip_prefix("### ") {
            out.extend(wrap_inline(h.trim(), "", "", cols, "\x1b[1;37m"));
            i += 1;
            continue;
        }
        if let Some(h) = t.strip_prefix("## ") {
            out.extend(wrap_inline(h.trim(), "", "", cols, "\x1b[1;33m"));
            i += 1;
            continue;
        }
        if let Some(h) = t.strip_prefix("# ") {
            out.extend(wrap_inline(h.trim(), "", "", cols, theme.header_sgr()));
            i += 1;
            continue;
        }

        // Callouts / blockquotes
        if let Some(q) = t.strip_prefix("> ") {
            let (bar_sgr, text) = if let Some(r) = q.strip_prefix("[!WARN]") {
                ("\x1b[33m", r.trim())
            } else if let Some(r) = q.strip_prefix("[!ERR]") {
                ("\x1b[31m", r.trim())
            } else if let Some(r) = q.strip_prefix("[!TIP]").or_else(|| q.strip_prefix("[!OK]")) {
                ("\x1b[32m", r.trim())
            } else if let Some(r) = q.strip_prefix("[!NOTE]").or_else(|| q.strip_prefix("[!INFO]")) {
                ("\x1b[36m", r.trim())
            } else {
                (theme.accent_sgr(), q)
            };
            let pref = format!("{bar_sgr}│\x1b[0m ");
            out.extend(wrap_inline(text, &pref, &pref, cols, ""));
            i += 1;
            continue;
        }

        // Checklists
        if let Some(item) = t.strip_prefix("- [x] ").or_else(|| t.strip_prefix("- [X] ")) {
            out.extend(wrap_inline(item, "\x1b[1;32m✓\x1b[0m ", "  ", cols, "\x1b[90m"));
            i += 1;
            continue;
        }
        if let Some(item) = t.strip_prefix("- [ ] ") {
            out.extend(wrap_inline(item, "\x1b[1;33m○\x1b[0m ", "  ", cols, ""));
            i += 1;
            continue;
        }

        // Bullet lists
        if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            let pref = format!("{}•\x1b[0m ", theme.accent_sgr());
            out.extend(wrap_inline(item, &pref, "  ", cols, ""));
            i += 1;
            continue;
        }

        // Numbered lists
        if let Some((num, rest)) = numbered_prefix(t) {
            let pref = format!("{}{num}.\x1b[0m ", theme.accent_sgr());
            let cont = " ".repeat(num.len() + 2);
            out.extend(wrap_inline(rest, &pref, &cont, cols, ""));
            i += 1;
            continue;
        }

        // Key :: Value inline row
        if let Some((k, v)) = t.split_once(" :: ") {
            out.extend(render_kv_items(
                &[(k.trim().to_string(), v.trim().to_string(), None)],
                cols,
                theme,
            ));
            i += 1;
            continue;
        }

        // Normal paragraph line
        out.extend(wrap_inline(trimmed, "", "", cols, ""));
        i += 1;
    }

    out
}

// ------------------------------------------------------------------ structured blocks

fn tone_sgr(tone: Option<&str>, default: &'static str) -> &'static str {
    match tone.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "ok" | "success" | "green" => "\x1b[1;32m",
        "warn" | "warning" | "yellow" => "\x1b[1;33m",
        "err" | "error" | "red" => "\x1b[1;31m",
        "info" | "cyan" => "\x1b[1;36m",
        "dim" | "muted" => "\x1b[90m",
        _ => default,
    }
}

fn render_blocks(blocks: &[J], cols: usize, theme: Theme) -> Vec<String> {
    let mut out = Vec::new();
    for b in blocks {
        let kind = b.get("type").and_then(|v| v.as_str()).unwrap_or("text");
        match kind {
            "heading" => {
                let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let level = b.get("level").and_then(|v| v.as_num()).unwrap_or(1.0) as u32;
                let sgr = match level {
                    1 => theme.header_sgr(),
                    2 => "\x1b[1;33m",
                    _ => "\x1b[1;37m",
                };
                out.extend(wrap_inline(text, "", "", cols, sgr));
            }
            "text" | "paragraph" => {
                let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let align = b
                    .get("align")
                    .and_then(|v| v.as_str())
                    .map(Align::parse)
                    .unwrap_or(Align::Left);
                for raw in text.lines() {
                    let wrapped = wrap_inline(raw, "", "", cols, "");
                    for l in wrapped {
                        out.push(if align == Align::Left {
                            l
                        } else {
                            pad_ansi(&l, cols, align)
                        });
                    }
                }
            }
            "divider" | "hr" => {
                let label = b.get("label").and_then(|v| v.as_str()).unwrap_or("").trim();
                if label.is_empty() {
                    out.push(format!("\x1b[90m{}\x1b[0m", "─".repeat(cols)));
                } else {
                    let lbl = ellipsize_plain(label, cols.saturating_sub(6));
                    let lw = visible_width(&lbl);
                    let rest = cols.saturating_sub(lw + 4);
                    out.push(format!(
                        "\x1b[90m── \x1b[1;37m{lbl}\x1b[90m {}\x1b[0m",
                        "─".repeat(rest)
                    ));
                }
            }
            "spacer" => {
                let n = b.get("lines").and_then(|v| v.as_num()).unwrap_or(1.0).clamp(1.0, 6.0) as usize;
                for _ in 0..n {
                    out.push(String::new());
                }
            }
            "kv" => {
                let mut items = Vec::new();
                if let Some(arr) = b.get("items").and_then(|v| v.as_arr()) {
                    for it in arr {
                        if let Some(pair) = it.as_arr() {
                            if pair.len() >= 2 {
                                let k = pair[0].as_str().unwrap_or("").to_string();
                                let v = json_scalar(&pair[1]);
                                items.push((k, v, None));
                            }
                        } else if matches!(it, J::Obj(_)) {
                            let k = it.get("key").or_else(|| it.get("label")).and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let v = it.get("value").map(json_scalar).unwrap_or_default();
                            let t = it.get("tone").and_then(|x| x.as_str()).map(|s| s.to_string());
                            items.push((k, v, t));
                        }
                    }
                }
                out.extend(render_kv_items(&items, cols, theme));
            }
            "table" => {
                let headers: Vec<String> = b
                    .get("headers")
                    .and_then(|v| v.as_arr())
                    .map(|a| a.iter().map(json_scalar).collect())
                    .unwrap_or_default();
                let rows: Vec<Vec<String>> = b
                    .get("rows")
                    .and_then(|v| v.as_arr())
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| r.as_arr())
                            .map(|r| r.iter().map(json_scalar).collect())
                            .collect()
                    })
                    .unwrap_or_default();
                let aligns: Vec<Align> = b
                    .get("align")
                    .and_then(|v| v.as_arr())
                    .map(|a| a.iter().map(|x| Align::parse(x.as_str().unwrap_or(""))).collect())
                    .unwrap_or_default();
                out.extend(render_table(&headers, &rows, &aligns, cols, theme));
            }
            "list" => {
                let ordered = matches!(b.get("ordered"), Some(J::Bool(true)));
                if let Some(items) = b.get("items").and_then(|v| v.as_arr()) {
                    for (idx, it) in items.iter().enumerate() {
                        let txt = json_scalar(it);
                        if ordered {
                            let num = (idx + 1).to_string();
                            let pref = format!("{}{num}.\x1b[0m ", theme.accent_sgr());
                            let cont = " ".repeat(num.len() + 2);
                            out.extend(wrap_inline(&txt, &pref, &cont, cols, ""));
                        } else {
                            let pref = format!("{}•\x1b[0m ", theme.accent_sgr());
                            out.extend(wrap_inline(&txt, &pref, "  ", cols, ""));
                        }
                    }
                }
            }
            "checklist" => {
                if let Some(items) = b.get("items").and_then(|v| v.as_arr()) {
                    for it in items {
                        let txt = it.get("text").and_then(|x| x.as_str()).unwrap_or("");
                        let done = matches!(it.get("done"), Some(J::Bool(true)));
                        if done {
                            out.extend(wrap_inline(txt, "\x1b[1;32m✓\x1b[0m ", "  ", cols, "\x1b[90m"));
                        } else {
                            out.extend(wrap_inline(txt, "\x1b[1;33m○\x1b[0m ", "  ", cols, ""));
                        }
                    }
                }
            }
            "bar" => {
                let label = b.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let val = b.get("value").and_then(|v| v.as_num()).unwrap_or(0.0);
                let max = b.get("max").and_then(|v| v.as_num()).unwrap_or(100.0);
                let unit = b.get("unit").and_then(|v| v.as_str()).unwrap_or("%");
                let tone = b.get("tone").and_then(|v| v.as_str());
                out.push(render_bar_line(label, val, max, unit, tone, cols));
            }
            "bars" => {
                if let Some(items) = b.get("items").and_then(|v| v.as_arr()) {
                    for it in items {
                        let label = it.get("label").and_then(|v| v.as_str()).unwrap_or("");
                        let val = it.get("value").and_then(|v| v.as_num()).unwrap_or(0.0);
                        let max = it.get("max").and_then(|v| v.as_num()).unwrap_or(100.0);
                        let unit = it.get("unit").and_then(|v| v.as_str()).unwrap_or("%");
                        let tone = it.get("tone").and_then(|v| v.as_str());
                        out.push(render_bar_line(label, val, max, unit, tone, cols));
                    }
                }
            }
            "spark" => {
                let label = b.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let unit = b.get("unit").and_then(|v| v.as_str()).unwrap_or("");
                let vals: Vec<f64> = b
                    .get("values")
                    .and_then(|v| v.as_arr())
                    .map(|a| a.iter().filter_map(|x| x.as_num()).collect())
                    .unwrap_or_default();
                out.push(render_spark_line(label, &vals, unit, cols, theme));
            }
            "callout" => {
                let tone = b.get("tone").and_then(|v| v.as_str());
                let title = b.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let bar = tone_sgr(tone, theme.accent_sgr());
                let pref = format!("{bar}│\x1b[0m ");
                if !title.is_empty() {
                    out.extend(wrap_inline(title, &pref, &pref, cols, bar));
                }
                for line in text.lines() {
                    out.extend(wrap_inline(line, &pref, &pref, cols, ""));
                }
            }
            "code" => {
                let lang = b.get("lang").and_then(|v| v.as_str()).unwrap_or("");
                let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                out.extend(render_code_block(lang, text, cols));
            }
            "columns" => {
                let left = b.get("left").and_then(|v| v.as_str()).unwrap_or("");
                let right = b.get("right").and_then(|v| v.as_str()).unwrap_or("");
                let ratio = b.get("ratio").and_then(|v| v.as_num()).unwrap_or(0.5);
                out.extend(render_two_columns(left, right, ratio, cols, theme));
            }
            "box" => {
                let title = b.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let tone = b.get("tone").and_then(|v| v.as_str());
                out.extend(render_box(title, text, tone, cols, theme));
            }
            _ => {
                if let Some(text) = b.get("text").and_then(|v| v.as_str()) {
                    out.extend(render_markdown(text, cols, theme));
                }
            }
        }
    }
    out
}

fn json_scalar(v: &J) -> String {
    match v {
        J::Str(s) => s.clone(),
        J::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{:.0}", n)
            } else {
                format!("{n}")
            }
        }
        J::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        J::Null => String::new(),
        _ => v.dump(),
    }
}

fn render_kv_items(
    items: &[(String, String, Option<String>)],
    cols: usize,
    theme: Theme,
) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v, tone) in items {
        let val_sgr = tone_sgr(tone.as_deref(), "\x1b[1;37m");
        let kw = visible_width(k);
        let vw = visible_width(v);
        if kw + vw + 2 <= cols {
            let gap = cols - kw - vw;
            let fill = if gap >= 4 {
                format!(" \x1b[90m{}\x1b[0m ", "·".repeat(gap - 2))
            } else {
                " ".repeat(gap)
            };
            out.push(format!("{}{k}\x1b[0m{fill}{val_sgr}{v}\x1b[0m", theme.accent_sgr()));
        } else {
            let k_room = (cols / 2).max(8);
            let k_fit = ellipsize_plain(k, k_room);
            let v_room = cols.saturating_sub(visible_width(&k_fit) + 2);
            let v_fit = ellipsize_plain(v, v_room);
            let gap = cols.saturating_sub(visible_width(&k_fit) + visible_width(&v_fit));
            out.push(format!(
                "{}{k_fit}\x1b[0m{}{val_sgr}{v_fit}\x1b[0m",
                theme.accent_sgr(),
                " ".repeat(gap)
            ));
        }
    }
    out
}

fn render_table(
    headers: &[String],
    rows: &[Vec<String>],
    aligns: &[Align],
    cols: usize,
    theme: Theme,
) -> Vec<String> {
    let ncols = headers
        .len()
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    if ncols == 0 {
        return Vec::new();
    }
    // Use compact ` │ ` (3 cells) when wide enough, else `│` (1 cell).
    let sep_w = if ncols <= 4 && cols >= ncols * 6 { 3 } else { 1 };
    let sep_str = if sep_w == 3 { "\x1b[90m │ \x1b[0m" } else { "\x1b[90m│\x1b[0m" };
    let rule_sep = if sep_w == 3 { "─┼─" } else { "┼" };
    let total_sep = sep_w * (ncols - 1);
    let avail = cols.saturating_sub(total_sep).max(ncols);

    // Measure natural max width per column.
    let mut nat = vec![1usize; ncols];
    for (c, h) in headers.iter().enumerate() {
        nat[c] = nat[c].max(visible_width(h));
    }
    for r in rows {
        for (c, cell) in r.iter().enumerate() {
            nat[c] = nat[c].max(visible_width(cell));
        }
    }

    // Allocate column widths to fit exactly <= avail.
    let sum: usize = nat.iter().sum();
    let mut widths = nat.clone();
    if sum > avail {
        // Proportionally shrink columns while keeping min width 3 (or 1 if very tight).
        let min_w = (avail / ncols).min(3).max(1);
        widths = nat
            .iter()
            .map(|&w| ((w * avail) / sum.max(1)).max(min_w))
            .collect();
        while widths.iter().sum::<usize>() > avail {
            if let Some((idx, _)) = widths.iter().enumerate().max_by_key(|(_, &w)| w) {
                if widths[idx] <= 1 {
                    break;
                }
                widths[idx] -= 1;
            } else {
                break;
            }
        }
    }

    let mut out = Vec::new();
    if !headers.is_empty() {
        let mut hline = String::new();
        for c in 0..ncols {
            if c > 0 {
                hline.push_str(sep_str);
            }
            let txt = headers.get(c).map(|s| s.as_str()).unwrap_or("");
            let al = aligns.get(c).copied().unwrap_or(Align::Left);
            let fit = ellipsize_plain(txt, widths[c]);
            hline.push_str(theme.header_sgr());
            hline.push_str(&pad_ansi(&fit, widths[c], al));
            hline.push_str("\x1b[0m");
        }
        out.push(hline);

        let mut rule = String::from("\x1b[90m");
        for (c, &w) in widths.iter().enumerate() {
            if c > 0 {
                rule.push_str(rule_sep);
            }
            rule.push_str(&"─".repeat(w));
        }
        rule.push_str("\x1b[0m");
        out.push(rule);
    }

    for r in rows {
        let mut line = String::new();
        for c in 0..ncols {
            if c > 0 {
                line.push_str(sep_str);
            }
            let txt = r.get(c).map(|s| s.as_str()).unwrap_or("");
            let al = aligns.get(c).copied().unwrap_or(Align::Left);
            let fit = ellipsize_plain(txt, widths[c]);
            let styled = style_inline_single(&fit);
            line.push_str(&pad_ansi(&styled, widths[c], al));
        }
        out.push(line);
    }
    out
}

fn style_inline_single(text: &str) -> String {
    let spans = parse_inline(text);
    let mut out = String::new();
    for sp in spans {
        if sp.sgr.is_empty() {
            out.push_str(&sp.text);
        } else {
            out.push_str(sp.sgr);
            out.push_str(&sp.text);
            out.push_str("\x1b[0m");
        }
    }
    out
}

fn render_bar_line(
    label: &str,
    val: f64,
    max: f64,
    unit: &str,
    tone: Option<&str>,
    cols: usize,
) -> String {
    let max = if max <= 0.0 { 100.0 } else { max };
    let ratio = (val / max).clamp(0.0, 1.0);
    let val_str = if unit == "%" {
        format!("{:.0}%", ratio * 100.0)
    } else if !unit.is_empty() {
        format!("{:.0}{unit}", val)
    } else {
        format!("{:.0}/{:.0}", val, max)
    };
    let bar_sgr = match tone {
        Some(t) if !t.is_empty() && !t.eq_ignore_ascii_case("auto") => tone_sgr(Some(t), "\x1b[32m"),
        _ => {
            if ratio >= 0.85 {
                "\x1b[31m"
            } else if ratio >= 0.60 {
                "\x1b[33m"
            } else {
                "\x1b[32m"
            }
        }
    };

    let lbl_room = if label.is_empty() {
        0
    } else {
        (cols / 3).clamp(6, 16)
    };
    let lbl_fit = if lbl_room > 0 {
        pad_ansi(&ellipsize_plain(label, lbl_room), lbl_room, Align::Left)
    } else {
        String::new()
    };
    let val_w = visible_width(&val_str);
    let overhead = (if lbl_room > 0 { lbl_room + 1 } else { 0 }) + 1 + val_w;
    let bar_w = cols.saturating_sub(overhead).max(6);
    let filled = ((ratio * bar_w as f64).round() as usize).min(bar_w);
    let empty = bar_w - filled;
    let bar = format!(
        "{bar_sgr}{}\x1b[90m{}\x1b[0m",
        "█".repeat(filled),
        "░".repeat(empty)
    );
    if lbl_room > 0 {
        format!("{lbl_fit} {bar} \x1b[1;37m{val_str}\x1b[0m")
    } else {
        format!("{bar} \x1b[1;37m{val_str}\x1b[0m")
    }
}

fn render_spark_line(
    label: &str,
    values: &[f64],
    unit: &str,
    cols: usize,
    theme: Theme,
) -> String {
    const TICKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() {
        return ellipsize_plain(label, cols);
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).abs();
    let last = *values.last().unwrap_or(&0.0);
    let stat = format!("{:.0}{unit}", last);
    let stat_w = visible_width(&stat);
    let lbl_w = if label.is_empty() {
        0
    } else {
        visible_width(label).min(cols / 3)
    };
    let lbl = if lbl_w > 0 {
        format!("{} ", ellipsize_plain(label, lbl_w))
    } else {
        String::new()
    };
    let room = cols.saturating_sub(visible_width(&lbl) + stat_w + 1).max(4);
    let sample: &[f64] = if values.len() > room {
        &values[values.len() - room..]
    } else {
        values
    };
    let mut spark = String::new();
    for &v in sample {
        let idx = if span < 1e-9 {
            3
        } else {
            (((v - min) / span) * 7.0).round().clamp(0.0, 7.0) as usize
        };
        spark.push(TICKS[idx]);
    }
    let gap = cols.saturating_sub(visible_width(&lbl) + sample.len() + stat_w);
    format!(
        "\x1b[90m{lbl}\x1b[0m{}{spark}\x1b[0m{}\x1b[1;37m{stat}\x1b[0m",
        theme.accent_sgr(),
        " ".repeat(gap)
    )
}

fn render_code_block(lang: &str, text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    if !lang.is_empty() {
        let tag = ellipsize_plain(lang, cols.saturating_sub(6));
        let rest = cols.saturating_sub(visible_width(&tag) + 4);
        out.push(format!("\x1b[90m┌ {tag} {}\x1b[0m", "─".repeat(rest)));
    }
    let inner_w = cols.saturating_sub(2).max(8);
    for raw in text.lines() {
        let chunks = wrap_plain(raw, inner_w);
        if chunks.is_empty() {
            out.push("\x1b[90m│\x1b[0m".to_string());
        } else {
            for c in chunks {
                out.push(format!("\x1b[90m│\x1b[0m \x1b[36m{c}\x1b[0m"));
            }
        }
    }
    out
}

fn render_two_columns(
    left: &str,
    right: &str,
    ratio: f64,
    cols: usize,
    theme: Theme,
) -> Vec<String> {
    let ratio = ratio.clamp(0.25, 0.75);
    let inner = cols.saturating_sub(3).max(8);
    let lw = ((inner as f64 * ratio).round() as usize).clamp(4, inner.saturating_sub(4).max(4));
    let rw = inner.saturating_sub(lw).max(4);
    let left_lines = render_markdown(left, lw, theme);
    let right_lines = render_markdown(right, rw, theme);
    let rows = left_lines.len().max(right_lines.len());
    let mut out = Vec::with_capacity(rows);
    for i in 0..rows {
        let l = left_lines.get(i).map(|s| s.as_str()).unwrap_or("");
        let r = right_lines.get(i).map(|s| s.as_str()).unwrap_or("");
        out.push(format!(
            "{}\x1b[90m │ \x1b[0m{}",
            pad_ansi(l, lw, Align::Left),
            pad_ansi(r, rw, Align::Left)
        ));
    }
    out
}

fn render_box(
    title: &str,
    text: &str,
    tone: Option<&str>,
    cols: usize,
    theme: Theme,
) -> Vec<String> {
    let cols = cols.max(8);
    let sgr = tone_sgr(tone, theme.accent_sgr());
    let inner_w = cols - 4;
    let mut out = Vec::new();
    if title.is_empty() {
        out.push(format!("{sgr}┌{}┐\x1b[0m", "─".repeat(cols - 2)));
    } else {
        let t = ellipsize_plain(title, cols.saturating_sub(6));
        let tw = visible_width(&t);
        let right = cols.saturating_sub(tw + 5);
        out.push(format!(
            "{sgr}┌─ \x1b[1;37m{t}{sgr} {}┐\x1b[0m",
            "─".repeat(right)
        ));
    }
    let body_lines = render_markdown(text, inner_w, theme);
    for l in body_lines {
        out.push(format!(
            "{sgr}│\x1b[0m {} {sgr}│\x1b[0m",
            pad_ansi(&l, inner_w, Align::Left)
        ));
    }
    out.push(format!("{sgr}└{}┘\x1b[0m", "─".repeat(cols - 2)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn fixed_size_pages_never_exceed_dimensions() {
        let body = (1..=25)
            .map(|i| format!("step {i}: verify subsystem {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let a = Artifact::from_markdown("Checklist", &body, 10, 53);
        assert_eq!(a.total_pages(), 3);
        for p in &a.pages {
            assert!(p.lines.len() <= 10, "page height exceeded: {}", p.lines.len());
            for l in &p.lines {
                assert!(visible_width(l) <= 53, "line overflowed 53 cells: {l:?}");
            }
        }
        assert!(a.save_text.contains("step 1:"));
        assert!(a.save_text.contains("step 25:"));
        assert!(!a.save_text.contains("\x1b["), "saved text must be ANSI-free");
    }

    #[test]
    fn explicit_page_breaks_split_pages_with_titles() {
        let md = "# Overview\nFirst page\n=== Metrics\nCPU :: 42%\nRAM :: 180MB\n===\nFinal page";
        let a = Artifact::from_markdown("Report", md, 10, 53);
        assert_eq!(a.total_pages(), 3);
        assert_eq!(a.pages[0].title, None);
        assert_eq!(a.pages[1].title.as_deref(), Some("Metrics"));
        assert_eq!(a.pages[2].title, None);
        assert!(a.pages[1].plain.iter().any(|l| l.contains("CPU") && l.contains("42%")));
    }

    #[test]
    fn structured_pages_and_blocks_render_within_panel() {
        let v = parse(
            r#"{
                "title": "Device Dashboard",
                "theme": "info",
                "pages": [
                    {
                        "title": "System",
                        "blocks": [
                            {"type": "kv", "items": [["Host", "cardputer"], ["RAM", "180/446 MB"]]},
                            {"type": "bar", "label": "Battery", "value": 78, "max": 100, "unit": "%"},
                            {"type": "spark", "label": "CPU", "values": [10, 25, 50, 75, 40], "unit": "%"}
                        ]
                    },
                    {
                        "title": "Compare",
                        "blocks": [
                            {"type": "columns", "left": "**Before**\n- slow\n- 120ms", "right": "**After**\n- fast\n- 12ms"},
                            {"type": "table", "headers": ["Name", "Size", "OK"], "rows": [["dst", "1.4M", "yes"], ["cfg", "2K", "yes"]], "align": ["left", "right", "center"]}
                        ]
                    },
                    {
                        "title": "Summary",
                        "layout": "center",
                        "blocks": [
                            {"type": "box", "title": "Ready", "text": "[OK] All checks passed", "tone": "ok"}
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();
        let res = Artifact::from_tool_json(&v, 10, 53).unwrap();
        assert_eq!(res.overflowed_pages, 0);
        assert_eq!(res.artifact.total_pages(), 3);
        assert_eq!(res.artifact.theme, Theme::Info);
        for (pi, p) in res.artifact.pages.iter().enumerate() {
            assert!(p.lines.len() <= 10, "page {pi} has {} lines", p.lines.len());
            for l in &p.lines {
                let w = visible_width(l);
                assert!(w <= 53, "page {pi} line exceeds 53 cells ({w}): {:?}", strip_ansi(l));
            }
        }
        assert!(res.artifact.pages[0].plain.iter().any(|l| l.contains("Battery") && l.contains("78%")));
        assert!(res.artifact.pages[1].plain.iter().any(|l| l.contains("Before") && l.contains("After")));
        assert!(res.artifact.pages[2].plain.iter().any(|l| l.contains("All checks passed")));
    }

    #[test]
    fn navigation_flips_whole_pages() {
        let md = "p1\n===\np2\n===\np3";
        let mut a = Artifact::from_markdown("Deck", md, 10, 53);
        assert_eq!(a.page, 0);
        assert!(!a.prev_page());
        assert!(a.next_page());
        assert_eq!(a.page, 1);
        assert!(a.last_page());
        assert_eq!(a.page, 2);
        assert!(!a.next_page());
        assert!(a.goto_page(0));
        assert_eq!(a.page, 0);
    }

    #[test]
    fn cjk_tables_and_wrapping_respect_cell_widths() {
        let md = "| 项目 | 状态 | 数值 |\n| --- | :---: | ---: |\n| 电池电量 | [OK] | 85% |\n| 无线网络信号强度测试 | [WARN] | -72dBm |";
        let a = Artifact::from_markdown("硬件状态", md, 10, 53);
        assert_eq!(a.total_pages(), 1);
        for l in &a.pages[0].lines {
            let w = visible_width(l);
            assert!(w <= 53, "CJK table row overflowed ({w}): {:?}", strip_ansi(l));
        }
    }
}
