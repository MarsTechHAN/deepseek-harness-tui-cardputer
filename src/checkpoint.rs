//! Turn checkpoints for /rewind (Claude Code / DSH checkpoint idea).
use crate::json::{self, J};
use std::path::PathBuf;

fn dir() -> PathBuf { crate::config::home().join("checkpoints") }

fn stamp() -> String {
    // Local wall-clock to the second, plus milliseconds: two checkpoints in
    // the same second must not overwrite each other (a quick rewind + retry),
    // while the filename still sorts chronologically.
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    let base = std::process::Command::new("date").arg("+%Y%m%d-%H%M%S").output()
        .ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}", std::process::id()));
    format!("{base}-{ms:03}")
}

/// Snapshot the current transcript before a turn starts. Keeps the newest 20.
///
/// `prompt` is the user input the turn is about to run, i.e. the message the
/// snapshot is "just before". Rewind lists those inputs (newest first), so the
/// row the user picks is a real previously typed message, not an opaque turn
/// number: restoring it puts that prompt back into the input line for editing.
pub fn save(msgs: &[J], title: &str, prompt: &str) {
    save_in(&dir(), msgs, title, prompt);
}

/// Testable core of `save`: write one checkpoint under an explicit directory.
fn save_in(dir: &std::path::Path, msgs: &[J], title: &str, prompt: &str) {
    let _ = std::fs::create_dir_all(dir);
    let prompt_flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = if !prompt_flat.is_empty() {
        prompt_flat.chars().take(120).collect::<String>()
    } else {
        msgs.iter().rev()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            .and_then(|m| m.get("content").and_then(|c| c.as_str()))
            .unwrap_or("").chars().take(60).collect::<String>()
    };
    let created = J::Num(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0));
    // Write the transcript straight into the document instead of cloning the
    // whole message array first (checkpoints are full-history snapshots).
    let doc = json::doc_messages(
        &[
            ("title", J::s(title)),
            ("prompt", J::s(&prompt_flat)),
            ("preview", J::s(&preview)),
            ("created", created),
        ],
        msgs,
    );
    let path = dir.join(format!("{}.json", stamp()));
    let _ = std::fs::write(&path, doc);
    let mut all = list_paths_in(dir);
    all.sort();
    while all.len() > 20 {
        let _ = std::fs::remove_file(all.remove(0));
    }
}

fn list_paths_in(dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json")).collect())
        .unwrap_or_default()
}

#[derive(Clone)]
pub struct Checkpoint {
    pub path: PathBuf,
    /// The user input this snapshot was taken just before (empty for an
    /// automatic/goal round).
    pub prompt: String,
    pub label: String,
    pub created: u64,
}

pub fn list() -> Vec<Checkpoint> {
    list_in(&dir())
}

fn list_in(dir: &std::path::Path) -> Vec<Checkpoint> {
    let mut out: Vec<(PathBuf, u64, Checkpoint)> = list_paths_in(dir).into_iter().filter_map(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        let v = json::parse(&text)?;
        let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let preview = v.get("preview").and_then(|x| x.as_str()).unwrap_or("").to_string();
        // Legacy checkpoints (before prompt-based rewind) only carry preview.
        let prompt = v
            .get("prompt")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let created = v.get("created").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
        let label = if !prompt.is_empty() {
            prompt.clone()
        } else if !preview.is_empty() {
            preview.clone()
        } else if !title.is_empty() {
            title.clone()
        } else {
            "(session start)".to_string()
        };
        Some((
            p.clone(),
            created,
            Checkpoint {
                path: p,
                prompt,
                label,
                created,
            },
        ))
    }).collect();
    // Newest first; same-second timestamps are ordered by the millisecond
    // suffix in the filename (created has only whole seconds).
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    out.into_iter().map(|(_, _, c)| c).collect()
}

/// Human preview lines for the rewind picker.
pub fn preview(path: &std::path::Path) -> Vec<String> {
    let (title, messages) = match load(path) {
        Some(v) => v,
        None => return vec!["(unreadable checkpoint)".to_string()],
    };
    let v = json::parse(&std::fs::read_to_string(path).unwrap_or_default()).unwrap_or(J::Null);
    let prompt = v.get("prompt").and_then(|x| x.as_str()).unwrap_or("");
    let mut out = vec![format!("title: {title}")];
    if !prompt.is_empty() {
        out.push(format!("prompt: {}", prompt.chars().take(120).collect::<String>()));
    }
    out.push(String::new());
    out.push("context before it (newest last):".to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "dst-checkpoint-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn msgs(text: &str) -> Vec<J> {
        vec![
            crate::harness::msg("system", "sys"),
            crate::harness::msg("user", text),
        ]
    }

    #[test]
    fn checkpoints_remember_the_prompt_they_precede() {
        let dir = tmpdir("prompt");
        save_in(&dir, &msgs("first"), "session", "first");
        std::thread::sleep(std::time::Duration::from_millis(3));
        save_in(&dir, &msgs("second"), "session", "what is in the photo?");
        let list = list_in(&dir);
        assert_eq!(list.len(), 2);
        // Newest first, labelled by the user input the snapshot precedes.
        assert_eq!(list[0].prompt, "what is in the photo?");
        assert_eq!(list[0].label, "what is in the photo?");
        assert_eq!(list[1].label, "first");
        // The restored history is the state *before* that prompt.
        let (_, restored) = load(&list[0].path).expect("loadable");
        assert_eq!(restored.len(), 2);
        assert_eq!(
            restored[1].get("content").and_then(|c| c.as_str()),
            Some("second")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_checkpoints_without_a_prompt_still_list() {
        let dir = tmpdir("legacy");
        let doc = json::doc_messages(
            &[("title", J::s("old session")), ("preview", J::s("an old prompt"))],
            &msgs("hello"),
        );
        std::fs::write(dir.join("20260101-000000.json"), doc).unwrap();
        let list = list_in(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "an old prompt");
        assert!(list[0].prompt.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_second_checkpoints_do_not_overwrite() {
        let dir = tmpdir("collide");
        save_in(&dir, &msgs("a"), "s", "a");
        save_in(&dir, &msgs("b"), "s", "b");
        assert_eq!(list_in(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
