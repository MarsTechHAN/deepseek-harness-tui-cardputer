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
            "-H",
            &format!("Authorization: Bearer {}", cfg.key),
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
    ]
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
    let guard = SKILLS
        .read()
        .map_err(|_| Fail::new("skills unavailable"))?;
    let all = guard.as_ref().ok_or_else(|| Fail::new("no skills loaded"))?;
    match crate::skills::body_for(all, &name) {
        Some(body) => Ok(obj(vec![
            ("name", J::s(&name)),
            ("body", J::s(&body)),
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
    let out = match c.output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No coreutils `timeout`: run without an executor deadline.
            Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .output()
                .map_err(|e| Fail::new(format!("could not start shell: {e}")))?
        }
        Err(e) => return Err(Fail::new(format!("could not start shell: {e}"))),
    };

    let code = out.status.code();
    Ok(obj(vec![
        ("exit", code.map(|c| J::Num(c as f64)).unwrap_or(J::Null)),
        ("timed_out", J::Bool(code == Some(124))),
        ("stdout", J::s(&String::from_utf8_lossy(&out.stdout))),
        ("stderr", J::s(&String::from_utf8_lossy(&out.stderr))),
    ]))
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
    s.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

fn read_exec(args: &J) -> Result<J, Fail> {
    let path = str_arg(args, "path").ok_or_else(|| Fail::new("missing 'path'"))?;
    let off = num_arg(args, "offset").unwrap_or(1.0).max(1.0) as usize;
    let lim = num_arg(args, "limit").unwrap_or(400.0).max(1.0) as usize;
    let text = std::fs::read_to_string(&path).map_err(|e| Fail::new(format!("{path}: {e}")))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = off.saturating_sub(1).min(lines.len());
    let end = (start + lim).min(lines.len());
    let body = lines[start..end].join("\n");
    Ok(obj(vec![
        ("path", J::s(&path)),
        ("start", J::Num((start + 1) as f64)),
        ("end", J::Num(end as f64)),
        ("total", J::Num(lines.len() as f64)),
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
    let total = v.get("total").and_then(|x| x.as_num()).unwrap_or(0.0) as usize;
    if total > end {
        let path = str_arg(args, "path").unwrap_or_default();
        Some(format!(
            "read_file returned lines up to {end} of {total} in {path}; {} line(s) remain. \
Call read_file again with offset={} to continue.",
            total - end,
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
            &format!("Authorization: Bearer {}", cfg.key),
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
