//! Cumulative token usage, in the spirit of Claude Code's `/usage`.
//!
//! DeepSeek reports input tokens split into cache hits and misses, so the
//! meaningful numbers are: cache-hit rate, uncached input, cache read, output.
//! Totals persist in `~/.dst/usage.json` across runs.

use crate::api::Usage;
use crate::json::{parse, J};
use std::sync::Mutex;

#[derive(Default, Clone)]
pub struct Stats {
    pub requests: u64,
    pub prompt: u64,
    pub completion: u64,
    pub cache_hit: u64,
    pub cache_miss: u64,
    /// Prompt size of the most recent request (context-pressure estimate).
    pub last_prompt: u64,
    /// Output size of the most recent request: it is re-sent in the next
    /// request too, so context pressure is prompt + completion.
    pub last_completion: u64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    requests: 0,
    prompt: 0,
    completion: 0,
    cache_hit: 0,
    cache_miss: 0,
    last_prompt: 0,
    last_completion: 0,
});

fn path() -> std::path::PathBuf {
    crate::config::home().join("usage.json")
}

pub fn load() {
    if let Ok(t) = std::fs::read_to_string(path()) {
        if let Some(v) = parse(&t) {
            let num = |k: &str| v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
            if let Ok(mut g) = STATS.lock() {
                *g = Stats {
                    requests: num("requests"),
                    prompt: num("prompt"),
                    completion: num("completion"),
                    cache_hit: num("cache_hit"),
                    cache_miss: num("cache_miss"),
                    last_prompt: num("last_prompt"),
                    last_completion: num("last_completion"),
                };
            }
        }
    }
}

fn save(g: &Stats) {
    let doc = J::o(vec![
        ("requests", J::Num(g.requests as f64)),
        ("prompt", J::Num(g.prompt as f64)),
        ("completion", J::Num(g.completion as f64)),
        ("cache_hit", J::Num(g.cache_hit as f64)),
        ("cache_miss", J::Num(g.cache_miss as f64)),
        ("last_prompt", J::Num(g.last_prompt as f64)),
        ("last_completion", J::Num(g.last_completion as f64)),
    ]);
    let _ = std::fs::write(path(), doc.dump());
}

pub fn record(u: &Usage) {
    if let Ok(mut g) = STATS.lock() {
        g.requests += 1;
        g.prompt += u.prompt;
        g.completion += u.completion;
        g.cache_hit += u.cache_hit;
        g.cache_miss += u.cache_miss;
        g.last_prompt = u.prompt;
        g.last_completion = u.completion;
        save(&g);
    }
}

pub fn record_opt(u: Option<&Usage>) {
    if let Some(u) = u {
        record(u);
    }
}

pub fn snapshot() -> Stats {
    STATS.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Tokens the *next* request will carry before it is even built: the last
/// prompt plus the last completion (which is now part of the history).
pub fn last_context_tokens() -> u64 {
    STATS
        .lock()
        .map(|g| g.last_prompt + g.last_completion)
        .unwrap_or(0)
}

fn bar(frac: f64, width: usize) -> String {
    let filled = ((frac.clamp(0.0, 1.0)) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width.saturating_sub(filled)))
}

fn human(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// Display width in terminal cells (CJK counts as two).
fn dw(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let u = c as u32;
            if (0x1100..=0x115F).contains(&u)
                || (0x2E80..=0xA4CF).contains(&u)
                || (0xAC00..=0xD7A3).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
                || (0xFF00..=0xFF60).contains(&u)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

fn row(label: &str, value: &str, bar: Option<String>, label_w: usize) -> String {
    let pad = label_w.saturating_sub(dw(label));
    let mut s = format!("{label}{} {:>7}", " ".repeat(pad), value);
    if let Some(b) = bar {
        s.push_str("  ");
        s.push_str(&b);
    }
    s
}

/// `/usage` panel: totals, cache-hit rate and a share bar per bucket.
pub fn report() -> Vec<String> {
    let g = snapshot();
    let input_total = g.cache_hit + g.cache_miss;
    let hit_rate = if input_total > 0 {
        g.cache_hit as f64 / input_total as f64
    } else {
        0.0
    };
    let total = (input_total + g.completion).max(1);
    let l_cache = crate::i18n::t("usage_cache_rate");
    let l_read = crate::i18n::t("usage_cache_read");
    let l_unc = crate::i18n::t("usage_uncached");
    let l_out = crate::i18n::t("usage_output");
    let l_in = crate::i18n::t("usage_input_total");
    // One bar width for every row, and a label column wide enough for the
    // longest translated label, so the bars line up in both languages.
    let label_w = [l_cache, l_read, l_unc, l_out, l_in]
        .iter()
        .map(|l| dw(l))
        .max()
        .unwrap_or(12)
        .max(11);
    let mut out = Vec::new();
    out.push(format!(
        "{} · {}",
        crate::i18n::t("usage_title"),
        crate::i18n::tf("usage_requests", &[&g.requests.to_string()])
    ));
    out.push(String::new());
    out.push(row(
        l_cache,
        &format!("{:.1}%", hit_rate * 100.0),
        Some(bar(hit_rate, 10)),
        label_w,
    ));
    out.push(row(
        l_read,
        &human(g.cache_hit),
        Some(bar(g.cache_hit as f64 / total as f64, 10)),
        label_w,
    ));
    out.push(row(
        l_unc,
        &human(g.cache_miss),
        Some(bar(g.cache_miss as f64 / total as f64, 10)),
        label_w,
    ));
    out.push(row(
        l_out,
        &human(g.completion),
        Some(bar(g.completion as f64 / total as f64, 10)),
        label_w,
    ));
    out.push(row(l_in, &human(input_total), None, label_w));
    out.push(String::new());
    out.push(crate::i18n::tf("usage_context", &[&human(g.last_prompt)]));
    out
}

/// Percentage of the context window used by the most recent request.
pub fn context_pct(window: u64) -> f64 {
    if window == 0 {
        return 0.0;
    }
    let last = last_context_tokens();
    (last as f64 / window as f64).clamp(0.0, 1.0)
}

/// Small ring glyph for the status line.
pub fn ring(pct: f64) -> &'static str {
    match (pct * 4.0).round() as u32 {
        0 => "○",
        1 => "◔",
        2 => "◑",
        3 => "◕",
        _ => "●",
    }
}
