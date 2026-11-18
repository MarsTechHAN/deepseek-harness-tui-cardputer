//! Skills: progressively disclosed instruction packages, like DSH's skill
//! mechanism.
//!
//! Layout (mirrors DSH):
//!
//! ```text
//! ~/.dst/skills/<skill-name>/
//!     SKILL.md            YAML frontmatter + markdown instructions
//!     ...supporting files the instructions may reference
//! ```
//!
//! `SKILL.md` starts with a small YAML frontmatter block:
//!
//! ```text
//! ---
//! name: pdf-report
//! description: Use when the user asks to summarise a PDF report.
//! ---
//! # Instructions
//! ...
//! ```
//!
//! Only `name`/`description` (the catalog entry) are loaded at startup; the
//! body is returned by the `skill` tool, so the model pays for it only when it
//! decides to use the skill.

use crate::json::J;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub dir: PathBuf,
    /// "model" for skills the agent wrote itself; empty for hand-installed ones.
    pub origin: String,
    /// Bumped by skill_save; hand-installed skills default to 1.
    pub version: u32,
    pub updated: String,
}

impl Skill {
    /// Is the package writable by dst (under the user skills dir, not /etc)?
    pub fn is_user(&self) -> bool {
        self.dir.starts_with(crate::config::skills_dir())
    }
}

/// Split a skill file's YAML frontmatter into (key/value pairs, body).
/// The apps layer reuses this so SKILL.md stays the one place the human title
/// and description live.
pub fn split_frontmatter(text: &str) -> (Vec<(String, String)>, String) {
    frontmatter(text)
}

fn frontmatter(text: &str) -> (Vec<(String, String)>, String) {
    let mut meta = Vec::new();
    if !text.starts_with("---") {
        return (meta, text.to_string());
    }
    let rest = &text[3..];
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    if let Some(end) = rest.find("\n---") {
        let head = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n').to_string();
        for line in head.lines() {
            if let Some((k, v)) = line.split_once(':') {
                let k = k.trim().to_string();
                let mut v = v.trim().to_string();
                if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                    || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
                {
                    v = v[1..v.len() - 1].to_string();
                }
                if !k.is_empty() {
                    meta.push((k, v));
                }
            }
        }
        (meta, body)
    } else {
        (meta, rest.to_string())
    }
}

/// Skills the model wrote itself (creator mode). They live in the same catalog
/// as hand-installed ones, so the prompt/catalog/skill-tool path needs no
/// special case; only the writing side is new.
pub const MAX_BODY: usize = 12_000;
pub const MAX_DESC: usize = 200;
pub const MAX_FILE: usize = 32 * 1024;
pub const MAX_TOTAL: u64 = 256 * 1024;

fn valid_name(name: &str) -> bool {
    let mut it = name.chars();
    match it.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    name.len() <= 48
        && it.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn safe_rel(path: &str) -> Option<String> {
    let p = path.trim();
    if p.is_empty() || p.chars().count() > 120 || p.starts_with('/') || p.contains('\\') {
        return None;
    }
    let mut parts = Vec::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.chars().any(|c| c.is_control()) {
            return None;
        }
        parts.push(seg);
    }
    Some(parts.join("/"))
}

/// Keep at most five snapshots in a skill's .history directory.
fn prune_history(history: &Path) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(history)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    if files.len() <= 5 {
        return;
    }
    files.sort();
    let drop = files.len() - 5;
    for p in files.into_iter().take(drop) {
        let _ = std::fs::remove_file(p);
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => total += dir_bytes(&e.path()),
                Ok(m) if m.is_file() => total += m.len(),
                _ => {}
            }
        }
    }
    total
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, content).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// Static list of skills saved during this turn, for the transcript notice.
static SAVED: std::sync::Mutex<Vec<(String, String, u32)>> = std::sync::Mutex::new(Vec::new());
/// Packages that became (or stayed) runnable this turn: a skill_file call
/// wrote meta.json, which is what turns a skill folder into an app.
static SAVED_APPS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Drain the packages that gained a meta.json this turn (creator-mode notice).
pub fn take_saved_apps() -> Vec<String> {
    match SAVED_APPS.lock() {
        Ok(mut g) => std::mem::take(&mut *g),
        Err(_) => Vec::new(),
    }
}

