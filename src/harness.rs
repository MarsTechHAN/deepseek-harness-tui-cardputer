//! The turn/step driver, ported from DSH `core/agent-loop`.
//!
//! A **step** is one model request plus the tools it calls. A **turn** is zero
//! or more steps. Event order is DSH's, minus the seams that do not exist on a
//! 512 MB single-user device (no sandbox, no approval, no subagents):
//!
//! ```text
//! turn/start
//!   agent/pre-step            (context pressure -> compact claimed history)
//!   step/start
//!   request                   (prompt + step tool schema set)
//!   assistant/message         (thinking, then streamed content)
//!   tool/call*  -> tools/pre-execute -> tools/execute -> tools/result*
//!   additionalContexts        (FIFO, injected as user/message after results)
//!   step/end
//!     -> another step while the model owes a request
//! turn/end
//! ```
//!
//! The driver never touches the terminal. It talks to a [`Ui`], which is also
//! the place the keyboard is polled from, so typing stays live during a turn
//! and `Esc` can cancel it.

use crate::api::{self, ApiEv, Cfg, StreamOut, ToolCall};
use crate::json::J;
use crate::tool::{self, Call, PipeEvent, ToolDef};
use std::sync::mpsc::TryRecvError;
use std::time::Instant;

/// Everything that bounds one turn. `0` means "no limit" throughout: the user
/// asked for the old hard-coded step cap to be removed, with an opt-in config
/// key instead.
#[derive(Clone)]
pub struct RunOpts {
    pub max_steps: u32,
    pub compact: crate::compact::Settings,
    pub context_time: bool,
    /// Minimum seconds between clock readings (DSH default 600).
    pub context_refresh_secs: u64,
    /// 1-based turn number used by the clock-context text.
    pub turn: u32,
    /// Context window of the configured model, 0 when unknown.
    pub window: u64,
    /// Global tool timeout cap in ms; 0 = each tool's own `timeout_ms`.
    pub tool_timeout_ms: u64,
    /// Tool-call permission policy.
    pub sandbox: crate::sandbox::Settings,
    /// Transient-failure retries and fallback models.
    pub retries: u32,
    pub fallbacks: Vec<String>,
}

impl Default for RunOpts {
    fn default() -> Self {
        RunOpts {
            max_steps: 0,
            compact: crate::compact::Settings::from_config(&crate::config::Config::default()),
            context_time: true,
            context_refresh_secs: 600,
            turn: 1,
            window: 0,
            tool_timeout_ms: 0,
            sandbox: crate::sandbox::Settings::default(),
            retries: 0,
            fallbacks: Vec::new(),
        }
    }
}

impl RunOpts {
    pub fn from_config(c: &crate::config::Config, window: u64) -> RunOpts {
        RunOpts {
            max_steps: c.max_steps,
            compact: crate::compact::Settings::from_config(c),
            context_time: c.context_time,
            context_refresh_secs: c.context_refresh_secs,
            turn: 1,
            window,
            tool_timeout_ms: c.tool_timeout_ms,
            sandbox: crate::sandbox::Settings::from_config(c),
            retries: c.api_retries,
            fallbacks: c.api_fallbacks.clone(),
        }
    }
}

/// What one turn did. The goal driver needs to know whether the turn ended by
/// itself (so the goal can be judged) or was interrupted/failed.
#[derive(Default)]
pub struct TurnReport {
    #[allow(dead_code)]
    pub steps: u32,
    /// The turn was cut by the configured step cap (model did not stop).
    pub limit_hit: bool,
    pub cancelled: bool,
    pub failed: bool,
    pub text: String,
}

/// What the UI wants done with the running turn.
#[derive(PartialEq, Eq)]
pub enum Action {
    Continue,
    Cancel,
}

