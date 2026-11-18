//! The tool set, declared in DSH's shape: parameters carry their own rules,
//! `execute` returns a canonical value, `render` projects it to model text.
//!
//! No sandbox and no approval policy on purpose: this runs as the device owner
//! on their own machine. The pipeline stages still exist (see `tool.rs`) so a
//! policy could be attached without touching a tool body.

use crate::json::J;
use crate::tool::{num_arg, obj, str_arg, Call, Fail, NullHost, Param, ToolDef};
use std::process::Command;

/// API config, installed once at startup so tools can reach DeepSeek's
/// server-side tool endpoints without every tool taking a config argument.
static CFG: std::sync::Mutex<Option<crate::api::Cfg>> = std::sync::Mutex::new(None);

pub fn set_cfg(c: crate::api::Cfg) {
    if let Ok(mut g) = CFG.lock() {
        *g = Some(c);
    }
}

fn cfg_get() -> Result<crate::api::Cfg, Fail> {
    CFG.lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| Fail::new("api config not initialised"))
}

fn endpoint(path: &str) -> Result<String, Fail> {
    let cfg = cfg_get()?;
    let url = format!("{}/{}", cfg.base.trim_end_matches('/'), path);
    // HTTPS only: never send the key over a plaintext transport.
    if !url.starts_with("https://") {
        return Err(Fail::new(format!("refusing non-HTTPS endpoint: {url}")));
    }
    Ok(url)
}

fn short_err(s: &str) -> String {
    let t: String = s.chars().take(200).collect();
    t.replace('\n', " ")
}

fn parse_or_fail(body: &str, what: &str) -> Result<J, Fail> {
    if body.trim().is_empty() {
        return Err(Fail::new(format!("{what}: empty response")));
    }
    let v = crate::json::parse(body)
        .ok_or_else(|| Fail::new(format!("{what}: bad JSON: {}", short_err(body))))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("request failed");
        return Err(Fail::new(msg.to_string()));
    }
    Ok(v)
}

fn post_json(path: &str, body: &str, what: &str) -> Result<J, Fail> {
    let url = endpoint(path)?;
    let cfg = cfg_get()?;
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "60",
            "--max-filesize",
            "2000000",
            "-H",
            &crate::api::auth_header(&cfg),
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            body,
            &url,
        ])
        .output()
        .map_err(|e| Fail::new(format!("curl: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Fail::new(format!(
            "{what}: empty response{}",
            if err.is_empty() { String::new() } else { format!(" ({err})") }
        )));
    }
    parse_or_fail(&stdout, what)
}