pub fn take_saved() -> Vec<(String, String, u32)> {
    match SAVED.lock() {
        Ok(mut g) => std::mem::take(&mut *g),
        Err(_) => Vec::new(),
    }
}

fn note_saved(sk: &Skill) {
    if let Ok(mut g) = SAVED.lock() {
        g.push((sk.name.clone(), sk.dir.display().to_string(), sk.version));
    }
}

/// Create or update a skill package from model-written instructions.
pub fn save_at(
    root: &Path,
    name: &str,
    description: &str,
    instructions: &str,
) -> Result<(Skill, bool), String> {
    // Accept a title-ish name ("Battery Report") by slugging whitespace.
    let name = name
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if !valid_name(&name) {
        return Err(format!(
            "invalid skill name '{name}': lowercase letters, digits, '_' or '-' (max 48)"
        ));
    }
    let description: String = description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_DESC)
        .collect();
    if description.is_empty() {
        return Err("missing 'description': when should the model use this skill".into());
    }
    let body = instructions.trim();
    if body.is_empty() {
        return Err("missing 'instructions': the self-contained skill body".into());
    }
    if body.chars().count() > MAX_BODY {
        return Err(format!("instructions are longer than {MAX_BODY} characters"));
    }
    let dir = root.join(&name);
    let existed = dir.join("SKILL.md").is_file();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Versioned updates: keep the previous SKILL.md under .history/v<N>.md so a
    // user can read or restore it, and bump the visible version.
    let version = if existed {
        let old_text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
        let (meta, _body) = frontmatter(&old_text);
        let old_version = meta
            .iter()
            .find(|(k, _)| k == "version")
            .and_then(|(_, v)| v.trim().parse::<u32>().ok())
            .unwrap_or(1);
        let history = dir.join(".history");
        if std::fs::create_dir_all(&history).is_ok() {
            let _ = write_atomic(&history.join(format!("v{old_version}.md")), &old_text);
            prune_history(&history);
        }
        old_version.saturating_add(1)
    } else {
        1
    };
    let updated = crate::apps::now_stamp();
    let text = format!(
        "---\nname: {name}\ndescription: {description}\norigin: model\nversion: {version}\nupdated: {updated}\n---\n\n{body}\n"
    );
    write_atomic(&dir.join("SKILL.md"), &text)?;
    let sk = Skill {
        name,
        description,
        body: body.to_string(),
        dir,
        origin: "model".to_string(),
        version,
        updated,
    };
    note_saved(&sk);
    Ok((sk, !existed))
}

pub fn save(name: &str, description: &str, instructions: &str) -> Result<(Skill, bool), String> {
    save_at(&crate::config::skills_dir(), name, description, instructions)
}

/// Write one supporting file inside an existing skill package.
pub fn write_file(name: &str, path: &str, content: &str) -> Result<(Skill, String, usize), String> {
    let root = crate::config::skills_dir();
    let want = name.trim().to_ascii_lowercase();
    if !valid_name(&want) {
        return Err(format!("invalid skill name '{name}'"));
    }
    let dir = root.join(&want);
    if !dir.join("SKILL.md").is_file() {
        return Err(format!("no skill '{want}'; call skill_save first"));
    }
    let rel = safe_rel(path)
        .ok_or_else(|| format!("invalid path '{path}': a relative file inside the skill dir"))?;
    if rel.eq_ignore_ascii_case("SKILL.md") {
        return Err("SKILL.md is managed by skill_save; write it through skill_save".into());
    }
    let dest = dir.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    if content.len() > MAX_FILE {
        return Err(format!("file is {} bytes; the per-file limit is {MAX_FILE}", content.len()));
    }
    let existing = dest.metadata().map(|m| m.len()).unwrap_or(0);
    if dir_bytes(&dir).saturating_sub(existing) + content.len() as u64 > MAX_TOTAL {
        return Err(format!("skill package would exceed {MAX_TOTAL} bytes"));
    }
    std::fs::write(&dest, content).map_err(|e| format!("{}: {e}", dest.display()))?;
    // meta.json is the app manifest: writing it is the moment a skill becomes
    // a runnable package, so it is announced separately from a skill save.
    if rel.eq_ignore_ascii_case("meta.json") {
        if let Ok(mut g) = SAVED_APPS.lock() {
            if !g.iter().any(|n| n == &want) {
                g.push(want.clone());
            }
        }
    }
    let sk = Skill {
        name: want,
        description: String::new(),
        body: String::new(),
        dir,
        origin: "model".to_string(),
        version: 1,
        updated: String::new(),
    };
    Ok((sk, rel, content.len()))
}

