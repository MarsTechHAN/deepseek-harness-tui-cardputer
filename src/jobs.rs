//! cron + watcher scheduling for dsh-lite (no subagents by design).
//!
//! Jobs live in `~/.dst/jobs.json`:
//!   {"jobs":[{"kind":"cron","spec":"*/5 * * * *","prompt":"...","last":0},
//!            {"kind":"watch","spec":"/path/to/file","prompt":"...","last":0}]}
//!
//! A job's prompt is run as a normal harness turn (same tool-call loop), while
//! the TUI is idle.  `last` is the last minute stamp (cron) or mtime (watch).

use crate::api::Cfg;
use crate::json::{parse, J};
use crate::tool::{self, Call, NullHost, ToolDef};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

pub type Msgs = Rc<RefCell<Vec<J>>>;

fn path() -> String {
    crate::config::jobs_path().to_string_lossy().to_string()
}

fn load() -> Vec<(String, String, String, u64)> {
    let mut out = Vec::new();
    if let Ok(s) = std::fs::read_to_string(path()) {
        if let Some(v) = parse(&s) {
            if let Some(arr) = v.get("jobs").and_then(|a| a.as_arr()) {
                for j in arr {
                    let kind = j.get("kind").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let spec = j.get("spec").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let prompt = j.get("prompt").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let last = j.get("last").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
                    if !kind.is_empty() && !prompt.is_empty() {
                        out.push((kind, spec, prompt, last));
                    }
                }
            }
        }
    }
    out
}

fn save(jobs: &[(String, String, String, u64)]) {
    let arr: Vec<J> = jobs
        .iter()
        .map(|(k, s, p, l)| {
            J::o(vec![
                ("kind", J::s(k)),
                ("spec", J::s(s)),
                ("prompt", J::s(p)),
                ("last", J::Num(*l as f64)),
            ])
        })
        .collect();
    let body = J::o(vec![("jobs", J::Arr(arr))]).dump();
    let _ = std::fs::create_dir_all(
        std::path::Path::new(&path()).parent().unwrap_or(std::path::Path::new("/tmp")),
    );
    let _ = std::fs::write(path(), body);
}

// ------------------------------------------------------------------ cron

/// current local time as (min, hour, day, month, weekday)
fn now_parts() -> Option<(u32, u32, u32, u32, u32)> {
    let o = std::process::Command::new("date")
        .arg("+%M %H %d %m %w")
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout);
    let v: Vec<u32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() == 5 {
        Some((v[0], v[1], v[2], v[3], v[4]))
    } else {
        None
    }
}

fn field_match(f: &str, v: u32) -> bool {
    for part in f.split(',') {
        if part == "*" {
            return true;
        }
        if let Some(step) = part.strip_prefix("*/") {
            if let Ok(n) = step.parse::<u32>() {
                if n > 0 && v % n == 0 {
                    return true;
                }
            }
        } else if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                if v >= a && v <= b {
                    return true;
                }
            }
        } else if let Ok(n) = part.parse::<u32>() {
            if n == v {
                return true;
            }
        }
    }
    false
}

fn cron_match(spec: &str, t: (u32, u32, u32, u32, u32)) -> bool {
    let f: Vec<&str> = spec.split_whitespace().collect();
    if f.len() != 5 {
        return false;
    }
    field_match(f[0], t.0) && field_match(f[1], t.1) && field_match(f[2], t.2)
        && field_match(f[3], t.3) && field_match(f[4], t.4)
}

fn minute_stamp() -> u64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs / 60
}

fn mtime(path: &str) -> Option<u64> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

// ------------------------------------------------------------------ tools