fn registry_all() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "bash",
            description: "Run a shell command and return its exit status and output. \
Use it for anything the other tools do not cover: inspecting the system, \
audio/display checks, package queries.",
            params: vec![
                Param::req("command", "string", "The shell command to execute, run with `sh -c`."),
                Param::opt(
                    "timeout_s",
                    "integer",
                    "Seconds before the command is killed. Defaults to 60. A command \
that legitimately runs longer should be started in the background with `&` and \
polled with a later call.",
                ),
            ],
            concurrency_safe: false,
            timeout_ms: 65_000,
            execute: bash_exec,
            schema_override: None,
            dynamic: None,
            render: bash_render,
            present: |a| format!("$ {}", short(&str_arg(a, "command").unwrap_or_default(), 34)),
            post: None,
        },
        ToolDef {
            name: "read_file",
            description: "Read a text file, optionally a line range. Returns numbered lines \
so a later edit can address them.",
            params: vec![
                Param::req("path", "string", "Path to the file. Relative paths resolve against the current directory."),
                Param::opt("offset", "integer", "1-based first line to return. Defaults to 1."),
                Param::opt("limit", "integer", "Maximum lines to return. Defaults to 400."),
            ],
            concurrency_safe: true,
            timeout_ms: 10_000,
            execute: read_exec,
            schema_override: None,
            dynamic: None,
            render: read_render,
            present: |a| format!("read {}", str_arg(a, "path").unwrap_or_default()),
            post: Some(read_post),
        },
        ToolDef {
            name: "read_image",
            description: "Read a PNG/JPEG/WebP/GIF file and return the image itself, so you can \
look at pictures, screenshots, diagrams and photos. Large images are downscaled by the \
endpoint automatically; do not install image libraries or create thumbnails to inspect an \
image. Use it when the user points at an image file or when a screenshot/diagram in the \
workspace would answer the question. Requires an image-capable model.",
            params: vec![Param::req(
                "file_path",
                "string",
                "Path to the image file, resolved by the filesystem backend.",
            )],
            concurrency_safe: true,
            timeout_ms: 10_000,
            execute: read_image_exec,
            schema_override: None,
            dynamic: None,
            render: read_image_render,
            present: read_image_present,
            post: None,
        },
        ToolDef {
            name: "write_file",
            description: "Create or overwrite a text file with the given content. \
Parent directories are created as needed. Read a file before overwriting it.",
            params: vec![
                Param::req("path", "string", "Path to write."),
                Param::req("content", "string", "Full file content. This replaces the file, it does not append."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: write_exec,
            schema_override: None,
            dynamic: None,
            render: write_render,
            present: |a| format!("write {}", str_arg(a, "path").unwrap_or_default()),
            post: None,
        },
        ToolDef {
            name: "list_dir",
            description: "List a directory. Directories are suffixed with `/`.",
            params: vec![Param::opt(
                "path",
                "string",
                "Directory to list. Defaults to the current directory.",
            )],
            concurrency_safe: true,
            timeout_ms: 10_000,
            execute: list_exec,
            schema_override: None,
            dynamic: None,
            render: list_render,
            present: |a| format!("ls {}", str_arg(a, "path").unwrap_or_else(|| ".".into())),
            post: None,
        },
        ToolDef {
            name: "glob",
            description: "Find files by glob pattern (e.g. **/*.rs) under the current directory or a given path. Returns relative paths, bounded to 400 results.",
            params: vec![
                Param::req("pattern", "string", "Glob pattern; ** matches across directories."),
                Param::opt("path", "string", "Directory to search (default: current directory)."),
            ],
            concurrency_safe: true,
            timeout_ms: 20_000,
            execute: glob_exec,
            render: glob_render,
            present: |a| format!("glob {}", short(&str_arg(a, "pattern").unwrap_or_default(), 30)),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "grep",
            description: "Search file contents with an extended regex under a path. Returns file:line matches, bounded output; use it before editing a file.",
            params: vec![
                Param::req("pattern", "string", "Extended regular expression."),
                Param::opt("path", "string", "File or directory to search (default: current directory)."),
            ],
            concurrency_safe: true,
            timeout_ms: 30_000,
            execute: grep_exec,
            render: grep_render,
            present: |a| format!("grep {}", short(&str_arg(a, "pattern").unwrap_or_default(), 30)),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "edit_file",
            description: "Replace an exact string in a text file in place. Read the file first; old_string must be unique unless replace_all is true. Use write_file for new files.",
            params: vec![
                Param::req("path", "string", "File to edit."),
                Param::req("old_string", "string", "Exact text to replace (must match verbatim)."),
                Param::req("new_string", "string", "Replacement text."),
                Param::opt("replace_all", "boolean", "Replace every occurrence (default false)."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: edit_exec,
            render: edit_render,
            present: |a| format!("edit {}", str_arg(a, "path").unwrap_or_default()),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "speak",
            description: "Read text out loud through the device speaker. Use it when the \
user asks to be told something aloud. Generate a SHORT spoken message (a sentence or \
two); requests longer than 150 words are rejected, so summarise and leave detail in \
your reply text.",
            params: vec![Param::req(
                "text",
                "string",
                "What to say. Maximum 150 words (a CJK character counts as one word). \
Longer input is rejected outright \u{2014} shorten it and call again.",
            )],
            concurrency_safe: false,
            timeout_ms: 30_000,
            execute: speak_exec,
            schema_override: None,
            dynamic: None,
            render: speak_render,
            present: |a| format!("speak {}", short(&str_arg(a, "text").unwrap_or_default(), 40)),
            post: None,
        },
        ToolDef {
            name: "skill",
            description: "Load the instructions of a packaged skill before acting on a \
task that matches it. The available skills and when to use them are listed in \
the SKILLS section of your system prompt; calling this returns the full text.",
            params: vec![Param::req(
                "name",
                "string",
                "Skill name exactly as listed in the SKILLS section.",
            )],
            concurrency_safe: true,
            timeout_ms: 10_000,
            execute: skill_exec,
            schema_override: None,
            dynamic: None,
            render: skill_render,
            present: |a| format!("skill {}", short(&str_arg(a, "name").unwrap_or_default(), 30)),
            post: None,
        },
        ToolDef {
            name: "cron",
            description: "Manage recurring prompts on a 5-field cron schedule. \
When a job fires its prompt runs as a new turn, so keep prompts self-contained.",
            params: vec![
                Param::one_of("action", "What to do.", &["add", "list", "remove"]),
                Param::opt("spec", "string", "Cron expression, e.g. `*/10 * * * *` for add; the job index for remove. Read it back with action=list."),
                Param::opt("prompt", "string", "Prompt to run when the job fires. Required for add."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: cron_exec,
            schema_override: None,
            dynamic: None,
            render: jobs_render,
            present: |a| format!("cron {}", str_arg(a, "action").unwrap_or_default()),
            post: None,
        },
        ToolDef {
            name: "watch",
            description: "Manage prompts that fire when a file's modification time changes. \
A job fires at most once per change and runs its prompt as a new turn.",
            params: vec![
                Param::one_of("action", "What to do.", &["add", "list", "remove"]),
                Param::opt("path", "string", "File or directory to watch for add; the job index for remove."),
                Param::opt("prompt", "string", "Prompt to run when the file changes. Required for add."),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: watch_exec,
            schema_override: None,
            dynamic: None,
            render: jobs_render,
            present: |a| format!("watch {}", str_arg(a, "action").unwrap_or_default()),
            post: None,
        },
        ToolDef {
            name: "search",
            description: "Search the web and return ranked results with a long summary of \
each page. Use it to find current information; do not guess at facts it can answer.",
            params: vec![
                Param::req(
                    "query",
                    "string",
                    "Search terms, 1-128 characters. Supports `site:`, `-site:` and quoted phrases.",
                ),
                Param::opt(
                    "count",
                    "integer",
                    "How many results to return, 1-50. Defaults to 10; on this small \
screen 3-5 is usually enough.",
                ),
                Param::opt(
                    "need_content",
                    "boolean",
                    "Also return the full page text in each result. Defaults to false: \
it is large, so prefer the summary and fetch a specific page instead.",
                ),
            ],
            concurrency_safe: true,
            timeout_ms: 65_000,
            execute: search_exec,
            schema_override: None,
            dynamic: None,
            render: search_render,
            present: |a| format!("search {}", short(&str_arg(a, "query").unwrap_or_default(), 34)),
            post: None,
        },
        ToolDef {
            name: "fetch",
            description: "Fetch one web page and return its title and Markdown text. \
Use it after search when a specific result needs reading.",
            params: vec![Param::req(
                "url",
                "string",
                "Absolute http(s) URL of the page to read, without embedded credentials.",
            )],
            concurrency_safe: true,
            timeout_ms: 65_000,
            execute: fetch_exec,
            schema_override: None,
            dynamic: None,
            render: fetch_render,
            present: |a| format!("fetch {}", short(&str_arg(a, "url").unwrap_or_default(), 34)),
            post: None,
        },
        ToolDef {
            name: "ocr",
            description: "Extract text from a PDF, Word, PowerPoint or image file on this \
device, returning the text page by page.",
            params: vec![
                Param::req("path", "string", "Path to the file to recognise."),
                Param::one_of_opt(
                    "purpose",
                    "Defaults to text_recognition. Use rich_layout_recognition when the \
page layout itself matters.",
                    &["text_recognition", "rich_layout_recognition"],
                ),
            ],
            concurrency_safe: false,
            timeout_ms: 300_000,
            execute: ocr_exec,
            schema_override: None,
            dynamic: None,
            render: ocr_render,
            present: |a| format!("ocr {}", short(&str_arg(a, "path").unwrap_or_default(), 40)),
            post: None,
        },
        ToolDef {
            name: "artifact",
            description: "Take over the whole screen with a panel you compose, instead of \
the scrolling chat. Use it when an answer needs more room than a few lines: tables, \
code, diagrams, step lists, comparisons. It stays up until the user dismisses it \
(Esc), so put the finished result in it, not a progress message.",
            params: vec![
                Param::req("title", "string", "Panel title, one line, max 53 characters."),
                Param::req(
                    "body",
                    "string",
                    "Panel content. One row per line. The panel shows about 10 rows of \
53 characters; content longer than the screen is scrollable, but keep it close to \
10 lines so it reads at a glance.",
                ),
            ],
            concurrency_safe: false,
            timeout_ms: 10_000,
            execute: artifact_exec,
            schema_override: None,
            dynamic: None,
            render: artifact_render,
            present: |a| format!("artifact {}", short(&str_arg(a, "title").unwrap_or_default(), 40)),
            post: None,
        },
        ToolDef {
            name: "state_report",
            description: "Report in one short line what you are doing right now, e.g. \
\"reading the battery files\" or \"writing run.sh\". The line is pinned to the top row \
of the panel while the turn runs, so the user sees the current stage instead of a \
frozen screen. Use it at stage boundaries of a long or multi-step task (before a \
batch of work, when switching to a different sub-problem), not for every call. \
The text must fit in one panel row: longer reports are rejected, so keep it to a \
few words.",
            params: vec![Param::req(
                "text",
                "string",
                "One line, no newlines, at most 50 display cells (CJK counts as 2). \
Say what is happening now, in the user's language: \"scanning /proc\".",
            )],
            concurrency_safe: true,
            timeout_ms: 5_000,
            execute: state_report_exec,
            schema_override: None,
            dynamic: None,
            render: state_report_render,
            present: |a| format!("state {}", short(&str_arg(a, "text").unwrap_or_default(), 30)),
            post: None,
        },
    ]
}

/// Panel width, installed by the TUI at startup. `state_report` enforces it so
/// a report can never be cut off: it must fit on the row, or it is rejected.
static PANEL_COLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(53);

pub fn set_panel_cols(cols: usize) {
    PANEL_COLS.store(cols.clamp(20, 200), std::sync::atomic::Ordering::Relaxed);
}

fn panel_cols() -> usize {
    PANEL_COLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Display cells on the 6x13 cell grid: CJK/full-width glyphs take two.
fn text_cells(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let u = c as u32;
            if (0x1100..=0x115F).contains(&u)
                || (0x2E80..=0xA4CF).contains(&u)
                || (0xAC00..=0xD7A3).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
                || (0xFE30..=0xFE6F).contains(&u)
                || (0xFF00..=0xFF60).contains(&u)
                || (0xFFE0..=0xFFE6).contains(&u)
                || (0x1F300..=0x1FAFF).contains(&u)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

fn state_report_exec(args: &J) -> Result<J, Fail> {
    let raw = str_arg(args, "text").ok_or_else(|| Fail::new("missing 'text'"))?;
    // One row means one line: fold any stray newline/tab run into spaces.
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(Fail::new("state_report text must not be empty"));
    }
    let cols = panel_cols();
    // The panel draws the icon and one space before the text.
    let limit = cols.saturating_sub(3);
    let cells = text_cells(&text);
    if cells > limit {
        return Err(Fail::new(format!(
            "state_report must fit on one {cols}-cell row (at most {limit} cells, \
CJK counts as 2): this one is {cells} cells. Shorten it, e.g. \"reading /proc\"."
        )));
    }
    Ok(obj(vec![
        ("text", J::s(&text)),
        ("cells", J::Num(cells as f64)),
        ("limit", J::Num(limit as f64)),
    ]))
}

fn state_report_render(_args: &J, v: &J) -> String {
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
    format!("noted: {text}")
}

static SKILLS: std::sync::RwLock<Option<Vec<crate::skills::Skill>>> =
    std::sync::RwLock::new(None);

/// Install the catalogs discovered at startup.
pub fn set_skills(v: Vec<crate::skills::Skill>) {
    if let Ok(mut g) = SKILLS.write() {
        *g = Some(v);
    }
}

fn skill_exec(args: &J) -> Result<J, Fail> {
    let name = str_arg(args, "name").ok_or_else(|| Fail::new("missing 'name'"))?;
    let cached = SKILLS.read().ok().and_then(|g| g.clone());
    // Creator mode lets the model write a skill and use it in the same turn:
    // fall back to a fresh disk scan when the startup catalog has no match.
    if let Some(all) = &cached {
        if let Some(body) = crate::skills::body_for(all, &name) {
            return Ok(obj(vec![
                ("name", J::s(&name)),
                ("body", J::s(&body)),
            ]));
        }
    }
    let fresh = crate::skills::load_all();
    match crate::skills::body_for(&fresh, &name) {
        Some(body) => Ok(obj(vec![
            ("name", J::s(&name)),
            ("body", J::s(&body)),
            ("fresh", J::Bool(true)),
        ])),
        None => Err(Fail::new(format!(
            "unknown skill '{name}' (see the SKILLS section of the prompt)"
        ))),
    }
}

fn skill_render(_args: &J, v: &J) -> String {
    v.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string()
}

/// Tools available for this endpoint. Server-side tools (search/fetch/ocr) and
/// speech only exist on DeepSeek's own API, so a third-party endpoint gets a
/// smaller, honest tool set instead of tools that always fail.
pub fn registry(caps: crate::config::Capabilities) -> Vec<ToolDef> {
    let web = ["search", "fetch", "ocr"];
    let mut seen = std::collections::HashSet::new();
    registry_all()
        .into_iter()
        .filter(|t| caps.web_tools || !web.contains(&t.name))
        .filter(|t| caps.speech || t.name != "speak")
        // The API rejects duplicated tool names, so a misconfigured MCP server
        // must never be able to break the agent.
        .filter(|t| seen.insert(t.name))
        .collect()
}

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('\u{2026}');
    }
    out.replace('\n', " ")
}

// ---------------------------------------------------------------------------
// bash
// ---------------------------------------------------------------------------

/// Cap one child stream at `cap` bytes while draining the rest into the void.
///
/// `Command::output()` collects the whole stream before the tool truncates it,
/// which means `cat big.log` / `yes` / a fat `dmesg` can allocate hundreds of
/// MB on a 446 MB device and OOM it. The executors read with this instead: the
/// first `cap` bytes are kept, the remainder is counted and discarded (so the
/// child never blocks on a full pipe), and the model is told it was truncated.
fn read_capped<R: std::io::Read + Send + 'static>(
    r: R,
    cap: usize,
) -> std::thread::JoinHandle<(Vec<u8>, usize, bool)> {
    std::thread::spawn(move || {
        let mut r = r;
        let mut out: Vec<u8> = Vec::with_capacity(cap.min(64 * 1024));
        let mut total = 0usize;
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total += n;
                    if out.len() < cap {
                        let room = cap - out.len();
                        out.extend_from_slice(&buf[..n.min(room)]);
                        if n > room {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
            }
        }
        (out, total, truncated)
    })
}

/// Per-stream capture caps for exec tools. 64 KB stdout is ~10 screens of text;
/// anything past it is truncated and reported.
const STREAM_CAP: usize = 64 * 1024;
const STREAM_ERR_CAP: usize = 16 * 1024;

/// Run a command with capped, concurrently drained stdout/stderr.
fn run_capped(mut cmd: std::process::Command) -> Result<(Option<i32>, Vec<u8>, usize, bool, Vec<u8>, usize, bool), Fail> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| Fail::new(format!("could not start command: {e}")))?;
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let h_out = out_pipe.map(|p| read_capped(p, STREAM_CAP));
    let h_err = err_pipe.map(|p| read_capped(p, STREAM_ERR_CAP));
    let status = child
        .wait()
        .map_err(|e| Fail::new(format!("command wait failed: {e}")))?;
    let (stdout, out_total, out_trunc) = h_out
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let (stderr, err_total, err_trunc) = h_err
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    Ok((status.code(), stdout, out_total, out_trunc, stderr, err_total, err_trunc))
}

fn bash_exec(args: &J) -> Result<J, Fail> {
    let cmd = str_arg(args, "command").ok_or_else(|| Fail::new("missing 'command'"))?;
    let secs = num_arg(args, "timeout_s").unwrap_or(60.0).max(1.0) as u64;
    let secs = secs.min(600);

    let mut c = Command::new("timeout");
    c.arg("-k")
        .arg("2")
        .arg(secs.to_string())
        .arg("sh")
        .arg("-c")
        .arg(&cmd);
    let (code, stdout, out_total, out_trunc, stderr, err_total, err_trunc) =
        match run_capped(c) {
            Ok(v) => v,
            Err(e) => {
                // No coreutils `timeout`: run without an executor deadline.
                if !e.message.contains("could not start command") {
                    return Err(e);
                }
                let mut plain = Command::new("sh");
                plain.arg("-c").arg(&cmd);
                match run_capped(plain) {
                    Ok(v) => v,
                    Err(e) => return Err(e),
                }
            }
        };
    let mut v = obj(vec![
        ("exit", code.map(|c| J::Num(c as f64)).unwrap_or(J::Null)),
        ("timed_out", J::Bool(code == Some(124))),
        ("stdout", J::s(&String::from_utf8_lossy(&stdout))),
        ("stderr", J::s(&String::from_utf8_lossy(&stderr))),
        ("stdout_bytes", J::Num(out_total as f64)),
        ("stderr_bytes", J::Num(err_total as f64)),
        ("truncated", J::Bool(out_trunc || err_trunc)),
    ]);
    if out_trunc || err_trunc {
        v.set("truncated_note", J::s(&format!(
            "output truncated: kept {} of {} stdout bytes / {} of {} stderr bytes",
            stdout.len(), out_total, stderr.len(), err_total
        )));
    }
    Ok(v)
}

fn bash_render(_args: &J, v: &J) -> String {
    let stdout = v.get("stdout").and_then(|x| x.as_str()).unwrap_or("");
    let stderr = v.get("stderr").and_then(|x| x.as_str()).unwrap_or("");
    let mut s = String::new();
    if !stdout.is_empty() {
        s.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(stderr);
    }
    if s.trim().is_empty() {
        let code = v
            .get("exit")
            .and_then(|x| x.as_num())
            .map(|n| n as i64)
            .unwrap_or(-1);
        s = format!("(no output, exit={code})");
    }
    if v.get("timed_out").map(|x| matches!(x, J::Bool(true))).unwrap_or(false) {
        s.push_str("\n[command hit its timeout and was killed]");
    }
    if v
        .get("truncated")
        .map(|x| matches!(x, J::Bool(true)))
        .unwrap_or(false)
    {
        let note = v
            .get("truncated_note")
            .and_then(|x| x.as_str())
            .unwrap_or("output truncated");
        s.push_str(&format!("\n[{note}]"));
    }
    s.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

fn read_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "path").ok_or_else(|| Fail::new("missing 'path'"))?;
    let off = num_arg(args, "offset").unwrap_or(1.0).max(1.0) as usize;
    let lim = num_arg(args, "limit").unwrap_or(400.0).max(1.0) as usize;
    // Stream only the requested range: reading a 100 MB log into a String (and
    // then into lines) just to return 400 lines is how a device runs out of
    // memory. 4 MB of single-line data is already far past one screen.
    let file = std::fs::File::open(&path).map_err(|e| Fail::new(format!("{path}: {e}")))?;
    let reader = std::io::BufReader::new(file);
    let mut body_lines: Vec<String> = Vec::new();
    let mut idx = 0usize;
    let mut more = false;
    let mut bytes = 0usize;
    for line in std::io::BufRead::lines(reader) {
        let line = line.map_err(|e| Fail::new(format!("{path}: {e}")))?;
        idx += 1;
        if idx < off {
            continue;
        }
        if body_lines.len() >= lim {
            more = true;
            break;
        }
        bytes += line.len() + 1;
        body_lines.push(line);
        if bytes > 4 * 1024 * 1024 {
            more = true;
            break;
        }
    }
    let body = body_lines.join("\n");
    let start = off.saturating_sub(1);
    let end = start + body_lines.len();
    Ok(obj(vec![
        ("path", J::s(&path)),
        ("start", J::Num((start + 1) as f64)),
        ("end", J::Num(end as f64)),
        ("total", J::Num(end as f64)),
        ("more", J::Bool(more)),
        ("text", J::s(&body)),
    ]))
}

