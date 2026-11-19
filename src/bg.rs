//! Background jobs and line watchers, modelled on DSH's job tools.
//!
//! A **job** is a detached shell process the agent can start, poll, wait for or
//! kill. A **watcher** is the same thing plus an inbox: every line it prints is
//! delivered to the agent as a user message (prefixed `[watcher <id>]`) on the
//! next idle tick, so long-running processes can drive the conversation.
//!
//! Output is kept as a bounded tail in memory (last `TAIL_MAX` lines) instead
//! of a log file: the device has 512 MB of RAM and a small SD card.

use crate::json::J;
use crate::tool::{Access, Fail, Param, ToolDef};
use std::collections::VecDeque;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

const TAIL_MAX: usize = 200;
const WAIT_CAP_S: u64 = 120;
/// A job line is delivered to the model and kept in the tail. A watcher that
/// prints one endless line must not buffer it forever: keep 8 KB and say so.
const LINE_CAP: usize = 8 * 1024;
/// Finished jobs are pruned beyond this many: a device that runs for weeks
/// with `job_start` must not keep every command and tail in RAM.
const MAX_JOBS: usize = 24;

struct Job {
    id: String,
    command: String,
    watcher: bool,
    child: Option<Child>,
    status: String,
    pid: u32,
    tail: Arc<Mutex<VecDeque<String>>>,
    started: u64,
}

