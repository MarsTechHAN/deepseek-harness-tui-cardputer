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

/// Executor deadline for a tool that does not declare its own `timeout_ms`.
/// DSH applies a policy default (`dsh-tool-call-timeout-policy`); 60 s is well
/// above every built-in tool and still short of a wedged turn.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// Default value advertised in JSON Schema for an optional parameter.
#[derive(Clone, Copy)]
pub enum DefaultVal {
    Bool(bool),
    Int(i64),
    Str(&'static str),
}

/// A declared tool parameter. Rules live on the parameter, not the tool
/// description (DSH tool-design: "put parameter rules on the parameter").
pub struct Param {
    pub name: &'static str,
    pub kind: &'static str,
    pub required: bool,
    pub description: &'static str,
    /// Non-empty means this parameter is an `enum`.
    pub values: &'static [&'static str],
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub default: Option<DefaultVal>,
}

impl Param {
    pub fn req(name: &'static str, kind: &'static str, description: &'static str) -> Self {
        Param {
            name,
            kind,
            required: true,
            description,
            values: &[],
            min: None,
            max: None,
            default: None,
        }
    }
    pub fn opt(name: &'static str, kind: &'static str, description: &'static str) -> Self {
        Param {
            name,
            kind,
            required: false,
            description,
            values: &[],
            min: None,
            max: None,
            default: None,
        }
    }
    pub fn one_of(
        name: &'static str,
        description: &'static str,
        values: &'static [&'static str],
    ) -> Self {
        Param {
            name,
            kind: "string",
            required: true,
            description,
            values,
            min: None,
            max: None,
            default: None,
        }
    }
    pub fn one_of_opt(
        name: &'static str,
        description: &'static str,
        values: &'static [&'static str],
    ) -> Self {
        Param {
            name,
            kind: "string",
            required: false,
            description,
            values,
            min: None,
            max: None,
            default: None,
        }
    }
    pub fn min(mut self, min: i64) -> Self {
        self.min = Some(min);
        self
    }
    pub fn range(mut self, min: i64, max: i64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
    pub fn default_int(mut self, v: i64) -> Self {
        self.default = Some(DefaultVal::Int(v));
        self
    }
    pub fn default_bool(mut self, v: bool) -> Self {
        self.default = Some(DefaultVal::Bool(v));
        self
    }
    pub fn default_str(mut self, v: &'static str) -> Self {
        self.default = Some(DefaultVal::Str(v));
        self
    }
    /// Safeguard escalation flag attached to permission-gated tools.
    pub fn safeguard_approval() -> Self {
        Param::opt(
            "request_user_approval",
            "boolean",
            "Set to true ONLY when retrying a call just blocked by the safeguard and you judge it is genuinely necessary; opens the user confirmation pop-up.",
        )
        .default_bool(false)
    }
    /// Safeguard explanation shown to the user in the approval pop-up.
    pub fn safeguard_reason() -> Self {
        Param::opt(
            "approval_reason",
            "string",
            "When request_user_approval is true, pass the prior safeguard block reason and briefly explain why this action is necessary.",
        )
    }
}

/// Resource access pattern of a single tool invocation for parallel scheduling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// No shared mutable resource (e.g. network read, progress row).
    Free,
    /// Shared read of a named resource (e.g. "file:<path>", "fs", "memory").
    Read(String),
    /// Exclusive mutation of a named resource.
    Write(String),
    /// Global barrier: waits for all earlier calls and blocks all later calls.
    Barrier,
}

fn keys_overlap(a: &str, b: &str) -> bool {
    a == b
        || (a == "fs" && b.starts_with("file:"))
        || (b == "fs" && a.starts_with("file:"))
}

