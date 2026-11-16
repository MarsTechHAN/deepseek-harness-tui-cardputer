//! Project instructions (DSH `agent-instructions` / Claude's CLAUDE.md).
//! Loads AGENTS.md from the workspace and ~/.dst, bounded, into the prompt.

pub const MAX_BYTES: usize = 16_000;

fn read(path: &std::path::Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut body: String = text.chars().take(MAX_BYTES).collect();
    if text.chars().count() > MAX_BYTES {
        body.push_str("\n... [instructions truncated]");
    }
    Some((path.display().to_string(), body))
}

/// All instruction files, nearest first, deduplicated.
pub fn load() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    for name in ["AGENTS.md", ".dst/AGENTS.md"] {
        let p = cwd.join(name);
        if let Some((path, body)) = read(&p) {
            if seen.insert(path.clone()) {
                out.push((path, body));
            }
        }
    }
    let global = crate::config::home().join("AGENTS.md");
    if let Some((path, body)) = read(&global) {
        if seen.insert(path.clone()) {
            out.push((path, body));
        }
    }
    out
}

/// Prompt section for the loaded instruction files, or empty.
pub fn prompt_section() -> String {
    let files = load();
    if files.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nPROJECT INSTRUCTIONS\nThese instructions come from files in the workspace or ~/.dst and are binding for this session.\n");
    for (path, body) in files {
        s.push_str(&format!("\n<instructions from=\"{path}\">\n{body}\n</instructions>\n"));
    }
    s
}

pub fn summary() -> Vec<String> {
    let files = load();
    if files.is_empty() {
        vec!["no AGENTS.md found (workspace or ~/.dst)".into()]
    } else {
        files.into_iter().map(|(p, b)| format!("{p}  ({} bytes)", b.len())).collect()
    }
}