fn lock<T>(m: &'static OnceLock<Mutex<T>>, init: impl FnOnce() -> T) -> MutexGuard<'static, T> {
    m.get_or_init(|| Mutex::new(init()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn jobs() -> MutexGuard<'static, Vec<Job>> {
    static JOBS: OnceLock<Mutex<Vec<Job>>> = OnceLock::new();
    lock(&JOBS, Vec::new)
}

fn inbox_mutex() -> &'static Mutex<VecDeque<(String, String)>> {
    static INBOX: OnceLock<Mutex<VecDeque<(String, String)>>> = OnceLock::new();
    INBOX.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn next_id(watcher: bool) -> String {
    static JOB: AtomicU32 = AtomicU32::new(0);
    static WATCH: AtomicU32 = AtomicU32::new(0);
    if watcher {
        format!("Watcher {}", WATCH.fetch_add(1, Ordering::SeqCst) + 1)
    } else {
        format!("Job {}", JOB.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_stream<R: std::io::Read + Send + 'static>(
    stream: R,
    tail: Arc<Mutex<VecDeque<String>>>,
    watcher_id: Option<String>,
) {
    std::thread::spawn(move || {
        // Read bytes, not `lines()`: an endless line would otherwise grow a
        // single String without bound inside BufRead::lines().
        let mut reader = std::io::BufReader::new(stream);
        let mut line: Vec<u8> = Vec::with_capacity(256);
        let mut discarding = false;
        let mut chunk = [0u8; 4096];
        let emit = |bytes: &[u8], truncated: bool| {
            let mut text = String::from_utf8_lossy(bytes).to_string();
            if truncated {
                text.push_str(" [line truncated]");
            }
            if let Ok(mut t) = tail.lock() {
                if t.len() >= TAIL_MAX {
                    t.pop_front();
                }
                t.push_back(text.clone());
            }
            if let Some(id) = &watcher_id {
                if let Ok(mut inbox) = inbox_mutex().try_lock() {
                    if inbox.len() > 100 {
                        inbox.pop_front();
                    }
                    let n = id.rsplit(' ').next().unwrap_or(id).to_string();
                    inbox.push_back((n, text));
                }
            }
        };
        loop {
            use std::io::Read;
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &chunk[..n] {
                        if discarding {
                            if b == b'\n' {
                                discarding = false;
                            }
                            continue;
                        }
                        if b == b'\n' || b == b'\r' {
                            if !line.is_empty() {
                                emit(&line, false);
                                line.clear();
                            }
                        } else if line.len() < LINE_CAP {
                            line.push(b);
                        } else {
                            emit(&line, true);
                            line.clear();
                            discarding = true;
                        }
                    }
                }
            }
        }
        if !line.is_empty() {
            emit(&line, false);
        }
    });
}

fn spawn(command: &str, watcher: bool) -> Result<Job, String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group: `kill -TERM -<pid>` then also reaches the children of
    // `sh -c 'daemon &'`, which would otherwise keep the pipes open forever.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let id = next_id(watcher);
    let pid = child.id();
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let watch_label = if watcher { Some(id.clone()) } else { None };
    if let Some(out) = child.stdout.take() {
        read_stream(out, tail.clone(), watch_label.clone());
    }
    if let Some(err) = child.stderr.take() {
        read_stream(err, tail.clone(), watch_label);
    }
    Ok(Job {
        id,
        command: command.to_string(),
        watcher,
        child: Some(child),
        status: "running".to_string(),
        pid,
        tail,
        started: now(),
    })
}

/// Kill a job's whole process group, then reap it. A bare `child.kill()` only
/// signals `sh`; a daemonizing command keeps the pipes (and the reader
/// threads) alive forever.
fn kill_job(child: &mut Child, pid: u32) {
    if pid > 0 {
        let _ = Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Drop the oldest finished jobs beyond `MAX_JOBS`, freeing their tail memory.
fn prune(guard: &mut Vec<Job>) {
    if guard.len() <= MAX_JOBS {
        return;
    }
    let mut excess = guard.len() - MAX_JOBS;
    let mut i = 0;
    while i < guard.len() && excess > 0 {
        if guard[i].child.is_none() {
            guard.remove(i);
            excess -= 1;
        } else {
            i += 1;
        }
    }
}

/// Reap finished children and prune the registry; called from the idle tick so
/// a job that exits while nobody polls it does not stay a zombie for days.
pub fn reap() {
    let mut guard = jobs();
    for job in guard.iter_mut() {
        refresh(job);
    }
    prune(&mut guard);
}

fn refresh(job: &mut Job) {
    if let Some(child) = job.child.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                job.status = match status.code() {
                    Some(c) => format!("exited {c}"),
                    None => "exited (signal)".to_string(),
                };
                job.child = None;
            }
            Ok(None) => {}
            Err(e) => {
                job.status = format!("unknown: {e}");
                job.child = None;
            }
        }
    }
}


fn tail_lines(job: &Job, max: usize) -> Vec<String> {
    job.tail
        .lock()
        .map(|t| t.iter().rev().take(max).rev().cloned().collect())
        .unwrap_or_default()
}

fn status_value(job: &Job, include_output: bool) -> J {
    let mut pairs = vec![
        ("id", J::s(&job.id)),
        ("status", J::s(&job.status)),
        ("pid", J::Num(job.pid as f64)),
        ("command", J::s(&job.command)),
        ("watcher", J::Bool(job.watcher)),
        ("age_s", J::Num(now().saturating_sub(job.started) as f64)),
    ];
    if include_output {
        let lines = tail_lines(job, 40).join("\n");
        pairs.push(("output", J::s(&lines)));
    }
    J::o(pairs)
}

/// Read-only snapshot of one background task for the TUI manager.
pub struct JobView {
    pub id: String,
    pub watcher: bool,
    pub status: String,
    pub command: String,
    pub age: u64,
}

/// All jobs/watchers, refreshed.
pub fn views() -> Vec<JobView> {
    let mut guard = jobs();
    guard
        .iter_mut()
        .map(|j| {
            refresh(j);
            JobView {
                id: j.id.clone(),
                watcher: j.watcher,
                status: j.status.clone(),
                command: j.command.clone(),
                age: now().saturating_sub(j.started),
            }
        })
        .collect()
}

pub fn has_any() -> bool {
    !views().is_empty()
}

/// `id: status` plus the recent output tail, for the manager panel.
pub fn output_of(id: &str, max: usize) -> String {
    let mut guard = jobs();
    if let Some(job) = guard.iter_mut().find(|j| j.id == id) {
        refresh(job);
        let mut out = format!("{}: {}", job.id, job.status);
        let tail = tail_lines(job, max);
        if !tail.is_empty() {
            out.push('\n');
            out.push_str(&tail.join("\n"));
        }
        out
    } else {
        format!("no job '{id}'")
    }
}

/// Stop a job or watcher and return its resulting status.
pub fn stop(id: &str) -> Result<String, String> {
    let mut guard = jobs();
    let job = guard
        .iter_mut()
        .find(|j| j.id == id)
        .ok_or_else(|| format!("no job '{id}'"))?;
    if let Some(mut child) = job.child.take() {
        kill_job(&mut child, job.pid);
    }
    job.status = "killed".to_string();
    Ok(job.status.clone())
}

// ---------------------------------------------------------------------------
// watcher inbox for the TUI
// ---------------------------------------------------------------------------

/// Drain pending watcher lines as `(id, line)` pairs.
pub fn poll_watchers() -> Vec<(String, String)> {
    reap();
    let mut inbox = inbox_mutex()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    while let Some(item) = inbox.pop_front() {
        out.push(item);
    }
    out
}

/// Kill every job and watcher (called when dst exits).
pub fn shutdown() {
    let mut guard = jobs();
    for job in guard.iter_mut() {
        if let Some(mut child) = job.child.take() {
            kill_job(&mut child, job.pid);
        }
        job.status = "killed (exit)".to_string();
    }
}

// ---------------------------------------------------------------------------
// tools
// ---------------------------------------------------------------------------

fn start_exec(args: &J, watcher: bool) -> Result<J, Fail> {
    let command = crate::tool::str_arg(args, "command")
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| Fail::new("missing 'command'"))?;
    let job = spawn(&command, watcher).map_err(Fail::new)?;
    let id = job.id.clone();
    let status = job.status.clone();
    if watcher {
        jobs().push(job);
        Ok(J::o(vec![
            ("id", J::s(&id)),
            ("status", J::s(&status)),
            ("note", J::s("every printed line will be delivered to the agent")),
        ]))
    } else {
        jobs().push(job);
        Ok(J::o(vec![
            ("id", J::s(&id)),
            ("status", J::s(&status)),
            ("hint", J::s("use job_output / job_wait / job_kill")),
        ]))
    }
}

