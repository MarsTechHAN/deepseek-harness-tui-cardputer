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

use std::path::PathBuf;

#[derive(Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub dir: PathBuf,
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

/// Scan `~/.dst/skills` (and `/etc/dst/skills`) for packages.
pub fn load_all() -> Vec<Skill> {
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
            out.push(Skill {
                name,
                description,
                body,
                dir,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The catalog block appended to the system prompt: name + when to use it.
pub fn catalog_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\nSKILLS\nThese packaged instructions are available on demand. When a task matches \
one, call the `skill` tool with its name before acting; do not guess at its contents.\n",
    );
    for sk in skills {
        s.push_str(&format!("- {}: {}\n", sk.name, sk.description));
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
    fn lookup_is_case_insensitive_and_reports_path() {
        let sk = Skill {
            name: "Demo".into(),
            description: "d".into(),
            body: "body".into(),
            dir: PathBuf::from("/tmp/demo"),
        };
        let all = vec![sk];
        let got = body_for(&all, "demo").unwrap();
        assert!(got.contains("body"));
        assert!(got.contains("/tmp/demo"));
        assert!(catalog_prompt(&all).contains("Demo: d"));
    }
}
