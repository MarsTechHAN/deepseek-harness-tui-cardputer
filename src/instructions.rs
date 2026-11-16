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

/// One memory file by path ("project" | "global" | absolute path).
pub fn read_file(which: &str) -> Result<(String, String), String> {
    let path = match which {
        "project" => std::env::current_dir().unwrap_or_else(|_| ".".into()).join("AGENTS.md"),
        "global" => crate::config::home().join("AGENTS.md"),
        other => std::path::PathBuf::from(other),
    };
    let body = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path.display().to_string(), body.chars().take(MAX_BYTES).collect()))
}

/// Append or replace one memory file. Project scope is workspace-safe;
/// global scope is user state and may hold preferences across projects.
pub fn write_memory(scope: &str, content: &str, replace: bool) -> Result<String, String> {
    let content = content.trim();
    if content.is_empty() {
        return Err("memory content must not be empty".into());
    }
    let path = match scope {
        "project" | "" => std::env::current_dir().unwrap_or_else(|_| ".".into()).join("AGENTS.md"),
        "global" => crate::config::home().join("AGENTS.md"),
        other => return Err(format!("unknown scope '{other}' (project|global)")),
    };
    let mut body = if replace {
        String::new()
    } else {
        std::fs::read_to_string(&path).unwrap_or_default()
    };
    if !body.trim().is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(content);
    body.push('\n');
    if body.len() > MAX_BYTES {
        return Err(format!("memory file would exceed {MAX_BYTES} bytes; trim it first"));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Keyword search across every memory file (names are not indexed; bodies).
pub fn search(query: &str) -> Vec<String> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for (path, body) in load() {
        for (i, line) in body.lines().enumerate() {
            if line.to_lowercase().contains(&q) {
                out.push(format!("{path}:{}: {}", i + 1, line.trim()));
                if out.len() >= 40 {
                    return out;
                }
            }
        }
    }
    out
}

fn ok(pairs: Vec<(&str, crate::json::J)>) -> crate::json::J {
    crate::tool::obj(pairs)
}

fn mem_read_exec(args: &crate::json::J) -> Result<crate::json::J, crate::tool::Fail> {
    let which = crate::tool::str_arg(args, "file").unwrap_or_else(|| "all".into());
    if which == "all" {
        let files = load();
        return Ok(ok(vec![("files", crate::json::J::Arr(files.into_iter().map(|(p, b)| {
            ok(vec![("path", crate::json::J::s(&p)), ("content", crate::json::J::s(&b))])
        }).collect()))]));
    }
    let (path, body) = read_file(&which).map_err(crate::tool::Fail::new)?;
    Ok(ok(vec![("path", crate::json::J::s(&path)), ("content", crate::json::J::s(&body))]))
}

fn mem_write_exec(args: &crate::json::J) -> Result<crate::json::J, crate::tool::Fail> {
    let scope = crate::tool::str_arg(args, "scope").unwrap_or_else(|| "project".into());
    let content = crate::tool::str_arg(args, "content").unwrap_or_default();
    let replace = args.get("replace").map(|b| matches!(b, crate::json::J::Bool(true))).unwrap_or(false);
    let path = write_memory(&scope, &content, replace).map_err(crate::tool::Fail::new)?;
    Ok(ok(vec![("path", crate::json::J::s(&path)), ("scope", crate::json::J::s(&scope))]))
}

fn mem_search_exec(args: &crate::json::J) -> Result<crate::json::J, crate::tool::Fail> {
    let q = crate::tool::str_arg(args, "query").unwrap_or_default();
    if q.trim().is_empty() {
        return Err(crate::tool::Fail::new("missing 'query'"));
    }
    Ok(ok(vec![("matches", crate::json::J::Arr(search(&q).into_iter().map(|x| crate::json::J::s(&x)).collect()))]))
}

fn mem_render(_a: &crate::json::J, v: &crate::json::J) -> String {
    if let Some(files) = v.get("files").and_then(|f| f.as_arr()) {
        if files.is_empty() { return "(no memory files)".into(); }
        let mut out = String::new();
        for f in files {
            let p = f.get("path").and_then(|x| x.as_str()).unwrap_or("");
            let c = f.get("content").and_then(|x| x.as_str()).unwrap_or("");
            out.push_str(&format!("== {p} ==\n{}\n", c.chars().take(4000).collect::<String>()));
        }
        return out.trim_end().to_string();
    }
    if let Some(m) = v.get("matches").and_then(|m| m.as_arr()) {
        if m.is_empty() { return "(no memory matches)".into(); }
        return m.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n");
    }
    if let Some(c) = v.get("content").and_then(|x| x.as_str()) {
        return c.to_string();
    }
    v.dump()
}

/// Claude-Code-aligned memory tools: read, write (append/replace), search.
pub fn tool_defs() -> Vec<crate::tool::ToolDef> {
    use crate::tool::{Param, ToolDef};
    vec![
        ToolDef {
            name: "memory_read",
            description: "Read the project memory (AGENTS.md in the workspace) and/or the global memory (~/.dst/AGENTS.md). Use it before changing conventions or when you need previously saved preferences.",
            params: vec![Param::opt("file", "string", "project | global | all (default all).")],
            concurrency_safe: true, timeout_ms: 5_000,
            execute: mem_read_exec, render: mem_render,
            present: |_a| "memory_read".into(), post: None,
            schema_override: None, dynamic: None,
        },
        ToolDef {
            name: "memory_write",
            description: "Save durable facts or user preferences to memory. scope=project writes ./AGENTS.md (workspace rules), scope=global writes ~/.dst/AGENTS.md (across projects). Append by default; replace=true overwrites. Keep entries short and factual, never secrets.",
            params: vec![
                Param::one_of("scope", "project or global.", &["project", "global"]),
                Param::req("content", "string", "Markdown text to save."),
                Param::opt("replace", "boolean", "Replace the file instead of appending."),
            ],
            concurrency_safe: false, timeout_ms: 10_000,
            execute: mem_write_exec, render: mem_render,
            present: |a| format!("memory_write {}: {}", crate::tool::str_arg(a, "scope").unwrap_or_else(|| "project".into()), crate::tool::str_arg(a, "content").unwrap_or_default().chars().take(24).collect::<String>()),
            post: None, schema_override: None, dynamic: None,
        },
        ToolDef {
            name: "memory_search",
            description: "Search project and global memory files for a keyword; returns matching lines with file:line.",
            params: vec![Param::req("query", "string", "Keyword to find in memory files.")],
            concurrency_safe: true, timeout_ms: 5_000,
            execute: mem_search_exec, render: mem_render,
            present: |a| format!("memory_search {}", crate::tool::str_arg(a, "query").unwrap_or_default()),
            post: None, schema_override: None, dynamic: None,
        },
    ]
}

pub fn summary() -> Vec<String> {
    let files = load();
    if files.is_empty() {
        vec!["no AGENTS.md found (workspace or ~/.dst)".into()]
    } else {
        files.into_iter().map(|(p, b)| format!("{p}  ({} bytes)", b.len())).collect()
    }
}
