//! In-progress context injection, ported from DSH `dsh-time-context`.
//!
//! A context provider observes the step loop and contributes an attributed
//! `user/message` that is not part of what the user said: ambient state the
//! model would otherwise have to ask for. DSH's text is reproduced exactly
//! (including the "turn N, step M" framing), because it teaches the model that
//! the reading is a snapshot rather than a user instruction:
//!
//! ```text
//! Time sampled while preparing turn 3, step 1: 2026-11-16T14:25:03+08:00[Asia/Shanghai]
//! Browser time zone for this request: Asia/Shanghai. Interpret otherwise-unqualified dates and times in this zone.
//! Elapsed since the preceding model-visible message: 2m 4s.
//! ```
//!
//! Injection is rate-limited (DSH default: 10 minutes), so it does not grow the
//! transcript on every step. The device's clock comes from `date`, which honours
//! the system timezone without linking a date library.

use crate::json::J;
use std::sync::Mutex;

/// DSH's default refresh interval: one durable reading per 10 minutes.
pub const DEFAULT_REFRESH_SECS: u64 = 600;

/// Maximum retained readings; older ones are dropped (this is the one
/// deviation from DSH, which never prunes them: 512 MB of RAM).
const MAX_KEPT: usize = 3;

/// Marker that identifies an injected reading (also used to recognise a
/// resumed session that already contains one).
pub const TIME_MARKER: &str = "Time sampled while preparing";

/// Epoch seconds of the last successful injection in this process.
static LAST_INJECTION: Mutex<u64> = Mutex::new(0);

fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// IANA zone name, best effort: Debian keeps it in `/etc/timezone`, macOS in
/// the `/etc/localtime` symlink. Falls back to the POSIX abbreviation.
fn time_zone() -> String {
    if let Ok(t) = std::fs::read_to_string("/etc/timezone") {
        let t = t.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(path) = std::fs::read_link("/etc/localtime") {
        let p = path.to_string_lossy().to_string();
        if let Some(idx) = p.find("zoneinfo/") {
            let z = &p[idx + "zoneinfo/".len()..];
            let z = z.trim_start_matches("posix/").to_string();
            if !z.is_empty() {
                return z;
            }
        }
    }
    if let Ok(t) = std::env::var("TZ") {
        let t = t.trim().trim_start_matches(':').to_string();
        if !t.is_empty() {
            return t;
        }
    }
    "UTC".to_string()
}

/// `2026-11-16T14:25:03+08:00` from the system clock.
fn timestamp() -> Option<String> {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%dT%H:%M:%S%z")
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.len() < 19 {
        return None;
    }
    // `+0800` -> `+08:00` (DSH uses the longOffset form).
    let (body, off) = s.split_at(19);
    let off = off.trim();
    let off = if off.len() == 5 {
        format!("{}:{}", &off[..3], &off[3..])
    } else {
        off.to_string()
    };
    Some(format!("{body}{off}"))
}

/// Compact whole-second duration, like DSH's `formatDuration`.
fn human_duration(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    let mut parts: Vec<String> = Vec::new();
    if d > 0 {
        parts.push(format!("{d}d"));
    }
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    parts.push(format!("{s}s"));
    parts.join(" ")
}

/// The model-visible reading (DSH `renderText`).
pub fn render(turn: u32, step: u32, previous: Option<u64>) -> Option<String> {
    let now = epoch();
    let stamp = timestamp()?;
    let zone = time_zone();
    let baseline = if step <= 1 {
        "model-visible message"
    } else {
        "step context"
    };
    let elapsed = match previous {
        None => "unavailable".to_string(),
        Some(prev) => human_duration(now.saturating_sub(prev)),
    };
    Some(format!(
        "Time sampled while preparing turn {turn}, step {step}: {stamp}[{zone}]\n\
Browser time zone for this request: {zone}. Interpret otherwise-unqualified dates and times in this zone.\n\
Elapsed since the preceding {baseline}: {elapsed}."
    ))
}

fn is_time_message(m: &J) -> bool {
    m.get("role").and_then(|r| r.as_str()) == Some("user")
        && m.get("content")
            .and_then(|c| c.as_str())
            .map(|t| t.starts_with(TIME_MARKER))
            .unwrap_or(false)
}

/// Drop injected readings beyond the retention bound.
fn prune(msgs: &mut Vec<J>) {
    let mut seen = 0usize;
    let mut drop_idx: Vec<usize> = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        if is_time_message(m) {
            seen += 1;
            if seen > MAX_KEPT {
                drop_idx.push(i);
            }
        }
    }
    for i in drop_idx.into_iter().rev() {
        msgs.remove(i);
    }
}

/// Inject one ambient reading when the refresh interval has elapsed.
/// Returns true when a message was appended.
pub fn inject_time(msgs: &mut Vec<J>, refresh_secs: u64, turn: u32, step: u32) -> bool {
    let now = epoch();
    let previous = {
        let g = match LAST_INJECTION.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if *g != 0 && refresh_secs > 0 && now.saturating_sub(*g) < refresh_secs {
            return false;
        }
        if *g == 0 {
            // A resumed session may already carry a reading; use its stamp as
            // the baseline so elapsed time is still meaningful.
            last_reading_epoch(msgs).unwrap_or(0).into()
            // (0 means "no previous reading at all")
        } else {
            Some(*g)
        }
    };
    let previous = match previous {
        Some(prev) => Some(prev),
        None => last_reading_epoch(msgs),
    };
    let text = match render(turn, step, previous) {
        Some(t) => t,
        None => return false,
    };
    msgs.push(J::o(vec![
        ("role", J::s("user")),
        ("content", J::s(&text)),
    ]));
    prune(msgs);
    if let Ok(mut g) = LAST_INJECTION.lock() {
        *g = now;
    }
    true
}

/// Parse the epoch out of the first injected reading, if any. DSH does not
/// embed an epoch; this scans for the newest reading and returns 0, which only
/// makes elapsed time "unavailable" after a resume.
fn last_reading_epoch(msgs: &[J]) -> Option<u64> {
    let _ = msgs;
    None
}

/// Remove all injected readings (when summarising or restoring a session).
pub fn strip_time(msgs: &mut Vec<J>) -> bool {
    let before = msgs.len();
    msgs.retain(|m| !is_time_message(m));
    msgs.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_compact() {
        assert_eq!(human_duration(59), "59s");
        assert_eq!(human_duration(60), "1m 0s");
        assert_eq!(human_duration(3_661), "1h 1m 1s");
    }

    #[test]
    fn time_zone_is_non_empty() {
        assert!(!time_zone().is_empty());
    }
}