/// True when an earlier call with `a` must finish before a later call with `b`.
pub fn access_conflicts(a: &Access, b: &Access) -> bool {
    match (a, b) {
        (Access::Barrier, _) | (_, Access::Barrier) => true,
        (Access::Free, _) | (_, Access::Free) => false,
        (Access::Read(_), Access::Read(_)) => false,
        (Access::Write(k1), Access::Read(k2))
        | (Access::Read(k1), Access::Write(k2))
        | (Access::Write(k1), Access::Write(k2)) => keys_overlap(k1, k2),
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
    /// DSH `isConcurrencySafe`: default scheduling hint when `access` is `None`.
    pub concurrency_safe: bool,
    /// Optional per-call resource access descriptor for fine-grained parallel
    /// execution. When `None`, `concurrency_safe=true` maps to `Access::Free`
    /// and `false` maps to `Access::Barrier`.
    pub access: Option<fn(&J) -> Access>,
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
    /// A verbatim JSON schema for `parameters` (used by generated tools whose
    /// schema comes from an MCP server rather than a parameter list).
    pub schema_override: Option<String>,
    /// Dynamic executor taking the tool name, for tools registered at runtime.
    pub dynamic: Option<fn(&str, &J) -> Result<J, Fail>>,
}

impl ToolDef {
    pub fn access_for(&self, args: &J) -> Access {
        match self.access {
            Some(f) => f(args),
            None if self.concurrency_safe => Access::Free,
            None => Access::Barrier,
        }
    }
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
        /// Parsed arguments as JSON text: the UI expands the call while it
        /// runs, so the user sees what the model is actually doing.
        args: String,
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

/// `tools/pre-execute`: parse, then validate parameter names, types, enums and
/// bounds. Returns the parsed args or an actionable denial reason.
fn pre_execute(def: &ToolDef, c: &Call) -> Result<J, String> {
    let raw = if c.raw_args.trim().is_empty() { "{}" } else { c.raw_args.as_str() };
    let args = crate::json::parse(raw).ok_or_else(|| "invalid JSON arguments".to_string())?;
    let pairs = match &args {
        J::Obj(kv) => kv,
        _ => return Err("arguments must be a JSON object".to_string()),
    };
    // Tools with a custom JSON schema (e.g. MCP or DSH-compat todo_write) own
    // their own field validation inside `execute`.
    if def.schema_override.is_some() {
        return Ok(args);
    }
    for (k, _) in pairs {
        if !def.params.iter().any(|p| p.name == k) {
            let allowed = def
                .params
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join(", ");
            return if allowed.is_empty() {
                Err(format!("unknown parameter '{k}' (tool '{}' takes no parameters)", def.name))
            } else {
                Err(format!("unknown parameter '{k}' (allowed: {allowed})"))
            };
        }
    }
    for p in &def.params {
        let v = args.get(p.name);
        if p.required && v.is_none() {
            return Err(format!("missing required parameter '{}'", p.name));
        }
        if let Some(v) = v {
            let ok = match p.kind {
                "string" => v.as_str().is_some(),
                "integer" => v.as_num().map(|n| n.is_finite() && n.fract() == 0.0).unwrap_or(false),
                "number" => v.as_num().map(|n| n.is_finite()).unwrap_or(false),
                "boolean" => matches!(v, J::Bool(_)),
                "array" => v.as_arr().is_some(),
                "object" => matches!(v, J::Obj(_)),
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
            if let Some(n) = v.as_num() {
                if let Some(min) = p.min {
                    if n < min as f64 {
                        return Err(format!("parameter '{}' must be >= {min}", p.name));
                    }
                }
                if let Some(max) = p.max {
                    if n > max as f64 {
                        return Err(format!("parameter '{}' must be <= {max}", p.name));
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
    /// `tools/pre-execute` permission gate. `Err(reason)` becomes a failed
    /// tool result; the tool body never runs.
    fn allow(&mut self, _name: &str, _args: &J) -> Result<(), String> {
        Ok(())
    }
    /// Periodic progress ping while a tool body is still running.
    fn tick(&mut self, _name: &str, _secs: f32) {}
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
    run_batch_with(reg, calls, 0, host)
}

/// Maximum tool calls executed simultaneously. Capped to stay responsive on
/// the 4-core / 512 MB Cardputer Zero without spawning unbounded threads.
const MAX_CONCURRENT_TOOLS: usize = 6;

fn settle_outcome(
    c: &Call,
    def: &ToolDef,
    args: &J,
    res: Result<J, String>,
) -> Outcome {
    match res {
        Ok(value) => {
            let rendered = (def.render)(args, &value);
            let content = finalize(c, rendered, false);
            let context = match def.post {
                Some(hook) => hook(args, &value),
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
        Err(message) => {
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
    }
}

fn call_budget(def: &ToolDef, timeout_cap_ms: u64) -> std::time::Duration {
    let mut budget_ms = if def.timeout_ms > 0 {
        def.timeout_ms
    } else {
        crate::tool::DEFAULT_TIMEOUT_MS
    };
    if timeout_cap_ms > 0 {
        budget_ms = budget_ms.min(timeout_cap_ms);
    }
    std::time::Duration::from_millis(budget_ms.max(1))
}

/// Run one batch with a global timeout cap (ms; 0 = use each tool's own
/// `timeout_ms`). Contiguous `concurrency_safe` calls run in parallel (up to
/// `MAX_CONCURRENT_TOOLS`); any exclusive tool acts as a barrier in model order.
pub fn run_batch_with(
    reg: &[ToolDef],
    calls: &[Call],
    timeout_cap_ms: u64,
    host: &mut dyn ToolHost,
) -> Vec<Outcome> {
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
                    args: args.dump(),
                });
                if let Err(reason) = host.allow(def.name, &args) {
                    let o = err_out(c, format!("permission denied: {reason}"));
                    host.call(PipeEvent::Made(o.clone()));
                    outs[i] = Some(o);
                    continue;
                }
                queue.push((i, def, args));
            }
            Err(reason) => {
                host.call(PipeEvent::Call {
                    id: c.id.clone(),
                    name: def.name.to_string(),
                    title: format!("{} (rejected)", def.name),
                    args: c.raw_args.clone(),
                });
                let o = err_out(c, reason);
                host.call(PipeEvent::Made(o.clone()));
                outs[i] = Some(o);
            }
        }
    }

    // ---- tools/execute via resource-aware parallel DAG scheduler ----
    //
    // Calls whose resource accesses do not conflict run in parallel (up to
    // MAX_CONCURRENT_TOOLS); conflicting calls wait for earlier conflicting
    // calls in model order. `tool/result` is emitted immediately as each call
    // settles, while the returned vector stays strictly in model order.
    let n = queue.len();
    if n > 0 {
        let accesses: Vec<Access> = queue
            .iter()
            .map(|(_, def, args)| def.access_for(args))
            .collect();
        let deps: Vec<Vec<usize>> = (0..n)
            .map(|k| {
                (0..k)
                    .filter(|&j| access_conflicts(&accesses[j], &accesses[k]))
                    .collect()
            })
            .collect();

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            Pending,
            Running,
            Done,
        }
        struct Node<'a> {
            call_idx: usize,
            def: &'a ToolDef,
            args: J,
            budget: std::time::Duration,
            started: Option<std::time::Instant>,
            state: State,
        }
        let mut nodes: Vec<Node<'_>> = queue
            .into_iter()
            .map(|(call_idx, def, args)| Node {
                call_idx,
                def,
                args,
                budget: call_budget(def, timeout_cap_ms),
                started: None,
                state: State::Pending,
            })
            .collect();

        let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<J, Fail>)>();
        let mut active = 0usize;
        let mut done_count = 0usize;
        let mut last_tick = std::time::Instant::now();

        while done_count < n {
            // Launch every ready node whose dependencies have all settled.
            for k in 0..n {
                if active >= MAX_CONCURRENT_TOOLS {
                    break;
                }
                if nodes[k].state != State::Pending {
                    continue;
                }
                if deps[k].iter().all(|&d| nodes[d].state == State::Done) {
                    let tx_c = tx.clone();
                    let exec = nodes[k].def.execute;
                    let dyn_fn = nodes[k].def.dynamic;
                    let tool_name = nodes[k].def.name;
                    let args_c = nodes[k].args.clone();
                    nodes[k].started = Some(std::time::Instant::now());
                    nodes[k].state = State::Running;
                    active += 1;
                    std::thread::spawn(move || {
                        let r = match dyn_fn {
                            Some(f) => f(tool_name, &args_c),
                            None => exec(&args_c),
                        };
                        let _ = tx_c.send((k, r));
                    });
                }
            }

            let settle_msg = |k: usize,
                              res: Result<J, Fail>,
                              nodes: &mut [Node<'_>],
                              outs: &mut [Option<Outcome>],
                              active: &mut usize,
                              done_count: &mut usize,
                              host: &mut dyn ToolHost| {
                if nodes[k].state == State::Running {
                    nodes[k].state = State::Done;
                    *active = active.saturating_sub(1);
                    *done_count += 1;
                    let c = &calls[nodes[k].call_idx];
                    let o = settle_outcome(
                        c,
                        nodes[k].def,
                        &nodes[k].args,
                        res.map_err(|f| f.message),
                    );
                    host.call(PipeEvent::Made(o.clone()));
                    outs[nodes[k].call_idx] = Some(o);
                }
            };

            match rx.recv_timeout(std::time::Duration::from_millis(15)) {
                Ok((k, res)) => {
                    settle_msg(k, res, &mut nodes, &mut outs, &mut active, &mut done_count, host);
                    while done_count < n {
                        match rx.try_recv() {
                            Ok((k2, res2)) => {
                                settle_msg(
                                    k2,
                                    res2,
                                    &mut nodes,
                                    &mut outs,
                                    &mut active,
                                    &mut done_count,
                                    host,
                                );
                            }
                            Err(_) => break,
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if last_tick.elapsed().as_millis() >= 200 {
                        last_tick = std::time::Instant::now();
                        if let Some(first_active) =
                            nodes.iter().find(|nd| nd.state == State::Running)
                        {
                            let label = if active > 1 {
                                format!("{} (+{})", calls[first_active.call_idx].name, active - 1)
                            } else {
                                calls[first_active.call_idx].name.clone()
                            };
                            let secs = first_active
                                .started
                                .map(|t| t.elapsed().as_secs_f32())
                                .unwrap_or(0.0);
                            host.tick(&label, secs);
                        }
                    }
                    if host.cancel() {
                        for nd in &mut nodes {
                            if nd.state != State::Done {
                                nd.state = State::Done;
                                let o = err_out(&calls[nd.call_idx], "cancelled by user");
                                host.call(PipeEvent::Made(o.clone()));
                                outs[nd.call_idx] = Some(o);
                            }
                        }
                        break;
                    }
                    for nd in &mut nodes {
                        if nd.state == State::Running {
                            if let Some(started) = nd.started {
                                if started.elapsed() >= nd.budget {
                                    nd.state = State::Done;
                                    active = active.saturating_sub(1);
                                    done_count += 1;
                                    let secs = nd.budget.as_secs_f64();
                                    let o = err_out(
                                        &calls[nd.call_idx],
                                        format!("tool timed out after {secs:.1}s"),
                                    );
                                    host.call(PipeEvent::Made(o.clone()));
                                    outs[nd.call_idx] = Some(o);
                                }
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    for nd in &mut nodes {
                        if nd.state != State::Done {
                            nd.state = State::Done;
                            let o = err_out(&calls[nd.call_idx], "tool worker exited");
                            host.call(PipeEvent::Made(o.clone()));
                            outs[nd.call_idx] = Some(o);
                        }
                    }
                    break;
                }
            }
        }
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
            if let Some(min) = p.min {
                o.push(("minimum".to_string(), J::Num(min as f64)));
            }
            if let Some(max) = p.max {
                o.push(("maximum".to_string(), J::Num(max as f64)));
            }
            if let Some(d) = p.default {
                let jv = match d {
                    DefaultVal::Bool(b) => J::Bool(b),
                    DefaultVal::Int(n) => J::Num(n as f64),
                    DefaultVal::Str(s) => J::s(s),
                };
                o.push(("default".to_string(), jv));
            }
            props.push((p.name.to_string(), J::Obj(o)));
            if p.required {
                required.push(J::s(p.name));
            }
        }
        let parameters = match &t.schema_override {
            Some(raw) => crate::json::parse(raw).unwrap_or_else(|| {
                J::Obj(vec![("type".to_string(), J::s("object"))])
            }),
            None => J::Obj(vec![
                ("type".to_string(), J::s("object")),
                ("properties".to_string(), J::Obj(props)),
                ("required".to_string(), J::Arr(required)),
                ("additionalProperties".to_string(), J::Bool(false)),
            ]),
        };
        arr.push(J::Obj(vec![
            ("type".to_string(), J::s("function")),
            (
                "function".to_string(),
                J::Obj(vec![
                    ("name".to_string(), J::s(t.name)),
                    ("description".to_string(), J::s(t.description)),
                    ("parameters".to_string(), parameters),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &'static str, timeout_ms: u64, body: fn(&J) -> Result<J, Fail>) -> ToolDef {
        ToolDef {
            name,
            description: "test",
            params: vec![],
            concurrency_safe: false,
            access: None,
            timeout_ms,
            execute: body,
            render: |_a, v| v.dump(),
            present: |_a| "test".into(),
            post: None,
            schema_override: None,
            dynamic: None,
        }
    }

    fn slow(_a: &J) -> Result<J, Fail> {
        std::thread::sleep(std::time::Duration::from_millis(300));
        Ok(J::s("late"))
    }

    fn fast(_a: &J) -> Result<J, Fail> {
        Ok(J::s("ok"))
    }

    struct Recorder {
        events: Vec<(bool, std::time::Instant)>,
    }

    impl ToolHost for Recorder {
        fn call(&mut self, ev: PipeEvent) {
            let is_call = matches!(ev, PipeEvent::Call { .. });
            self.events.push((is_call, std::time::Instant::now()));
        }
        fn cancel(&mut self) -> bool {
            false
        }
    }

    #[test]
    fn call_event_precedes_result_and_execution() {
        let reg = vec![def("fast", 1_000, fast)];
        let calls = vec![Call {
            id: "1".into(),
            name: "fast".into(),
            raw_args: "{}".into(),
        }];
        let mut h = Recorder { events: Vec::new() };
        let outs = run_batch(&reg, &calls, &mut h);
        assert!(!outs[0].is_error);
        // tool/call first, tool/result second (not the other way round).
        assert_eq!(h.events.len(), 2);
        assert!(h.events[0].0, "first event must be tool/call");
        assert!(!h.events[1].0, "second event must be tool/result");
        assert!(h.events[0].1 <= h.events[1].1);
    }

    #[test]
    fn executor_deadline_marks_tool_as_timed_out() {
        let reg = vec![def("slow", 50, slow)];
        let calls = vec![Call {
            id: "1".into(),
            name: "slow".into(),
            raw_args: "{}".into(),
        }];
        let mut h = Recorder { events: Vec::new() };
        let outs = run_batch(&reg, &calls, &mut h);
        assert!(outs[0].is_error, "timed-out tool must be an error result");
        assert!(
            outs[0].content.contains("timed out"),
            "unexpected content: {}",
            outs[0].content
        );
        assert_eq!(h.events.len(), 2, "call and result still both emitted");
    }

    fn sleep_ms(a: &J) -> Result<J, Fail> {
        let ms = num_arg(a, "ms").unwrap_or(100.0) as u64;
        std::thread::sleep(std::time::Duration::from_millis(ms));
        Ok(J::s(&format!("slept-{ms}")))
    }

    #[test]
    fn concurrency_safe_calls_run_in_parallel_and_return_in_model_order() {
        let mut d = def("nap", 2_000, sleep_ms);
        d.concurrency_safe = true;
        d.params = vec![Param::opt("ms", "integer", "duration").range(1, 1000)];
        let reg = vec![d];
        let calls = vec![
            Call { id: "a".into(), name: "nap".into(), raw_args: r#"{"ms":120}"#.into() },
            Call { id: "b".into(), name: "nap".into(), raw_args: r#"{"ms":120}"#.into() },
            Call { id: "c".into(), name: "nap".into(), raw_args: r#"{"ms":120}"#.into() },
        ];
        let t0 = std::time::Instant::now();
        let outs = run_batch(&reg, &calls, &mut NullHost);
        let elapsed = t0.elapsed().as_millis();
        assert!(elapsed < 300, "parallel wave should overlap, took {elapsed}ms");
        assert_eq!(outs.len(), 3);
        assert_eq!(outs[0].call_id, "a");
        assert_eq!(outs[1].call_id, "b");
        assert_eq!(outs[2].call_id, "c");
        assert!(outs.iter().all(|o| !o.is_error));
    }

    #[test]
    fn pre_execute_rejects_unknown_params_floats_and_out_of_range() {
        let mut d = def("bounded", 1_000, fast);
        d.params = vec![
            Param::req("count", "integer", "item count").range(1, 10).default_int(5),
        ];
        let reg = vec![d];
        let schema = definitions(&reg).dump();
        assert!(schema.contains(r#""minimum":1"#), "{schema}");
        assert!(schema.contains(r#""maximum":10"#), "{schema}");
        assert!(schema.contains(r#""default":5"#), "{schema}");

        let bad_calls = vec![
            Call { id: "1".into(), name: "bounded".into(), raw_args: r#"{"count":5,"extra":1}"#.into() },
            Call { id: "2".into(), name: "bounded".into(), raw_args: r#"{"count":2.5}"#.into() },
            Call { id: "3".into(), name: "bounded".into(), raw_args: r#"{"count":0}"#.into() },
            Call { id: "4".into(), name: "bounded".into(), raw_args: r#"{"count":11}"#.into() },
        ];
        let outs = run_batch(&reg, &bad_calls, &mut NullHost);
        assert!(outs[0].is_error && outs[0].content.contains("unknown parameter 'extra'"));
        assert!(outs[1].is_error && outs[1].content.contains("must be integer"));
        assert!(outs[2].is_error && outs[2].content.contains(">= 1"));
        assert!(outs[3].is_error && outs[3].content.contains("<= 10"));
    }

    #[test]
    fn dag_runs_independent_writes_in_parallel_while_serializing_same_resource() {
        static ORDER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        fn record(a: &J) -> Result<J, Fail> {
            let tag = str_arg(a, "tag").unwrap_or_default();
            let ms = num_arg(a, "ms").unwrap_or(0.0) as u64;
            if ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            ORDER.lock().unwrap().push(tag.clone());
            Ok(J::s(&tag))
        }
        fn key_access(a: &J) -> Access {
            Access::Write(format!("file:{}", str_arg(a, "res").unwrap_or_default()))
        }
        let mut w = def("write_res", 2_000, record);
        w.access = Some(key_access);
        w.params = vec![
            Param::req("res", "string", "resource"),
            Param::req("tag", "string", "tag"),
            Param::opt("ms", "integer", "delay"),
        ];
        ORDER.lock().unwrap().clear();
        let reg = vec![w];
        // a1 (120ms) and a2 share "a" -> a2 MUST run after a1.
        // b1 (120ms) touches "b" -> runs in parallel with a1!
        let calls = vec![
            Call { id: "1".into(), name: "write_res".into(), raw_args: r#"{"res":"a","tag":"a1","ms":120}"#.into() },
            Call { id: "2".into(), name: "write_res".into(), raw_args: r#"{"res":"a","tag":"a2","ms":10}"#.into() },
            Call { id: "3".into(), name: "write_res".into(), raw_args: r#"{"res":"b","tag":"b1","ms":120}"#.into() },
        ];
        let t0 = std::time::Instant::now();
        let outs = run_batch(&reg, &calls, &mut NullHost);
        let elapsed = t0.elapsed().as_millis();
        assert!(elapsed < 230, "independent writes should overlap, took {elapsed}ms");
        assert_eq!(outs[0].call_id, "1");
        assert_eq!(outs[1].call_id, "2");
        assert_eq!(outs[2].call_id, "3");
        let order = ORDER.lock().unwrap().clone();
        let p_a1 = order.iter().position(|x| x == "a1").unwrap();
        let p_a2 = order.iter().position(|x| x == "a2").unwrap();
        assert!(p_a1 < p_a2, "same-resource calls must stay ordered: {order:?}");
    }
}