fn read_render(_args: &J, v: &J) -> String {
    let body = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
    let start = v.get("start").and_then(|x| x.as_num()).unwrap_or(1.0) as usize;
    if body.is_empty() {
        return "(empty file)".to_string();
    }
    body.lines()
        .enumerate()
        .map(|(i, l)| format!("{:>5}| {}", start + i, l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// post-execute: tell the model when the range it asked for did not cover the
/// file, so it does not have to re-read blindly.
fn read_post(args: &J, v: &J) -> Option<String> {
    let end = v.get("end").and_then(|x| x.as_num()).unwrap_or(0.0) as usize;
    let more = v
        .get("more")
        .map(|x| matches!(x, J::Bool(true)))
        .unwrap_or(false);
    if more {
        let path = str_arg(args, "path").unwrap_or_default();
        Some(format!(
            "read_file returned lines up to {end} of {path}; more lines remain. \
Call read_file again with offset={} to continue.",
            end + 1
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// write_file / list_dir
// ---------------------------------------------------------------------------

fn write_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "path").ok_or_else(|| Fail::new("missing 'path'"))?;
    let content = str_arg(args, "content").unwrap_or_default();
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Fail::new(format!("{}: {e}", parent.display())))?;
        }
    }
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| Fail::new(format!("{path}: {e}")))?;
    Ok(obj(vec![
        ("path", J::s(&path)),
        ("bytes", J::Num(content.len() as f64)),
    ]))
}

fn write_render(_args: &J, v: &J) -> String {
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let bytes = v.get("bytes").and_then(|x| x.as_num()).unwrap_or(0.0);
    format!("wrote {bytes:.0} bytes to {path}")
}

fn list_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "path").unwrap_or_else(|| ".".to_string());
    let rd = std::fs::read_dir(&path).map_err(|e| Fail::new(format!("{path}: {e}")))?;
    let mut items: Vec<String> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push(if is_dir { format!("{name}/") } else { name });
    }
    items.sort();
    Ok(obj(vec![
        ("path", J::s(&path)),
        ("entries", J::Arr(items.into_iter().map(|s| J::s(&s)).collect())),
    ]))
}

