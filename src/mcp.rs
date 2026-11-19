//! Minimal MCP (Model Context Protocol) client over stdio.
//!
//! Servers are declared in `~/.dst/mcp.json` (same shape as most MCP clients):
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "time": { "command": "python3", "args": ["/home/me/time_server.py"] },
//!     "fs":   { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] }
//!   }
//! }
//! ```
//!
//! At startup each enabled server is spawned, `initialize` is performed, and
//! its `tools/list` becomes part of dst's tool registry as
//! `mcp__<server>__<tool>`. Calls are synchronous JSON-RPC with a deadline; a
//! server that fails to start is skipped and logged, never fatal.

use crate::json::{parse, J};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::time::Duration;

pub struct McpTool {
    pub full_name: String,
    pub description: String,
    pub schema: String,
}

struct Server {
    name: String,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u64,
    /// Kept so dst can kill and reap the server on exit: a dropped `Child` is
    /// not waited for, and MCP servers (npx/node) would outlive dst otherwise.
    child: Option<Child>,
}

pub struct Manager {
    servers: Vec<Server>,
    pub tools: Vec<McpTool>,
}

const PROTOCOL: &str = "2024-11-05";

fn log(line: &str) {
    use std::io::Write as _;
    let path = crate::config::log_dir().join("mcp.log");
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > 256 * 1024 {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn send(stdin: &mut ChildStdin, msg: &str) -> Result<(), String> {
    stdin
        .write_all(msg.as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|e| format!("write: {e}"))
}

fn read_response(rx: &Receiver<String>, id: u64, timeout: Duration) -> Result<J, String> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let v = match parse(&line) {
                    Some(v) => v,
                    None => continue,
                };
                // Notifications have no id; skip until our response arrives.
                let got = v.get("id").and_then(|i| i.as_num()).map(|n| n as u64);
                if got != Some(id) {
                    continue;
                }
                if let Some(err) = v.get("error") {
                    let m = err
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("MCP error");
                    return Err(m.to_string());
                }
                return v.get("result").cloned().ok_or_else(|| "empty result".into());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err("server closed".into()),
        }
    }
    Err("timed out".into())
}

fn spawn_server(name: &str, cfg: &J) -> Result<Server, String> {
    let command = cfg
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or_else(|| "missing command".to_string())?;
    let mut cmd = Command::new(command);
    if let Some(args) = cfg.get("args").and_then(|a| a.as_arr()) {
        for a in args {
            if let Some(s) = a.as_str() {
                cmd.arg(s);
            }
        }
    }
    if let Some(env) = cfg.get("env") {
        if let J::Obj(kv) = env {
            for (k, v) in kv {
                if let Some(s) = v.as_str() {
                    cmd.env(k, s);
                }
            }
        }
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {command}: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let (tx, rx) = channel::<String>();
    std::thread::spawn(move || {
        let r = BufReader::new(stdout);
        for line in r.lines() {
            match line {
                Ok(l) => {
                    // A server can print a single huge JSON line; keep the
                    // reader channel bounded instead of buffering it whole.
                    let l = if l.len() > 512 * 1024 {
                        let cut: String = l.chars().take(512 * 1024).collect();
                        format!("{cut} [truncated]")
                    } else {
                        l
                    };
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut server = Server {
        name: name.to_string(),
        stdin,
        rx,
        next_id: 1,
        child: Some(child),
    };

    // initialize
    let id = server.next_id;
    server.next_id += 1;
    let init = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"{PROTOCOL}","capabilities":{{}},"clientInfo":{{"name":"dst","version":"0.1.0"}}}}}}"#
    );
    send(&mut server.stdin, &init)?;
    read_response(&server.rx, id, Duration::from_secs(10))?;
    send(
        &mut server.stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )?;
    Ok(server)
}

fn list_tools(server: &mut Server) -> Result<Vec<McpTool>, String> {
    let id = server.next_id;
    server.next_id += 1;
    send(
        &mut server.stdin,
        &format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#),
    )?;
    let result = read_response(&server.rx, id, Duration::from_secs(10))?;
    let mut out = Vec::new();
    if let Some(tools) = result.get("tools").and_then(|t| t.as_arr()) {
        for t in tools {
            let name = t.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let desc = t
                .get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("MCP tool")
                .to_string();
            let schema = t
                .get("inputSchema")
                .map(|s| s.dump())
                .unwrap_or_else(|| r#"{"type":"object","properties":{}}"#.to_string());
            out.push(McpTool {
                full_name: format!("mcp__{}__{}", server.name, sanitize(name)),
                description: desc,
                schema,
            });
        }
    }
    Ok(out)
}

pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

/// Start every enabled server declared in `~/.dst/mcp.json`.
pub fn start_all() -> Manager {
    let mut mgr = Manager {
        servers: Vec::new(),
        tools: Vec::new(),
    };
    let text = match std::fs::read_to_string(crate::config::mcp_path()) {
        Ok(t) => t,
        Err(_) => return mgr,
    };
    let doc = match parse(&text) {
        Some(d) => d,
        None => {
            log("mcp.json is not valid JSON");
            return mgr;
        }
    };
    let servers = match doc.get("mcpServers") {
        Some(J::Obj(kv)) => kv.clone(),
        _ => return mgr,
    };
    for (name, cfg) in servers {
        if cfg
            .get("disabled")
            .map(|d| matches!(d, J::Bool(true)))
            .unwrap_or(false)
        {
            continue;
        }
        match spawn_server(&name, &cfg) {
            Ok(mut srv) => match list_tools(&mut srv) {
                Ok(tools) => {
                    log(&format!("{}: {} tools", name, tools.len()));
                    mgr.tools.extend(tools);
                    mgr.servers.push(srv);
                }
                Err(e) => log(&format!("{name}: tools/list failed: {e}")),
            },
            Err(e) => log(&format!("{name}: {e}")),
        }
    }
    mgr
}

impl Manager {
    fn shutdown(&mut self) {
        for srv in &mut self.servers {
            if let Some(mut child) = srv.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.servers.clear();
    }

    /// Call `mcp__<server>__<tool>` with a JSON argument object.
    pub fn call(&mut self, full_name: &str, args: &J) -> Result<J, String> {
        let rest = full_name.strip_prefix("mcp__").ok_or("not an mcp tool")?;
        let (server, tool) = rest
            .split_once("__")
            .ok_or("malformed mcp tool name")?;
        let srv = self
            .servers
            .iter_mut()
            .find(|s| s.name == server)
            .ok_or_else(|| format!("unknown MCP server '{server}'"))?;
        let id = srv.next_id;
        srv.next_id += 1;
        let malformed = {
            let tool_name = tool.to_string();
            let req = format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool_name}","arguments":{}}}}}"#,
                args.dump()
            );
            send(&mut srv.stdin, &req)
        };
        malformed?;
        let result = read_response(&srv.rx, id, Duration::from_secs(60))?;
        // Normalise MCP content blocks to one text value.
        let mut text = String::new();
        let is_error = result
            .get("isError")
            .map(|v| matches!(v, J::Bool(true)))
            .unwrap_or(false);
        if let Some(blocks) = result.get("content").and_then(|c| c.as_arr()) {
            for b in blocks {
                if let Some(t) = b.get("text").and_then(|x| x.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                } else {
                    text.push_str(&b.dump());
                }
            }
        }
        if text.is_empty() {
            text = result.dump();
        }
        let mut o = crate::tool::obj(vec![("text", J::s(&text))]);
        if is_error {
            o = crate::tool::obj(vec![("text", J::s(&text)), ("error", J::Bool(true))]);
        }
        Ok(o)
    }
}

static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

/// Kill and reap every MCP server. Called on exit (and through the drop guard
/// below, so a diagnostics run such as `--tools` cannot orphan them either).
pub fn shutdown() {
    let mut mgr = match MANAGER.lock() {
        Ok(mut g) => g.take(),
        Err(_) => return,
    };
    if let Some(m) = mgr.as_mut() {
        m.shutdown();
    }
}

/// Drops with `main`, so every return path kills the servers it started.
pub struct ShutdownOnDrop;
impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        shutdown();
    }
}