fn job_start_exec(args: &J) -> Result<J, Fail> {
    start_exec(args, false)
}

fn watcher_start_exec(args: &J) -> Result<J, Fail> {
    start_exec(args, true)
}

fn list_value(watchers: bool) -> J {
    let mut guard = jobs();
    let mut arr = Vec::new();
    for job in guard.iter_mut() {
        if job.watcher != watchers {
            continue;
        }
        refresh(job);
        arr.push(status_value(job, false));
    }
    J::o(vec![("jobs", J::Arr(arr))])
}

fn job_list_exec(_args: &J) -> Result<J, Fail> {
    Ok(list_value(false))
}

fn watcher_list_exec(_args: &J) -> Result<J, Fail> {
    Ok(list_value(true))
}

fn output_exec(args: &J) -> Result<J, Fail> {
    let id = crate::tool::str_arg(args, "id").unwrap_or_default();
    let max = crate::tool::num_arg(args, "lines").unwrap_or(40.0).clamp(1.0, 200.0) as usize;
    let mut guard = jobs();
    let idx = guard.iter().position(|j| j.id == id).ok_or_else(|| Fail::new(format!("no job '{id}'")))?;
    refresh(&mut guard[idx]);
    let job = &guard[idx];
    Ok(J::o(vec![
        ("id", J::s(&job.id)),
        ("status", J::s(&job.status)),
        ("output", J::s(&tail_lines(job, max).join("\n"))),
    ]))
}