fn list_render(_args: &J, v: &J) -> String {
    match v.get("entries").and_then(|x| x.as_arr()) {
        Some(a) if !a.is_empty() => a
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => "(empty directory)".to_string(),
    }
}


// ---------------------------------------------------------------------------
// coding tools aligned with DSH/Claude Code: glob, grep, edit_file
// ---------------------------------------------------------------------------

fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    fn go(p: &[char], n: &[char]) -> bool {
        if p.is_empty() { return n.is_empty(); }
        match p[0] {
            '*' => {
                if p.get(1) == Some(&'*') {
                    return go(&p[2..], n) || (!n.is_empty() && go(p, &n[1..]));
                }
                go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..]))
            }
            '?' => !n.is_empty() && go(&p[1..], &n[1..]),
            c => !n.is_empty() && n[0] == c && go(&p[1..], &n[1..])
        }
    }
    go(&p, &n)
}

fn glob_exec(args: &J) -> Result<J, Fail> {
    let pattern = str_arg(args, "pattern").ok_or_else(|| Fail::new("missing 'pattern'"))?;
    let base = str_arg(args, "path").unwrap_or_else(|| ".".into());
    let mut out: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, pattern: &str, depth: usize, out: &mut Vec<String>) {
        if depth > 6 || out.len() >= 400 { return; }
        let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().to_string();
            if p.is_dir() {
                if !rel.starts_with(".git") { walk(&p, base, pattern, depth + 1, out); }
            } else if glob_match(pattern, &rel) || glob_match(pattern, p.file_name().and_then(|x| x.to_str()).unwrap_or("")) {
                out.push(rel);
                if out.len() >= 400 { return; }
            }
        }
    }
    let base = std::path::PathBuf::from(&base);
    walk(&base, &base, &pattern, 0, &mut out);
    Ok(obj(vec![
        ("files", J::Arr(out.iter().map(|f| J::s(f)).collect())),
        ("count", J::Num(out.len() as f64)),
    ]))
}

