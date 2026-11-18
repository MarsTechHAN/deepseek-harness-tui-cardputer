//! Turn checkpoints for /rewind (Claude Code / DSH checkpoint idea).
use crate::json::{self, J};
use std::path::PathBuf;

fn dir() -> PathBuf { crate::config::home().join("checkpoints") }

fn stamp() -> String {
    std::process::Command::new("date").arg("+%Y%m%d-%H%M%S").output()
        .ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{}", std::process::id()))
}

/// Snapshot the current transcript before a turn starts. Keeps the newest 20.
pub fn save(msgs: &[J], title: &str) {
    let _ = std::fs::create_dir_all(dir());
    let preview = msgs.iter().rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .and_then(|m| m.get("content").and_then(|c| c.as_str()))
        .unwrap_or("").chars().take(60).collect::<String>();
    let created = J::Num(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0));
    // Write the transcript straight into the document instead of cloning the
    // whole message array first (checkpoints are full-history snapshots).
    let doc = json::doc_messages(
        &[("title", J::s(title)), ("preview", J::s(&preview)), ("created", created)],
        msgs,
    );
    let path = dir().join(format!("{}.json", stamp()));
    let _ = std::fs::write(&path, doc);
    let mut all = list_paths();
    all.sort();
    while all.len() > 20 {
        let _ = std::fs::remove_file(all.remove(0));
    }
}

fn list_paths() -> Vec<PathBuf> {
    std::fs::read_dir(dir()).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json")).collect())
        .unwrap_or_default()
}

pub struct Checkpoint { pub path: PathBuf, pub label: String }

pub fn list() -> Vec<Checkpoint> {
    let mut out: Vec<(PathBuf, u64, Checkpoint)> = list_paths().into_iter().filter_map(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        let v = json::parse(&text)?;
        let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let preview = v.get("preview").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let created = v.get("created").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let label = if !title.is_empty() {
            title
        } else if !preview.is_empty() {
            preview.chars().take(44).collect::<String>()
        } else {
            "(session start)".to_string()
        };
        let _ = stem;
        Some((p.clone(), created, Checkpoint { path: p, label }))
    }).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out.into_iter().map(|(_, _, c)| c).collect()
}

/// Human preview lines for the rewind picker.
pub fn preview(path: &std::path::Path) -> Vec<String> {
    let (title, messages) = match load(path) {
        Some(v) => v,
        None => return vec!["(unreadable checkpoint)".to_string()],
    };
    let mut out = vec![format!("title: {title}")];
    let mut recent: Vec<(bool, String)> = Vec::new();
    for m in messages.iter().rev() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let text = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if text.trim().is_empty() || (role != "user" && role != "assistant") {
            continue;
        }
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        recent.push((role == "user", flat.chars().take(40).collect()));
        if recent.len() >= 3 {
            break;
        }
    }
    for (is_user, text) in recent.into_iter().rev() {
        out.push(format!("  {} {}", if is_user { ">" } else { "<" }, text));
    }
    out
}

/// Returns the checkpoint title and messages.
pub fn load(path: &std::path::Path) -> Option<(String, Vec<J>)> {
    let v = json::parse(&std::fs::read_to_string(path).ok()?)?;
    Some((
        v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        v.get("messages")?.as_arr()?.clone(),
    ))
}