fn wait_exec(args: &J) -> Result<J, Fail> {
    let id = crate::tool::str_arg(args, "id").unwrap_or_default();
    let timeout = crate::tool::num_arg(args, "timeout_s")
        .unwrap_or(30.0)
        .clamp(1.0, WAIT_CAP_S as f64) as u64;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    loop {
        {
            let mut guard = jobs();
            let idx = guard.iter().position(|j| j.id == id).ok_or_else(|| Fail::new(format!("no job '{id}'")))?;
            refresh(&mut guard[idx]);
            let job = &guard[idx];
            if job.child.is_none() {
                return Ok(status_value(job, true));
            }
        }
        if std::time::Instant::now() >= deadline {
            let mut guard = jobs();
            let idx = guard.iter().position(|j| j.id == id).unwrap();
            refresh(&mut guard[idx]);
            let job = &guard[idx];
            return Ok(J::o(vec![
                ("id", J::s(&job.id)),
                ("status", J::s(&job.status)),
                ("still_running", J::Bool(true)),
                ("waited_s", J::Num(timeout as f64)),
                ("output", J::s(&tail_lines(job, 40).join("\n"))),
            ]));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn kill_exec(args: &J) -> Result<J, Fail> {
    let id = crate::tool::str_arg(args, "id").unwrap_or_default();
    let mut guard = jobs();
    let idx = guard.iter().position(|j| j.id == id).ok_or_else(|| Fail::new(format!("no job '{id}'")))?;
    let job = &mut guard[idx];
    let was_running = job.child.is_some();
    if let Some(mut child) = job.child.take() {
        kill_job(&mut child, job.pid);
    }
    job.status = if was_running {
        "killed".to_string()
    } else {
        format!("already {}", job.status)
    };
    Ok(J::o(vec![
        ("id", J::s(&job.id)),
        ("status", J::s(&job.status)),
        ("output", J::s(&tail_lines(job, 40).join("\n"))),
    ]))
}

fn kill_any_exec(args: &J) -> Result<J, Fail> {
    let id = crate::tool::str_arg(args, "id").unwrap_or_default();
    let mut guard = jobs();
    let idx = guard.iter().position(|j| j.id == id).ok_or_else(|| Fail::new(format!("no watcher '{id}'")))?;
    let job = &mut guard[idx];
    if let Some(mut child) = job.child.take() {
        kill_job(&mut child, job.pid);
    }
    job.status = "stopped".to_string();
    Ok(J::o(vec![
        ("id", J::s(&job.id)),
        ("status", J::s(&job.status)),
        ("lines_seen", J::Num(tail_lines(job, TAIL_MAX).len() as f64)),
    ]))
}

fn list_render(_args: &J, v: &J) -> String {
    let arr = v.get("jobs").and_then(|j| j.as_arr()).cloned().unwrap_or_default();
    if arr.is_empty() {
        return "(no jobs)".to_string();
    }
    let mut out = Vec::new();
    for job in arr {
        let id = job.get("id").and_then(|x| x.as_str()).unwrap_or("?");
        let st = job.get("status").and_then(|x| x.as_str()).unwrap_or("?");
        let cmd = job.get("command").and_then(|x| x.as_str()).unwrap_or("");
        let age = job.get("age_s").and_then(|x| x.as_num()).unwrap_or(0.0) as u64;
        out.push(format!("{id} {st} {age}s: {}", super_short(cmd, 40)));
    }
    out.join("\n")
}

fn status_render(_args: &J, v: &J) -> String {
    let mut out = String::new();
    if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
        let st = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
        out.push_str(&format!("{id}: {st}"));
        if v.get("still_running").map(|b| matches!(b, J::Bool(true))).unwrap_or(false) {
            out.push_str(" (still running)");
        }
    }
    if let Some(o) = v.get("output").and_then(|x| x.as_str()) {
        if !o.is_empty() {
            out.push('\n');
            out.push_str(o);
        }
    }
    if out.is_empty() {
        out = v.dump();
    }
    out
}

fn super_short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('\u{2026}');
    }
    out.replace('\n', " ")
}

fn job_alloc_write(_a: &J) -> Access {
    Access::Write("job_alloc".into())
}
fn job_alloc_read(_a: &J) -> Access {
    Access::Read("job_alloc".into())
}
fn job_id_read(a: &J) -> Access {
    Access::Read(format!("job:{}", crate::tool::str_arg(a, "id").unwrap_or_default()))
}
fn job_id_write(a: &J) -> Access {
    Access::Write(format!("job:{}", crate::tool::str_arg(a, "id").unwrap_or_default()))
}
fn watcher_alloc_write(_a: &J) -> Access {
    Access::Write("watcher_alloc".into())
}
fn watcher_alloc_read(_a: &J) -> Access {
    Access::Read("watcher_alloc".into())
}
fn watcher_id_write(a: &J) -> Access {
    Access::Write(format!("watcher:{}", crate::tool::str_arg(a, "id").unwrap_or_default()))
}