/// Install the started manager (called once from `main`).
pub fn install(mgr: Manager) {
    if let Ok(mut g) = MANAGER.lock() {
        *g = Some(mgr);
    }
}

/// Dynamic executor shared by every MCP-backed tool definition.
/// Server -> advertised tool names, for the extensions menu.
pub fn server_summary() -> Vec<(String, Vec<String>)> {
    let guard = match MANAGER.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let mgr = match guard.as_ref() {
        Some(m) => m,
        None => return Vec::new(),
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for t in &mgr.tools {
        let rest = t.full_name.strip_prefix("mcp__").unwrap_or(&t.full_name);
        let mut parts = rest.splitn(2, "__");
        let server = parts.next().unwrap_or("?").to_string();
        let tool = parts.next().unwrap_or("?").to_string();
        match out.iter_mut().find(|(s, _)| *s == server) {
            Some((_, tools)) => tools.push(tool),
            None => out.push((server, vec![tool])),
        }
    }
    out
}

pub fn call_entry(full_name: &str, args: &J) -> Result<J, crate::tool::Fail> {
    let mut guard = MANAGER
        .lock()
        .map_err(|_| crate::tool::Fail::new("mcp unavailable"))?;
    let mgr = guard
        .as_mut()
        .ok_or_else(|| crate::tool::Fail::new("mcp not started"))?;
    let clean_args = crate::sandbox::strip_meta_args(args);
    match mgr.call(full_name, &clean_args) {
        Ok(v) => {
            if v.get("error").map(|e| matches!(e, J::Bool(true))).unwrap_or(false) {
                return Err(crate::tool::Fail::new(
                    v.get("text").and_then(|t| t.as_str()).unwrap_or("mcp error"),
                ));
            }
            Ok(v)
        }
        Err(e) => Err(crate::tool::Fail::new(e)),
    }
}

fn mcp_access(_a: &J) -> crate::tool::Access {
    crate::tool::Access::Write("mcp".into())
}

/// Tool definitions for every MCP tool discovered at startup.
pub fn tool_defs() -> Vec<crate::tool::ToolDef> {
    let guard = match MANAGER.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let mgr = match guard.as_ref() {
        Some(m) => m,
        None => return Vec::new(),
    };
    mgr.tools
        .iter()
        .map(|t| crate::tool::ToolDef {
            // Leaked name is acceptable: the registry is built once per process
            // and lives for its whole lifetime.
            name: Box::leak(t.full_name.clone().into_boxed_str()),
            description: Box::leak(t.description.clone().into_boxed_str()),
            // The raw JSON schema is embedded in the description because the
            // registry's parameter model is for hand-written tools.
            schema_override: Some(t.schema.clone()),
            params: Vec::new(),
            concurrency_safe: false,
            access: Some(mcp_access),
            timeout_ms: 65_000,
            execute: |_| Err(crate::tool::Fail::new("mcp tool needs dynamic dispatch")),
            dynamic: Some(call_entry),
            render: |_a, v| {
                v.get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string()
            },
            present: |_a| "mcp call".to_string(),
            post: None,
        })
        .collect()
}