fn glob_render(_a: &J, v: &J) -> String {
    let files = v.get("files").and_then(|f| f.as_arr()).cloned().unwrap_or_default();
    if files.is_empty() { return "(no files matched)".into(); }
    files.iter().filter_map(|f| f.as_str()).collect::<Vec<_>>().join("\n")
}

fn grep_exec(args: &J) -> Result<J, Fail> {
    let pattern = str_arg(args, "pattern").ok_or_else(|| Fail::new("missing 'pattern'"))?;
    let path = str_arg(args, "path").unwrap_or_else(|| ".".into());
    let mut c = Command::new("grep");
    c.args(["-rnE", "--exclude-dir=.git", "--exclude-dir=target", "-m", "200", &pattern, &path]);
    let (code, stdout, _total, out_trunc, _stderr, _, _) = run_capped(c)?;
    let mut text = String::from_utf8_lossy(&stdout).to_string();
    let mut truncated = out_trunc;
    if text.len() > 6000 {
        text.truncate(6000);
        truncated = true;
    }
    if truncated {
        text.push_str("\n... [truncated]");
    }
    Ok(obj(vec![
        ("output", J::s(&text)),
        ("matches", J::Num(text.lines().count() as f64)),
        ("exit", J::Num(code.unwrap_or(-1) as f64)),
        ("truncated", J::Bool(truncated)),
    ]))
}