#[allow(dead_code)] // events carry step numbers for future front ends
pub enum Ev {
    TurnStart,
    StepStart(u32),
    /// Transient model delta (`agent/assistant-stream` chunk).
    Delta(String),
    /// Live thinking progress. `chars` is how much has arrived so far.
    Thinking { secs: f32, chars: usize },
    /// A piece of the reasoning text, shown live and folded when content starts.
    ThinkingDelta(String),
    /// A tool body has been running for `secs` seconds (live progress ping).
    ToolTick { name: String, secs: f32 },
    /// Thinking finished; the UI should collapse it to one line.
    ThinkingDone { secs: f32, chars: usize },
    /// Settled assistant text (`assistant/message`).
    Assistant(String),
    /// `tool/call`, logged before execution. `title` is UI presentation only;
    /// `args` is the parsed argument JSON, which the UI expands into the card
    /// so the user can see exactly what is running.
    ToolCall {
        id: String,
        name: String,
        title: String,
        args: String,
    },
    /// `tool/result`, settled in model order.
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    /// Progress line from the `state_report` tool: one short sentence the TUI
    /// pins to the top row until the next transcript line arrives.
    StateReport(String),
    /// `additionalContexts` injected as a user message after the batch.
    Context(String),
    /// Automatic context compaction started for this many tokens.
    Compacting { tokens: u64, messages: usize },
    /// Compaction finished; `ok=false` means the history was left untouched.
    CompactingDone { ok: bool, note: String },
    /// The agent asked to take over the screen with a composed panel.
    Artifact { title: String, body: String },
    /// Harness-level notice (limit reached, failure, compaction).
    Note(String),
    /// Informational notice (compaction done, pruned, step budget reached).
    Info(String),
    /// Actionable recovery hint shown highlighted below an error.
    Hint(String),
    StepEnd(u32),
    TurnEnd { steps: u32 },
}

/// Everything the driver needs from the front end.
pub trait Ui {
    fn on(&mut self, ev: Ev);
    /// Called continuously while the request is in flight. Implementations
    /// must not block for long: the keyboard is read here.
    fn pump(&mut self) -> Action;
    /// True once the interrupt belongs to the whole turn (Esc, voice), not
    /// just to the tool call that happened to be running. The driver checks it
    /// after every batch so Esc really stops the turn instead of handing
    /// cancelled tool results back to the model for another step.
    fn interrupted(&self) -> bool {
        false
    }
    /// Strict-mode permission pop-up. Returning false rejects the call.
    fn confirm(&mut self, request: &crate::sandbox::Approval) -> bool;
    /// A message typed while busy that should be injected at the next step
    /// boundary (DSH `steer`), not interrupt the turn.
    fn steer(&mut self) -> Option<String> {
        None
    }
    /// Creator mode: hand the panel to a saved app until it exits, is stopped
    /// by a long-press of Esc, or times out. Front ends without an interactive
    /// screen reject the request instead of hanging.
    fn run_app(
        &mut self,
        _app: &crate::apps::App,
        _timeout_s: u64,
        _args: &str,
    ) -> Result<String, String> {
        Err("app_run needs the interactive screen (the TUI); this front end cannot take over the panel"
            .to_string())
    }
}

/// Bridges the tool executor's host callbacks back onto the UI, so a running
/// tool call can still deliver Esc.
struct UiHost<'a> {
    ui: &'a mut dyn Ui,
    api: Cfg,
    sandbox: crate::sandbox::Settings,
}

impl<'a> tool::ToolHost for UiHost<'a> {
    fn call(&mut self, ev: PipeEvent) {
        match ev {
            PipeEvent::Call { id, name, title, args } => {
                self.ui.on(Ev::ToolCall { id, name, title, args })
            }
            PipeEvent::Made(o) => {
                // A progress report is a UI event, not a tool card: surface it
                // the moment the call settles, before the batch moves on.
                let report = if o.name == "state_report" && !o.is_error {
                    o.value
                        .as_ref()
                        .and_then(|v| v.get("text"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };
                self.ui.on(Ev::ToolResult {
                    id: o.call_id,
                    name: o.name,
                    content: o.content,
                    is_error: o.is_error,
                });
                if let Some(text) = report {
                    self.ui.on(Ev::StateReport(text));
                }
            }
        }
    }
    fn cancel(&mut self) -> bool {
        // A tool may be waiting for the panel (creator mode app_run): service
        // that request before pumping keys, so the app starts within 15 ms.
        crate::apps::service_launch(self.ui);
        self.ui.pump() == Action::Cancel
    }
    fn tick(&mut self, name: &str, secs: f32) {
        self.ui.on(Ev::ToolTick { name: name.to_string(), secs });
    }
    fn allow(&mut self, name: &str, args: &J) -> Result<(), String> {
        struct UiConfirmer<'a> {
            ui: &'a mut dyn Ui,
        }
        impl crate::sandbox::Confirmer for UiConfirmer<'_> {
            fn confirm(&mut self, request: &crate::sandbox::Approval) -> bool {
                self.ui.confirm(request)
            }
        }
        let mut confirmer = UiConfirmer { ui: &mut *self.ui };
        crate::sandbox::authorize(name, args, &self.sandbox, &self.api, &mut confirmer)
    }
}

