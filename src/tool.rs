//! Tool contracts + execution pipeline, ported from DSH `@deepseek-ai/dsh-tools`.
//!
//! The two structural rules that matter, and that the earlier version broke:
//!
//! 1. `execute()` returns a *canonical JSON value*, never model-facing prose.
//!    `render()` projects that value to text. This keeps "what happened"
//!    separate from "what the model reads", and lets a result be re-rendered
//!    (TUI card vs model text) without re-running the side effect.
//! 2. The executor owns the pipeline around the tool body:
//!
//!    tool/call -> tools/pre-execute -> [guards] -> tools/execute (deadline)
//!              -> execute() -> projectContent -> tools/post-execute
//!              -> finalizeContent -> tool/result
//!
//!    A tool body knows nothing about validation, truncation, deadlines or
//!    context injection. No tool on this device is sandboxed by design; the
//!    `pre`/`post` stages are still present so policy has a place to live.
//!
//! `additionalContexts` produced by post-execute are returned on the outcome
//! and injected by the harness as a `user/message` *after* the recorded
//! `tool/result`s of the batch, exactly as DSH orders it.

use crate::json::J;

/// Bounded output: DSH's "prefer bounded outputs" convention.
pub const MAX_OUT: usize = 6000;

/// A declared tool parameter. Rules live on the parameter, not the tool
/// description (DSH tool-design: "put parameter rules on the parameter").
pub struct Param {
    pub name: &'static str,
    pub kind: &'static str,
    pub required: bool,
    pub description: &'static str,
    /// Non-empty means this parameter is an `enum`.
    pub values: &'static [&'static str],
}

impl Param {
    pub fn req(name: &'static str, kind: &'static str, description: &'static str) -> Self {
        Param { name, kind, required: true, description, values: &[] }
    }
    pub fn opt(name: &'static str, kind: &'static str, description: &'static str) -> Self {
        Param { name, kind, required: false, description, values: &[] }
    }
    pub fn one_of(
        name: &'static str,
        description: &'static str,
        values: &'static [&'static str],
    ) -> Self {
        Param { name, kind: "string", required: true, description, values }
    }
    pub fn one_of_opt(
        name: &'static str,
        description: &'static str,
        values: &'static [&'static str],
    ) -> Self {
        Param { name, kind: "string", required: false, description, values }
    }
}

/// Structured tool failure. Becomes an `isError` tool result; it is not a
/// message the tool renders itself.
pub struct Fail {
    pub message: String,
}

impl Fail {
    pub fn new(message: impl Into<String>) -> Self {
        Fail { message: message.into() }
    }
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub params: Vec<Param>,
    /// DSH `isConcurrencySafe`: may this call overlap a sibling call?
    /// Only `true` opts in; everything else runs exclusively, in model order.
    pub concurrency_safe: bool,
    /// DSH `timeoutMs`: cooperative deadline budget owned by the executor.
    /// Never sent to the model.
    pub timeout_ms: u64,
    /// Canonical value out. Must not render prose.
    pub execute: fn(&J) -> Result<J, Fail>,
    /// Pure projection from (args, value) to model-facing text.
    pub render: fn(&J, &J) -> String,
    /// Pure projection from args to a one-line pending card title (UI only).
    pub present: fn(&J) -> String,
    /// post-execute policy hook: may attach `additionalContexts`.
    pub post: Option<fn(&J, &J) -> Option<String>>,
}

/// One tool call as the model emitted it.
#[derive(Clone)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub raw_args: String,
}

#[derive(Clone)]
pub struct Outcome {
    pub call_id: String,
    pub name: String,
    /// Final model-facing content.
    pub content: String,
    pub is_error: bool,
    /// Context to inject as a user message after this batch's results.
    pub context: Option<String>,
    /// Canonical value, kept for the UI/`meta` projection.
    pub value: Option<J>,
}

/// Pipeline observability handed to the harness/TUI.
pub enum PipeEvent {
    /// `tool/call` — logged before execution.
    Call {
        id: String,
        name: String,
        title: String,
    },
    /// `tool/result` — settled, in model order.
    Made(Outcome),
}

