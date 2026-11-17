//! Creator mode: persistent app packages + full-screen app runtime.
//!
//! The design lives in `docs/CREATOR.md`. In short: in creator mode the model
//! owns the device, so every piece of work it builds is saved as an *app
//! package* under `~/.dst/apps/<category>/<name>/`:
//!
//! ```text
//! app.json    machine manifest (name, title, description, kind, entry, version)
//! APP.md      short annotation written for the user who opens /apps later
//! run.sh      entry program and any supporting files
//! ```
//!
//! The catalog (name/title/description/kind) is injected into the system
//! prompt each turn, exactly like the skills catalog: cheap one-line entries,
//! detail read on demand with `read_file`. `app_run` hands the panel to a saved
//! app: the runner forwards the keyboard over a pipe and keeps one platform
//! invariant that no app can break - a long-press of Esc always kills the app
//! and gives the screen back to the chat.

use crate::json::J;
use crate::tool::{num_arg, obj, str_arg, Fail, Param, ToolDef};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Hard bounds so a bad turn cannot fill the microSD through the tools.
pub const MAX_APPS: usize = 200;
pub const MAX_INDEX: usize = 40;
pub const MAX_FILE_BYTES: usize = 32 * 1024;
pub const MAX_APP_BYTES: usize = 256 * 1024;
pub const MAX_ANNOTATION: usize = 4000;
pub const MAX_TITLE: usize = 60;
pub const MAX_DESC: usize = 160;
pub const DEFAULT_TIMEOUT_S: u64 = 60;
pub const MAX_TIMEOUT_S: u64 = 600;
/// User-initiated runs from /apps: long-press Esc is the exit, not a timer.
pub const DIRECT_TIMEOUT_S: u64 = 3_600;

/// One recorded run/test of an app (written back into app.json).
#[derive(Clone, Debug, Default)]
pub struct RunStamp {
    pub at: String,
    pub secs: f32,
    /// "exit 0" | "exit 3" | "timeout 2s" | "esc" | "signal"
    pub outcome: String,
}

/// A saved app package, loaded from `<root>/<category>/<name>/app.json`.
#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub title: String,
    pub category: String,
    pub description: String,
    /// `sh` | `python` | `doc`
    pub kind: String,
    /// Relative entry file inside the app dir ("" for `doc`).
    pub entry: String,
    pub version: u32,
    pub created: String,
    pub updated: String,
    pub dir: PathBuf,
    /// Last full-screen run and last headless test, so the user and the model
    /// can see whether an app still works without running it.
    pub last_run: Option<RunStamp>,
    pub last_test: Option<RunStamp>,
    /// Binaries the app needs on the device (checked before every run/test).
    pub requires: Vec<String>,
    /// Arguments passed when the user launches the app from /apps without
    /// typing any (model-initiated app_run args still win when provided).
    pub default_args: String,
    /// Record the app's stdout/stderr to state/last-run.log while it runs.
    /// Implies a piped stdout (not a tty), so it is for text apps.
    pub capture: bool,
}

impl App {
    pub fn label(&self) -> String {
        format!("{}/{}", self.category, self.name)
    }
    /// Model-facing and UI-facing absolute path.
    pub fn path(&self) -> String {
        self.dir.display().to_string()
    }
}

pub fn apps_dir() -> PathBuf {
    crate::config::home().join("apps")
}

