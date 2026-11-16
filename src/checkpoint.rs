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
    let doc = J::o(vec![
        ("title", J::s(title)),
        ("preview", J::s(&preview)),
        ("created", J::Num(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0))),
        ("messages", J::Arr(msgs.to_vec())),
    ]);
    let path = dir().join(format!("{}.json", stamp()));
    let _ = std::fs::write(&path, doc.dump());
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

pub struct Checkpoint { pub path: PathBuf, pub label: String, pub preview: String }

pub fn list() -> Vec<Checkpoint> {
    let mut out: Vec<(PathBuf, u64, Checkpoint)> = list_paths().into_iter().filter_map(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        let v = json::parse(&text)?;
        let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let preview = v.get("preview").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let created = v.get("created").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let label = if title.is_empty() {
            format!("#{} {}", stem.rsplit('-').next().unwrap_or(""), preview)
        } else { title };
        Some((p.clone(), created, Checkpoint { path: p, label, preview }))
    }).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out.into_iter().map(|(_, _, c)| c).collect()
}

/// Returns the checkpoint title and messages.
pub fn load(path: &std::path::Path) -> Option<(String, Vec<J>)> {
    let v = json::parse(&std::fs::read_to_string(path).ok()?)?;
    Some((
        v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        v.get("messages")?.as_arr()?.clone(),
    ))
}
