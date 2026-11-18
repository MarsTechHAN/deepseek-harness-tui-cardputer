//! Apps are skills that can run.
//!
//! Creator mode has one unit: the **skill package** under
//! `~/.dst/skills/<name>/`. A skill becomes runnable by adding `meta.json`
//! next to its `SKILL.md`:
//!
//! ```text
//! ~/.dst/skills/battery-viewer/
//!     SKILL.md        catalog entry: name/description frontmatter + instructions
//!     meta.json       optional manifest: makes the package runnable
//!     run.sh          entry program (kind=sh)
//!     state/          runtime state written by the app ($DST_APP_STATE)
//! ```
//!
//! `meta.json`:
//!
//! ```json
//! {
//!   "title": "Battery viewer",
//!   "kind": "sh",
//!   "entry": "run.sh",
//!   "requires": ["sh"],
//!   "default_args": "--fast",
//!   "capture": false,
//!   "version": 1,
//!   "updated": "2026-11-17T15:00:00+08:00",
//!   "last_run": {"at": "2026-11-17T15:01:00+08:00", "secs": 1.2, "outcome": "exit 0"}
//! }
//! ```
//!
//! There is no separate app store and no app-writing tool: the model creates
//! the package with `skill_save` + `skill_file` (writing `meta.json` is the
//! step that makes it an app), and this module only *reads* the folder to
//! list/run/test it. Discovery is file-based, so the SKILLS catalog in the
//! prompt, the `/apps` launcher and the model's own `read_file` all see the
//! same thing.
//!
//! What stays here is the part that needs the platform: the screen-takeover
//! runner (long-press Esc + timeout + ulimits + scrubbed env), the headless
//! tester the model uses to verify behaviour, and the run/test history.

use crate::json::J;
use crate::tool::{num_arg, obj, str_arg, Fail, Param, ToolDef};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

pub const MAX_APPS: usize = 200;
pub const DEFAULT_TIMEOUT_S: u64 = 60;
pub const MAX_TIMEOUT_S: u64 = 600;
/// User-initiated runs from /apps: long-press Esc is the exit, not a timer.
pub const DIRECT_TIMEOUT_S: u64 = 3_600;

/// One recorded run/test of an app (written back into meta.json).
#[derive(Clone, Debug, Default)]
pub struct RunStamp {
    pub at: String,
    pub secs: f32,
    /// "exit 0" | "exit 3" | "timeout 2s" | "esc" | "signal"
    pub outcome: String,
}

/// A runnable skill package, loaded from `<skills>/<name>/meta.json`.
#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub title: String,
    pub description: String,
    /// `sh` | `python` | `binary` | `prompt` | `doc`
    pub kind: String,
    /// Relative entry file inside the skill dir ("" for `doc`/`prompt`).
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
    /// Display label. Skills are flat, so the name is the label.
    pub fn label(&self) -> String {
        self.name.clone()
    }
    /// Model-facing and UI-facing absolute path.
    pub fn path(&self) -> String {
        self.dir.display().to_string()
    }
}

/// Runnable packages live in the skill library: one package format, one place.
pub fn apps_dir() -> PathBuf {
    crate::config::skills_dir()
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

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push_str("...");
    }
    out.replace('\n', " ")
}

fn stamp_json(r: &RunStamp) -> J {
    obj(vec![
        ("at", J::s(&r.at)),
        ("secs", J::Num(r.secs as f64)),
        ("outcome", J::s(&r.outcome)),
    ])
}