pub(crate) fn now_stamp() -> String {
    let out = Command::new("date").arg("+%Y-%m-%dT%H:%M:%S%z").output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.len() >= 19 {
                let (body, off) = s.split_at(19);
                let off = off.trim();
                if off.len() == 5 {
                    return format!("{body}{}:{}", &off[..3], &off[3..]);
                }
                return format!("{body}{off}");
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

/// Valid slug: lowercase ascii + digits + `_`/`-`, starts alphanumeric.
pub fn valid_slug(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    s.len() <= 32 && it.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A file path inside an app dir: relative, no `..`, no empty segments.
fn safe_rel_path(p: &str) -> Option<String> {
    let p = p.trim();
    if p.is_empty() || p.chars().count() > 120 || p.starts_with('/') || p.contains('\\') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if seg.chars().any(|c| c.is_control()) {
            return None;
        }
        parts.push(seg);
    }
    Some(parts.join("/"))
}

fn reserved_name(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p == "app.json" || p == "app.md" || p == "prompt.md"
}

fn stamp_json(r: &RunStamp) -> J {
    obj(vec![
        ("at", J::s(&r.at)),
        ("secs", J::Num(r.secs as f64)),
        ("outcome", J::s(&r.outcome)),
    ])
}

fn parse_stamp(v: &J) -> Option<RunStamp> {
    let at = v.get("at")?.as_str()?.to_string();
    Some(RunStamp {
        at,
        secs: v.get("secs").and_then(|x| x.as_num()).unwrap_or(0.0) as f32,
        outcome: v
            .get("outcome")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn manifest_json(a: &App) -> J {
    let mut pairs = vec![
        ("name", J::s(&a.name)),
        ("title", J::s(&a.title)),
        ("category", J::s(&a.category)),
        ("description", J::s(&a.description)),
        ("kind", J::s(&a.kind)),
        ("entry", J::s(&a.entry)),
        ("version", J::Num(a.version as f64)),
        ("created", J::s(&a.created)),
        ("updated", J::s(&a.updated)),
    ];
    if let Some(r) = &a.last_run {
        pairs.push(("last_run", stamp_json(r)));
    }
    if let Some(r) = &a.last_test {
        pairs.push(("last_test", stamp_json(r)));
    }
    if !a.requires.is_empty() {
        pairs.push((
            "requires",
            J::Arr(a.requires.iter().map(|r| J::s(r)).collect()),
        ));
    }
    if !a.default_args.is_empty() {
        pairs.push(("default_args", J::s(&a.default_args)));
    }
    if a.capture {
        pairs.push(("capture", J::Bool(true)));
    }
    obj(pairs)
}

fn parse_manifest(dir: &Path) -> Option<App> {
    let text = std::fs::read_to_string(dir.join("app.json")).ok()?;
    let v = crate::json::parse(&text)?;
    let name = v.get("name")?.as_str()?.trim().to_string();
    let category = v
        .get("category")
        .and_then(|x| x.as_str())
        .unwrap_or("apps")
        .trim()
        .to_string();
    if !valid_slug(&name) || !valid_slug(&category) {
        return None;
    }
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("doc").to_string();
    if !matches!(kind.as_str(), "sh" | "python" | "binary" | "doc" | "prompt") {
        return None;
    }
    Some(App {
        name,
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        category,
        description: v
            .get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        entry: v
            .get("entry")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        kind,
        version: v.get("version").and_then(|x| x.as_num()).unwrap_or(1.0).max(1.0) as u32,
        created: v.get("created").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        updated: v.get("updated").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        dir: dir.to_path_buf(),
        last_run: v.get("last_run").and_then(parse_stamp),
        last_test: v.get("last_test").and_then(parse_stamp),
        capture: v
            .get("capture")
            .map(|b| matches!(b, J::Bool(true)))
            .unwrap_or(false),
        default_args: v
            .get("default_args")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        requires: v
            .get("requires")
            .and_then(|x| x.as_arr())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Every well-formed app package under `root`, sorted by category then name.
pub fn load_at(root: &Path) -> Vec<App> {
    let mut out = Vec::new();
    let cats = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for cat in cats.flatten() {
        let cdir = cat.path();
        if !cdir.is_dir() {
            continue;
        }
        let names = match std::fs::read_dir(&cdir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in names.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            if let Some(app) = parse_manifest(&dir) {
                out.push(app);
                if out.len() >= MAX_APPS {
                    break;
                }
            }
        }
        if out.len() >= MAX_APPS {
            break;
        }
    }
    out.sort_by(|a, b| (a.category.as_str(), a.name.as_str()).cmp(&(b.category.as_str(), b.name.as_str())));
    out
}

pub fn load() -> Vec<App> {
    load_at(&apps_dir())
}

/// Resolve `name` or `category/name` against the loaded apps.
pub fn find(name: &str) -> Option<App> {
    let want = name.trim();
    if want.is_empty() {
        return None;
    }
    let apps = load();
    if let Some((cat, n)) = want.split_once('/') {
        return apps
            .into_iter()
            .find(|a| a.category.eq_ignore_ascii_case(cat) && a.name.eq_ignore_ascii_case(n));
    }
    apps.into_iter().find(|a| a.name.eq_ignore_ascii_case(want))
}

/// Compact "HH:MM" from an ISO stamp (only the time of a run matters in the
/// 12-row panel; the full timestamp stays in app.json).
pub fn stamp_hhmm(at: &str) -> String {
    let idx = at.find('T').map(|i| i + 1).unwrap_or(0);
    at[idx..].chars().take(5).collect()
}

/// A failed last run/test is the health flag shown in the UI and the catalog.
pub fn is_broken(app: &App) -> bool {
    [&app.last_run, &app.last_test].iter().any(|st| {
        st.as_ref()
            .map(|r| {
                !(r.outcome.is_empty() || r.outcome == "exit 0" || r.outcome == "esc")
            })
            .unwrap_or(false)
    })
}

/// Newest run/test timestamp (ISO strings with the same local offset sort
/// chronologically, so no date parsing is needed for "most recently used").
pub fn last_activity(app: &App) -> Option<&str> {
    let run = app.last_run.as_ref().map(|r| r.at.as_str()).filter(|s| !s.is_empty());
    let test = app.last_test.as_ref().map(|r| r.at.as_str()).filter(|s| !s.is_empty());
    match (run, test) {
        (Some(a), Some(b)) => Some(if a >= b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// "HH:MM" of the last use, for the /apps value column.
pub fn last_used_hhmm(app: &App) -> Option<String> {
    last_activity(app).map(stamp_hhmm)
}

/// The request body of a prompt app (never the generated APP.md header).
pub fn prompt_body(app: &App) -> String {
    let body = std::fs::read_to_string(app.dir.join("PROMPT.md"))
        .unwrap_or_else(|_| annotation(app));
    body.trim().to_string()
}

pub fn annotation(app: &App) -> String {
    std::fs::read_to_string(app.dir.join("APP.md")).unwrap_or_default()
}

/// Files in the app dir with their sizes, sorted, excluding the manifest.
pub fn files(app: &App) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(&app.dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name == "app.json" || name.starts_with('.') {
            continue;
        }
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        out.push((name, size));
    }
    out.sort();
    out
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    for e in rd.flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => total += dir_bytes(&e.path()),
            Ok(m) if m.is_file() => total += m.len(),
            _ => {}
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

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn chmod(_path: &Path, _mode: u32) {}

fn clean_line(s: &str, max: usize) -> String {
    let one: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(max).collect()
}

/// Create or update an app package. Returns the saved app and whether it was
/// newly created.
pub fn save_at(root: &Path, args: &J) -> Result<(App, bool), String> {
    let name = str_arg(args, "name")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !valid_slug(&name) {
        return Err(format!(
            "invalid app name '{name}': use lowercase letters, digits, '_' or '-' (max 32)"
        ));
    }
    let category = str_arg(args, "category")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "apps".to_string());
    if !valid_slug(&category) {
        return Err(format!(
            "invalid category '{category}': use lowercase letters, digits, '_' or '-' (max 32)"
        ));
    }
    let title = clean_line(&str_arg(args, "title").unwrap_or_default(), MAX_TITLE);
    if title.is_empty() {
        return Err("missing 'title': one short human title for the app".into());
    }
    let description = clean_line(&str_arg(args, "description").unwrap_or_default(), MAX_DESC);
    if description.is_empty() {
        return Err("missing 'description': one line saying what the app is for".into());
    }
    let annotation = str_arg(args, "annotation").unwrap_or_default();
    let annotation = annotation.trim();
    if annotation.is_empty() {
        return Err("missing 'annotation': the short APP.md text the user will read in /apps".into());
    }
    if annotation.chars().count() > MAX_ANNOTATION {
        return Err(format!("annotation is longer than {MAX_ANNOTATION} characters"));
    }
    let kind = str_arg(args, "kind")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "doc".to_string());
    if !matches!(kind.as_str(), "sh" | "python" | "binary" | "doc" | "prompt") {
        return Err(format!(
            "invalid kind '{kind}': use sh, python, binary, prompt or doc"
        ));
    }
    let entry = str_arg(args, "entry").unwrap_or_default();
    let entry = if kind == "doc" || kind == "prompt" {
        String::new()
    } else {
        match safe_rel_path(&entry) {
            Some(p) => p,
            None => {
                return Err(format!(
                    "invalid entry '{entry}': a relative file inside the app directory (e.g. run.sh)"
                ))
            }
        }
    };
    if kind != "doc" && kind != "prompt" && entry.is_empty() {
        return Err("missing 'entry': the file app_run will execute".into());
    }
    if kind == "binary" && entry.is_empty() {
        return Err("binary apps need 'entry': the executable file".into());
    }

    let dir = root.join(&category).join(&name);
    let existed = dir.join("app.json").is_file();
    let (created, version, last_run, last_test, old_capture, old_args, old_requires) = if existed {
        let old = parse_manifest(&dir).ok_or_else(|| {
            format!("{} already exists but its app.json is unreadable; fix it by hand", dir.display())
        })?;
        (
            old.created,
            old.version.saturating_add(1),
            old.last_run,
            old.last_test,
            old.capture,
            old.default_args,
            old.requires,
        )
    } else {
        (now_stamp(), 1, None, None, false, String::new(), Vec::new())
    };
    let capture = args
        .get("capture")
        .map(|b| matches!(b, J::Bool(true)))
        .unwrap_or(old_capture);
    let default_args = str_arg(args, "default_args")
        .map(|s| s.trim().chars().take(200).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or(old_args);
    if default_args.chars().count() > 200 {
        return Err("default_args is longer than 200 characters".into());
    }
    let requires: Vec<String> = match args.get("requires") {
        Some(J::Arr(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(J::Str(s)) => s
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => old_requires,
    };
    let mut requires = requires;
    requires.truncate(8);
    for r in &requires {
        if r.chars().count() > 64
            || !r
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        {
            return Err(format!(
                "invalid requirement '{r}': use a command name or absolute path"
            ));
        }
    }
    let app = App {
        name,
        title,
        category,
        description,
        kind,
        entry,
        version,
        created,
        updated: now_stamp(),
        dir: dir.clone(),
        last_run,
        last_test,
        capture,
        default_args,
        requires,
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    chmod(&dir, 0o755);
    // APP.md is the annotation itself plus a title header and a files list, so
    // `/apps` and `read_file` can show a usable document without extra data.
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n{}\n\n", app.title, app.description));
    md.push_str(&format!("**Kind:** {}  \n", app.kind));
    if !app.entry.is_empty() {
        md.push_str(&format!("**Entry:** `{}`  \n", app.entry));
    }
    md.push_str(&format!("**Path:** `{}`  \n\n", app.path()));
    md.push_str(annotation.trim());
    md.push('\n');
    write_atomic(&dir.join("app.json"), &(manifest_json(&app).dump() + "\n"))?;
    write_atomic(&dir.join("APP.md"), &md)?;
    // A prompt app's executable content is its prompt; keep it separate from
    // the human-facing annotation so metadata headers never pollute a request.
    if app.kind == "prompt" {
        write_atomic(&dir.join("PROMPT.md"), &format!("{}\n", annotation.trim()))?;
    } else {
        let _ = std::fs::remove_file(dir.join("PROMPT.md"));
    }
    Ok((app, !existed))
}

pub fn save(args: &J) -> Result<(App, bool), String> {
    save_at(&apps_dir(), args)
}

/// Write one file inside an existing app package.
pub fn write_file_at(root: &Path, args: &J) -> Result<(App, String, usize, bool), String> {
    let name = str_arg(args, "name").unwrap_or_default();
    let app = find_at(root, &name).ok_or_else(|| {
        format!("no app '{name}'; call app_save first to create the package")
    })?;
    let raw = str_arg(args, "path").unwrap_or_default();
    let path = safe_rel_path(&raw)
        .ok_or_else(|| format!("invalid path '{raw}': a relative file inside the app directory"))?;
    if reserved_name(&path) {
        return Err(format!(
            "'{path}' is managed by app_save (manifest/annotation); write it through app_save"
        ));
    }
    let content = str_arg(args, "content").unwrap_or_default();
    if content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "file is {} bytes; the per-file limit is {MAX_FILE_BYTES}",
            content.len()
        ));
    }
    let dest = app.dir.join(&path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let grown = dir_bytes(&app.dir).saturating_sub(
        dest.metadata().map(|m| m.len()).unwrap_or(0),
    ) + content.len() as u64;
    if grown > MAX_APP_BYTES as u64 {
        return Err(format!("app directory would exceed {MAX_APP_BYTES} bytes"));
    }
    std::fs::write(&dest, &content).map_err(|e| format!("{}: {e}", dest.display()))?;
    let executable = !app.entry.is_empty() && path == app.entry;
    chmod(&dest, if executable { 0o755 } else { 0o644 });
    let entry_ready = app.entry.is_empty() || app.dir.join(&app.entry).is_file();
    Ok((app, path, content.len(), entry_ready))
}

pub fn write_file(args: &J) -> Result<(App, String, usize, bool), String> {
    write_file_at(&apps_dir(), args)
}

fn find_at(root: &Path, name: &str) -> Option<App> {
    let want = name.trim();
    if want.is_empty() {
        return None;
    }
    let apps = load_at(root);
    if let Some((cat, n)) = want.split_once('/') {
        return apps
            .into_iter()
            .find(|a| a.category.eq_ignore_ascii_case(cat) && a.name.eq_ignore_ascii_case(n));
    }
    apps.into_iter().find(|a| a.name.eq_ignore_ascii_case(want))
}

pub fn delete(app: &App) -> Result<(), String> {
    let root = apps_dir();
    let cat = root.join(&app.category);
    if !app.dir.starts_with(&cat) {
        return Err("refusing to delete a directory outside the app root".into());
    }
    std::fs::remove_dir_all(&app.dir).map_err(|e| format!("{}: {e}", app.dir.display()))?;
    // Remove the category dir when it is now empty.
    let _ = std::fs::remove_dir(&cat);
    Ok(())
}

/// Is `prog` on PATH? python is optional on the device, sh is not.
fn command_exists(prog: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if !dir.is_empty() && Path::new(dir).join(prog).is_file() {
                return true;
            }
        }
    }
    false
}

/// Write one run/test stamp back into the app's manifest (best effort: a run
/// must not fail because history could not be written).
fn record_stamp(app: &App, key: &str, outcome: &str, secs: f32) {
    let path = app.dir.join("app.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut v = match crate::json::parse(&text) {
        Some(v) => v,
        None => return,
    };
    let stamp = RunStamp {
        at: now_stamp(),
        secs,
        outcome: outcome.to_string(),
    };
    v.set(key, stamp_json(&stamp));
    let _ = write_atomic(&path, &(v.dump() + "\n"));
}

/// Split user/model supplied app arguments into argv tokens.
fn app_argv(args: &str) -> Vec<String> {
    args.split_whitespace()
        .filter(|a| !a.is_empty())
        .map(|a| a.to_string())
        .take(32)
        .collect()
}

/// Refuse to run/test an app whose declared requirements are missing, with an
/// error that names the exact binary (the device may not have python/curl).
pub fn check_requires(app: &App) -> Result<(), String> {
    for req in &app.requires {
        let ok = if req.starts_with('/') {
            Path::new(req).is_file()
        } else {
            command_exists(req)
        };
        if !ok {
            return Err(format!(
                "app '{}' requires '{}', which is not installed on this device; \
install it or change the app to use something present",
                app.label(),
                req
            ));
        }
    }
    Ok(())
}

/// Like `run_plan`, with extra arguments appended after the entry file.
pub fn run_plan_args(app: &App, args: &str) -> Result<(String, Vec<String>), String> {
    if args.chars().count() > 200 || args.contains('\0') {
        return Err("app arguments are too long (max 200 characters)".into());
    }
    check_requires(app)?;
    let extra = app_argv(args);
    let (prog, mut argv) = run_plan_plain(app)?;
    argv.extend(extra);
    Ok((prog, argv))
}

fn run_plan_plain(app: &App) -> Result<(String, Vec<String>), String> {
    let entry = app.dir.join(&app.entry);
    match app.kind.as_str() {
        "sh" => {
            if !entry.is_file() {
                return Err(format!("entry '{}' is missing; write it with app_file", app.entry));
            }
            Ok(("/bin/sh".to_string(), vec![entry.display().to_string()]))
        }
        "python" => {
            let prog = if command_exists("python3") {
                "python3"
            } else if command_exists("python") {
                "python"
            } else {
                return Err("python is not installed on this device; use kind=sh".into());
            };
            if !entry.is_file() {
                return Err(format!("entry '{}' is missing; write it with app_file", app.entry));
            }
            Ok((prog.to_string(), vec![entry.display().to_string()]))
        }
        "binary" => {
            if !entry.is_file() {
                return Err(format!("entry '{}' is missing; write it with app_file", app.entry));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&entry).map(|m| m.permissions().mode()).unwrap_or(0);
                if mode & 0o111 == 0 {
                    return Err(format!(
                        "entry '{}' is not executable; chmod +x it (e.g. via bash) before app_run",
                        app.entry
                    ));
                }
            }
            Ok((entry.display().to_string(), Vec::new()))
        }
        "doc" => Err("doc apps have no program; open APP.md in /apps instead".into()),
        "prompt" => Err(
            "prompt apps are instructions, not programs; app_run returns their prompt for you to follow"
                .into(),
        ),
        other => Err(format!("unknown app kind '{other}'")),
    }
}

// ---------------------------------------------------------------------------
// catalog / prompt
// ---------------------------------------------------------------------------

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push_str("...");
    }
    out.replace('\n', " ")
}

/// One-line-per-app catalog, skills-style (name/title/description/kind only).
pub fn catalog(apps: &[App]) -> String {
    let mut s = String::new();
    for app in apps.iter().take(MAX_INDEX) {
        let desc = if app.description.is_empty() {
            String::new()
        } else {
            format!(": {}", short(&app.description, 90))
        };
        let kind = if app.entry.is_empty() {
            app.kind.clone()
        } else {
            format!("{}:{}", app.kind, app.entry)
        };
        // A failed last run/test is the one thing the model must know before
        // deciding to reuse or rewrite an app; successful history stays out of
        // the prompt to keep the catalog cheap.
        let mut health = String::new();
        for (label, stamp) in [("last run", &app.last_run), ("last test", &app.last_test)] {
            if let Some(st) = stamp {
                let bad = !(st.outcome == "exit 0" || st.outcome == "esc" || st.outcome.is_empty());
                if bad {
                    health.push_str(&format!(" (!{label}: {})", st.outcome));
                    break;
                }
            }
        }
        s.push_str(&format!(
            "- {} — {} [{}]{}{}\n",
            app.label(),
            if app.title.is_empty() { &app.name } else { &app.title },
            kind,
            health,
            desc
        ));
    }
    if apps.len() > MAX_INDEX {
        s.push_str(&format!("- (+{} more)\n", apps.len() - MAX_INDEX));
    }
    s
}

/// The creator-mode block appended to the system prompt, or "" in normal mode.
pub fn prompt_section(creator: bool, apps: &[App]) -> String {
    if !creator {
        return String::new();
    }
    let root = apps_dir().display().to_string();
    let mut s = String::from("\n\nCREATOR MODE — you own this device\n");
    s.push_str(
        "You are the only application on this Cardputer Zero. The panel, the keyboard and \
the storage are yours: everything the user sees is something you built or will build.\n\
\n\
APPS — persistent things you own\n\
- App root: ",
    );
    s.push_str(&root);
    s.push_str(
        "/<category>/<name>/ holds app.json (manifest), APP.md (annotation), the entry \
program and any supporting files.\n\
- When the user asks you to build, change or automate something for the device, persist it \
as an app before your final answer: app_save creates/updates the package and its annotation, \
app_file writes the files, app_run runs it full-screen on the panel.\n\
- Write APP.md for the user who opens /apps next week: what it does, how to use it, how it \
works, which files matter. Keep it short and factual; the title/description are one line each.\n\
- Update an existing app instead of duplicating it: keep its name, bump the version, rewrite \
only the files that changed. The installed catalog below is your list of what already exists.\n\
- A pure question (\"what is...\") stays a chat answer; a document-only deliverable is a valid \
app (kind=doc, APP.md is the content).\n\
- Device limits for apps: 320x170 px, 53 columns x 12 rows, 8 ANSI colours, no mouse. Use sh \
+ coreutils (/proc, /sys); python only if the device has it. No installs, no services, no \
compilation, no full-screen programs that need a GUI toolkit.\n\
- app_run needs an interactive screen: it fails from background jobs and --once, which is \
expected. It takes optional `args` (space-separated, passed to the entry as argv) and a timeout.\n\
- App state: write runtime state under $DST_APP_STATE (a `state/` dir inside the app); keep \
APP.md and app.json free of generated data. Apps draw on stdout (newlines are CRLF-safe) and \
read keys from stdin; the platform guarantees a long-press of Esc and a timeout.\n\
- Kinds: sh and python scripts, binary (an executable file), prompt (a reusable request you \
follow when run; APP.md is the prompt), doc.\n\
- APP CONTRACT (bounded, the platform enforces what it can): an app owns the whole screen \
while it runs and dst is paused; it may not change system configuration, install software, \
start services, or write outside its package/state/tmp; it must exit (0 = success) and leave \
the screen recoverable (the platform repaints). Never build an app that blocks the keyboard \
or ignores the Esc/timeout backstop.\n\
- Verify before you show it: call app_test (captured stdout/stderr/exit, log file under \
~/.dst/tmp) and fix failures; test the happy path plus bad input. app_test is headless by \
default; with tty=true it runs under a pseudo-terminal like app_run, which is how you test \
[ -t ] branches and ANSI/TUI output (the pty has no window size, so COLUMNS/LINES stand in \
for the real size). app_run is for the user to see the real full-screen behaviour.\n\
- app_export / app_import move a package as a .tar.gz under ~/.dst/tmp (backup, copy off the \
device, restore later); an imported archive must contain exactly one <category>/<name>/ tree \
and is validated before it is installed (replace=true to overwrite).\n\
- Declare the binaries your app needs in `requires` (e.g. [\"python3\", \"tar\"]); dst checks \
them before every run/test and fails with an actionable error if one is missing.\n\
- Use default_args for an app that needs a fixed argument when the user launches it from /apps; \
app_run args override them for a one-off run.\n\
- Set capture=true on an app that prints text and you want the run remembered: dst records its \
stdout/stderr to state/last-run.log (shown in /apps with `l`) and mentions the path in app_run's \
result. Capture makes stdout a pipe, not a tty, so do not use it for full-screen TUI apps.\n\
\n\
WHAT TO PERSIST — app vs skill vs memory\n\
- app: something runnable on the device (app_save/app_file, verified with app_test).\n\
- skill: a systematic, self-contained engineering package you worked out (procedure, format, \
checklist, support files) that will be useful again — save it with skill_save/skill_file. \
Updating a skill bumps its version and keeps the old body under .history/ (restore one with \
skill_rollback); it joins the SKILLS catalog next turn and is read with the skill tool (also \
usable in the same turn).\n\

- memory: short durable facts or preferences (memory_write); memory is fragments, a skill is \
a complete procedure. Prefer a skill over dumping a long procedure into memory.\n\
\n\
CONSENSUS — the platform guarantees this, you cannot override it\n\
- Long-press Esc always leaves an app and returns to the chat: the runner watches the \
keyboard and kills the app. Never try to swallow Esc or build a fixed-duration blocking \
screen.\n\
- Apps run full-screen; keep drawing inside 53 columns x 12 rows and always leave a way to \
stop (the platform Esc already does).\n",
    );
    s.push_str(&format!("\nINSTALLED APPS ({})\n", apps.len()));
    if apps.is_empty() {
        s.push_str("(none yet — this is the first app you build)\n");
    } else {
        s.push_str(&catalog(apps));
    }
    s
}

// ---------------------------------------------------------------------------
// tool definitions
// ---------------------------------------------------------------------------

fn save_exec(args: &J) -> Result<J, Fail> {
    let (app, created) = save(args).map_err(Fail::new)?;
    note_saved(&app);
    let entry_missing = !app.entry.is_empty() && !app.dir.join(&app.entry).is_file();
    Ok(obj(vec![
        ("name", J::s(&app.label())),
        ("title", J::s(&app.title)),
        ("path", J::s(&app.path())),
        ("kind", J::s(&app.kind)),
        ("entry", J::s(&app.entry)),
        ("version", J::Num(app.version as f64)),
        ("created", J::Bool(created)),
        ("entry_missing", J::Bool(entry_missing)),
    ]))
}

fn save_render(_a: &J, v: &J) -> String {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("");
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let version = v.get("version").and_then(|x| x.as_num()).unwrap_or(1.0) as u32;
    let mut s = format!("saved app {name} v{version} ({kind}) at {path}");
    if v.get("entry_missing").map(|b| matches!(b, J::Bool(true))).unwrap_or(false) {
        let entry = v.get("entry").and_then(|x| x.as_str()).unwrap_or("");
        s.push_str(&format!(
            "\nentry '{entry}' is not written yet; create it with app_file before app_run"
        ));
    }
    s
}

fn save_present(a: &J) -> String {
    format!(
        "save {}",
        str_arg(a, "name").unwrap_or_else(|| "app".into())
    )
}

fn save_post(a: &J, v: &J) -> Option<String> {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let entry = v.get("entry").and_then(|x| x.as_str()).unwrap_or("");
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("");
    let _ = a;
    let mut s = format!(
        "App package '{name}' is installed at {path}. It appears in INSTALLED APPS from your next turn on."
    );
    if kind != "doc" && !entry.is_empty() {
        s.push_str(&format!(
            " Write the program with app_file (entry file: {entry}); rewrite an existing file to change behaviour."
        ));
    }
    Some(s)
}

fn file_exec(args: &J) -> Result<J, Fail> {
    let (app, path, bytes, entry_ready) = write_file(args).map_err(Fail::new)?;
    Ok(obj(vec![
        ("name", J::s(&app.label())),
        ("path", J::s(&path)),
        ("bytes", J::Num(bytes as f64)),
        ("entry_ready", J::Bool(entry_ready)),
    ]))
}

fn file_render(_a: &J, v: &J) -> String {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let bytes = v.get("bytes").and_then(|x| x.as_num()).unwrap_or(0.0) as usize;
    let mut s = format!("wrote {path} ({bytes} bytes) in app {name}");
    if v.get("entry_ready").map(|b| matches!(b, J::Bool(true))).unwrap_or(false) {
        s.push_str("; the entry is ready to run");
    }
    s
}

fn file_present(a: &J) -> String {
    format!(
        "write {}/{}",
        str_arg(a, "name").unwrap_or_default(),
        str_arg(a, "path").unwrap_or_default()
    )
}

fn run_exec(args: &J) -> Result<J, Fail> {
    let name = str_arg(args, "name").unwrap_or_default();
    let app = find(&name).ok_or_else(|| {
        Fail::new(format!(
            "no app '{name}'; saved apps are listed in INSTALLED APPS"
        ))
    })?;
    let timeout = num_arg(args, "timeout_s")
        .unwrap_or(DEFAULT_TIMEOUT_S as f64)
        .clamp(1.0, MAX_TIMEOUT_S as f64) as u64;
    let supplied = str_arg(args, "args").unwrap_or_default();
    let extra = if supplied.trim().is_empty() {
        app.default_args.clone()
    } else {
        supplied
    };
    if app.kind == "prompt" {
        let body = prompt_body(&app);
        if body.trim().is_empty() {
            return Err(Fail::new(format!(
                "prompt app '{}' has an empty APP.md; nothing to follow",
                app.label()
            )));
        }
        let result = format!(
            "app '{}' is a prompt app. Follow these instructions now for the current request:\n\n{}",
            app.label(),
            body.trim()
        );
        return Ok(obj(vec![
            ("name", J::s(&app.label())),
            ("result", J::s(&result)),
        ]));
    }
    let result = request_launch(&app, timeout, &extra).map_err(Fail::new)?;
    Ok(obj(vec![
        ("name", J::s(&app.label())),
        ("result", J::s(&result)),
    ]))
}

/// Outcome of one headless app test (the model's own way of checking an app
/// without giving it the panel).
struct HeadlessRun {
    code: Option<i32>,
    timed_out: bool,
    secs: f32,
    stdout: String,
    stderr: String,
    log_path: String,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

const CAPTURE_CAP: usize = 64 * 1024;

fn read_capped(mut r: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<(Vec<u8>, usize)> {
    std::thread::spawn(move || {
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        let mut total = 0usize;
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total += n;
                    if out.len() < CAPTURE_CAP {
                        let room = CAPTURE_CAP - out.len();
                        out.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
        }
        (out, total)
    })
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Run an app under a pseudo-terminal (headless testing of TUI-ish apps):
/// `script(1)` allocates the pty and we capture its output. The pty window size
/// is 0x0 (script cannot set it), so apps that honour COLUMNS/LINES still see
/// the panel grid; apps that query the size via ioctl see zeros.
fn tty_command(prog: &str, argv: &[String]) -> Command {
    let caps = "ulimit -c 0 2>/dev/null; ulimit -t 120 2>/dev/null; \
                ulimit -f 2048 2>/dev/null; ";
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        let wrapper = format!("{caps}exec \"$0\" \"$@\"");
        cmd.arg("-q")
            .arg("/dev/null")
            .arg("/bin/sh")
            .arg("-c")
            .arg(wrapper)
            .arg(prog)
            .args(argv);
    } else {
        let mut line = format!("{}", shell_quote(prog));
        for a in argv {
            line.push(' ');
            line.push_str(&shell_quote(a));
        }
        cmd.arg("-q")
            .arg("-e")
            .arg("-c")
            .arg(format!("{caps}exec {line}"))
            .arg("/dev/null");
    }
    cmd
}

fn run_headless(
    app: &App,
    args: &str,
    stdin_text: &str,
    timeout_s: u64,
    tty: bool,
) -> Result<HeadlessRun, String> {
    let (prog, argv) = run_plan_args(app, args)?;
    let state_dir = app.dir.join("state");
    let _ = std::fs::create_dir_all(&state_dir);
    let started = std::time::Instant::now();
    let mut cmd = if tty {
        tty_command(&prog, &argv)
    } else {
        app_command(&prog, &argv)
    };
    cmd.current_dir(&app.dir);
    app_env(&mut cmd, app, &state_dir);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start app '{}' ({prog}): {e}", app.label()))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A pty is in canonical mode: without a newline the app's `read` blocks.
        let text = if tty && !stdin_text.is_empty() && !stdin_text.ends_with('\n') {
            format!("{stdin_text}\n")
        } else {
            stdin_text.to_string()
        };
        let _ = stdin.write_all(text.as_bytes());
        let _ = stdin.flush();
        if tty && !stdin_text.is_empty() {
            // The pty needs a moment to deliver the input before we close the
            // pipe; otherwise script(1) can drop the bytes on startup.
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        // Dropping closes the pipe: apps that read until EOF terminate.
    }
    let out_thread = child.stdout.take().map(read_capped);
    let err_thread = child.stderr.take().map(read_capped);
    let timeout = std::time::Duration::from_secs(timeout_s.max(1));
    let mut timed_out = false;
    let mut status = None;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                status = Some(st);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (stdout, stdout_bytes) = out_thread
        .and_then(|h| h.join().ok())
        .unwrap_or((Vec::new(), 0));
    let (stderr, stderr_bytes) = err_thread
        .and_then(|h| h.join().ok())
        .unwrap_or((Vec::new(), 0));
    let secs = started.elapsed().as_secs_f32();
    let code = status.and_then(|s| s.code());
    let mut log = String::new();
    log.push_str(&format!(
        "# app_test {} {}{}\n",
        app.label(),
        args,
        if tty { " (pty)" } else { "" }
    ));
    log.push_str(&format!(
        "# exit={:?} timed_out={} secs={secs:.2} stdout={stdout_bytes}B stderr={stderr_bytes}B\n",
        code, timed_out
    ));
    log.push_str("\n=== stdout ===\n");
    log.push_str(&String::from_utf8_lossy(&stdout));
    log.push_str("\n=== stderr ===\n");
    log.push_str(&String::from_utf8_lossy(&stderr));
    let safe: String = app
        .label()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let dir = crate::config::tmp_dir();
    let _ = std::fs::create_dir_all(&dir);
    let log_path = dir.join(format!("app-test-{safe}.log"));
    let _ = std::fs::write(&log_path, &log);
    let outcome = if timed_out {
        format!("timeout {timeout_s}s")
    } else {
        match code {
            Some(c) => format!("exit {c}"),
            None => "signal".to_string(),
        }
    };
    record_stamp(app, "last_test", &outcome, secs);
    Ok(HeadlessRun {
        code,
        timed_out,
        secs,
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
        log_path: log_path.display().to_string(),
        stdout_bytes,
        stderr_bytes,
    })
}

fn test_exec(args: &J) -> Result<J, Fail> {
    let name = str_arg(args, "name").unwrap_or_default();
    let app = find(&name).ok_or_else(|| {
        Fail::new(format!(
            "no app '{name}'; saved apps are listed in INSTALLED APPS"
        ))
    })?;
    if app.kind == "prompt" || app.kind == "doc" {
        return Err(Fail::new(format!(
            "app '{}' is a {} app and has no program to test",
            app.label(),
            app.kind
        )));
    }
    let timeout = num_arg(args, "timeout_s")
        .unwrap_or(10.0)
        .clamp(1.0, 120.0) as u64;
    let extra = str_arg(args, "args").unwrap_or_default();
    let stdin_text = str_arg(args, "stdin").unwrap_or_default();
    let tty = args.get("tty").map(|b| matches!(b, J::Bool(true))).unwrap_or(false);
    let run = run_headless(&app, &extra, &stdin_text, timeout, tty).map_err(Fail::new)?;
    Ok(obj(vec![
        ("name", J::s(&app.label())),
        ("exit", J::Num(run.code.map(|c| c as f64).unwrap_or(-1.0))),
        ("timed_out", J::Bool(run.timed_out)),
        ("secs", J::Num(run.secs as f64)),
        ("stdout", J::s(&run.stdout)),
        ("stderr", J::s(&run.stderr)),
        ("stdout_bytes", J::Num(run.stdout_bytes as f64)),
        ("stderr_bytes", J::Num(run.stderr_bytes as f64)),
        ("log", J::s(&run.log_path)),
        ("tty", J::Bool(tty)),
    ]))
}

fn test_render(_a: &J, v: &J) -> String {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let exit = v.get("exit").and_then(|x| x.as_num()).unwrap_or(-1.0) as i32;
    let timed = v
        .get("timed_out")
        .map(|b| matches!(b, J::Bool(true)))
        .unwrap_or(false);
    let secs = v.get("secs").and_then(|x| x.as_num()).unwrap_or(0.0);
    let log = v.get("log").and_then(|x| x.as_str()).unwrap_or("");
    let out = v.get("stdout").and_then(|x| x.as_str()).unwrap_or("");
    let err = v.get("stderr").and_then(|x| x.as_str()).unwrap_or("");
    let on_tty = v.get("tty").map(|b| matches!(b, J::Bool(true))).unwrap_or(false);
    let mut s = format!(
        "app_test {name}: exit={exit}{} in {secs:.2}s ({})",
        if timed { " TIMEOUT" } else { "" },
        if on_tty { "pty, no screen" } else { "headless, no screen" }
    );
    if !out.trim().is_empty() {
        s.push_str(&format!("\n--- stdout ---\n{}", short(out.trim_end(), 1200)));
    }
    if !err.trim().is_empty() {
        s.push_str(&format!("\n--- stderr ---\n{}", short(err.trim_end(), 1200)));
    }
    s.push_str(&format!("\nfull log: {log}"));
    s
}

fn export_exec(args: &J) -> Result<J, Fail> {
    let name = str_arg(args, "name").unwrap_or_default();
    let app = find(&name).ok_or_else(|| {
        Fail::new(format!(
            "no app '{name}'; saved apps are listed in INSTALLED APPS"
        ))
    })?;
    let root = apps_dir();
    let safe: String = app
        .label()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let dir = crate::config::tmp_dir();
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("app-{safe}.tar.gz"));
    let out = Command::new("tar")
        .arg("-czf")
        .arg(&dest)
        .arg("-C")
        .arg(&root)
        .arg(app.label())
        .output()
        .map_err(|e| Fail::new(format!("tar is not available: {e}")))?;
    if !out.status.success() {
        return Err(Fail::new(format!(
            "tar failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    Ok(obj(vec![
        ("name", J::s(&app.label())),
        ("path", J::s(&dest.display().to_string())),
        ("bytes", J::Num(bytes as f64)),
    ]))
}

fn count_tree(dir: &Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => {
                    let (f, b) = count_tree(&e.path());
                    files += f;
                    bytes += b;
                }
                Ok(m) if m.is_file() => {
                    files += 1;
                    bytes += m.len();
                }
                _ => {}
            }
        }
    }
    (files, bytes)
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

fn import_exec(args: &J) -> Result<J, Fail> {
    let raw = str_arg(args, "path").unwrap_or_default();
    if raw.trim().is_empty() {
        return Err(Fail::new("missing 'path': an archive under ~/.dst/tmp"));
    }
    let replace = args
        .get("replace")
        .map(|b| matches!(b, J::Bool(true)))
        .unwrap_or(false);
    let tmp_root = crate::config::tmp_dir();
    let _ = std::fs::create_dir_all(&tmp_root);
    let candidate = if raw.trim().starts_with('/') || raw.trim().starts_with('~') {
        crate::config::expand(raw.trim())
    } else {
        tmp_root.join(raw.trim())
    };
    let canon_tmp = std::fs::canonicalize(&tmp_root)
        .map_err(|e| Fail::new(format!("tmp dir: {e}")))?;
    let archive = std::fs::canonicalize(&candidate)
        .map_err(|e| Fail::new(format!("archive not found: {e}")))?;
    if !archive.starts_with(&canon_tmp) {
        return Err(Fail::new(
            "refusing to import from outside ~/.dst/tmp; copy the archive there first",
        ));
    }
    let name = archive
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("archive");
    if !(name.ends_with(".tar.gz") || name.ends_with(".tgz")) {
        return Err(Fail::new("archive must be a .tar.gz (use app_export)"));
    }
    if let Ok(m) = archive.metadata() {
        if m.len() > 4 * 1024 * 1024 {
            return Err(Fail::new("archive is larger than 4 MB"));
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let work = tmp_root.join(format!("import-{}-{stamp}", std::process::id()));
    cleanup(&work);
    std::fs::create_dir_all(&work).map_err(|e| Fail::new(format!("{}: {e}", work.display())))?;
    let out = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .output()
        .map_err(|e| {
            cleanup(&work);
            Fail::new(format!("tar is not available: {e}"))
        })?;
    if !out.status.success() {
        cleanup(&work);
        return Err(Fail::new(format!(
            "tar failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // The archive must be exactly one <category>/<name>/ package; anything else
    // (dotfiles, multiple roots, absolute paths escaping the temp dir) fails.
    let mut roots: Vec<PathBuf> = std::fs::read_dir(&work)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    if roots.len() != 1 {
        cleanup(&work);
        return Err(Fail::new(
            "archive must contain exactly one <category>/<name>/ package",
        ));
    }
    let cat_dir = roots.pop().unwrap();
    let cat_name = cat_dir
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_string();
    if !valid_slug(&cat_name) {
        cleanup(&work);
        return Err(Fail::new(format!("invalid category in archive: '{cat_name}'")));
    }
    let mut apps_found: Vec<PathBuf> = std::fs::read_dir(&cat_dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    if apps_found.len() != 1 {
        cleanup(&work);
        return Err(Fail::new(
            "archive must contain exactly one app directory inside the category",
        ));
    }
    let src_dir = apps_found.pop().unwrap();
    let src_app = match parse_manifest(&src_dir) {
        Some(a) => a,
        None => {
            cleanup(&work);
            return Err(Fail::new("archive has no valid app.json"));
        }
    };
    let (files, bytes) = count_tree(&src_dir);
    if files > 200 || bytes > MAX_APP_BYTES as u64 {
        cleanup(&work);
        return Err(Fail::new(format!(
            "archive package is too big ({files} files, {bytes} bytes)"
        )));
    }
    let target = apps_dir().join(&src_app.category).join(&src_app.name);
    let existed = target.exists();
    if existed && !replace {
        cleanup(&work);
        return Err(Fail::new(format!(
            "app {} already exists; pass replace=true to overwrite",
            src_app.label()
        )));
    }
    if existed {
        std::fs::remove_dir_all(&target)
            .map_err(|e| Fail::new(format!("{}: {e}", target.display())))?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Fail::new(format!("{}: {e}", parent.display())))?;
    }
    std::fs::rename(&src_dir, &target).map_err(|e| {
        cleanup(&work);
        Fail::new(format!("could not install into {}: {e}", target.display()))
    })?;
    cleanup(&work);
    Ok(obj(vec![
        ("name", J::s(&src_app.label())),
        ("path", J::s(&target.display().to_string())),
        ("files", J::Num(files as f64)),
        ("replaced", J::Bool(existed)),
    ]))
}

fn import_render(_a: &J, v: &J) -> String {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let files = v.get("files").and_then(|x| x.as_num()).unwrap_or(0.0) as usize;
    let replaced = v
        .get("replaced")
        .map(|b| matches!(b, J::Bool(true)))
        .unwrap_or(false);
    format!(
        "{} app {name} ({files} files) at {}; it appears in INSTALLED APPS next turn",
        if replaced { "replaced" } else { "imported" },
        v.get("path").and_then(|x| x.as_str()).unwrap_or("")
    )
}

fn import_present(a: &J) -> String {
    format!(
        "import {}",
        str_arg(a, "path")
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or("archive")
    )
}

fn export_render(_a: &J, v: &J) -> String {
    format!(
        "exported app {} to {} ({} bytes); keep it as a backup or copy it off the device",
        v.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        v.get("path").and_then(|x| x.as_str()).unwrap_or(""),
        v.get("bytes").and_then(|x| x.as_num()).unwrap_or(0.0) as usize
    )
}

fn export_present(a: &J) -> String {
    format!("export {}", str_arg(a, "name").unwrap_or_else(|| "app".into()))
}

fn test_present(a: &J) -> String {
    format!("test {}", str_arg(a, "name").unwrap_or_else(|| "app".into()))
}

fn run_render(_a: &J, v: &J) -> String {
    v.get("result")
        .and_then(|x| x.as_str())
        .unwrap_or("app_run finished")
        .to_string()
}

fn run_present(a: &J) -> String {
    format!(
        "run {}",
        str_arg(a, "name").unwrap_or_else(|| "app".into())
    )
}

/// Creator-mode tools. Registered only while `tui.mode: creator`.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "app_save",
            description: "Create or update a persistent app package under ~/.dst/apps (creator mode). \
Every build request must end with this, plus app_file for the program. Writes app.json (manifest) and \
APP.md (short annotation for the user). Updating an existing name bumps its version and keeps files \
you do not rewrite.",
            params: vec![
                Param::req("name", "string", "Slug: lowercase letters/digits/_/-, unique in its category."),
                Param::req("title", "string", "Short human title shown in /apps (max 60 chars)."),
                Param::req("description", "string", "One line: what this app is for (max 160 chars)."),
                Param::req("annotation", "string", "APP.md body for the user: what it does, how to use it, how it works, files. Markdown."),
                Param::opt("category", "string", "Grouping dir: system, tools, games, apps (default apps)."),
                Param::one_of_opt(
                    "kind",
                    "How app_run executes it: sh/python script, binary executable, prompt (instructions), doc.",
                    &["sh", "python", "binary", "prompt", "doc"],
                ),
                Param::opt("entry", "string", "Relative entry file (e.g. run.sh); required unless kind=doc."),
                Param::opt("capture", "boolean", "Record stdout/stderr of app_run into state/last-run.log and replay it from /apps. Sets stdout to a pipe (not a tty): use for text apps, not full-screen TUIs."),
                Param::opt("default_args", "string", "Arguments used when the user launches the app from /apps (max 200 chars); app_run args override them."),
                Param::opt("requires", "array", "Command names or absolute paths the app needs (e.g. [\"python3\",\"/usr/bin/curl\"]); dst refuses to run/test when one is missing. Max 8."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: save_exec,
            render: save_render,
            present: save_present,
            post: Some(save_post),
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "app_file",
            description: "Write one text file inside a saved app package (~/.dst/apps/<category>/<name>/). \
Use it for the entry program and supporting files; the app directory is the only place it can write.",
            params: vec![
                Param::req("name", "string", "App name or category/name from INSTALLED APPS."),
                Param::req("path", "string", "Relative path inside the app dir, e.g. run.sh."),
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
        ToolDef {
            name: "app_test",
            description: "Run a saved app HEADLESS (pipes, no screen) and capture its output, exit code \
and duration. Use this to verify an app's behaviour yourself before app_run: it can drive stdin with \
the `stdin` argument, pass argv with `args`, and writes the full log to a file you can read with \
read_file. This is the model's version of the simulator the developers use.",
            params: vec![
                Param::req("name", "string", "App name or category/name from INSTALLED APPS."),
                Param::opt("args", "string", "Space-separated argv for the app."),
                Param::opt("stdin", "string", "Text piped to the app's stdin (closed afterwards)."),
                Param::opt("timeout_s", "integer", "Seconds before the app is killed (default 10, max 120)."),
                Param::opt("tty", "boolean", "Run under a pseudo-terminal (like app_run) so [ -t ] branches and TUI output can be tested; the pty has no window size (COLUMNS/LINES env still say 53x12)."),
            ],
            concurrency_safe: false,
            timeout_ms: 130_000,
            execute: test_exec,
            render: test_render,
            present: test_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "app_export",
            description: "Pack a saved app (manifest, annotation, program and state dir) into a \
.tar.gz under ~/.dst/tmp and return the path. Use it for backups or to hand an app to the user \
without touching the panel.",
            params: vec![
                Param::req("name", "string", "App name or category/name from INSTALLED APPS."),
            ],
            concurrency_safe: false,
            timeout_ms: 20_000,
            execute: export_exec,
            render: export_render,
            present: export_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "app_import",
            description: "Restore an app package from a .tar.gz under ~/.dst/tmp (as produced by \
app_export). The archive must contain exactly one <category>/<name>/ tree; it is validated and \
installed into ~/.dst/apps. Pass replace=true to overwrite an installed app.",
            params: vec![
                Param::req("path", "string", "Archive path under ~/.dst/tmp (absolute, ~/..., or a bare file name)."),
                Param::opt("replace", "boolean", "Overwrite an already installed app with the same name."),
            ],
            concurrency_safe: false,
            timeout_ms: 20_000,
            execute: import_exec,
            render: import_render,
            present: import_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "app_run",
            description: "Run a saved app full-screen on the device panel and return when it exits, \
is stopped by a long-press of Esc, or reaches timeout_s. The user sees the app, not the chat, while \
it runs; do not repeat the app's content afterwards. Only available in the interactive TUI.",
            params: vec![
                Param::req("name", "string", "App name or category/name from INSTALLED APPS."),
                Param::opt("timeout_s", "integer", "Seconds before the platform stops the app (default 60, max 600)."),
                Param::opt("args", "string", "Space-separated arguments passed to the app after its entry file (max 200 chars)."),
            ],
            concurrency_safe: false,
            timeout_ms: (MAX_TIMEOUT_S + 60) * 1_000,
            execute: run_exec,
            render: run_render,
            present: run_present,
            post: None,
            schema_override: None,
            dynamic: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// screen takeover: request/reply between a tool thread and the TUI
// ---------------------------------------------------------------------------

enum LaunchMsg {
    Picked,
    Done(Result<String, String>),
}

struct LaunchRequest {
    app: App,
    timeout_s: u64,
    args: String,
    tx: std::sync::mpsc::Sender<LaunchMsg>,
}

static PENDING: Mutex<Option<LaunchRequest>> = Mutex::new(None);
static SCREEN_AVAILABLE: Mutex<bool> = Mutex::new(false);
/// Apps saved during the current turn, for the transcript notice. (label, path)
static SAVED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Drain the apps saved since the last call (creator-mode turn notice).
pub fn take_saved() -> Vec<(String, String)> {
    match SAVED.lock() {
        Ok(mut g) => std::mem::take(&mut *g),
        Err(_) => Vec::new(),
    }
}

fn note_saved(app: &App) {
    if let Ok(mut g) = SAVED.lock() {
        g.push((app.label(), app.path()));
    }
}

/// The interactive TUI calls this once the panel is ready.
pub fn set_screen_available(on: bool) {
    if let Ok(mut g) = SCREEN_AVAILABLE.lock() {
        *g = on;
    }
}

fn screen_available() -> bool {
    SCREEN_AVAILABLE.lock().map(|g| *g).unwrap_or(false)
}

/// Blocking request from a tool thread: "give the panel to this app".
pub fn request_launch(app: &App, timeout_s: u64, args: &str) -> Result<String, String> {
    if !screen_available() {
        return Err(
            "app_run needs the interactive screen; it is not available in --once, background jobs or --call"
                .to_string(),
        );
    }
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let mut g = PENDING.lock().map_err(|_| "app launcher state is poisoned")?;
        if g.is_some() {
            return Err("another app is already using the screen".to_string());
        }
        *g = Some(LaunchRequest {
            app: app.clone(),
            timeout_s,
            args: args.chars().take(200).collect(),
            tx,
        });
    }
    match rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(LaunchMsg::Picked) => {}
        Ok(LaunchMsg::Done(r)) => return r,
        Err(_) => {
            let mut g = PENDING.lock().map_err(|_| "app launcher state is poisoned")?;
            *g = None;
            return Err("the TUI did not pick up the app run request".to_string());
        }
    }
    match rx.recv_timeout(std::time::Duration::from_secs(timeout_s + 90)) {
        Ok(LaunchMsg::Done(r)) => r,
        _ => Err("the app did not return control to dst".to_string()),
    }
}

/// Called from the tool executor's UI pump (every ~15 ms while tools run).
pub fn service_launch(ui: &mut dyn crate::harness::Ui) {
    if !screen_available() {
        return;
    }
    let req = match PENDING.lock() {
        Ok(mut g) => g.take(),
        Err(_) => return,
    };
    let req = match req {
        Some(r) => r,
        None => return,
    };
    let _ = req.tx.send(LaunchMsg::Picked);
    let result = ui.run_app(&req.app, req.timeout_s, &req.args);
    let _ = req.tx.send(LaunchMsg::Done(result));
}

// ---------------------------------------------------------------------------
// keyboard hold detection (unit-testable)
// ---------------------------------------------------------------------------

const ESC_HOLD_BYTES: u32 = 3;
const ESC_HOLD_MS: u128 = 450;

/// Long-press Esc: key auto-repeat produces several ESC bytes for a real hold,
/// while a tap is a single byte. Mirrors the Tab hold detector.
#[derive(Default)]
pub struct EscWatch {
    n: u32,
    first: Option<std::time::Instant>,
}

impl EscWatch {
    pub fn new() -> EscWatch {
        EscWatch::default()
    }
    /// Feed one byte; true means the user has been holding Esc.
    pub fn feed(&mut self, byte: u8, now: std::time::Instant) -> bool {
        if byte != 0x1b {
            self.n = 0;
            self.first = None;
            return false;
        }
        let first = *self.first.get_or_insert(now);
        self.n += 1;
        self.n >= ESC_HOLD_BYTES && now.duration_since(first).as_millis() >= ESC_HOLD_MS
    }
}

fn stty(args: &[&str]) {
    crate::stty_tty(args);
}

fn clear_screen() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[0m\x1b[?25h\x1b[2J\x1b[H");
    let _ = out.flush();
}

/// Start an app under the platform's bounded-run contract: no core dumps, a
/// CPU-time cap and a per-file size cap (the prompt's APP CONTRACT explains the
/// same rules to the model; this is the part the platform can enforce).
fn app_command(prog: &str, argv: &[String]) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(
            "ulimit -c 0 2>/dev/null; ulimit -t 120 2>/dev/null; \
             ulimit -f 2048 2>/dev/null; exec \"$0\" \"$@\"",
        )
        .arg(prog)
        .args(argv);
    cmd
}

/// Environment for an app process: a minimal, scrubbed environment with its
/// own HOME/TMPDIR inside the package state. dst's own environment can hold
/// credentials (API keys, DSH tokens), so nothing is inherited except PATH,
/// locale and terminal basics.
fn app_env(cmd: &mut Command, app: &App, state_dir: &Path) {
    let home = state_dir.join("home");
    let tmp = state_dir.join("tmp");
    let _ = std::fs::create_dir_all(&home);
    let _ = std::fs::create_dir_all(&tmp);
    let path = std::env::var("PATH").unwrap_or_else(|_| {
        "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string()
    });
    cmd.env_clear()
        .env("PATH", path)
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("SHELL", "/bin/sh")
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("DST_APP", app.label())
        .env("DST_APP_DIR", &app.dir)
        .env("DST_APP_STATE", state_dir)
        .env("DST_COLS", "53")
        .env("DST_ROWS", "12")
        .env("COLUMNS", "53")
        .env("LINES", "12");
    for key in ["LANG", "LC_ALL", "TZ"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
}

/// Turn captured terminal bytes into a readable log: strip SGR/CSI/OSC
/// sequences, treat CR as newline and drop backspaces/control codes.
pub fn strip_for_log(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            match it.peek() {
                Some('[') => {
                    it.next();
                    // CSI: parameters/intermediates, then one final byte @..~
                    while let Some(n) = it.next() {
                        if ('@'..='~').contains(&n) && n != '[' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    it.next();
                    // OSC: terminated by BEL or ESC \\
                    while let Some(n) = it.next() {
                        if n == '\x07' || n == '\x1b' {
                            break;
                        }
                    }
                }
                _ => {
                    it.next();
                }
            }
            continue;
        }
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                out.push('\n');
            }
            '\x08' => {
                out.pop();
            }
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn spawn_reader<R: Read + Send + 'static>(
    mut pipe: R,
    tx: std::sync::mpsc::Sender<Vec<u8>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    if chunk.len() >= cap {
        buf.clear();
        buf.extend_from_slice(&chunk[chunk.len() - cap..]);
        return;
    }
    let overflow = buf.len() + chunk.len();
    if overflow > cap {
        let drop = overflow - cap;
        buf.drain(0..drop.min(buf.len()));
    }
    buf.extend_from_slice(chunk);
}

/// Run a saved app on the panel. Restores the screen state the TUI needs.
pub fn run_on_screen(app: &App, timeout_s: u64) -> Result<String, String> {
    run_on_screen_args(app, timeout_s, "")
}

/// Run a saved app with arguments (`app_run args` / future user args).
pub fn run_on_screen_args(app: &App, timeout_s: u64, args: &str) -> Result<String, String> {
    let (prog, argv) = run_plan_args(app, args)?;
    // A stable place for app state: survives runs, keeps APP.md/listing clean.
    let state_dir = app.dir.join("state");
    let _ = std::fs::create_dir_all(&state_dir);
    let started = std::time::Instant::now();
    let label = app.label();
    let _ = std::io::stdout().flush();
    clear_screen();
    // The app gets the tty for its output but reads keys from a pipe we forward.
    // dst's own raw mode cleared OPOST, so a plain `printf "line\n"` would
    // step down without returning to column 0; turn NL -> CR-NL back on for the
    // duration of the app (the TUI redraws with explicit CRs, so it does not
    // need it, and the raw mode is restored after the app returns).
    stty(&["raw", "-echo", "min", "0", "time", "0", "opost", "onlcr"]);
    let mut cmd = app_command(&prog, &argv);
    cmd.current_dir(&app.dir);
    app_env(&mut cmd, app, &state_dir);
    cmd.stdin(Stdio::piped());
    if app.capture {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start app '{label}' ({prog}): {e}"))?;
    let child_id = child.id();
    // capture=true: stdout/stderr are pipes; forward bytes to the panel while
    // keeping the last 64 KB for state/last-run.log.
    let mut captured: Vec<u8> = Vec::new();
    let (cap_tx, cap_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let mut readers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    if app.capture {
        if let Some(out) = child.stdout.take() {
            readers.push(spawn_reader(out, cap_tx.clone()));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(spawn_reader(err, cap_tx.clone()));
        }
    }
    drop(cap_tx);
    let mut to_child = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            stty(&["min", "0", "time", "0"]);
            clear_screen();
            return Err(format!("could not connect to app '{label}' input"));
        }
    };
    let mut watch = EscWatch::new();
    let mut outcome = "exit";
    let mut status: Option<std::process::ExitStatus> = None;
    let timeout = std::time::Duration::from_secs(timeout_s.max(1));
    let mut buf = [0u8; 32];
    let mut stdin = std::io::stdin();
    loop {
        while let Ok(chunk) = cap_rx.try_recv() {
            let _ = std::io::stdout().write_all(&chunk);
            let _ = std::io::stdout().flush();
            append_capped(&mut captured, &chunk, CAPTURE_CAP);
        }
        let n = stdin.read(&mut buf).unwrap_or(0);
        if n > 0 {
            for b in &buf[..n] {
                if watch.feed(*b, std::time::Instant::now()) {
                    outcome = "esc";
                    break;
                }
            }
            let _ = to_child.write_all(&buf[..n]);
            let _ = to_child.flush();
            if outcome == "esc" {
                break;
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        match child.try_wait() {
            Ok(Some(st)) => {
                status = Some(st);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if started.elapsed() >= timeout {
            outcome = "timeout";
            break;
        }
    }
    if outcome != "exit" {
        let _ = Command::new("kill")
            .args(["-TERM", &child_id.to_string()])
            .output();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
        }
    }
    let status = status.or_else(|| child.wait().ok());
    drop(to_child);
    // Capture mode: wait for the readers to reach EOF, flush what is left to
    // the panel, then persist the readable log next to the app state.
    let mut log_path: Option<PathBuf> = None;
    if app.capture {
        for h in readers {
            let _ = h.join();
        }
        for chunk in cap_rx.try_iter() {
            let _ = std::io::stdout().write_all(&chunk);
            let _ = std::io::stdout().flush();
            append_capped(&mut captured, &chunk, CAPTURE_CAP);
        }
        let path = state_dir.join("last-run.log");
        let text = strip_for_log(&captured);
        if std::fs::write(&path, &text).is_ok() {
            log_path = Some(path);
        }
    }
    // Long-press Esc: the key keeps auto-repeating until the user releases it.
    // Those trailing ESC bytes would reach the TUI and be read as "cancel the
    // turn", so swallow input until the keyboard has been quiet for 300 ms
    // (bounded at 2 s so a jammed key cannot hold the app screen forever).
    if outcome == "esc" {
        let mut quiet_at = std::time::Instant::now();
        let deadline = quiet_at + std::time::Duration::from_secs(2);
        let mut sink = [0u8; 32];
        while std::time::Instant::now() < deadline {
            match stdin.read(&mut sink) {
                Ok(n) if n > 0 => quiet_at = std::time::Instant::now(),
                _ => {
                    if quiet_at.elapsed() >= std::time::Duration::from_millis(300) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
    // The TUI's turn loop expects raw + non-blocking reads after the takeover.
    stty(&["raw", "-echo", "min", "0", "time", "0"]);
    clear_screen();
    let secs = started.elapsed().as_secs_f32();
    let code = status.and_then(|s| s.code());
    let outcome_label = match outcome {
        "esc" => "esc".to_string(),
        "timeout" => format!("timeout {timeout_s}s"),
        _ => match code {
            Some(c) => format!("exit {c}"),
            None => "signal".to_string(),
        },
    };
    record_stamp(app, "last_run", &outcome_label, secs);
    let result = match outcome {
        "esc" => format!(
            "app '{label}' stopped by long-press Esc after {secs:.1}s; screen back on dst."
        ),
        "timeout" => format!(
            "app '{label}' stopped by timeout_s={timeout_s} after {secs:.1}s; screen back on dst. Raise timeout_s or open it from /apps for longer."
        ),
        _ => match code {
            Some(0) => format!(
                "app '{label}' exited (code 0) after {secs:.1}s; screen back on dst. Do not repeat what it showed."
            ),
            Some(c) => format!(
                "app '{label}' exited (code {c}) after {secs:.1}s; screen back on dst."
            ),
            None => format!(
                "app '{label}' was terminated after {secs:.1}s; screen back on dst."
            ),
        },
    };
    let mut result = result;
    if let Some(path) = &log_path {
        result.push_str(&format!(
            " Output was recorded to {} (read it with read_file; it is also replayed from /apps with `l`).",
            path.display()
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, kind: &str, entry: &str) -> App {
        App {
            name: name.into(),
            title: "T".into(),
            category: "system".into(),
            description: "d".into(),
            kind: kind.into(),
            entry: entry.into(),
            version: 1,
            created: String::new(),
            updated: String::new(),
            dir: PathBuf::from("/nonexistent"),
            last_run: None,
            last_test: None,
            capture: false,
            default_args: String::new(),
            requires: Vec::new(),
        }
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "dst-apps-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn args(pairs: &[(&str, &str)]) -> J {
        let mut v = obj(vec![]);
        for (k, x) in pairs {
            v.set(k, J::s(x));
        }
        v
    }

    #[test]
    fn slug_and_path_validation() {
        assert!(valid_slug("battery_viewer"));
        assert!(valid_slug("a1-b"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("Upper"));
        assert!(!valid_slug("has space"));
        assert!(!valid_slug("../x"));
        assert_eq!(safe_rel_path("run.sh").as_deref(), Some("run.sh"));
        assert_eq!(safe_rel_path("a/b.txt").as_deref(), Some("a/b.txt"));
        assert!(safe_rel_path("../etc/passwd").is_none());
        assert!(safe_rel_path("/etc/passwd").is_none());
        assert!(safe_rel_path("a//b").is_none());
    }

    #[test]
    fn save_and_write_roundtrip() {
        let root = tmpdir("roundtrip");
        let (app, created) = save_at(
            &root,
            &args(&[
                ("name", "Battery_Viewer"),
                ("category", "system"),
                ("title", "Battery viewer"),
                ("description", "Shows the charge level."),
                ("annotation", "Run it to see the battery."),
                ("kind", "sh"),
                ("entry", "run.sh"),
            ]),
        )
        .unwrap();
        assert!(created);
        assert_eq!(app.name, "battery_viewer");
        assert_eq!(app.version, 1);
        assert!(app.dir.join("app.json").is_file());
        let md = std::fs::read_to_string(app.dir.join("APP.md")).unwrap();
        assert!(md.contains("Battery viewer"));
        assert!(md.contains("Run it to see the battery."));

        // Updating in place bumps the version and keeps the created stamp.
        let (again, created) = save_at(
            &root,
            &args(&[
                ("name", "battery_viewer"),
                ("category", "system"),
                ("title", "Battery viewer"),
                ("description", "Shows charge and state."),
                ("annotation", "v2"),
                ("kind", "sh"),
                ("entry", "run.sh"),
            ]),
        )
        .unwrap();
        assert!(!created);
        assert_eq!(again.version, 2);
        let loaded = load_at(&root);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].description, "Shows charge and state.");

        let (written, path, bytes, ready) = write_file_at(
            &root,
            &args(&[
                ("name", "battery_viewer"),
                ("path", "run.sh"),
                ("content", "#!/bin/sh\necho hi\n"),
            ]),
        )
        .unwrap();
        assert_eq!(path, "run.sh");
        assert_eq!(bytes, 18);
        assert!(ready, "entry exists after write");
        assert!(written.dir.join("run.sh").is_file());
        // app_file must never touch the manifest or escape the app dir.
        assert!(write_file_at(&root, &args(&[("name", "battery_viewer"), ("path", "app.json"), ("content", "x")])).is_err());
        assert!(write_file_at(&root, &args(&[("name", "battery_viewer"), ("path", "../x"), ("content", "x")])).is_err());
        assert!(write_file_at(&root, &args(&[("name", "battery_viewer"), ("path", "x"), ("content", &"a".repeat(MAX_FILE_BYTES + 1))])).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_validates_fields() {
        let root = tmpdir("validate");
        let base = [
            ("name", "ok"),
            ("title", "T"),
            ("description", "D"),
            ("annotation", "A"),
        ];
        assert!(save_at(&root, &args(&base)).is_ok(), "doc apps need no entry");
        assert!(save_at(&root, &args(&[("name", "Bad Name"), ("title", "t"), ("description", "d"), ("annotation", "a")])).is_err());
        assert!(save_at(&root, &args(&[("name", "x"), ("title", ""), ("description", "d"), ("annotation", "a")])).is_err());
        assert!(save_at(&root, &args(&[("name", "x"), ("title", "t"), ("description", "d"), ("annotation", ""), ])).is_err());
        assert!(save_at(&root, &args(&[("name", "x"), ("title", "t"), ("description", "d"), ("annotation", "a"), ("kind", "sh")])).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_history_roundtrips_and_survives_saves() {
        let root = tmpdir("history");
        let (app, _) = save_at(
            &root,
            &args(&[
                ("name", "runner"),
                ("title", "Runner"),
                ("description", "d"),
                ("annotation", "a"),
                ("kind", "sh"),
                ("entry", "run.sh"),
            ]),
        )
        .unwrap();
        std::fs::write(app.dir.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        record_stamp(&app, "last_run", "exit 3", 1.25);
        record_stamp(&app, "last_test", "exit 0", 0.5);
        let loaded = load_at(&root);
        assert_eq!(loaded[0].last_run.as_ref().unwrap().outcome, "exit 3");
        assert_eq!(loaded[0].last_run.as_ref().unwrap().secs, 1.25);
        assert_eq!(loaded[0].last_test.as_ref().unwrap().outcome, "exit 0");
        // A manifest update (new description) keeps the recorded history.
        let (again, _) = save_at(
            &root,
            &args(&[
                ("name", "runner"),
                ("title", "Runner"),
                ("description", "now with more docs"),
                ("annotation", "a"),
                ("kind", "sh"),
                ("entry", "run.sh"),
            ]),
        )
        .unwrap();
        assert_eq!(again.version, 2);
        let reloaded = load_at(&root);
        assert_eq!(reloaded[0].description, "now with more docs");
        assert_eq!(reloaded[0].last_run.as_ref().unwrap().outcome, "exit 3");
        assert_eq!(reloaded[0].last_test.as_ref().unwrap().outcome, "exit 0");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn catalog_and_prompt() {
        let mut a = app("battery_viewer", "sh", "run.sh");
        a.description = "charge level".into();
        a.title = "Battery viewer".into();
        let apps = vec![a];
        let cat = catalog(&apps);
        assert!(cat.contains("system/battery_viewer — Battery viewer [sh:run.sh]: charge level"),
            "{cat}");
        let prompt = prompt_section(true, &apps);
        assert!(prompt.contains("CREATOR MODE"));
        assert!(prompt.contains("CONSENSUS"));
        assert!(prompt.contains("Long-press Esc always leaves an app"));
        assert!(prompt.contains("INSTALLED APPS (1)"));
        assert!(prompt_section(false, &apps).is_empty());
        let empty = prompt_section(true, &[]);
        assert!(empty.contains("(none yet"));
    }

    #[test]
    fn run_plan_selects_interpreter() {
        assert!(run_plan_args(&app("x", "doc", ""), "").is_err());
        let missing = app("x", "sh", "run.sh");
        let err = run_plan_args(&missing, "").unwrap_err();
        assert!(err.contains("missing"), "{err}");
    }

    #[test]
    fn captured_output_becomes_a_readable_log() {
        let out = strip_for_log(b"\x1b[31mred\x1b[0m\r\nplain \x1b[1;32mgreen\x1b[0m\n");
        assert_eq!(out, "red\nplain green\n");
        assert!(!out.contains('\x1b'));
        // A bare CR (progress line) becomes a newline; backspace eats a char.
        assert_eq!(strip_for_log(b"a\rb"), "a\nb");
        assert_eq!(strip_for_log(b"ab\x08c"), "ac");
        let mut buf: Vec<u8> = Vec::new();
        append_capped(&mut buf, b"12345", 4);
        assert_eq!(buf, b"2345");
        append_capped(&mut buf, b"678", 4);
        assert_eq!(buf, b"5678");
    }

    #[test]
    fn requirements_are_preflighted() {
        let mut a = app("x", "sh", "run.sh");
        a.requires = vec!["sh".into()];
        assert!(check_requires(&a).is_ok(), "sh is on PATH");
        a.requires = vec!["/bin/sh".into()];
        assert!(check_requires(&a).is_ok(), "absolute path exists");
        a.requires = vec!["dst-missing-tool-xyz".into()];
        let err = check_requires(&a).unwrap_err();
        assert!(err.contains("dst-missing-tool-xyz"), "{err}");
        assert!(err.contains("requires"), "{err}");
        a.requires = vec!["/no/such/binary".into()];
        assert!(check_requires(&a).is_err());
    }

    #[test]
    fn run_arguments_become_argv() {
        let root = tmpdir("argv");
        let (app, _) = save_at(
            &root,
            &args(&[
                ("name", "greeter"),
                ("title", "Greeter"),
                ("description", "d"),
                ("annotation", "a"),
                ("kind", "sh"),
                ("entry", "run.sh"),
            ]),
        )
        .unwrap();
        std::fs::write(app.dir.join("run.sh"), "#!/bin/sh
echo hi
").unwrap();
        let (prog, argv) = run_plan_args(&app, "  one   two  ").unwrap();
        assert_eq!(prog, "/bin/sh");
        assert_eq!(argv.len(), 3);
        assert!(argv[0].ends_with("run.sh"));
        assert_eq!(&argv[1..], &["one", "two"]);
        assert!(run_plan_args(&app, &"a".repeat(201)).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn esc_watch_needs_a_hold() {
        let t0 = std::time::Instant::now();
        let mut w = EscWatch::new();
        assert!(!w.feed(0x1b, t0));
        assert!(!w.feed(0x1b, t0));
        assert!(!w.feed(0x1b, t0), "three bytes in a burst is a tap, not a hold");
        let mut w = EscWatch::new();
        assert!(!w.feed(0x1b, t0));
        assert!(!w.feed(0x1b, t0 + std::time::Duration::from_millis(120)));
        assert!(w.feed(0x1b, t0 + std::time::Duration::from_millis(500)));
        let mut w = EscWatch::new();
        assert!(!w.feed(0x1b, t0));
        assert!(!w.feed(b'a', t0 + std::time::Duration::from_millis(100)));
        // The non-ESC byte resets the hold window.
        assert!(!w.feed(0x1b, t0 + std::time::Duration::from_millis(400)));
        assert!(!w.feed(0x1b, t0 + std::time::Duration::from_millis(800)));
    }

    #[test]
    fn tool_defs_cover_the_creator_surface() {
        let defs = tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec!["app_save", "app_file", "app_test", "app_export", "app_import", "app_run"]
        );
        let schema = crate::tool::definitions(&defs).dump();
        assert!(schema.contains("\"kind\""));
        assert!(schema.contains("\"entry\""));
        assert!(schema.contains("\"timeout_s\""));
        assert!(schema.contains("\"stdin\""));
    }
}