pub fn tool_exec(action: &str, kind: &str, spec: &str, prompt: &str) -> String {
    let mut jobs = load();
    match action {
        "add" => {
            if !kind.eq("cron") && !kind.eq("watch") {
                return "error: kind must be cron or watch".into();
            }
            if kind == "cron" && spec.split_whitespace().count() != 5 {
                return "error: cron spec must be 5 fields, e.g. \"*/5 * * * *\"".into();
            }
            if kind == "watch" && !std::path::Path::new(spec).exists() {
                return format!("error: watch path does not exist: {spec}");
            }
            jobs.push((
                kind.to_string(),
                spec.to_string(),
                prompt.to_string(),
                0,
            ));
            save(&jobs);
            format!("added {kind} job ({spec}) — {} job(s) total", jobs.len())
        }
        "list" => {
            if jobs.is_empty() {
                return "(no jobs)".into();
            }
            jobs.iter()
                .enumerate()
                .map(|(i, (k, s, p, l))| {
                    let short: String = p.chars().take(50).collect();
                    format!("{i}. {k} {s}  last={l}  \"{short}\"")
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "remove" => {
            let idx: usize = match spec.trim().parse() {
                Ok(i) => i,
                Err(_) => return "error: pass the job index to remove (see action=list)".into(),
            };
            if idx >= jobs.len() {
                return "error: index out of range".into();
            }
            let (k, s, _, _) = jobs.remove(idx);
            save(&jobs);
            format!("removed {k} {s}")
        }
        other => format!("error: unknown action '{other}' (add/list/remove)"),
    }
}

// ------------------------------------------------------------- scheduler

/// Compact settings for scheduler turns, installed by main from the live
/// config. Cron/watcher turns write into the shared history unattended, so
/// they must respect the same compaction limit as a human turn or the history
/// grows with wall-clock time.
static COMPACT: std::sync::Mutex<Option<crate::compact::Settings>> = std::sync::Mutex::new(None);

pub fn set_compact_settings(s: crate::compact::Settings) {
    if let Ok(mut g) = COMPACT.lock() {
        *g = Some(s);
    }
}

/// Minimal `Ui` for compaction/messages started outside the TUI pump.
struct CronUi;
impl crate::harness::Ui for CronUi {
    fn on(&mut self, _ev: crate::harness::Ev) {}
    fn pump(&mut self) -> crate::harness::Action {
        crate::harness::Action::Continue
    }
    fn confirm(&mut self, _request: &crate::sandbox::Approval) -> bool {
        false
    }
}

/// Hard ceiling independent of token accounting: if compaction is disabled or
/// cannot shrink the history, dropping the oldest tail keeps the process
/// bounded instead of growing until the device OOMs.
const MAX_CRON_MESSAGES: usize = 600;

fn trim_history(msgs: &mut Vec<J>) {
    if msgs.len() <= MAX_CRON_MESSAGES {
        return;
    }
    let drop = msgs.len() - MAX_CRON_MESSAGES;
    // Keep the system prompt; never start the tail with an orphan tool result.
    let mut cut = 1 + drop;
    while cut < msgs.len()
        && msgs
            .get(cut)
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            == Some("tool")
    {
        cut += 1;
    }
    msgs.drain(1..cut.min(msgs.len()));
}

fn run_prompt(
    cfg: &Cfg,
    reg: &[ToolDef],
    tdefs: &J,
    msgs: &Msgs,
    scr: &mut crate::Scr,
    prompt: &str,
    tag: &str,
    window: u64,
) {
    scr.push(&format!("⟳ {tag}: {prompt}"), crate::K_TOOL);
    msgs.borrow_mut().push(J::o(vec![
        ("role", J::s("user")),
        ("content", J::s(prompt)),
    ]));
    let compact = COMPACT.lock().ok().and_then(|g| g.clone());
    for _ in 0..6 {
        {
            let mut m = msgs.borrow_mut();
            if let Some(set) = &compact {
                let mut ui = CronUi;
                let _ = crate::compact::maybe_compact(cfg, set, &mut m, window, tdefs, &mut ui);
            }
            trim_history(&mut m);
        }
        let snapshot = crate::harness::serialize_msgs(&msgs.borrow());
        let mut acc = String::new();
        let r = crate::api::chat_stream_json(cfg, &snapshot, tdefs, |d| acc.push_str(d));
        match r {
            Ok(out) => {
                if !acc.trim().is_empty() {
                    scr.push(acc.trim(), crate::K_NORM);
                }
                if out.calls.is_empty() {
                    msgs.borrow_mut().push(J::o(vec![
                        ("role", J::s("assistant")),
                        ("content", J::s(&acc)),
                    ]));
                    break;
                }
                msgs.borrow_mut().push(assistant(&acc, &out.calls));
                let calls: Vec<Call> = out
                    .calls
                    .iter()
                    .map(|c| {
                        scr.push(&format!("⚙ {}({})", c.name, short(&c.args, 50)), crate::K_TOOL);
                        Call {
                            id: c.id.clone(),
                            name: c.name.clone(),
                            raw_args: c.args.clone(),
                        }
                    })
                    .collect();
                let mut host = NullHost;
                let outcomes = tool::run_batch(reg, &calls, &mut host);
                for o in outcomes {
                    scr.push(
                        &format!("  ↳ {}", o.content.lines().take(2).collect::<Vec<_>>().join(" ")),
                        crate::K_RES,
                    );
                    msgs.borrow_mut().push(J::o(vec![
                        ("role", J::s("tool")),
                        ("tool_call_id", J::s(&o.call_id)),
                        ("content", J::s(&o.content)),
                    ]));
                    if let Some(ctx) = o.context {
                        msgs.borrow_mut().push(J::o(vec![
                            ("role", J::s("user")),
                            ("content", J::s(&ctx)),
                        ]));
                    }
                }
            }
            Err(e) => {
                scr.push(&crate::i18n::tf("ui_job_error", &[&e.to_string()]), crate::K_ERR);
                break;
            }
        }
    }
}

fn assistant(text: &str, calls: &[crate::api::ToolCall]) -> J {
    let arr: Vec<J> = calls
        .iter()
        .map(|c| {
            J::o(vec![
                ("id", J::s(&c.id)),
                ("type", J::s("function")),
                (
                    "function",
                    J::o(vec![("name", J::s(&c.name)), ("arguments", J::s(&c.args))]),
                ),
            ])
        })
        .collect();
    J::o(vec![
        ("role", J::s("assistant")),
        ("content", if text.is_empty() { J::Null } else { J::s(text) }),
        ("tool_calls", J::Arr(arr)),
    ])
}

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect::<String>().replace('\n', " ")
}

/// Called from the TUI idle loop. Fires at most one due job per call.
pub fn poll(cfg: &Cfg, reg: &[ToolDef], tdefs: &J, msgs: &Msgs, scr: &mut crate::Scr) {
    let window = scr.ctx_window;
    let mut jobs = load();
    if jobs.is_empty() {
        return;
    }
    let now = match now_parts() {
        Some(t) => t,
        None => return,
    };
    let stamp = minute_stamp();
    let mut fired: Option<usize> = None;
    for (i, (kind, spec, _prompt, last)) in jobs.iter().enumerate() {
        if kind == "cron" {
            if cron_match(spec, now) && *last != stamp {
                fired = Some(i);
                break;
            }
        } else {
            if let Some(m) = mtime(spec) {
                if m != *last {
                    fired = Some(i);
                    break;
                }
            }
        }
    }
    let i = match fired {
        Some(i) => i,
        None => return,
    };
    let (kind, spec, prompt, _) = jobs[i].clone();
    if kind == "cron" {
        jobs[i].3 = stamp;
    } else {
        jobs[i].3 = mtime(&spec).unwrap_or(0);
    }
    save(&jobs);
    let tag = if kind == "cron" { "cron" } else { "watch" };
    run_prompt(cfg, reg, tdefs, msgs, scr, &prompt, &format!("{tag} {spec}"), window);
}