/// History snapshot versions present for a skill, newest first.
pub fn history_versions(sk: &Skill) -> Vec<u32> {
    let mut out: Vec<u32> = std::fs::read_dir(sk.dir.join(".history"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let n = name.strip_prefix('v')?.strip_suffix(".md")?;
                    n.parse::<u32>().ok()
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_unstable();
    out.reverse();
    out
}

/// Restore the body of an earlier version as a new skill version. The current
/// body is archived first (same rule as any update), so a rollback is itself
/// reversible. Returns (new skill, restored-from version, new version).
pub fn rollback_at(root: &Path, name: &str, version: Option<u32>) -> Result<(Skill, u32, u32), String> {
    let want = name.trim().to_ascii_lowercase();
    let dir = root.join(&want);
    let current = load_meta()
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(&want) && s.dir == dir)
        .or_else(|| {
        let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
        let (meta, body) = frontmatter(&text);
        let name = meta
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| want.clone());
        let description = meta
            .iter()
            .find(|(k, _)| k == "description")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        Some(Skill {
            name,
            description,
            body,
            dir: dir.clone(),
            origin: "model".to_string(),
            version: 1,
            updated: String::new(),
        })
    })
    .ok_or_else(|| format!("no skill '{want}'"))?;
    if !current.is_user() && current.dir != dir {
        return Err("refusing to roll back a skill outside the user skills dir".into());
    }
    let versions = history_versions(&current);
    if versions.is_empty() {
        return Err(format!(
            "skill '{want}' has no history yet; save it again to create versions"
        ));
    }
    let target = match version {
        Some(v) => versions
            .iter()
            .copied()
            .find(|x| *x == v)
            .ok_or_else(|| {
                format!(
                    "no version v{v} for '{want}'; available: {}",
                    versions.iter().map(|x| format!("v{x}")).collect::<Vec<_>>().join(", ")
                )
            })?,
        None => {
            let next = versions.iter().copied().find(|v| *v < current.version).or_else(|| versions.first().copied());
            next.ok_or_else(|| format!("no version older than v{} for '{want}'", current.version))?
        }
    };
    let old_text = std::fs::read_to_string(current.dir.join(".history").join(format!("v{target}.md")))
        .map_err(|e| format!("history v{target}: {e}"))?;
    let (meta, body) = frontmatter(&old_text);
    let description = meta
        .iter()
        .find(|(k, _)| k == "description")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| current.description.clone());
    let (sk, _created) = save_at(root, &want, &description, &body)?;
    let new_version = sk.version;
    Ok((sk, target, new_version))
}

pub fn rollback(name: &str, version: Option<u32>) -> Result<(Skill, u32, u32), String> {
    rollback_at(&crate::config::skills_dir(), name, version)
}

/// Remove a user-owned skill package (refuses /etc or anything outside the
/// user skills dir). Returns the package path that was removed.
pub fn delete(sk: &Skill) -> Result<String, String> {
    let root = crate::config::skills_dir();
    if !sk.dir.starts_with(&root) {
        return Err("refusing to delete a skill outside the user skills dir".into());
    }
    std::fs::remove_dir_all(&sk.dir).map_err(|e| format!("{}: {e}", sk.dir.display()))?;
    Ok(sk.dir.display().to_string())
}

