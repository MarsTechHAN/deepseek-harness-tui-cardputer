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

/// Bound on steps per turn, so a looping model cannot wedge the device.
const MAX_STEPS: u32 = 12;

/// Bound on retained history. `SOFT` triggers compaction; `KEEP` is how many
/// trailing messages survive it.
const HISTORY_SOFT: usize = 45;
const HISTORY_KEEP: usize = 28;

/// What the UI wants done with the running turn.
#[derive(PartialEq, Eq)]
pub enum Action {
    Continue,
    Cancel,
}

pub enum Ev {
    TurnStart,
    StepStart(u32),
    /// Transient model delta (`agent/assistant-stream` chunk).
    Delta(String),
    /// Live thinking progress. `chars` is how much has arrived so far.
    Thinking { secs: f32, chars: usize },
    /// A piece of the reasoning text, shown live and folded when content starts.
    ThinkingDelta(String),
    /// Thinking finished; the UI should collapse it to one line.
    ThinkingDone { secs: f32, chars: usize },
    /// Settled assistant text (`assistant/message`).
    Assistant(String),
    /// `tool/call`, logged before execution. `title` is UI presentation only.
    ToolCall {
        id: String,
        name: String,
        title: String,
    },
    /// `tool/result`, settled in model order.
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    /// `additionalContexts` injected as a user message after the batch.
    Context(String),
    /// The agent asked to take over the screen with a composed panel.
    Artifact { title: String, body: String },
    /// Harness-level notice (limit reached, failure, compaction).
    Note(String),
    StepEnd(u32),
    TurnEnd { steps: u32 },
}

/// Everything the driver needs from the front end.
pub trait Ui {
    fn on(&mut self, ev: Ev);
    /// Called continuously while the request is in flight. Implementations
    /// must not block for long: the keyboard is read here.
    fn pump(&mut self) -> Action;
}

/// Bridges the tool executor's host callbacks back onto the UI, so a running
/// tool call can still deliver Esc.
struct UiHost<'a> {
    ui: &'a mut dyn Ui,
}

impl<'a> tool::ToolHost for UiHost<'a> {
    fn call(&mut self, ev: PipeEvent) {
        match ev {
            PipeEvent::Call { id, name, title } => self.ui.on(Ev::ToolCall { id, name, title }),
            PipeEvent::Made(o) => self.ui.on(Ev::ToolResult {
                id: o.call_id,
                name: o.name,
                content: o.content,
                is_error: o.is_error,
            }),
        }
    }
    fn cancel(&mut self) -> bool {
        self.ui.pump() == Action::Cancel
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

/// `agent/pre-step`: bound the retained history before the request is derived.
/// Cuts only at a user-message boundary so a `tool_calls` message is never
/// separated from its `tool` results.
fn pre_step(msgs: &mut Vec<J>, ui: &mut dyn Ui) {
    if msgs.len() <= HISTORY_SOFT {
        return;
    }
    let tail_start = msgs.len().saturating_sub(HISTORY_KEEP);
    let mut cut = tail_start;
    while cut < msgs.len() {
        let is_user = msgs[cut]
            .get("role")
            .and_then(|r| r.as_str())
            .map(|r| r == "user")
            .unwrap_or(false);
        if is_user {
            break;
        }
        cut += 1;
    }
    if cut <= 1 || cut >= msgs.len() {
        return;
    }
    let dropped = cut - 1;
    msgs.drain(1..cut);
    ui.on(Ev::Note(format!(
        "compacted: dropped {dropped} earlier messages ({} kept)",
        msgs.len()
    )));
}

/// One streamed request. Drains the API channel while pumping the UI, so the
/// user can keep typing and can cancel with Esc.
fn stream_once(
    cfg: &Cfg,
    snapshot: &J,
    defs: &J,
    ui: &mut dyn Ui,
) -> Result<StreamOut, (StreamOut, bool)> {
    let chat = match api::spawn_chat(cfg, snapshot, defs) {
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
            ui.on(Ev::Note("cancelled".into()));
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

/// Run one turn: `input` is the claimed user message.
pub fn run_turn(
    cfg: &Cfg,
    msgs: &mut Vec<J>,
    reg: &[ToolDef],
    input: &str,
    ui: &mut dyn Ui,
) {
    ui.on(Ev::TurnStart);
    msgs.push(msg("user", input));

    let defs = tool::definitions(reg);
    let mut steps = 0u32;

    loop {
        if steps >= MAX_STEPS {
            ui.on(Ev::Note(format!(
                "step limit reached ({MAX_STEPS}); stopping this turn"
            )));
            break;
        }

        pre_step(msgs, ui);

        steps += 1;
        ui.on(Ev::StepStart(steps));

        let snapshot = J::Arr(msgs.clone());
        let out = match stream_once(cfg, &snapshot, &defs, ui) {
            Ok(o) => {
                crate::usage::record_opt(o.usage.as_ref());
                o
            }
            Err((partial, was_cancelled)) => {
                let text = partial.text.trim().to_string();
                if !text.is_empty() {
                    ui.on(Ev::Assistant(text.clone()));
                    msgs.push(msg("assistant", &text));
                }
                if !was_cancelled {
                    ui.on(Ev::Note(format!("request failed: {}", partial.finish)));
                }
                ui.on(Ev::StepEnd(steps));
                break;
            }
        };

        let text = out.text.trim().to_string();
        if !text.is_empty() {
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
            let mut host = UiHost { ui: &mut *ui };
            tool::run_batch(reg, &calls, &mut host)
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
        for c in contexts {
            ui.on(Ev::Context(c.clone()));
            msgs.push(msg("user", &c));
        }

        ui.on(Ev::StepEnd(steps));
    }

    ui.on(Ev::TurnEnd { steps });
}

/// A UI that ignores everything: used by background jobs and `--once`-style
/// callers that have nothing to draw.
struct NullUi;
impl Ui for NullUi {
    fn on(&mut self, _ev: Ev) {}
    fn pump(&mut self) -> Action {
        Action::Continue
    }
}

pub fn run_prompt(cfg: &Cfg, msgs: &mut Vec<J>, reg: &[ToolDef], input: &str) {
    let mut ui = NullUi;
    run_turn(cfg, msgs, reg, input, &mut ui);
}