fn err_out(c: &Call, message: impl Into<String>) -> Outcome {
    Outcome {
        call_id: c.id.clone(),
        name: c.name.clone(),
        content: format!("error: {}", message.into()),
        is_error: true,
        context: None,
        value: None,
    }
}

/// `tools/pre-execute`: parse, then validate required/typed parameters.
/// Returns the parsed args or the denial reason.
fn pre_execute(def: &ToolDef, c: &Call) -> Result<J, String> {
    let raw = if c.raw_args.trim().is_empty() { "{}" } else { c.raw_args.as_str() };
    let args = crate::json::parse(raw).ok_or_else(|| "invalid JSON arguments".to_string())?;
    if !matches!(args, J::Obj(_)) {
        return Err("arguments must be a JSON object".to_string());
    }
    for p in &def.params {
        let v = args.get(p.name);
        if p.required && v.is_none() {
            return Err(format!("missing required parameter '{}'", p.name));
        }
        if let Some(v) = v {
            let ok = match p.kind {
                "string" => v.as_str().is_some(),
                "integer" | "number" => v.as_num().is_some(),
                "boolean" => matches!(v, J::Bool(_)),
                _ => true,
            };
            if !ok {
                return Err(format!("parameter '{}' must be {}", p.name, p.kind));
            }
            if !p.values.is_empty() {
                match v.as_str() {
                    Some(s) if p.values.contains(&s) => {}
                    _ => {
                        return Err(format!(
                            "parameter '{}' must be one of: {}",
                            p.name,
                            p.values.join(", ")
                        ))
                    }
                }
            }
        }
    }
    Ok(args)
}

/// `finalizeContent`: last-mile content-only invariant.
fn finalize(c: &Call, mut content: String, is_error: bool) -> String {
    if is_error && content.trim().is_empty() {
        content = format!("error: {} failed", c.name);
    }
    if content.trim().is_empty() {
        content = "(no output)".to_string();
    }
    if content.len() > MAX_OUT {
        let mut end = MAX_OUT;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        let cut = content.len() - end;
        content = format!("{}\n... [truncated {} bytes]", &content[..end], cut);
    }
    content
}

/// The executor's view of the front end while a batch runs. Lets the driver
/// keep the keyboard live and notice Esc while a slow tool is running.
pub trait ToolHost {
    fn call(&mut self, ev: PipeEvent);
    /// True when the user asked to cancel.
    fn cancel(&mut self) -> bool;
}

/// Host for callers with no UI (background jobs).
pub struct NullHost;
impl ToolHost for NullHost {
    fn call(&mut self, _ev: PipeEvent) {}
    fn cancel(&mut self) -> bool {
        false
    }
}