pub fn msg(role: &str, content: &str) -> J {
    J::o(vec![("role", J::s(role)), ("content", J::s(content))])
}

fn assistant_with_tools(text: &str, calls: &[ToolCall]) -> J {
    let mut arr = Vec::new();
    for c in calls {
        arr.push(J::o(vec![
            ("id", J::s(&c.id)),
            ("type", J::s("function")),
            (
                "function",
                J::o(vec![("name", J::s(&c.name)), ("arguments", J::s(&c.args))]),
            ),
        ]));
    }
    J::o(vec![
        ("role", J::s("assistant")),
        ("content", J::s(text)),
        ("tool_calls", J::Arr(arr)),
    ])
}

/// Actionable next step for a failed request, shown as a dim info line.
/// i18n key of the recovery hint for a provider error, if one is known.
fn actionable_hint(message: &str) -> Option<&'static str> {
    let m = message.to_ascii_lowercase();
    if m.contains("401") || m.contains("403") || m.contains("unauthorized")
        || m.contains("invalid api key") || m.contains("authentication")
    {
        Some("err_hint_auth")
    } else if m.contains("429") || m.contains("rate limit") || m.contains("quota") {
        Some("err_hint_rate")
    } else if is_transient(message) || m.contains("network") {
        Some("err_hint_network")
    } else {
        None
    }
}

/// Whether a provider error is a context-window overflow (`dsh-agent-loop`
/// treats this as an explicit compaction trigger).
/// Transient network/provider failures worth retrying (and switching model).
fn is_transient(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("curl")
        || m.contains("connection")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("502")
        || m.contains("503")
        || m.contains("504")
        || m.contains("429")
        || m.contains("rate limit")
        || m.contains("too many requests")
        || m.contains("network")
        || m.contains("stream closed")
        || m.contains("no data from api")
}

fn is_context_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("context length")
        || m.contains("context window")
        || m.contains("context_length_exceeded")
        || m.contains("maximum context")
        || m.contains("too many tokens")
        || (m.contains("prompt") && m.contains("too long"))
        || (m.contains("exceed") && m.contains("token"))
}

/// Verbatim DSH `dsh-repeat-tool-reminder` text (first threshold and later
/// thresholds differ; both are reproduced).
fn repeat_reminder(name: &str, args: &str, count: u32) -> String {
    if count <= 3 {
        "You are repeating the exact same tool call with identical arguments. Carefully analyze the previous result before calling again: if the task is not complete, try a different approach or different arguments instead of repeating the call."
            .to_string()
    } else {
        let shown: String = args.chars().take(500).collect();
        let extra = args.chars().count().saturating_sub(500);
        let args_text = if extra > 0 {
            format!("{shown}\u{2026} (+{extra} more chars)")
        } else {
            shown
        };
        format!(
            "Repeated tool call detected:\n- tool: {name}\n- consecutive_calls: {count}\n- arguments: {args_text}\nThe repeated calls are not making progress. Do not call this tool with these exact arguments again. Inspect the latest result and choose a different action, different arguments, or finish the task if enough evidence has been gathered."
        )
    }
}

/// Serialise the transcript directly to JSON. Avoids the deep clone of the
/// whole message tree that `J::Arr(msgs.clone()).dump()` used to make on every
/// step (the context can be hundreds of KB near the limit).
pub(crate) fn serialize_msgs(msgs: &[J]) -> String {
    let mut s = String::with_capacity(msgs.len() * 192);
    s.push('[');
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&m.dump());
    }
    s.push(']');
    s
}

