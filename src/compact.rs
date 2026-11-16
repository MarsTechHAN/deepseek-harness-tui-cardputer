//! Context compaction, ported from DSH `dsh-compaction-basic`.
//!
//! DSH's algorithm, reproduced here:
//!
//! 1. **Trigger on request pressure**, at `agent/pre-step`, before the next
//!    request is derived:
//!    ```text
//!    messageBudget = contextWindow - reservedOutput
//!    pressureBudget = messageBudget - headroomTokens
//!    threshold = floor(min(contextWindow * thresholdRatio,
//!                          pressureBudget,
//!                          configured absolute cap))
//!    ```
//!    The default ratios are DSH's: 0.8 of the window, 64K headroom, retain 16%
//!    of the message budget. The absolute cap (default 512K) is the user-facing
//!    knob that makes "compact at 512K" explicit.
//!
//! 2. **Prune first**: oversized tool-result middles in the shadowed region are
//!    replaced with a marker. This is model-free and may already put the
//!    request back under the threshold, in which case no summary is spent.
//!
//! 3. **Select a range**: keep the newest tail worth `retainTokens`, then move
//!    the cut earlier until it sits on a user-message boundary so an
//!    `assistant/tool_calls` message is never separated from its `tool` results.
//!
//! 4. **One-shot summary call**: replay the conversation's own system prompt
//!    and shadowed messages, then append DSH's compaction instruction as the
//!    final user message (that keeps the provider's prefix cache warm; the
//!    instruction is the only novel text).
//!
//! 5. **Frame the replacement** as one `user` message carrying the DSH
//!    checkpoint preamble plus `<compacted-summary>…</compacted-summary>`, and
//!    reject a summary that is not actually smaller than the region it replaces.
//!
//! Summarisation streams through the same UI pump as a normal request, so Esc
//! cancels it and typing stays live.

use crate::api::{self, Cfg};
use crate::config;
use crate::harness::{self, Action, Ev};
use crate::json::J;
use std::sync::mpsc::TryRecvError;

/// DSH defaults (`dsh-compaction-basic/src/config.ts`).
pub const DEFAULT_THRESHOLD_RATIO: f64 = 0.8;
pub const DEFAULT_RETAIN_RATIO: f64 = 0.16;
pub const DEFAULT_HEADROOM_TOKENS: u64 = 65_536;
const DEFAULT_RETRIES: u32 = 1;
/// Model-free pruning of old tool results inside the shadowed region.
const PRUNE_KEEP_HEAD: usize = 4_096;
const PRUNE_KEEP_TAIL: usize = 1_024;
const PRUNE_MIN: usize = 8_192;

/// Settings derived from the persistent config.
#[derive(Clone)]
pub struct Settings {
    pub enabled: bool,
    /// Absolute pressure cap in tokens; 0 = use the window ratio only.
    pub threshold_tokens: u64,
    pub threshold_ratio: f64,
    pub headroom_tokens: u64,
    pub retain_ratio: f64,
    pub model: String,
    pub max_tokens: u32,
}

impl Settings {
    pub fn from_config(c: &config::Config) -> Settings {
        Settings {
            enabled: c.compact_enabled,
            threshold_tokens: c.compact_threshold,
            threshold_ratio: DEFAULT_THRESHOLD_RATIO,
            headroom_tokens: DEFAULT_HEADROOM_TOKENS,
            retain_ratio: DEFAULT_RETAIN_RATIO,
            model: c.compact_model.clone(),
            max_tokens: c.compact_max_tokens.max(256),
        }
    }

    /// `(thresholdTokens, retainTokens)` for one context window and output
    /// reservation, following DSH `resolveCompactSpec`.
    pub fn budgets(&self, window: u64, reserved_output: u64) -> (u64, u64) {
        let mut cap = if self.threshold_tokens > 0 {
            self.threshold_tokens
        } else {
            u64::MAX
        };
        let mut retain;
        if window == 0 {
            // Unknown window: treat the absolute cap as the budget.
            retain = (cap.min(1_000_000) as f64 * self.retain_ratio) as u64;
            return (cap, retain.max(2_000));
        }
        let message_budget = window.saturating_sub(reserved_output);
        let pressure_budget = message_budget.saturating_sub(self.headroom_tokens);
        let ratio_cap = (window as f64 * self.threshold_ratio) as u64;
        cap = cap.min(ratio_cap).min(pressure_budget.max(1));
        retain = (message_budget as f64 * self.retain_ratio) as u64;
        if retain >= cap {
            retain = cap / 2;
        }
        (cap, retain)
    }
}