/// Run one batch. Ordered pre-execute, execute, ordered post-execute.
///
/// Each tool body runs on its own thread so the host can keep pumping the UI
/// and observe a cancel request. A slow command must not freeze the prompt:
/// that is the whole reason this is not a plain call.
pub fn run_batch(reg: &[ToolDef], calls: &[Call], host: &mut dyn ToolHost) -> Vec<Outcome> {
    let mut outs: Vec<Option<Outcome>> = (0..calls.len()).map(|_| None).collect();

    // ---- tools/pre-execute, in model order ----
    let mut queue: Vec<(usize, &ToolDef, J)> = Vec::new();
    for (i, c) in calls.iter().enumerate() {
        let def = match reg.iter().find(|t| t.name == c.name) {
            Some(d) => d,
            None => {
                let o = err_out(c, format!("unknown tool '{}'", c.name));
                host.call(PipeEvent::Made(o.clone()));
                outs[i] = Some(o);
                continue;
            }
        };
        match pre_execute(def, c) {
            Ok(args) => {
                host.call(PipeEvent::Call {
                    id: c.id.clone(),
                    name: def.name.to_string(),
                    title: (def.present)(&args),
                });
                queue.push((i, def, args));
            }
            Err(reason) => {
                host.call(PipeEvent::Call {
                    id: c.id.clone(),
                    name: def.name.to_string(),
                    title: format!("{} (rejected)", def.name),
                });
                let o = err_out(c, reason);
                host.call(PipeEvent::Made(o.clone()));
                outs[i] = Some(o);
            }
        }
    }

    // ---- tools/execute, pumping the host between polls ----
    let mut cancelled = false;
    let mut made: Vec<(usize, Outcome)> = Vec::new();
    for (i, def, args) in queue {
        let c = &calls[i];
        if cancelled {
            made.push((i, err_out(c, "cancelled by user")));
            continue;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let exec = def.execute;
        let args_c = args.clone();
        std::thread::spawn(move || {
            let _ = tx.send(exec(&args_c));
        });

        let mut settled: Option<Result<J, String>> = None;
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(15)) {
                Ok(Ok(v)) => {
                    settled = Some(Ok(v));
                    break;
                }
                Ok(Err(f)) => {
                    settled = Some(Err(f.message));
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if host.cancel() {
                        cancelled = true;
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    settled = Some(Err("tool worker exited".into()));
                    break;
                }
            }
        }

        let outcome = match settled {
            None => err_out(c, "cancelled by user"),
            Some(Ok(value)) => {
                let rendered = (def.render)(&args, &value);
                let content = finalize(c, rendered, false);
                let context = match def.post {
                    Some(hook) => hook(&args, &value),
                    None => None,
                };
                Outcome {
                    call_id: c.id.clone(),
                    name: c.name.clone(),
                    content,
                    is_error: false,
                    context,
                    value: Some(value),
                }
            }
            Some(Err(message)) => {
                let content = finalize(c, message, true);
                Outcome {
                    call_id: c.id.clone(),
                    name: c.name.clone(),
                    content,
                    is_error: true,
                    context: None,
                    value: None,
                }
            }
        };
        made.push((i, outcome));
    }

    // ---- tool/result, model order ----
    for (i, o) in made {
        host.call(PipeEvent::Made(o.clone()));
        outs[i] = Some(o);
    }

    outs.into_iter()
        .map(|o| {
            o.unwrap_or(Outcome {
                call_id: String::new(),
                name: String::new(),
                content: "unsettled call".to_string(),
                is_error: true,
                context: None,
                value: None,
            })
        })
        .collect()
}

/// Build the OpenAI-compatible `tools` array from the registry, so the schema
/// has exactly one source of truth.
pub fn definitions(reg: &[ToolDef]) -> J {
    let mut arr = Vec::new();
    for t in reg {
        let mut props = Vec::new();
        let mut required = Vec::new();
        for p in &t.params {
            let mut o = vec![
                ("type".to_string(), J::s(p.kind)),
                ("description".to_string(), J::s(p.description)),
            ];
            if !p.values.is_empty() {
                o.push((
                    "enum".to_string(),
                    J::Arr(p.values.iter().map(|v| J::s(v)).collect()),
                ));
            }
            props.push((p.name.to_string(), J::Obj(o)));
            if p.required {
                required.push(J::s(p.name));
            }
        }
        arr.push(J::Obj(vec![
            ("type".to_string(), J::s("function")),
            (
                "function".to_string(),
                J::Obj(vec![
                    ("name".to_string(), J::s(t.name)),
                    ("description".to_string(), J::s(t.description)),
                    (
                        "parameters".to_string(),
                        J::Obj(vec![
                            ("type".to_string(), J::s("object")),
                            ("properties".to_string(), J::Obj(props)),
                            ("required".to_string(), J::Arr(required)),
                            ("additionalProperties".to_string(), J::Bool(false)),
                        ]),
                    ),
                ]),
            ),
        ]));
    }
    J::Arr(arr)
}

// ---------------------------------------------------------------------------
// render helpers shared by tool definitions
// ---------------------------------------------------------------------------

pub fn obj(pairs: Vec<(&str, J)>) -> J {
    J::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn num_arg(args: &J, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_num())
}

pub fn str_arg(args: &J, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}