/// Registry entries for the job/watcher tools.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "job_start",
            description: "Start a shell command as a background job and return its id. Use it for \
long-running work (downloads, builds, long scans) so other tool calls can continue. Poll with \
job_output, wait with job_wait, stop with job_kill.",
            params: vec![
                Param::req("command", "string", "Shell command to run with `sh -c`."),
                Param::safeguard_approval(),
                Param::safeguard_reason(),
            ],
            concurrency_safe: false,
            access: Some(job_alloc_write),
            timeout_ms: 10_000,
            execute: job_start_exec,
            render: status_render,
            present: |a| format!("job_start {}", super_short(&crate::tool::str_arg(a, "command").unwrap_or_default(), 30)),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "job_list",
            description: "List background jobs with their status and age.",
            params: vec![],
            concurrency_safe: true,
            access: Some(job_alloc_read),
            timeout_ms: 5_000,
            execute: job_list_exec,
            render: list_render,
            present: |_a| "job_list".into(),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "job_output",
            description: "Read the recent output of a background job without waiting.",
            params: vec![
                Param::req("id", "string", "Job id returned by job_start."),
                Param::opt("lines", "integer", "How many tail lines to return.")
                    .range(1, 200)
                    .default_int(40),
            ],
            concurrency_safe: true,
            access: Some(job_id_read),
            timeout_ms: 10_000,
            execute: output_exec,
            render: status_render,
            present: |a| format!("job_output {}", crate::tool::str_arg(a, "id").unwrap_or_default()),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "job_wait",
            description: "Wait for a background job to finish, up to timeout_s seconds (max 120). \
Returns the final status and output, or still_running=true on timeout.",
            params: vec![
                Param::req("id", "string", "Job id returned by job_start."),
                Param::opt("timeout_s", "integer", "Seconds to wait.")
                    .range(1, 120)
                    .default_int(30),
            ],
            concurrency_safe: true,
            access: Some(job_id_read),
            timeout_ms: (WAIT_CAP_S + 20) * 1_000,
            execute: wait_exec,
            render: status_render,
            present: |a| format!("job_wait {}", crate::tool::str_arg(a, "id").unwrap_or_default()),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "job_kill",
            description: "Kill a background job (or watcher) and return its last output.",
            params: vec![Param::req("id", "string", "Job id.")],
            concurrency_safe: false,
            access: Some(job_id_write),
            timeout_ms: 10_000,
            execute: kill_exec,
            render: status_render,
            present: |a| format!("job_kill {}", crate::tool::str_arg(a, "id").unwrap_or_default()),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "watcher_start",
            description: "Start a background watcher: every line the command prints is delivered to \
you as a user message prefixed [watcher <id>]. Use it to react to logs, servers, sensors or any \
process that emits lines over time. Stop it with watcher_stop.",
            params: vec![
                Param::req("command", "string", "Shell command whose stdout/stderr lines are watched."),
                Param::safeguard_approval(),
                Param::safeguard_reason(),
            ],
            concurrency_safe: false,
            access: Some(watcher_alloc_write),
            timeout_ms: 10_000,
            execute: watcher_start_exec,
            render: status_render,
            present: |a| format!("watcher_start {}", super_short(&crate::tool::str_arg(a, "command").unwrap_or_default(), 26)),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "watcher_list",
            description: "List active line watchers.",
            params: vec![],
            concurrency_safe: true,
            access: Some(watcher_alloc_read),
            timeout_ms: 5_000,
            execute: watcher_list_exec,
            render: list_render,
            present: |_a| "watcher_list".into(),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "watcher_stop",
            description: "Stop a line watcher.",
            params: vec![Param::req("id", "string", "Watcher id returned by watcher_start.")],
            concurrency_safe: false,
            access: Some(watcher_id_write),
            timeout_ms: 10_000,
            execute: kill_any_exec,
            render: status_render,
            present: |a| format!("watcher_stop {}", crate::tool::str_arg(a, "id").unwrap_or_default()),
            post: None,
            schema_override: None,
            dynamic: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_run_wait_and_kill() {
        let out = spawn("echo hello-bg", false).expect("spawn");
        let id = out.id.clone();
        jobs().push(out);
        let waited = wait_exec(&J::o(vec![("id", J::s(&id)), ("timeout_s", J::Num(10.0))]))
            .map_err(|e| e.message)
            .expect("wait");
        assert!(status_render(&J::Null, &waited).contains("hello-bg"));
        assert!(status_render(&J::Null, &waited).contains("exited 0"));

        let slow = spawn("sleep 30", false).expect("spawn slow");
        let slow_id = slow.id.clone();
        jobs().push(slow);
        let killed = kill_exec(&J::o(vec![("id", J::s(&slow_id))]))
            .map_err(|e| e.message)
            .expect("kill");
        assert!(status_render(&J::Null, &killed).contains("killed"));
    }

    #[test]
    fn watcher_lines_are_queued() {
        let w = spawn("echo watched-line", true).expect("spawn watcher");
        jobs().push(w);
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline && got.is_empty() {
            got = poll_watchers();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(got.iter().any(|(_, line)| line.contains("watched-line")),
                "watcher lines: {got:?}");
    }
}