/// `512000` -> `512K`, for `/config` and notices.
pub fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Heuristic token estimate for one message. DSH's meter charges 4 characters
/// per token plus a small per-block/per-role overhead; CJK is priced at one
/// token per character because DeepSeek's tokenizer is roughly glyph-based for
/// Chinese.
pub fn estimate_msg(m: &J) -> u64 {
    let mut latin = 0u64;
    let mut cjk = 0u64;
    if let Some(c) = m.get("content").and_then(|c| c.as_str()) {
        for ch in c.chars() {
            let u = ch as u32;
            if matches!(u, 0x2E80..=0x9FFF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFF00..=0xFFEF) {
                cjk += 1;
            } else {
                latin += 1;
            }
        }
    }
    if let Some(calls) = m.get("tool_calls").and_then(|c| c.as_arr()) {
        for c in calls {
            latin += c.dump().chars().count() as u64;
        }
    }
    8 + cjk + (latin + 3) / 4
}

pub fn estimate_tokens(msgs: &[J]) -> u64 {
    msgs.iter().map(estimate_msg).sum()
}

/// True when the next request should be compacted first.
pub fn should_compact(set: &Settings, msgs: &[J], window: u64, reserved_output: u64) -> bool {
    if !set.enabled || msgs.len() < 4 {
        return false;
    }
    let (threshold, _) = set.budgets(window, reserved_output);
    let est = estimate_tokens(msgs);
    let seen = crate::usage::last_context_tokens();
    est.max(seen) >= threshold
}

/// Middle-prune oversized tool results in the shadowed region. DSH's
/// `tool-result-pruner` does the same thing with its own character budget;
/// keeping the head and tail preserves the command and its verdict.
fn prune_tool_results(msgs: &mut [J], from: usize, to: usize) -> usize {
    let mut pruned = 0usize;
    for m in msgs.iter_mut().take(to).skip(from) {
        if m.get("role").and_then(|r| r.as_str()) != Some("tool") {
            continue;
        }
        let content = match m.get("content").and_then(|c| c.as_str()) {
            Some(c) => c.to_string(),
            None => continue,
        };
        if content.chars().count() <= PRUNE_MIN {
            continue;
        }
        let head: String = content.chars().take(PRUNE_KEEP_HEAD).collect();
        let tail: String = content
            .chars()
            .rev()
            .take(PRUNE_KEEP_TAIL)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let new = format!("{head}\n\n[... tool result middle pruned ...]\n\n{tail}");
        *m = J::o(vec![
            ("role", J::s("tool")),
            (
                "tool_call_id",
                m.get("tool_call_id").cloned().unwrap_or(J::Null),
            ),
            ("content", J::s(&new)),
        ]);
        pruned += 1;
    }
    pruned
}