fn grep_render(_a: &J, v: &J) -> String {
    let t = v.get("output").and_then(|x| x.as_str()).unwrap_or("");
    if t.trim().is_empty() { "(no matches)".into() } else { t.trim_end().to_string() }
}

fn edit_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "path").ok_or_else(|| Fail::new("missing 'path'"))?;
    let old = str_arg(args, "old_string").ok_or_else(|| Fail::new("missing 'old_string'"))?;
    let new = str_arg(args, "new_string").ok_or_else(|| Fail::new("missing 'new_string'"))?;
    let all = matches!(args.get("replace_all"), Some(J::Bool(true)));
    if old.is_empty() {
        return Err(Fail::new("old_string must not be empty (use write_file to create a file)"));
    }
    // Editing needs the whole file in memory; refuse a monster instead of
    // letting a 100 MB log allocate several hundred MB.
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > 8 * 1024 * 1024 {
            return Err(Fail::new(format!(
                "{path} is {} MB; edit_file is for source-sized files (max 8 MB)",
                m.len() / (1024 * 1024)
            )));
        }
    }
    let text = std::fs::read_to_string(&path).map_err(|e| Fail::new(format!("{path}: {e}")))?;
    let count = text.matches(&old).count();
    if count == 0 {
        return Err(Fail::new("old_string not found in file; read the file first"));
    }
    if count > 1 && !all {
        return Err(Fail::new(format!(
            "old_string occurs {count} times; pass replace_all=true or include more context"
        )));
    }
    let out = if all { text.replace(&old, &new) } else { text.replacen(&old, &new, 1) };
    std::fs::write(&path, out).map_err(|e| Fail::new(format!("{path}: {e}")))?;
    Ok(obj(vec![
        ("path", J::s(&path)),
        ("replaced", J::Num(if all { count } else { 1 } as f64)),
    ]))
}

fn edit_render(_a: &J, v: &J) -> String {
    format!(
        "updated {} ({} replacement{})",
        v.get("path").and_then(|p| p.as_str()).unwrap_or("file"),
        v.get("replaced").and_then(|n| n.as_num()).unwrap_or(1.0) as u64,
        if v.get("replaced").and_then(|n| n.as_num()).unwrap_or(1.0) > 1.0 { "s" } else { "" }
    )
}

// ---------------------------------------------------------------------------
// speak -- max 150 words, longer calls are rejected
// ---------------------------------------------------------------------------

/// Upper bound on spoken length. Counted as words for spaced languages and as
/// one word per CJK character. A longer request is rejected so the model has to
/// summarise instead of streaming an essay into the speaker.
const MAX_SPEECH_WORDS: usize = 150;

fn count_words(text: &str) -> usize {
    let mut words = 0usize;
    let mut latin_token = false;
    for ch in text.chars() {
        let u = ch as u32;
        let is_cjk = matches!(u, 0x3000..=0x9FFF | 0xFF00..=0xFFEF);
        if is_cjk {
            if latin_token {
                words += 1;
                latin_token = false;
            }
            words += 1;
        } else if ch.is_whitespace() {
            if latin_token {
                words += 1;
                latin_token = false;
            }
        } else {
            latin_token = true;
        }
    }
    if latin_token {
        words += 1;
    }
    words
}

fn speak_exec(args: &J) -> Result<J, Fail> {
    let text = str_arg(args, "text").ok_or_else(|| Fail::new("missing 'text'"))?;
    let words = count_words(&text);
    if words > MAX_SPEECH_WORDS {
        return Err(Fail::new(format!(
            "speech rejected: {words} words exceeds the {MAX_SPEECH_WORDS}-word limit. \
Summarise what matters to the user and call speak again, or put the detail in your reply text."
        )));
    }
    let helper = "/home/hanxiao/dsh-lite/voice/voice_helper.py";
    let _ = helper;
    crate::voice::tts(&text).map_err(Fail::new)?;
    Ok(obj(vec![
        ("spoken", J::s(&text)),
        ("words", J::Num(words as f64)),
    ]))
}