fn ok(pairs: Vec<(&str, J)>) -> J {
    crate::tool::obj(pairs)
}

fn save_exec(args: &J) -> Result<J, crate::tool::Fail> {
    let name = crate::tool::str_arg(args, "name").unwrap_or_default();
    let description = crate::tool::str_arg(args, "description").unwrap_or_default();
    let instructions = crate::tool::str_arg(args, "instructions").unwrap_or_default();
    let (sk, created) = save(&name, &description, &instructions).map_err(crate::tool::Fail::new)?;
    Ok(ok(vec![
        ("name", J::s(&sk.name)),
        ("path", J::s(&sk.dir.display().to_string())),
        ("created", J::Bool(created)),
        ("version", J::Num(sk.version as f64)),
    ]))
}

fn save_render(_a: &J, v: &J) -> String {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let version = v.get("version").and_then(|x| x.as_num()).unwrap_or(1.0) as u32;
    format!(
        "saved skill {name} v{version} at {path}; the previous body is kept under .history/, and the skill is loadable with the skill tool (catalog updates next turn)"
    )
}

fn save_present(a: &J) -> String {
    format!("skill_save {}", crate::tool::str_arg(a, "name").unwrap_or_default())
}

fn file_exec(args: &J) -> Result<J, crate::tool::Fail> {
    let name = crate::tool::str_arg(args, "name").unwrap_or_default();
    let path = crate::tool::str_arg(args, "path").unwrap_or_default();
    let content = crate::tool::str_arg(args, "content").unwrap_or_default();
    let (sk, rel, bytes) =
        write_file(&name, &path, &content).map_err(crate::tool::Fail::new)?;
    Ok(ok(vec![
        ("name", J::s(&sk.name)),
        ("path", J::s(&rel)),
        ("bytes", J::Num(bytes as f64)),
    ]))
}

fn file_render(_a: &J, v: &J) -> String {
    format!(
        "wrote {} ({} bytes) in skill {}",
        v.get("path").and_then(|x| x.as_str()).unwrap_or(""),
        v.get("bytes").and_then(|x| x.as_num()).unwrap_or(0.0) as usize,
        v.get("name").and_then(|x| x.as_str()).unwrap_or("")
    )
}

fn file_present(a: &J) -> String {
    format!(
        "skill_file {}/{}",
        crate::tool::str_arg(a, "name").unwrap_or_default(),
        crate::tool::str_arg(a, "path").unwrap_or_default()
    )
}

fn rollback_exec(args: &J) -> Result<J, crate::tool::Fail> {
    let name = crate::tool::str_arg(args, "name").unwrap_or_default();
    let version = crate::tool::num_arg(args, "version").map(|v| v.max(1.0) as u32);
    let (sk, from, new) = rollback(&name, version).map_err(crate::tool::Fail::new)?;
    Ok(ok(vec![
        ("name", J::s(&sk.name)),
        ("restored_from", J::Num(from as f64)),
        ("version", J::Num(new as f64)),
        ("path", J::s(&sk.dir.display().to_string())),
    ]))
}

fn rollback_render(_a: &J, v: &J) -> String {
    format!(
        "rolled back skill {} to the body of v{} (now v{}); the previous current body is kept under .history/",
        v.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        v.get("restored_from").and_then(|x| x.as_num()).unwrap_or(0.0) as u32,
        v.get("version").and_then(|x| x.as_num()).unwrap_or(0.0) as u32
    )
}

fn rollback_present(a: &J) -> String {
    format!("rollback {}", crate::tool::str_arg(a, "name").unwrap_or_default())
}