/// Render one message the way the summariser replays it. This is only used to
/// recognise a resumed session; the real summarisation call replays messages.
fn _unused_transcript(msgs: &[J]) -> String {
    msgs.iter()
        .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The summarisation instruction, VERBATIM from DSH
/// `dsh-compaction-basic/src/summarizer.ts` (`COMPACTION_INSTRUCTION`).
pub const COMPACTION_INSTRUCTION: &str = "\
You are now acting as a compaction engine for this AI coding assistant. Condense the conversation ABOVE into a structured checkpoint that lets another model resume the work with no loss of essential context.

Output EXACTLY the Markdown structure below: keep every section, in order. Use terse bullets, not prose paragraphs. Write \"(none)\" for an empty section — never drop a section.

## Primary Request and Intent
- [the user's original and evolving goals; quote verbatim where the exact wording matters]

## Key Technical Concepts
- [technologies, frameworks, patterns, and conventions in play]

## Files and Code
- [exact path: why it matters, key changes or snippets]

## Errors and Fixes
- [error: how it was resolved, plus any related user feedback]

## Pending Jobs
- [explicitly requested work not yet completed]

## Current Work
- [precisely what was in progress at this checkpoint]

## Next Step
- [the single next action, directly in line with the most recent request, or \"(none)\"]

## Critical Context
- [decisions and their rationale, constraints, user preferences, open questions, data needed to continue]

Rules:
- Write concise English engineering prose. Preserve exact file paths, commands, error strings, identifiers, numeric values, function signatures, and syntax fragments.
- Capture user feedback and explicit instructions faithfully, especially corrections.
- Do NOT mention this summarization request or that the context was compacted.
- Output only the checkpoint text: do not call any tool or take any other action.
- If the conversation already contains a <compacted-summary> block, it is a PRIOR checkpoint. Do not copy it forward verbatim: preserve still-true facts, drop stale ones, and merge newer information into a single consolidated summary under the same structure.";

/// Framing that makes the replacement message established context (VERBATIM
/// from DSH `CHECKPOINT_PREAMBLE` + summary tags).
pub const CHECKPOINT_PREAMBLE: &str = "This is an automatically generated checkpoint condensing an earlier span of the conversation to free up context. Treat the captured context as established background and build on it without restating it. Continue the task directly from the messages that follow, without acknowledging this checkpoint.";

/// A compaction attempt result, for the UI notice.
pub struct Report {
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub dropped_messages: usize,
    pub pruned: usize,
}

/// `agent/pre-step`: prune and compact when the next request would be too
/// large. Returns the report when a summary was landed.
pub fn maybe_compact(
    cfg: &Cfg,
    set: &Settings,
    msgs: &mut Vec<J>,
    window: u64,
    defs: &J,
    ui: &mut dyn harness::Ui,
) -> Option<Report> {
    if !should_compact(set, msgs, window, cfg.max_tokens as u64) {
        return None;
    }
    let (threshold, retain) = set.budgets(window, cfg.max_tokens as u64);
    // Model-free pass first: pruning may already be enough.
    let mut cut0 = boundary_for_retain(msgs, retain);
    let mut r = retain;
    while cut0 <= 1 && r > 1 {
        r /= 2;
        cut0 = boundary_for_retain(msgs, r);
    }
    let pruned = if cut0 > 1 {
        prune_tool_results(msgs, 1, cut0)
    } else {
        0
    };
    if pruned > 0 && !should_compact(set, msgs, window, cfg.max_tokens as u64) {
        ui.on(Ev::Info(format!(
            "pruned {pruned} old tool results; context is under {}-token budget",
            human_tokens(threshold)
        )));
        return None;
    }
    match compact(cfg, set, msgs, window, defs, ui) {
        Ok(report) => {
            ui.on(Ev::Info(format!(
                "compacted: {} -> {} tokens ({} messages, {} pruned)",
                human_tokens(report.before_tokens),
                human_tokens(report.after_tokens),
                report.dropped_messages,
                report.pruned
            )));
            Some(report)
        }
        Err(e) => {
            ui.on(Ev::CompactingDone {
                ok: false,
                note: e.clone(),
            });
            None
        }
    }
}

/// Find the cut index that leaves at least `retain` tokens of tail, aligned
/// backwards to a user-message boundary.
fn boundary_for_retain(msgs: &[J], retain: u64) -> usize {
    if msgs.len() < 2 {
        return 0;
    }
    // retain=0 means "keep no verbatim tail" (manual `/compact`): the whole
    // non-system history is shadowed.
    if retain == 0 {
        return msgs.len();
    }
    let mut accumulated = 0u64;
    let mut keep_from = msgs.len();
    for i in (0..msgs.len()).rev() {
        accumulated += estimate_msg(&msgs[i]);
        keep_from = i;
        if accumulated >= retain {
            break;
        }
    }
    if keep_from <= 1 {
        return 0;
    }
    // Never split a step: walk to the nearest preceding user message.
    let mut cut = keep_from;
    while cut > 1 {
        if msgs[cut].get("role").and_then(|r| r.as_str()) == Some("user") {
            return cut;
        }
        cut -= 1;
    }
    0
}

/// Summarise the older span of `msgs` into one checkpoint message. On any
/// failure the transcript is left untouched.
pub fn compact(
    cfg: &Cfg,
    set: &Settings,
    msgs: &mut Vec<J>,
    window: u64,
    defs: &J,
    ui: &mut dyn harness::Ui,
) -> Result<Report, String> {
    let before_tokens = estimate_tokens(msgs);
    // Manual compaction of a short session has only [system, user, assistant]
    // to work with; anything less than one user turn is not summarisable.
    if msgs.len() < 3 {
        return Err("nothing to compact".into());
    }
    let (threshold, retain) = set.budgets(window, cfg.max_tokens as u64);
    let mut attempts = 0u32;
    let mut last_pruned = 0usize;
    loop {
        attempts += 1;
        if attempts > 1 + DEFAULT_RETRIES {
            return Err(format!(
                "compaction still above threshold after {attempts} attempts"
            ));
        }
        // The retention budget scales with the context window, not with what
        // the transcript actually holds: a small history exceeding the
        // configured absolute cap can therefore leave no cut point. Halve the
        // tail budget until a boundary exists (DSH's retainTokens is always
        // below the window's pressure threshold, so this only affects the
        // absolute-cap extension).
        let mut retain_local = retain;
        let mut cut = boundary_for_retain(msgs, retain_local);
        let mut halvings = 0;
        while cut <= 1 && retain_local > 1 && halvings < 12 {
            retain_local /= 2;
            cut = boundary_for_retain(msgs, retain_local);
            halvings += 1;
        }
        if cut <= 1 && msgs.len() > 4 {
            // Last resort: keep only the newest user turn.
            cut = msgs.len() - 1;
            while cut > 2 && msgs[cut].get("role").and_then(|r| r.as_str()) != Some("user") {
                cut -= 1;
            }
        }
        if cut <= 1 {
            return Err(format!(
                "no safe compaction range can reduce context below threshold \
({} estimated tokens >= threshold {})",
                estimate_tokens(msgs),
                human_tokens(threshold)
            ));
        }
        let pruned = prune_tool_results(msgs, 1, cut);
        last_pruned += pruned;
        let shadowed: Vec<J> = msgs[1..cut].to_vec();
        let shadowed_tokens = estimate_tokens(&shadowed);
        if shadowed_tokens == 0 {
            return Err("nothing to compact".into());
        }

        ui.on(Ev::Compacting {
            tokens: before_tokens,
            messages: shadowed.len(),
        });

        // Replay the conversation's own head + shadowed messages, then append
        // the instruction as the final user message (DSH's cache-friendly call).
        let mut req: Vec<J> = Vec::with_capacity(shadowed.len() + 2);
        req.push(msgs[0].clone());
        req.extend(shadowed.iter().cloned());
        req.push(harness::msg("user", COMPACTION_INSTRUCTION));
        let messages = J::Arr(req);

        let mut scfg = cfg.clone();
        if !set.model.is_empty() {
            scfg.model = set.model.clone();
        }
        scfg.max_tokens = set.max_tokens;
        let summary = stream_summary(&scfg, &messages, defs, ui)?;
        let summary = summary.trim().to_string();
        if summary.is_empty() {
            return Err("summarisation produced no text summary content".into());
        }

        let framed = format!(
            "{CHECKPOINT_PREAMBLE}\n\n<compacted-summary>\n{summary}\n</compacted-summary>"
        );
        let replacement = harness::msg("user", &framed);
        let after_tokens = estimate_tokens(std::slice::from_ref(&replacement));
        if after_tokens >= shadowed_tokens {
            return Err("summary is not smaller than the shadowed region".into());
        }

        let dropped = shadowed.len();
        let mut rebuilt: Vec<J> = Vec::with_capacity(msgs.len() - dropped + 1);
        rebuilt.push(msgs[0].clone());
        rebuilt.push(replacement);
        rebuilt.extend_from_slice(&msgs[cut..]);
        *msgs = rebuilt;

        let now_tokens = estimate_tokens(msgs);
        let report = Report {
            before_tokens,
            after_tokens: now_tokens,
            dropped_messages: dropped,
            pruned: last_pruned,
        };
        if now_tokens < threshold {
            return Ok(report);
        }
        // If the only thing left to summarise is an existing checkpoint, a
        // second summary cannot shrink it (the preamble alone is most of its
        // size): accept the partial reduction instead of spending another
        // model call.
        let next_cut = boundary_for_retain(msgs, retain);
        if next_cut <= 1
            || msgs[1..next_cut].iter().all(|m| {
                m.get("content")
                    .and_then(|c| c.as_str())
                    .map(|t| t.starts_with(CHECKPOINT_PREAMBLE))
                    .unwrap_or(false)
            })
        {
            return Ok(report);
        }
        // DSH retries while still above threshold. On a device the system
        // prompt alone can exceed a small absolute cap, so a partial reduction
        // is still a win: retry once, then return the successful (smaller)
        // history instead of discarding it as a failure.
        if attempts > DEFAULT_RETRIES {
            return Ok(report);
        }
    }
}

/// Run the summarisation request on the UI thread, pumping the keyboard so Esc
/// can cancel it and typing stays live.
fn stream_summary(
    cfg: &Cfg,
    messages: &J,
    defs: &J,
    ui: &mut dyn harness::Ui,
) -> Result<String, String> {
    let chat = api::spawn_chat(cfg, messages, defs)?;
    let mut text = String::new();
    loop {
        let mut settled: Option<Result<(), String>> = None;
        loop {
            match chat.rx.try_recv() {
                Ok(api::ApiEv::Content(c)) => text.push_str(&c),
                Ok(api::ApiEv::Reason(_)) => {}
                Ok(api::ApiEv::Done(_)) => {
                    settled = Some(Ok(()));
                    break;
                }
                Ok(api::ApiEv::Failed(e)) => {
                    settled = Some(Err(e));
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    settled = Some(Err("stream closed unexpectedly".into()));
                    break;
                }
            }
        }
        if let Some(r) = settled {
            return r.map(|_| text);
        }
        if ui.pump() == Action::Cancel {
            api::kill(chat.pid);
            let deadline = std::time::Instant::now();
            while deadline.elapsed().as_millis() < 2000 {
                if let Ok(api::ApiEv::Done(_)) | Ok(api::ApiEv::Failed(_)) =
                    chat.rx.recv_timeout(std::time::Duration::from_millis(50))
                {
                    break;
                }
            }
            return Err("compaction cancelled".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> J {
        J::o(vec![("role", J::s(role)), ("content", J::s(text))])
    }

    fn set() -> Settings {
        Settings {
            enabled: true,
            threshold_tokens: 512_000,
            threshold_ratio: DEFAULT_THRESHOLD_RATIO,
            headroom_tokens: DEFAULT_HEADROOM_TOKENS,
            retain_ratio: DEFAULT_RETAIN_RATIO,
            model: String::new(),
            max_tokens: 8_192,
        }
    }

    #[test]
    fn estimate_counts_cjk_and_latin() {
        let est = estimate_tokens(&[msg("user", "hello world"), msg("user", "你好世界")]);
        assert!(est >= 20 && est < 40, "unexpected estimate: {est}");
    }

    #[test]
    fn budgets_follow_dsh_formula() {
        // window 1M, output 64K, headroom 64K: ratio 0.8*1M = 800K,
        // pressure = 1M-64K-64K = 896K, absolute cap 512K wins.
        let (threshold, _) = set().budgets(1_048_576, 65_536);
        assert_eq!(threshold, 512_000);
        // window 128K: pressure budget 128K-4K-64K = 60K.
        let (small, retain) = set().budgets(131_072, 4_096);
        assert_eq!(small, 61_440);
        assert!(retain < small);
    }

    #[test]
    fn retain_boundary_lands_on_user_message() {
        let mut msgs = vec![msg("system", "sys")];
        msgs.push(msg("user", "a"));
        msgs.push(msg("assistant", "b"));
        msgs.push(msg("user", "c"));
        msgs.push(msg("assistant", "d"));
        msgs.push(msg("tool", "e"));
        let cut = boundary_for_retain(&msgs, 1);
        assert_eq!(msgs[cut].get("role").unwrap().as_str(), Some("user"));
    }

    #[test]
    fn human_tokens_formats() {
        assert_eq!(human_tokens(512_000), "512K");
        assert_eq!(human_tokens(900), "900");
    }
}