fn speak_render(_args: &J, v: &J) -> String {
    let spoken = v.get("spoken").and_then(|x| x.as_str()).unwrap_or("");
    let words = v.get("words").and_then(|x| x.as_num()).unwrap_or(0.0);
    format!("spoke {words:.0} words: {spoken}")
}

// ---------------------------------------------------------------------------
// cron / watch -- delegate to the job store
// ---------------------------------------------------------------------------

fn cron_exec(args: &J) -> Result<J, Fail> {
    let action = str_arg(args, "action").unwrap_or_else(|| "list".into());
    let spec = str_arg(args, "spec").unwrap_or_default();
    let prompt = str_arg(args, "prompt").unwrap_or_default();
    Ok(obj(vec![(
        "result",
        J::s(&crate::jobs::tool_exec(&action, "cron", &spec, &prompt)),
    )]))
}

fn watch_exec(args: &J) -> Result<J, Fail> {
    let action = str_arg(args, "action").unwrap_or_else(|| "list".into());
    let spec = str_arg(args, "path").unwrap_or_default();
    let prompt = str_arg(args, "prompt").unwrap_or_default();
    Ok(obj(vec![(
        "result",
        J::s(&crate::jobs::tool_exec(&action, "watch", &spec, &prompt)),
    )]))
}

fn jobs_render(_args: &J, v: &J) -> String {
    v.get("result")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// compatibility shim for the job runner, which executes a single prompt
// ---------------------------------------------------------------------------

pub fn exec(name: &str, raw_args: &str) -> String {
    let reg = registry_all();
    let calls = vec![Call {
        id: "j".to_string(),
        name: name.to_string(),
        raw_args: raw_args.to_string(),
    }];
    let mut host = NullHost;
    let mut outs = crate::tool::run_batch(&reg, &calls, &mut host);
    outs.pop().map(|o| o.content).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// DeepSeek server-side tools: search / fetch / ocr
// ---------------------------------------------------------------------------

fn search_exec(args: &J) -> Result<J, Fail> {
    let query = str_arg(args, "query").ok_or_else(|| Fail::new("missing 'query'"))?;
    if query.trim().is_empty() {
        return Err(Fail::new("query must not be blank"));
    }
    let count = num_arg(args, "count").unwrap_or(10.0).clamp(1.0, 50.0);
    let need_content = matches!(args.get("need_content"), Some(J::Bool(true)));
    let body = obj(vec![
        ("query", J::s(&query)),
        ("count", J::Num(count)),
        ("need_content", J::Bool(need_content)),
    ])
    .dump();
    let v = post_json("tools/search", &body, "search")?;
    let results = v
        .get("results")
        .and_then(|r| r.as_arr())
        .cloned()
        .unwrap_or_default();
    Ok(obj(vec![
        ("query", J::s(&query)),
        ("count", J::Num(results.len() as f64)),
        ("results", J::Arr(results)),
    ]))
}

fn search_render(_args: &J, v: &J) -> String {
    let mut out = String::new();
    if let Some(arr) = v.get("results").and_then(|r| r.as_arr()) {
        for (i, r) in arr.iter().enumerate() {
            let title = r.get("title").and_then(|x| x.as_str()).unwrap_or("");
            let url = r.get("url").and_then(|x| x.as_str()).unwrap_or("");
            let score = r.get("rerank_score").and_then(|x| x.as_num()).unwrap_or(0.0);
            let summary = r
                .get("summary")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| r.get("snippet").and_then(|x| x.as_str()))
                .unwrap_or("");
            out.push_str(&format!(
                "[{}] {} (score {:.1})\n{}\n{}\n\n",
                i + 1,
                title,
                score,
                url,
                summary
            ));
        }
    }
    if out.trim().is_empty() {
        "(no results)".to_string()
    } else {
        out.trim_end().to_string()
    }
}

fn fetch_exec(args: &J) -> Result<J, Fail> {
    let url = str_arg(args, "url").ok_or_else(|| Fail::new("missing 'url'"))?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Fail::new("url must be an absolute http(s) URL"));
    }
    let body = obj(vec![("url", J::s(&url))]).dump();
    let v = post_json("tools/fetch", &body, "fetch")?;
    let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let content = v
        .get("content")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Ok(obj(vec![
        ("url", J::s(&url)),
        ("title", J::s(&title)),
        ("content", J::s(&content)),
    ]))
}

fn fetch_render(_args: &J, v: &J) -> String {
    let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("");
    let content = v.get("content").and_then(|x| x.as_str()).unwrap_or("");
    let mut s = String::new();
    if !title.is_empty() {
        s.push_str(&format!("# {title}\n\n"));
    }
    s.push_str(content);
    if s.trim().is_empty() {
        "(empty page)".to_string()
    } else {
        s
    }
}