fn parse_stamp(v: &J) -> Option<RunStamp> {
    if !matches!(v, J::Obj(_)) {
        return None;
    }
    Some(RunStamp {
        at: v.get("at").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        secs: v.get("secs").and_then(|x| x.as_num()).unwrap_or(0.0) as f32,
        outcome: v
            .get("outcome")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn str_vec(v: Option<&J>) -> Vec<String> {
    v.and_then(|x| x.as_arr())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

/// Canonical `meta.json` value for an app (used when a run/test stamp is
/// written back; the model owns the other fields).
fn meta_json(a: &App) -> J {
    let mut v = obj(vec![
        ("title", J::s(&a.title)),
        ("kind", J::s(&a.kind)),
        ("entry", J::s(&a.entry)),
        ("requires", J::Arr(a.requires.iter().map(|r| J::s(r)).collect())),
        ("default_args", J::s(&a.default_args)),
        ("capture", J::Bool(a.capture)),
        ("version", J::Num(a.version as f64)),
        ("created", J::s(&a.created)),
        ("updated", J::s(&a.updated)),
    ]);
    if let Some(r) = &a.last_run {
        v.set("last_run", stamp_json(r));
    }
    if let Some(t) = &a.last_test {
        v.set("last_test", stamp_json(t));
    }
    v
}

/// Read the human title/description from SKILL.md (the app manifest only holds
/// machine fields, so the catalog text stays in one place).
fn skill_meta(dir: &Path) -> (String, String) {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    let (meta, _body) = crate::skills::split_frontmatter(&text);
    let get = |k: &str| {
        meta.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    (get("name"), get("description"))
}

/// Every runnable package under `root` (skill dir + meta.json), sorted by name.
pub fn load_at(root: &Path) -> Vec<App> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in rd.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || dir.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true) {
            continue;
        }
        if let Some(app) = parse_meta(&dir) {
            out.push(app);
            if out.len() >= MAX_APPS {
                break;
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn load() -> Vec<App> {
    load_at(&apps_dir())
}

/// Resolve `name` (or a legacy `category/name`) against the loaded apps.
pub fn find(name: &str) -> Option<App> {
    find_at(&apps_dir(), name)
}

pub fn find_at(root: &Path, name: &str) -> Option<App> {
    let want = name.trim();
    if want.is_empty() {
        return None;
    }
    let want = want.rsplit('/').next().unwrap_or(want);
    load_at(root)
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(want))
}

fn parse_meta(dir: &Path) -> Option<App> {
    let name = dir.file_name()?.to_string_lossy().to_string();
    let text = std::fs::read_to_string(dir.join("meta.json")).ok()?;
    let v = match crate::json::parse(&text)? {
        J::Obj(_) => crate::json::parse(&text).unwrap(),
        _ => return None,
    };
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let kind = {
        let k = get("kind");
        if k.is_empty() { "sh".to_string() } else { k }
    };
    if !matches!(kind.as_str(), "sh" | "python" | "binary" | "prompt" | "doc") {
        return None;
    }
    let entry = get("entry");
    if !matches!(kind.as_str(), "doc" | "prompt") && entry.is_empty() {
        return None;
    }
    let (_, skill_desc) = skill_meta(dir);
    let title = {
        let t = get("title");
        if t.is_empty() {
            name.replace(['-', '_'], " ")
        } else {
            t
        }
    };
    Some(App {
        name,
        title,
        description: {
            let d = get("description");
            if d.is_empty() { skill_desc } else { d }
        },
        version: v.get("version").and_then(|x| x.as_num()).unwrap_or(1.0) as u32,
        created: get("created"),
        updated: get("updated"),
        kind,
        entry,
        dir: dir.to_path_buf(),
        last_run: v.get("last_run").and_then(parse_stamp),
        last_test: v.get("last_test").and_then(parse_stamp),
        requires: str_vec(v.get("requires")),
        default_args: get("default_args"),
        capture: matches!(v.get("capture"), Some(J::Bool(true))),
    })
}

/// Catalog marker for a runnable skill: "sh:run.sh", plus the failure of the
/// last run/test when there is one. None when the package has no meta.json
/// (a plain skill, not an app).
pub fn run_marker(dir: &Path) -> Option<String> {
    let app = parse_meta(dir)?;
    let mut m = if app.entry.is_empty() {
        app.kind.clone()
    } else {
        format!("{}:{}", app.kind, app.entry)
    };
    for (label, stamp) in [("last run", &app.last_run), ("last test", &app.last_test)] {
        if let Some(st) = stamp {
            let bad = !(st.outcome.is_empty() || st.outcome == "exit 0" || st.outcome == "esc");
            if bad {
                m.push_str(&format!(" (!{label}: {})", st.outcome));
                break;
            }
        }
    }
    Some(m)
}

/// Whether a directory is a runnable package (has meta.json).
pub fn meta_exists(dir: &Path) -> bool {
    dir.join("meta.json").is_file()
}

/// Compact "HH:MM" from an ISO stamp (only the time of a run matters in the
/// 12-row panel; the full timestamp stays in meta.json).
pub fn stamp_hhmm(at: &str) -> String {
    let idx = at.find('T').map(|i| i + 1).unwrap_or(0);
    at[idx..].chars().take(5).collect()
}

/// A failed last run/test is the health flag shown in the UI and the catalog.
pub fn is_broken(app: &App) -> bool {
    [&app.last_run, &app.last_test].iter().any(|st| {
        st.as_ref()
            .map(|r| !(r.outcome.is_empty() || r.outcome == "exit 0" || r.outcome == "esc"))
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

/// The request body of a prompt app: PROMPT.md when present, else the skill
/// instructions.
pub fn prompt_body(app: &App) -> String {
    let body = std::fs::read_to_string(app.dir.join("PROMPT.md"))
        .unwrap_or_else(|_| annotation(app));
    body.trim().to_string()
}

/// The skill instructions (SKILL.md body): what the user reads in /apps and
/// what the model follows for `kind=prompt`.
pub fn annotation(app: &App) -> String {
    let text = std::fs::read_to_string(app.dir.join("SKILL.md")).unwrap_or_default();
    crate::skills::split_frontmatter(&text).1.trim().to_string()
}

/// Files in the package with their sizes, sorted, for the /apps detail panel.
pub fn files(app: &App) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        let rd = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return,
        };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(base)
                .map(|x| x.display().to_string())
                .unwrap_or_default();
            if rel.starts_with(".history") || rel.starts_with("state/") {
                continue;
            }
            match e.metadata() {
                Ok(m) if m.is_dir() => walk(base, &p, out),
                Ok(m) if m.is_file() => out.push((rel, m.len())),
                _ => {}
            }
            if out.len() > 40 {
                return;
            }
        }
    }
    walk(&app.dir, &app.dir, &mut out);
    out.sort();
    out
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, content).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

pub fn delete(app: &App) -> Result<(), String> {
    let root = apps_dir();
    if !app.dir.starts_with(&root) || app.dir == root {
        return Err("refusing to delete a directory outside the skill library".into());
    }
    std::fs::remove_dir_all(&app.dir).map_err(|e| format!("{}: {e}", app.dir.display()))
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

/// Write one run/test stamp back into meta.json (best effort: a run must not
/// fail because history could not be written).
fn record_stamp(app: &App, key: &str, outcome: &str, secs: f32) {
    let path = app.dir.join("meta.json");
    let mut a = app.clone();
    let stamp = RunStamp {
        at: now_stamp(),
        secs,
        outcome: outcome.to_string(),
    };
    if key == "last_test" {
        a.last_test = Some(stamp);
    } else {
        a.last_run = Some(stamp);
    }
    let _ = write_atomic(&path, &(meta_json(&a).dump() + "\n"));
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
                return Err(format!(
                    "entry '{}' is missing; write it with skill_file",
                    app.entry
                ));
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
                return Err(format!(
                    "entry '{}' is missing; write it with skill_file",
                    app.entry
                ));
            }
            Ok((prog.to_string(), vec![entry.display().to_string()]))
        }
        "binary" => {
            if !entry.is_file() {
                return Err(format!(
                    "entry '{}' is missing; write it with skill_file",
                    app.entry
                ));
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
        "doc" => Err("doc apps have no program; read SKILL.md in /apps instead".into()),
        "prompt" => Err(
            "prompt apps are instructions, not programs; app_run returns their prompt for you to follow"
                .into(),
        ),
        other => Err(format!("unknown app kind '{other}'")),
    }
}

// ---------------------------------------------------------------------------
// prompt
// ---------------------------------------------------------------------------

/// The creator-mode block appended to the system prompt. The catalog itself is
/// the SKILLS catalog (see skills::catalog_prompt), which marks runnable skills
/// with their kind and entry file; there is no second "apps" list.
pub fn prompt_section() -> String {
    let root = apps_dir().display().to_string();
    let mut s = String::from("\n\nCREATOR MODE — you own this device\n");
    s.push_str(
        "You are the only application on this Cardputer Zero. The panel, the keyboard and \
the storage are yours: everything the user sees is something you built or will build.\n\
\n\
ONE UNIT: THE SKILL PACKAGE\n",
    );
    s.push_str(&format!(
        "- Everything you persist lives in ",
    ));
    s.push_str(&root);
    s.push_str(
        "/<name>/: SKILL.md (frontmatter name+description, then the instructions), optional \
supporting files, and optional meta.json.\n\
- A skill package is an APP when its folder also has meta.json: that file declares kind \
(sh|python|binary|prompt|doc), entry, requires, default_args and capture. Multiplied by \
nothing else — there is no separate app store and no app_* writing tool.\n\
- Create/update a skill with skill_save (SKILL.md + frontmatter, versioned) and put every other \
file — run.sh, PROMPT.md, meta.json — in the folder with skill_file. Writing meta.json is the \
step that makes the package runnable, and the platform reads it to list, run and test the app.\n\
- The SKILLS catalog injected below is the single source of truth: a runnable skill shows its \
kind and entry (`[sh:run.sh]`) and a failed last run/test. Read details with read_file (SKILL.md, \
meta.json) instead of guessing; there is no app_list tool because the folder IS the list.\n\
- Update an existing package instead of duplicating it: keep its name, rewrite only the files \
that changed; skill_save bumps the version and keeps the old SKILL.md under .history/ (restore \
it with skill_rollback).\n\
- A pure question (\"what is...\") stays a chat answer. A deliverable that needs no program is \
still a valid app: kind=doc (SKILL.md is the content) or no meta.json at all (a plain skill).\n\
- Device limits for apps: 320x170 px, 53 columns x 12 rows, 8 ANSI colours, no mouse. Use sh + \
coreutils (/proc, /sys); python only when present. No installs, no services, no compilation.\n\
- App state: write runtime state under $DST_APP_STATE (a `state/` dir inside the package); keep \
SKILL.md and meta.json free of generated data. Apps draw on stdout and read keys from stdin; the \
platform guarantees a long-press of Esc and a timeout.\n\
- kinds: sh and python scripts, binary (an executable file), prompt (a reusable request you \
follow when run: PROMPT.md or the SKILL.md body), doc (instructions only).\n\
- APP CONTRACT (the platform enforces what it can): an app owns the whole screen while it runs \
and dst is paused; it may not change system configuration, install software, start services, or \
write outside its package/state/tmp; it must exit (0 = success) and leave the screen \
recoverable. Never build an app that blocks the keyboard or ignores the Esc/timeout backstop.\n\
- Verify before you show it: call app_test (captured stdout/stderr/exit, log file under \
~/.dst/tmp) and fix failures; test the happy path plus bad input. app_test is headless by \
default; with tty=true it runs under a pseudo-terminal like app_run, which is how you test \
[ -t ] branches and ANSI/TUI output (the pty has no window size, so COLUMNS/LINES stand in for \
the real size). app_run is for the user to see the real full-screen behaviour.\n\
- Declare the binaries an app needs in meta.json `requires` (e.g. [\"python3\", \"tar\"]); dst \
checks them before every run/test and fails with an actionable error if one is missing.\n\
- Use default_args for an app that needs fixed arguments when launched from /apps; app_run args \
override them for a one-off run.\n\
- Set capture=true on a text app you want remembered: dst records its stdout/stderr to \
state/last-run.log (shown in /apps with `l`) and names the path in app_run's result. Capture \
makes stdout a pipe, not a tty, so do not use it for full-screen TUI apps.\n\
\n\
SKILL vs MEMORY\n\
- skill: a systematic, self-contained engineering package (procedure, format, checklist, \
support files) that will be useful again. Runnable or not, it is one package in one folder.\n\
- memory: short durable facts or preferences (memory_write); memory is fragments, a skill is a \
complete procedure. Prefer a skill over dumping a long procedure into memory.\n\
\n\
WHAT THE USER SEES\n\
- /apps lists the runnable packages (skill folders with meta.json), newest activity first: \
Enter runs, Right reads SKILL.md, l replays the last captured output, i asks you to improve it, \
x deletes it. Long-press Esc always leaves a running app.\n\
- A saved skill is announced in the transcript; a package that gained meta.json is announced \
as a saved app. After a turn that wrote files but saved no skill, you are nudged to persist it.\n",
    );
    s
}

// ---------------------------------------------------------------------------
// tool definitions: execution only (writing is skill_save/skill_file)
// ---------------------------------------------------------------------------

fn run_exec(args: &J) -> Result<J, Fail> {
    let name = str_arg(args, "name").unwrap_or_default();
    let app = find(&name).ok_or_else(|| {
        Fail::new(format!(
            "no runnable app '{name}'; the SKILLS catalog lists packages with [kind:entry], \
and a folder becomes runnable when it has meta.json"
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
                "prompt app '{}' has an empty PROMPT.md/SKILL.md; nothing to follow",
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
            "no runnable app '{name}'; the SKILLS catalog lists packages with [kind:entry], \
and a folder becomes runnable when it has meta.json"
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
            name: "app_test",
            description: "Run a runnable skill (a skill folder with meta.json) HEADLESS — pipes, no \
screen — and capture its output, exit code and duration. Use this to verify an app's behaviour \
yourself before app_run: it can drive stdin with the `stdin` argument, pass argv with `args`, and \
writes the full log to a file you can read with read_file. This is the model's version of the \
simulator the developers use.",
            params: vec![
                Param::req("name", "string", "Skill name (the folder under ~/.dst/skills that has meta.json)."),
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
            name: "app_run",
            description: "Run a runnable skill (a skill folder with meta.json: kind/entry) full-screen \
on the device panel and return when it exits, is stopped by a long-press of Esc, or reaches \
timeout_s. The user sees the app, not the chat, while it runs; do not repeat the app's content \
afterwards. Only available in the interactive TUI.",
            params: vec![
                Param::req("name", "string", "Skill name (the folder under ~/.dst/skills that has meta.json)."),
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
    use crate::json::J;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "dst-apps-{tag}-{}-{}",
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

    /// Create a skill package; `meta` adds meta.json (making it an app).
    fn package(root: &Path, name: &str, kind: &str, entry: &str, meta: bool) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!(
            "---\nname: {name}\ndescription: Use when the user needs {name}.\n---\n\n# {name}\n\nInstructions for {name}.\n"
        );
        std::fs::write(dir.join("SKILL.md"), text).unwrap();
        if meta {
            let mut m = obj(vec![
                ("title", J::s(&name.replace('-', " "))),
                ("kind", J::s(kind)),
                ("entry", J::s(entry)),
                ("version", J::Num(3.0)),
                ("updated", J::s("2026-11-17T15:00:00+08:00")),
            ]);
            if entry == "main.py" {
                m.set("requires", J::Arr(vec![J::s("python3")]));
            }
            std::fs::write(dir.join("meta.json"), m.dump()).unwrap();
            if !entry.is_empty() {
                std::fs::write(dir.join(entry), "echo hi\n").unwrap();
            }
        }
        dir
    }

    #[test]
    fn runnable_packages_are_loaded_from_skill_folders() {
        let root = tmpdir("load");
        package(&root, "battery-viewer", "sh", "run.sh", true);
        package(&root, "plain-skill", "", "", false);
        let apps = load_at(&root);
        assert_eq!(apps.len(), 1, "only packages with meta.json are apps");
        let a = &apps[0];
        assert_eq!(a.name, "battery-viewer");
        assert_eq!(a.title, "battery viewer");
        assert_eq!(a.kind, "sh");
        assert_eq!(a.entry, "run.sh");
        assert_eq!(a.version, 3);
        assert!(a.description.starts_with("Use when"));
        assert!(a.dir.ends_with("battery-viewer"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn meta_needs_a_known_kind_and_an_entry() {
        let root = tmpdir("meta");
        package(&root, "no-entry", "sh", "", true);
        package(&root, "bad-kind", "wasm", "run.sh", true);
        package(&root, "good", "sh", "run.sh", true);
        let apps = load_at(&root);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "good");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_accepts_the_name_and_legacy_labels() {
        let root = tmpdir("find");
        package(&root, "battery-viewer", "sh", "run.sh", true);
        let a = load_at(&root).remove(0);
        assert_eq!(a.label(), "battery-viewer");
        // The launcher used to print category/name; old prompts and saved
        // sessions may still carry that form, so it keeps resolving.
        assert_eq!(
            find_at(&root, "battery-viewer").map(|x| x.name),
            Some("battery-viewer".into())
        );
        assert_eq!(
            find_at(&root, "system/battery-viewer").map(|x| x.name),
            Some("battery-viewer".into())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_plan_selects_interpreter_and_takes_argv() {
        let root = tmpdir("plan");
        package(&root, "hello", "sh", "run.sh", true);
        let a = load_at(&root).remove(0);
        let (prog, argv) = run_plan_args(&a, "--fast two").unwrap();
        assert_eq!(prog, "/bin/sh");
        assert!(argv[0].ends_with("run.sh"));
        assert_eq!(&argv[1..], &["--fast".to_string(), "two".to_string()]);
        // A script kind with a missing entry is an actionable error, not a panic.
        let mut broken = a.clone();
        broken.entry = "missing.sh".into();
        let err = run_plan_args(&broken, "").unwrap_err();
        assert!(err.contains("missing.sh") && err.contains("skill_file"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn requirements_are_preflighted() {
        let root = tmpdir("req");
        package(&root, "pyapp", "python", "main.py", true);
        let a = load_at(&root).remove(0);
        match run_plan_args(&a, "") {
            Ok((prog, _)) => assert!(prog.contains("python")),
            Err(e) => assert!(e.contains("python"), "unexpected error: {e}"),
        }
        let mut missing = a.clone();
        missing.requires = vec!["definitely-not-a-binary-xyz".into()];
        let err = check_requires(&missing).unwrap_err();
        assert!(err.contains("definitely-not-a-binary-xyz"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_stamps_are_written_back_into_meta_json() {
        let root = tmpdir("stamp");
        package(&root, "clock", "sh", "run.sh", true);
        let a = load_at(&root).remove(0);
        record_stamp(&a, "last_test", "exit 3", 1.5);
        let after = load_at(&root).remove(0);
        let st = after.last_test.as_ref().expect("last_test recorded");
        assert_eq!(st.outcome, "exit 3");
        assert!(st.secs > 1.0);
        // The model's own fields survive the write-back.
        assert_eq!(after.version, a.version);
        assert_eq!(after.entry, "run.sh");
        assert_eq!(after.title, a.title);
        assert!(is_broken(&after));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn esc_watch_needs_a_hold() {
        let mut w = EscWatch::new();
        let t0 = std::time::Instant::now();
        assert!(!w.feed(0x1b, t0));
        assert!(!w.feed(0x1b, t0 + std::time::Duration::from_millis(100)));
        assert!(!w.feed(0x1b, t0 + std::time::Duration::from_millis(200)));
        let mut w = EscWatch::new();
        assert!(!w.feed(0x1b, t0));
        assert!(!w.feed(0x1b, t0 + std::time::Duration::from_millis(300)));
        assert!(w.feed(0x1b, t0 + std::time::Duration::from_millis(500)));
    }

    #[test]
    fn tool_defs_are_execution_only() {
        let names: Vec<&str> = tool_defs().iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["app_test", "app_run"]);
        // Writing a package is skill_save/skill_file; there is no app_* writer.
        assert!(!names.iter().any(|n| *n == "app_save" || *n == "app_file"));
    }
}
