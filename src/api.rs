//! OpenAI-compatible streaming chat client over a `curl` subprocess.
//!
//! Using curl instead of an HTTP/TLS crate keeps the binary tiny and the build
//! dependency-free (no openssl/rustls, no crates.io).
//!
//! Two entry points:
//!   `spawn_chat` streams into a channel so the UI thread can keep reading the
//!   keyboard (type-ahead, ESC to cancel) while the model is working.
//!   `chat_stream` is the blocking wrapper used by background jobs.

use crate::json::{parse, J};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone)]
pub struct Cfg {
    pub base: String,
    pub key: String,
    pub model: String,
    pub max_tokens: u32,
    /// Reasoning effort tier (`auto` = do not send the parameter).
    pub effort: String,
}

#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

#[derive(Default)]
pub struct StreamOut {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub finish: String,
}

/// One stream event. `Reason` is the model's thinking, `Content` the answer.
pub enum ApiEv {
    Reason(String),
    Content(String),
    Done(StreamOut),
    Failed(String),
}

pub struct Chat {
    pub rx: Receiver<ApiEv>,
    pub pid: u32,
}

/// Ask curl to stop; the reader thread then sees EOF and settles the stream.
pub fn kill(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).output();
}

fn body_json(cfg: &Cfg, messages: &J, tools: &J) -> String {
    let mut fields = vec![
        ("model".to_string(), J::s(&cfg.model)),
        ("messages".to_string(), messages.clone()),
        ("tools".to_string(), tools.clone()),
        ("stream".to_string(), J::Bool(true)),
        ("max_tokens".to_string(), J::Num(cfg.max_tokens as f64)),
        ("tool_choice".to_string(), J::s("auto")),
        ("parallel_tool_calls".to_string(), J::Bool(true)),
    ];
    if cfg.effort != "auto" && !cfg.effort.is_empty() {
        fields.push(("reasoning_effort".to_string(), J::s(&cfg.effort)));
    }
    J::Obj(fields).dump()
}

fn apply_line(line: &str, out: &mut StreamOut, tx: &std::sync::mpsc::Sender<ApiEv>) -> bool {
    let t = line.trim_start();
    let data = match t.strip_prefix("data:") {
        Some(d) => d.trim(),
        None => return false,
    };
    if data == "[DONE]" {
        return true;
    }
    let v = match parse(data) {
        Some(v) => v,
        None => return false,
    };
    let choice = match v.get("choices").and_then(|c| c.as_arr()).and_then(|a| a.get(0)) {
        Some(c) => c,
        None => return false,
    };
    if let Some(f) = choice.get("finish_reason").and_then(|f| f.as_str()) {
        if !f.is_empty() {
            out.finish = f.to_string();
        }
    }
    let delta = match choice.get("delta") {
        Some(d) => d,
        None => return false,
    };
    // Thinking is streamed separately from the answer by DeepSeek reasoning
    // models; the UI shows it live and collapses it once content starts.
    let reason = delta
        .get("reasoning_content")
        .and_then(|c| c.as_str())
        .or_else(|| delta.get("reasoning").and_then(|c| c.as_str()));
    if let Some(r) = reason {
        if !r.is_empty() {
            let _ = tx.send(ApiEv::Reason(r.to_string()));
        }
    }
    if let Some(c) = delta.get("content").and_then(|c| c.as_str()) {
        if !c.is_empty() {
            out.text.push_str(c);
            let _ = tx.send(ApiEv::Content(c.to_string()));
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_arr()) {
        for tc in tcs {
            let idx = tc.get("index").and_then(|i| i.as_num()).unwrap_or(0.0) as usize;
            if idx > 15 {
                continue;
            }
            while out.calls.len() <= idx {
                out.calls.push(ToolCall::default());
            }
            let slot = &mut out.calls[idx];
            if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                if !id.is_empty() {
                    slot.id = id.to_string();
                }
            }
            if let Some(f) = tc.get("function") {
                if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                    slot.name.push_str(n);
                }
                if let Some(a) = f.get("arguments").and_then(|x| x.as_str()) {
                    slot.args.push_str(a);
                }
            }
        }
    }
    false
}