fn ocr_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "path").ok_or_else(|| Fail::new("missing 'path'"))?;
    if !std::path::Path::new(&path).is_file() {
        return Err(Fail::new(format!("no such file: {path}")));
    }
    let purpose = str_arg(args, "purpose").unwrap_or_else(|| "text_recognition".into());
    if purpose != "text_recognition" && purpose != "rich_layout_recognition" {
        return Err(Fail::new(
            "purpose must be text_recognition or rich_layout_recognition",
        ));
    }
    let url = endpoint("tools/ocr")?;
    let cfg = cfg_get()?;
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "300",
            "-H",
            &crate::api::auth_header(&cfg),
            "--form",
            &format!("purpose={purpose}"),
            "--form",
            &format!("file=@{path}"),
            &url,
        ])
        .output()
        .map_err(|e| Fail::new(format!("curl: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if stdout.trim().is_empty() {
        return Err(Fail::new(format!(
            "ocr: empty response ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    parse_or_fail(&stdout, "ocr")
}

fn ocr_render(_args: &J, v: &J) -> String {
    let mut out = String::new();
    let pages = v.get("pages").and_then(|p| p.as_arr());
    if let Some(pages) = pages {
        for p in pages {
            let idx = p.get("index").and_then(|x| x.as_num()).unwrap_or(0.0) as i64;
            let text = p.get("text").and_then(|x| x.as_str()).unwrap_or("");
            let cut = p
                .get("finish_reason")
                .and_then(|x| x.as_str())
                .map(|r| r == "length")
                .unwrap_or(false);
            out.push_str(&format!("--- page {} ---\n{}", idx + 1, text));
            if cut {
                out.push_str("\n[page output hit the length limit and is incomplete]");
            }
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        "(no text recognised)".to_string()
    } else {
        out.trim_end().to_string()
    }
}

// ---------------------------------------------------------------------------
// read_image (same contract and name as DeepSeek Harness)
// ---------------------------------------------------------------------------

fn read_image_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "file_path")
        .filter(|p| !p.trim().is_empty())
        .or_else(|| str_arg(args, "path"))
        .ok_or_else(|| Fail::new("missing 'file_path'"))?;
    let a = crate::vision::Attachment::load(&path).map_err(Fail::new)?;
    Ok(obj(vec![
        ("path", J::s(&a.path)),
        ("mime", J::s(a.mime)),
        ("bytes", J::Num(a.bytes.len() as f64)),
        ("width", J::Num(a.width as f64)),
        ("height", J::Num(a.height as f64)),
        // `data_url` is consumed by the harness to build the image block; it is
        // deliberately not rendered (the model gets the picture itself).
        ("data_url", J::s(&a.data_url())),
    ]))
}

fn read_image_render(_args: &J, v: &J) -> String {
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let path = crate::config::home_shorten(path);
    let mime = v.get("mime").and_then(|x| x.as_str()).unwrap_or("image");
    let w = v.get("width").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
    let h = v.get("height").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
    let kb = (v.get("bytes").and_then(|x| x.as_num()).unwrap_or(0.0) as u64 / 1024).max(1);
    let dims = if w > 0 && h > 0 {
        format!("{w}x{h}")
    } else {
        "size unknown".to_string()
    };
    format!(
        "read image {path} ({mime}, {dims}, {kb} KB). The image is attached to this tool result; \
describe or answer from what you can see."
    )
}

fn read_image_present(a: &J) -> String {
    format!(
        "read_image {}",
        short(&str_arg(a, "file_path").or_else(|| str_arg(a, "path")).unwrap_or_default(), 34)
    )
}

fn artifact_exec(args: &J) -> Result<J, Fail> {
    let title = str_arg(args, "title").ok_or_else(|| Fail::new("missing 'title'"))?;
    let body = str_arg(args, "body").ok_or_else(|| Fail::new("missing 'body'"))?;
    if body.trim().is_empty() {
        return Err(Fail::new("body is empty"));
    }
    let lines = body.lines().count();
    let widest = body
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    Ok(obj(vec![
        ("title", J::s(&title)),
        ("body", J::s(&body)),
        ("lines", J::Num(lines as f64)),
        ("widest", J::Num(widest as f64)),
    ]))
}

fn artifact_render(_args: &J, v: &J) -> String {
    let lines = v.get("lines").and_then(|x| x.as_num()).unwrap_or(0.0);
    format!("panel shown full-screen ({lines:.0} rows); user dismisses it with Esc")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "dst-tools-{}-{name}",
            std::process::id()
        ));
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&4u32.to_be_bytes());
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(&p, png).unwrap();
        p
    }

    #[test]
    fn read_image_returns_the_picture_as_a_data_url() {
        let p = tiny_png("read_image.png");
        let args = crate::json::parse(&format!(
            r#"{{"file_path":"{}"}}"#,
            p.to_str().unwrap()
        ))
        .unwrap();
        let v = match read_image_exec(&args) {
            Ok(v) => v,
            Err(e) => panic!("image should load: {}", e.message),
        };
        assert_eq!(v.get("mime").and_then(|m| m.as_str()), Some("image/png"));
        assert_eq!(v.get("width").and_then(|w| w.as_num()), Some(4.0));
        let url = v.get("data_url").and_then(|u| u.as_str()).expect("data url");
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        let text = read_image_render(&args, &v);
        assert!(text.contains("read image"), "{text}");
        assert!(text.contains("4x3"), "{text}");
        let title = read_image_present(&args);
        assert!(title.starts_with("read_image "));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn read_image_rejects_missing_and_non_image_files() {
        let missing = crate::json::parse(r#"{"file_path":"/nonexistent/x.png"}"#).unwrap();
        assert!(read_image_exec(&missing).is_err());
        let p = std::env::temp_dir().join(format!("dst-tools-{}-text.png", std::process::id()));
        std::fs::write(&p, b"not an image").unwrap();
        let args = crate::json::parse(&format!(
            r#"{{"file_path":"{}"}}"#,
            p.to_str().unwrap()
        ))
        .unwrap();
        let err = read_image_exec(&args).unwrap_err();
        assert!(err.message.contains("JPEG"), "{}", err.message);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn read_image_is_registered_and_concurrency_safe() {
        let caps = crate::config::Capabilities {
            web_tools: true,
            speech: true,
        };
        let reg = registry(caps);
        let def = reg
            .iter()
            .find(|t| t.name == "read_image")
            .expect("read_image registered");
        assert!(def.concurrency_safe);
        assert!(def.params.iter().any(|p| p.name == "file_path" && p.required));
    }
}