/// Creator-mode tools for writing the model's own skills. Registered only while
/// creator mode is on; reading/using skills works in every mode.
pub fn tool_defs() -> Vec<crate::tool::ToolDef> {
    use crate::tool::{Param, ToolDef};
    vec![
        ToolDef {
            name: "skill_save",
            description: "Create or update a package in the skill library (~/.dst/skills/<name>/): \
SKILL.md with name/description frontmatter plus the instructions. Use it for a systematic, \
self-contained engineering package you worked out (a procedure with steps, formats, checklists) \
and for anything you want to persist for the device. NOT for short facts (use memory_write). \
A package becomes a runnable APP when you also write meta.json into it with skill_file. \
Updating keeps the previous body under .history/vN.md and bumps the version; the package joins \
the SKILLS catalog on your next turn.",
            params: vec![
                Param::req("name", "string", "Slug: lowercase letters/digits/_/-, unique."),
                Param::req("description", "string", "One line: when the model should use this skill (max 200 chars)."),
                Param::req("instructions", "string", "The complete self-contained markdown body (max 12000 chars)."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: save_exec,
            render: save_render,
            present: save_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "skill_rollback",
            description: "Restore the body of an earlier version of one of your skills (skill_save \
keeps .history/v<N>.md snapshots). The current body is archived first and the restored content \
becomes a new version, so a rollback is reversible. Omit 'version' to go back one step.",
            params: vec![
                Param::req("name", "string", "Skill name."),
                Param::opt("version", "integer", "History version to restore (e.g. 1); omit for the previous version."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: rollback_exec,
            render: rollback_render,
            present: rollback_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "skill_file",
            description: "Write one supporting file inside a saved skill package (~/.dst/skills/<name>/). \
Use it for the entry program (run.sh), templates, reference data — and for meta.json, which is \
what turns the package into a runnable app: {\"title\", \"kind\": sh|python|binary|prompt|doc, \
\"entry\": \"run.sh\", \"requires\": [...], \"default_args\", \"capture\"}.",
            params: vec![
                Param::req("name", "string", "Skill name."),
                Param::req("path", "string", "Relative path inside the skill dir."),
                Param::req("content", "string", "Full file content (max 32 KB)."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: file_exec,
            render: file_render,
            present: file_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
    ]
}

/// Scan every skill package, including disabled ones (extensions menu).
pub fn load_meta() -> Vec<Skill> {
    let mut out = Vec::new();
    for root in [
        PathBuf::from("/etc/dst/skills"),
        crate::config::skills_dir(),
    ] {
        let rd = match std::fs::read_dir(&root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let md = dir.join("SKILL.md");
            let text = match std::fs::read_to_string(&md) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let (meta, body) = frontmatter(&text);
            let dir_name = dir
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("skill")
                .to_string();
            let name = meta
                .iter()
                .find(|(k, _)| k == "name")
                .map(|(_, v)| v.clone())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| dir_name.clone());
            let description = meta
                .iter()
                .find(|(k, _)| k == "description")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let origin = meta
                .iter()
                .find(|(k, _)| k == "origin")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let version = meta
                .iter()
                .find(|(k, _)| k == "version")
                .and_then(|(_, v)| v.trim().parse::<u32>().ok())
                .unwrap_or(1);
            let updated = meta
                .iter()
                .find(|(k, _)| k == "updated")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            out.push(Skill {
                name,
                description,
                body,
                dir,
                origin,
                version,
                updated,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Enabled skills: everything except names listed in `skills.disabled`.
pub fn load_all() -> Vec<Skill> {
    let disabled = crate::config::Config::load().disabled_skills;
    let mut all = load_meta();
    if !disabled.is_empty() {
        all.retain(|sk| !disabled.iter().any(|d| d == &sk.name));
    }
    all
}

/// The catalog block appended to the system prompt: name + when to use it.
pub fn catalog_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\nSKILLS\nThese packaged instructions are available on demand. When a task matches \
one, call the `skill` tool with its name before acting; do not guess at its contents.\nA \
package that also has meta.json is a runnable APP: its line carries [kind:entry], /apps lists \
it, and app_run/app_test execute it.\n",
    );
    for sk in skills {
        // Runnable packages are announced with their entry so the model knows
        // which file to fix without opening meta.json first.
        let run = crate::apps::run_marker(&sk.dir)
            .map(|m| format!(" [{m}]"))
            .unwrap_or_default();
        if sk.version > 1 {
            s.push_str(&format!(
                "- {} (v{}){}: {}\n",
                sk.name, sk.version, run, sk.description
            ));
        } else {
            s.push_str(&format!("- {}{}: {}\n", sk.name, run, sk.description));
        }
    }
    s
}

/// Body returned to the model by the `skill` tool, with the package path so it
/// can resolve relative references.
pub fn body_for(skills: &[Skill], name: &str) -> Option<String> {
    let n = name.trim().trim_start_matches('/');
    skills
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(n))
        .map(|s| {
            format!(
                "# skill: {}\n(package: {})\n\n{}",
                s.name,
                s.dir.display(),
                s.body.trim()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let text = "---\nname: demo\ndescription: \"Use for demos\"\n---\n# Steps\n1. do it\n";
        let (meta, body) = frontmatter(text);
        assert_eq!(meta.len(), 2);
        assert_eq!(meta[1].1, "Use for demos");
        assert!(body.contains("# Steps"));
    }

    #[test]
    fn no_frontmatter_keeps_body() {
        let (meta, body) = frontmatter("# Just markdown\n");
        assert!(meta.is_empty());
        assert!(body.contains("Just markdown"));
    }

    #[test]
    fn writes_and_reloads_a_skill() {
        let root = std::env::temp_dir().join(format!(
            "dst-skill-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (sk, created) = save_at(
            &root,
            "Battery Report",
            "Use when the user asks for a battery report",
            "# Steps\n1. read the capacity\n2. render a table\n",
        )
        .unwrap();
        assert!(created);
        assert_eq!(sk.name, "battery-report");
        assert!(sk.dir.join("SKILL.md").is_file());
        let text = std::fs::read_to_string(sk.dir.join("SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: battery-report\n"));
        assert!(text.contains("origin: model"));
        assert!(text.contains("version: 1"));
        assert!(text.contains("description: Use when the user asks for a battery report"));
        assert!(text.contains("# Steps"));
        assert!(save_at(&root, "bad/name", "d", "b").is_err());
        assert!(save_at(&root, "x", "", "b").is_err());
        assert!(save_at(&root, "x", "d", "").is_err());
        assert!(write_file("battery-report", "notes.txt", "hi").is_err()
            || crate::config::skills_dir() != root);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rollback_restores_an_earlier_body_as_a_new_version() {
        let root = std::env::temp_dir().join(format!(
            "dst-skill-rb-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (_s, _) = save_at(&root, "report", "when reporting", "ORIGINAL v1 body").unwrap();
        let (s2, _) = save_at(&root, "report", "when reporting", "SECOND v2 body").unwrap();
        assert_eq!(s2.version, 2);
        let dir = root.join("report");
        assert!(dir.join(".history").join("v1.md").is_file());
        let (s3, from, new) = rollback_at(&root, "report", Some(1)).unwrap();
        assert_eq!(from, 1);
        assert_eq!(new, 3);
        assert_eq!(s3.version, 3);
        let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(text.contains("ORIGINAL v1 body"), "{text}");
        // The current body (v2) was archived before the restore.
        let v2 = std::fs::read_to_string(dir.join(".history").join("v2.md")).unwrap();
        assert!(v2.contains("SECOND v2 body"), "{v2}");
        assert!(rollback_at(&root, "report", Some(9)).is_err(), "unknown version is an error");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lookup_is_case_insensitive_and_reports_path() {
        let sk = Skill {
            name: "Demo".into(),
            description: "d".into(),
            body: "body".into(),
            dir: PathBuf::from("/tmp/demo"),
            origin: String::new(),
            version: 1,
            updated: String::new(),
        };
        let all = vec![sk];
        let got = body_for(&all, "demo").unwrap();
        assert!(got.contains("body"));
        assert!(got.contains("/tmp/demo"));
        assert!(catalog_prompt(&all).contains("Demo: d"));
    }
}
