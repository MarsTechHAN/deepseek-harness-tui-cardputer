//! Saved-session identity and the model-facing `rename` tool.
//!
//! DSH keeps a session title that names the work, not the transcript: it is
//! what `/resume` shows when the user picks a session back up. dst stores it
//! inside the session JSON (`title`) and exposes a `rename` tool so the model
//! keeps the name current as the task changes; a human can use `/rename`.
//!
//! The active session is process state: the tool and the TUI both need to know
//! which file a rename writes to, and the file must stay in sync with the
//! in-memory title used by `/resume`.

use crate::json::{self, J};
use crate::tool::{Fail, Param, ToolDef};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Longest accepted title. The resume menu is 53 cells wide; longer names are
/// truncated for display anyway, so reject essays at the tool boundary.
pub const MAX_TITLE_CHARS: usize = 60;

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    title: String,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

/// Normalise a title: one line, collapsed whitespace, bounded length.
pub fn normalize(name: &str) -> Result<String, String> {
    let one_line: String = name
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();
    let collapsed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return Err("title must be a non-empty single line".into());
    }
    let mut title: String = collapsed.chars().take(MAX_TITLE_CHARS).collect();
    title = title.trim().to_string();
    Ok(title)
}

/// Read the stored title of a session file.
pub fn title_of(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v = json::parse(&text)?;
    let t = v.get("title").and_then(|x| x.as_str())?.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn write_title(path: &std::path::Path, title: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut v = json::parse(&text).ok_or_else(|| "session file is not valid JSON".to_string())?;
    if !matches!(v, J::Obj(_)) {
        return Err("session file is not an object".into());
    }
    v.set("title", J::s(title));
    std::fs::write(path, v.dump()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Make `path` the active session, loading its stored title.
pub fn set_current(path: PathBuf) {
    let title = title_of(&path).unwrap_or_default();
    if let Ok(mut g) = state().lock() {
        *g = State {
            path: Some(path),
            title,
        };
    }
}

/// Forget the active session (used by tests and a future "new session" action).
#[allow(dead_code)]
pub fn clear_current() {
    if let Ok(mut g) = state().lock() {
        *g = State::default();
    }
}

#[allow(dead_code)]
pub fn current_path() -> Option<PathBuf> {
    state().lock().ok().and_then(|g| g.path.clone())
}

pub fn title() -> String {
    state().lock().map(|g| g.title.clone()).unwrap_or_default()
}

/// Rename the active session and persist it into the session file.
pub fn rename(name: &str) -> Result<String, String> {
    let title = normalize(name)?;
    if let Ok(mut g) = state().lock() {
        if let Some(path) = &g.path {
            write_title(path, &title)?;
        }
        g.title = title.clone();
    }
    Ok(title)
}

/// Human-facing preview lines for the `/resume` menu expansion: what the
/// session is about and where it left off.
pub fn preview(path: &std::path::Path) -> Vec<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return vec![format!("(unreadable: {e})")],
    };
    let v = match json::parse(&text) {
        Some(v) => v,
        None => return vec!["(session file is not valid JSON)".to_string()],
    };
    let mut out = Vec::new();
    let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("");
    out.push(if title.is_empty() {
        "(untitled session: /rename or ask the model to rename)".to_string()
    } else {
        format!("title: {title}")
    });
    if let Some(model) = v.get("model").and_then(|x| x.as_str()) {
        out.push(format!("model: {model}"));
    }
    let msgs = v.get("messages").and_then(|m| m.as_arr()).cloned().unwrap_or_default();
    let user_count = msgs
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .count();
    out.push(format!("turns: {user_count} user, {} messages", msgs.len()));
    if let Some(last_user) = msgs
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
    {
        if let Some(t) = last_user.get("content").and_then(|c| c.as_str()) {
            out.push(format!("last user: {}", one_line(t, 46)));
        }
    }
    if let Some(last_assistant) = msgs
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
    {
        if let Some(t) = last_assistant.get("content").and_then(|c| c.as_str()) {
            if !t.trim().is_empty() {
                out.push(format!("last reply: {}", one_line(t, 44)));
            }
        }
    }
    out
}

fn one_line(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(n).collect();
    if flat.chars().count() > n {
        out.push('\u{2026}');
    }
    out
}

// ---------------------------------------------------------------------------
// model-facing tool
// ---------------------------------------------------------------------------

fn rename_exec(args: &J) -> Result<J, Fail> {
    let name = crate::tool::str_arg(args, "name").unwrap_or_default();
    let title = rename(&name).map_err(Fail::new)?;
    Ok(J::o(vec![("title", J::s(&title))]))
}

fn rename_render(_args: &J, value: &J) -> String {
    match value.get("title").and_then(|t| t.as_str()) {
        Some(t) => format!("Session renamed to \"{t}\"."),
        None => "Session renamed.".to_string(),
    }
}

/// Add `rename` to the registry: the model keeps the resume title current.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "rename",
        description: "Rename the current saved session. This title is what the user sees in \
/resume, so keep it short and current: call it as soon as you understand the task, and update it \
when the task changes substantially (a new sub-task, a different bug, a finished phase).",
        params: vec![Param::req(
            "name",
            "string",
            "New session title: one line, a few words, at most 60 characters.",
        )],
        concurrency_safe: false,
        timeout_ms: 5_000,
        execute: rename_exec,
        render: rename_render,
        present: |a| {
            format!(
                "rename session: {}",
                one_line(&crate::tool::str_arg(a, "name").unwrap_or_default(), 32)
            )
        },
        post: None,
        schema_override: None,
        dynamic: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_single_line_and_bounds() {
        assert_eq!(normalize("  fix\n the   display ").unwrap(), "fix the display");
        assert!(normalize("   ").is_err());
        let long = "x".repeat(200);
        assert_eq!(normalize(&long).unwrap().chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn title_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dst-title-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.json");
        std::fs::write(&p, "{\"messages\":[]}").unwrap();
        set_current(p.clone());
        assert_eq!(title(), "");
        assert_eq!(rename("whale animation").unwrap(), "whale animation");
        assert_eq!(title_of(&p).as_deref(), Some("whale animation"));
        assert!(preview(&p).iter().any(|l| l.contains("whale animation")));
        clear_current();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