/// One streamed request. Drains the API channel while pumping the UI, so the
/// user can keep typing and can cancel with Esc.
fn stream_once(
    cfg: &Cfg,
    messages_json: &str,
    defs: &J,
    ui: &mut dyn Ui,
) -> Result<StreamOut, (StreamOut, bool)> {
    let chat = match api::spawn_chat_json(cfg, messages_json, defs) {
        Ok(c) => c,
        Err(e) => {
            let mut o = StreamOut::default();
            o.finish = format!("spawn failed: {e}");
            return Err((o, false));
        }
    };

    let mut acc = String::new();
    let mut reason_chars = 0usize;
    // The clock starts with the request, not with the first reasoning token:
    // a model that emits no reasoning_content still gets a "thinking" line
    // instead of a silent screen.
    let mut think_t0: Option<Instant> = Some(Instant::now());
    let mut last_think: Option<Instant> = None;
    let cancelled = false;

    loop {
        let mut settled: Option<Result<StreamOut, String>> = None;
        loop {
            match chat.rx.try_recv() {
                Ok(ApiEv::Reason(r)) => {
                    reason_chars += r.chars().count();
                    ui.on(Ev::ThinkingDelta(r));
                }
                Ok(ApiEv::Content(c)) => {
                    if let Some(t0) = think_t0.take() {
                        ui.on(Ev::ThinkingDone {
                            secs: t0.elapsed().as_secs_f32(),
                            chars: reason_chars,
                        });
                    }
                    acc.push_str(&c);
                    ui.on(Ev::Delta(c));
                }
                Ok(ApiEv::Done(o)) => {
                    settled = Some(Ok(o));
                    break;
                }
                Ok(ApiEv::Failed(e)) => {
                    settled = Some(Err(e));
                    break;
                }
                Err(TryRecvError::Empty) => {
                    // Repaint the thinking line a few times a second.
                    if let Some(t0) = think_t0 {
                        let due = last_think
                            .map(|t| t.elapsed().as_millis() >= 150)
                            .unwrap_or(true);
                        if due {
                            ui.on(Ev::Thinking {
                                secs: t0.elapsed().as_secs_f32(),
                                chars: reason_chars,
                            });
                            last_think = Some(Instant::now());
                        }
                    }
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    settled = Some(Err("stream closed unexpectedly".into()));
                    break;
                }
            }
        }
        if let Some(r) = settled {
            if let Some(t0) = think_t0.take() {
                ui.on(Ev::ThinkingDone {
                    secs: t0.elapsed().as_secs_f32(),
                    chars: reason_chars,
                });
            }
            return match r {
                Ok(o) => Ok(o),
                Err(e) => {
                    let mut o = StreamOut::default();
                    o.text = acc;
                    o.finish = e;
                    Err((o, cancelled))
                }
            };
        }

        if !cancelled && ui.pump() == Action::Cancel {
            api::kill(chat.pid);
            ui.on(Ev::Info(crate::i18n::t("ui_cancelled").to_string()));
            // Keep draining until the killed curl settles, otherwise the
            // reader thread outlives the turn.
            let deadline = Instant::now();
            while deadline.elapsed().as_millis() < 2000 {
                if let Ok(ApiEv::Done(_)) | Ok(ApiEv::Failed(_)) = chat.rx.recv_timeout(
                    std::time::Duration::from_millis(50),
                ) {
                    break;
                }
            }
            let mut o = StreamOut::default();
            o.text = acc;
            o.finish = "cancelled".into();
            return Err((o, true));
        }

        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

/// Absolute history ceiling, independent of token accounting. Compaction is
/// the normal bound, but it can be disabled, fail, or compute an unbounded
/// budget when the endpoint reports no context window; without this a long
/// session grows until the device runs out of memory.
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

/// Cheap size estimate of one JSON value, without serializing it.
fn j_len(v: &J) -> usize {
    match v {
        J::Null => 4,
        J::Bool(_) => 5,
        J::Num(_) => 8,
        J::Str(s) => s.len() + 2,
        J::Arr(a) => 2 + a.iter().map(j_len).sum::<usize>(),
        J::Obj(kv) => 2 + kv.iter().map(|(k, v)| k.len() + 4 + j_len(v)).sum::<usize>(),
    }
}

fn approx_bytes(msgs: &[J]) -> usize {
    msgs.iter().map(j_len).sum()
}

/// Drop the oldest messages until the history fits in half the hard cap (so it
/// does not retrigger on every step). The system prompt stays, and the tail
/// never starts with an orphan tool result.
fn hard_trim(msgs: &mut Vec<J>) -> bool {
    if msgs.len() <= 3 || approx_bytes(msgs) <= MAX_HISTORY_BYTES {
        return false;
    }
    let target = MAX_HISTORY_BYTES / 2;
    let mut start = msgs.len();
    let mut tail = 0usize;
    while start > 1 {
        let add = j_len(&msgs[start - 1]);
        if tail + add > target {
            break;
        }
        tail += add;
        start -= 1;
    }
    while start < msgs.len() {
        let role = msgs[start].get("role").and_then(|r| r.as_str());
        if role == Some("tool") {
            start += 1;
        } else {
            break;
        }
    }
    if start <= 1 {
        return false;
    }
    msgs.drain(1..start);
    true
}

/// Run one turn: `input` is the claimed user message.
pub fn run_turn(
    cfg: &Cfg,
    opts: &RunOpts,
    msgs: &mut Vec<J>,
    reg: &[ToolDef],
    input: &str,
    ui: &mut dyn Ui,
) -> TurnReport {
    ui.on(Ev::TurnStart);
    msgs.push(msg("user", input));

    let defs = tool::definitions(reg);
    let mut steps = 0u32;
    let mut report = TurnReport::default();
    // DSH `dsh-repeat-tool-reminder`: identical consecutive calls are a loop,
    // not progress. Counted per turn, reset by any different call.
    let mut repeat_sig = String::new();
    let mut repeat_count = 0u32;

    loop {
        // `max_steps == 0` is unlimited (the old hard-coded cap of 12 is gone;
        // a looping model is instead bounded by Esc, the goal driver, or the
        // user's own config).
        if opts.max_steps > 0 && steps >= opts.max_steps {
            report.limit_hit = true;
            ui.on(Ev::Info(crate::i18n::tf(
                "ui_step_limit",
                &[&opts.max_steps.to_string()],
            )));
            break;
        }

        if ui.interrupted() {
            report.cancelled = true;
            ui.on(Ev::StepEnd(steps));
            break;
        }

        // Hard ceiling before compaction runs: even with compaction disabled
        // or failing, the process must stay bounded on a 446 MB device.
        if hard_trim(msgs) {
            ui.on(Ev::Info(crate::i18n::t("ui_history_trimmed").to_string()));
        }

        // agent/pre-step: keep the request it is about to send inside the
        // context window. This is token based (see compact.rs), not the old
        // message-count heuristic.
        crate::compact::maybe_compact(cfg, &opts.compact, msgs, opts.window, &defs, ui);

        // Steering: messages typed while busy are injected at this step
        // boundary, exactly like DSH `agent.steer` (next-step inbox).
        while let Some(s) = ui.steer() {
            let s = s.trim().to_string();
            if !s.is_empty() {
                ui.on(Ev::Context(s.clone()));
                msgs.push(msg("user", &s));
            }
        }

        // additionalContexts: ambient state is refreshed per request and is
        // rate-limited like DSH's `dsh-time-context`.
        if opts.context_time {
            crate::context::inject_time(
                msgs,
                opts.context_refresh_secs,
                opts.turn,
                steps + 1,
            );
        }

        steps += 1;
        ui.on(Ev::StepStart(steps));

        // Transient failures retry with backoff; if fallback models are
        // configured they are tried in order (DSH llm-retry style).
        let mut attempt = 0u32;
        let mut active = cfg.clone();
        let out = loop {
            let snapshot = serialize_msgs(msgs);
            match stream_once(&active, &snapshot, &defs, ui) {
                Ok(o) => {
                    crate::usage::record_opt(o.usage.as_ref());
                    break Ok(o);
                }
                Err((partial, was_cancelled)) if !was_cancelled
                    && attempt < opts.retries
                    && is_transient(&partial.finish) =>
                {
                    attempt += 1;
                    if let Some(m) = opts.fallbacks.get(attempt as usize - 1) {
                        active.model = m.clone();
                    }
                    ui.on(Ev::Info(crate::i18n::tf(
                        "ui_retry",
                        &[
                            &attempt.to_string(),
                            &opts.retries.to_string(),
                            &active.model,
                        ],
                    )));
                    std::thread::sleep(std::time::Duration::from_millis(250 * attempt as u64));
                }
                Err((partial, was_cancelled)) => break Err((partial, was_cancelled)),
            }
        };
        let out = match out {
            Ok(o) => o,
            Err((partial, was_cancelled)) => {
                // DSH's `context-overflow` trigger: a provider-confirmed
                // overflow bypasses the pressure threshold and forces one
                // compaction, then retries the same request once.
                if !was_cancelled && is_context_overflow(&partial.finish) && opts.compact.enabled {
                    ui.on(Ev::Info(
                        crate::i18n::t("ui_context_overflow").to_string(),
                    ));
                    let mut forced = opts.compact.clone();
                    forced.threshold_tokens = 0;
                    if crate::compact::compact(cfg, &forced, msgs, opts.window, &defs, ui).is_ok() {
                        steps = steps.saturating_sub(1);
                        continue;
                    }
                }
                let text = partial.text.trim().to_string();
                if !text.is_empty() {
                    ui.on(Ev::Assistant(text.clone()));
                    msgs.push(msg("assistant", &text));
                }
                report.cancelled = was_cancelled;
                report.failed = !was_cancelled;
                if !was_cancelled {
                    ui.on(Ev::Note(crate::i18n::tf(
                        "ui_request_failed",
                        &[&partial.finish],
                    )));
                    if let Some(key) = actionable_hint(&partial.finish) {
                        ui.on(Ev::Hint(crate::i18n::t(key).to_string()));
                    }
                }
                ui.on(Ev::StepEnd(steps));
                break;
            }
        };

        let text = out.text.trim().to_string();
        if !text.is_empty() {
            report.text = text.clone();
            ui.on(Ev::Assistant(text.clone()));
        }

        if out.calls.is_empty() {
            if !text.is_empty() {
                msgs.push(msg("assistant", &text));
            }
            ui.on(Ev::StepEnd(steps));
            break;
        }

        msgs.push(assistant_with_tools(&text, &out.calls));

        let calls: Vec<Call> = out
            .calls
            .iter()
            .map(|c| Call {
                id: c.id.clone(),
                name: c.name.clone(),
                raw_args: c.args.clone(),
            })
            .collect();

        let outcomes = {
            let mut host = UiHost {
                ui: &mut *ui,
                api: cfg.clone(),
                sandbox: opts.sandbox.clone(),
            };
            tool::run_batch_with(reg, &calls, opts.tool_timeout_ms, &mut host)
        };

        let mut contexts: Vec<String> = Vec::new();
        for o in &outcomes {
            msgs.push(J::o(vec![
                ("role", J::s("tool")),
                ("tool_call_id", J::s(&o.call_id)),
                ("content", J::s(&o.content)),
            ]));
            if let Some(c) = &o.context {
                contexts.push(c.clone());
            }
            if o.name == "artifact" && !o.is_error {
                if let Some(v) = &o.value {
                    let title = v
                        .get("title")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let body = v
                        .get("body")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !body.is_empty() {
                        ui.on(Ev::Artifact { title, body });
                    }
                }
            }
        }
        if ui.interrupted() {
            // The user interrupted while the batch ran: the turn stops here.
            // The tool results stay in the history (the transcript is real),
            // and the next queued message is dispatched by the front end.
            report.cancelled = true;
            ui.on(Ev::Info(crate::i18n::t("ui_cancelled").to_string()));
            ui.on(Ev::StepEnd(steps));
            break;
        }

        // Identical-call detection (DSH thresholds 3/5/8; the first two are
        // enough on a 512 MB device).
        if let Some(first) = calls.first() {
            let sig = format!("{}|{}", first.name, first.raw_args);
            if sig == repeat_sig {
                repeat_count += 1;
            } else {
                repeat_sig = sig;
                repeat_count = 1;
            }
            if repeat_count == 3 || repeat_count == 5 {
                contexts.push(repeat_reminder(&first.name, &first.raw_args, repeat_count));
            }
        }

        for c in contexts {
            ui.on(Ev::Context(c.clone()));
            msgs.push(msg("user", &c));
        }

        ui.on(Ev::StepEnd(steps));
    }

    // The ambient time message is not conversation: drop it so a restored
    // session starts with the real transcript.
    crate::context::strip_time(msgs);
    report.steps = steps;
    ui.on(Ev::TurnEnd { steps });
    report
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actionable_hints_cover_common_failures() {
        assert_eq!(actionable_hint("API error: 401 unauthorized"), Some("err_hint_auth"));
        assert_eq!(actionable_hint("429 rate limit"), Some("err_hint_rate"));
        assert_eq!(actionable_hint("curl: connection timed out"), Some("err_hint_network"));
        assert_eq!(actionable_hint("stream closed unexpectedly"), Some("err_hint_network"));
        assert!(actionable_hint("bad request: invalid tool").is_none());
    }
}
