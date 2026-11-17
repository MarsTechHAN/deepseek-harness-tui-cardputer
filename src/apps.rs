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

fn now_stamp() -> String {
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
    p == "app.json" || p == "app.md"
}

fn manifest_json(a: &App) -> J {
    obj(vec![
        ("name", J::s(&a.name)),
        ("title", J::s(&a.title)),
        ("category", J::s(&a.category)),
        ("description", J::s(&a.description)),
        ("kind", J::s(&a.kind)),
        ("entry", J::s(&a.entry)),
        ("version", J::Num(a.version as f64)),
        ("created", J::s(&a.created)),
        ("updated", J::s(&a.updated)),
    ])
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
    if !matches!(kind.as_str(), "sh" | "python" | "doc") {
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
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                if m.is_file() {
                    total += m.len();
                }
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
    if !matches!(kind.as_str(), "sh" | "python" | "doc") {
        return Err(format!("invalid kind '{kind}': use sh, python or doc"));
    }
    let entry = str_arg(args, "entry").unwrap_or_default();
    let entry = if kind == "doc" {
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
    if kind != "doc" && entry.is_empty() {
        return Err("missing 'entry': the file app_run will execute".into());
    }

    let dir = root.join(&category).join(&name);
    let existed = dir.join("app.json").is_file();
    let (created, version) = if existed {
        let old = parse_manifest(&dir).ok_or_else(|| {
            format!("{} already exists but its app.json is unreadable; fix it by hand", dir.display())
        })?;
        (old.created, old.version.saturating_add(1))
    } else {
        (now_stamp(), 1)
    };
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

/// Interpreter + argv for running an app on the device.
pub fn run_plan(app: &App) -> Result<(String, Vec<String>), String> {
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
        "doc" => Err("doc apps have no program; open APP.md in /apps instead".into()),
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
        s.push_str(&format!(
            "- {} — {} [{}]{}\n",
            app.label(),
            if app.title.is_empty() { &app.name } else { &app.title },
            app.kind,
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
expected.\n\
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
    let result = request_launch(&app, timeout).map_err(Fail::new)?;
    Ok(obj(vec![
        ("name", J::s(&app.label())),
        ("result", J::s(&result)),
    ]))
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
                Param::one_of_opt("kind", "How app_run executes it.", &["sh", "python", "doc"]),
                Param::opt("entry", "string", "Relative entry file (e.g. run.sh); required unless kind=doc."),
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
            name: "app_run",
            description: "Run a saved app full-screen on the device panel and return when it exits, \
is stopped by a long-press of Esc, or reaches timeout_s. The user sees the app, not the chat, while \
it runs; do not repeat the app's content afterwards. Only available in the interactive TUI.",
            params: vec![
                Param::req("name", "string", "App name or category/name from INSTALLED APPS."),
                Param::opt("timeout_s", "integer", "Seconds before the platform stops the app (default 60, max 600)."),
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
pub fn request_launch(app: &App, timeout_s: u64) -> Result<String, String> {
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
    let result = ui.run_app(&req.app, req.timeout_s);
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

/// Run a saved app on the panel. Restores the screen state the TUI needs.
pub fn run_on_screen(app: &App, timeout_s: u64) -> Result<String, String> {
    let (prog, args) = run_plan(app)?;
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
    let mut cmd = Command::new(&prog);
    cmd.args(&args)
        .current_dir(&app.dir)
        .env("DST_APP", &label)
        .env("DST_APP_DIR", &app.dir)
        .env("DST_COLS", "53")
        .env("DST_ROWS", "12")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start app '{label}' ({prog}): {e}"))?;
    let child_id = child.id();
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
    fn catalog_and_prompt() {
        let mut a = app("battery_viewer", "sh", "run.sh");
        a.description = "charge level".into();
        a.title = "Battery viewer".into();
        let apps = vec![a];
        let cat = catalog(&apps);
        assert!(cat.contains("system/battery_viewer — Battery viewer [sh]: charge level"));
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
        assert!(run_plan(&app("x", "doc", "")).is_err());
        let missing = app("x", "sh", "run.sh");
        let err = run_plan(&missing).unwrap_err();
        assert!(err.contains("missing"), "{err}");
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
        assert_eq!(names, vec!["app_save", "app_file", "app_run"]);
        let schema = crate::tool::definitions(&defs).dump();
        assert!(schema.contains("\"kind\""));
        assert!(schema.contains("\"entry\""));
        assert!(schema.contains("\"timeout_s\""));
    }
}