/// Start a streamed request. Returns immediately; events arrive on `rx`.
pub fn spawn_chat(cfg: &Cfg, messages: &J, tools: &J) -> Result<Chat, String> {
    let body = body_json(cfg, messages, tools);
    let url = format!("{}/chat/completions", cfg.base.trim_end_matches('/'));
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "-N",
            "--max-time",
            "300",
            "-H",
            &format!("Authorization: Bearer {}", cfg.key),
            "-H",
            "Content-Type: application/json",
            "-H",
            "Accept: text/event-stream",
            "--data-binary",
            "@-",
            &url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn curl: {e}"))?;

    let pid = child.id();
    {
        let mut si = child.stdin.take().ok_or("no stdin")?;
        si.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take();
    let (tx, rx) = channel();

    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut out = StreamOut::default();
        let mut got_data = false;
        // Keep the raw body: an API rejection arrives as JSON, not SSE, and
        // "no data from API" without the reason is useless for the user.
        let mut raw = String::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if !line.trim_start().starts_with("data:") && raw.len() < 600 {
                raw.push_str(&line);
                raw.push('\n');
            }
            if line.trim_start().starts_with("data:") {
                got_data = true;
            }
            if apply_line(&line, &mut out, &tx) {
                break;
            }
        }
        let mut err = String::new();
        if let Some(mut se) = stderr {
            let mut s = String::new();
            let _ = se.read_to_string(&mut s);
            err = s.trim().to_string();
        }
        let _ = child.wait();
        if !got_data {
            let body = raw.trim();
            let msg = if !err.is_empty() {
                err
            } else if !body.is_empty() {
                // Pull out {"error":{"message":...}} when present.
                let hint = parse(body)
                    .and_then(|v| {
                        v.get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| body.chars().take(200).collect());
                format!("API error: {hint}")
            } else {
                "no data from API (check network / key)".into()
            };
            let _ = tx.send(ApiEv::Failed(msg));
        } else {
            let _ = tx.send(ApiEv::Done(out));
        }
    });

    Ok(Chat { rx, pid })
}

/// Models the endpoint advertises, with their supported effort levels.
pub fn list_models(cfg: &Cfg) -> Result<Vec<(String, String, Vec<String>)>, String> {
    let url = format!("{}/models", cfg.base.trim_end_matches('/'));
    if !url.starts_with("https://") {
        return Err("refusing non-HTTPS endpoint".into());
    }
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "20",
            "-H",
            &format!("Authorization: Bearer {}", cfg.key),
            &url,
        ])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let v = parse(&text).ok_or_else(|| format!("bad JSON: {}", text.chars().take(120).collect::<String>()))?;
    if let Some(err) = v.get("error") {
        let m = err.get("message").and_then(|x| x.as_str()).unwrap_or("model list failed");
        return Err(m.to_string());
    }
    let mut models = Vec::new();
    if let Some(arr) = v.get("data").and_then(|d| d.as_arr()) {
        for m in arr {
            let id = m.get("id").and_then(|x| x.as_str()).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let name = m
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or(id)
                .to_string();
            let efforts = m
                .get("effort")
                .and_then(|e| e.get("supported_levels"))
                .and_then(|l| l.as_arr())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            models.push((id.to_string(), name, efforts));
        }
    }
    Ok(models)
}

/// Blocking wrapper for background jobs.
pub fn chat_stream<F: FnMut(&str)>(
    cfg: &Cfg,
    messages: &J,
    tools: &J,
    mut on_delta: F,
) -> Result<StreamOut, String> {
    let chat = spawn_chat(cfg, messages, tools)?;
    loop {
        match chat.rx.recv() {
            Ok(ApiEv::Content(c)) => on_delta(&c),
            Ok(ApiEv::Reason(_)) => {}
            Ok(ApiEv::Done(o)) => return Ok(o),
            Ok(ApiEv::Failed(e)) => return Err(e),
            Err(_) => return Err("stream closed unexpectedly".into()),
        }
    }
}
