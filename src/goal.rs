//! Same-session goals, ported from DSH `dsh-goal`, `dsh-command-goal`,
//! `dsh-tool-goal` and `dsh-goal-round-driver`.
//!
//! A goal is a persisted objective that keeps the session working across
//! automatic continuation rounds. The model judges progress itself through the
//! `get_goal` / `create_goal` / `update_goal` tools: it marks the goal
//! `complete` only when the objective is actually achieved, and `blocked` only
//! when one concrete condition has persisted for several rounds. The round
//! driver then injects DSH's `<goal_round>` prompt until the goal leaves the
//! active phase, a round cap is reached, or the user stops it.
//!
//! Deliberate simplifications for a 512 MB handheld:
//! - one goal per `~/.dst` home (not one per session),
//! - activation is process-local (as in DSH), so a restarted session with an
//!   active goal waits for the user to re-arm it with `/goal resume`,
//! - authority checks are narrower: the model may not resume a paused goal and
//!   must not report `blocked` before the configured minimum round count.

use crate::json::{self, J};
use crate::tool::{self, Fail, Param, ToolDef};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// True while a turn is an automatic goal round (model has goal-round
/// authority, not a direct human turn).
static IN_ROUND: AtomicBool = AtomicBool::new(false);

pub fn set_in_round(v: bool) {
    IN_ROUND.store(v, Ordering::SeqCst);
}

pub fn in_round() -> bool {
    IN_ROUND.load(Ordering::SeqCst)
}

/// Minimum admitted rounds before the model may self-report `blocked`
/// (DSH `blockedAfterConsecutiveRounds` default).
pub const BLOCKED_AFTER_ROUNDS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Active,
    Paused,
    Blocked,
    Complete,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Active => "active",
            Phase::Paused => "paused",
            Phase::Blocked => "blocked",
            Phase::Complete => "complete",
        }
    }
    fn parse(s: &str) -> Option<Phase> {
        match s {
            "active" => Some(Phase::Active),
            "paused" => Some(Phase::Paused),
            "blocked" => Some(Phase::Blocked),
            "complete" => Some(Phase::Complete),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Goal {
    pub id: String,
    pub revision: u32,
    pub objective: String,
    pub phase: Phase,
    pub rounds_started: u32,
    /// 0 = unlimited (dst extension; DSH requires a positive cap).
    pub max_rounds: u32,
    pub blocked_reason: Option<String>,
    /// Process-local continuation activation. Not persisted: after a restart an
    /// active goal is disarmed until the user resumes it (DSH behaviour).
    pub armed: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Goal {
    pub fn is_active(&self) -> bool {
        self.phase == Phase::Active && self.armed
    }

}

fn state() -> &'static Mutex<Option<Goal>> {
    static STATE: OnceLock<Mutex<Option<Goal>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn path() -> std::path::PathBuf {
    crate::config::home().join("goal.json")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn gen_id() -> String {
    // No rand crate: time + pid is unique enough for a single-user device.
    format!("g{:x}{:x}", now(), std::process::id())
}

/// Load the persisted goal into memory once per process.
pub fn load() -> Option<Goal> {
    let mut g = state().lock().ok()?;
    if g.is_none() {
        if let Ok(text) = std::fs::read_to_string(path()) {
            if let Some(v) = json::parse(&text) {
                let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let n = |k: &str| v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0) as u32;
                let phase = Phase::parse(&s("phase")).unwrap_or(Phase::Active);
                let objective = s("objective");
                if !objective.is_empty() {
                    *g = Some(Goal {
                        id: s("id"),
                        revision: n("revision").max(1),
                        objective,
                        phase,
                        rounds_started: n("rounds_started"),
                        max_rounds: n("max_rounds"),
                        blocked_reason: v
                            .get("blocked_reason")
                            .and_then(|x| x.as_str())
                            .map(|x| x.to_string()),
                        armed: false,
                        created_at: n("created_at") as u64,
                        updated_at: n("updated_at") as u64,
                    });
                }
            }
        }
    }
    g.clone()
}

/// Current goal with no disk read.
pub fn current() -> Option<Goal> {
    state().lock().ok().and_then(|g| g.clone())
}

fn save(g: &Option<Goal>) -> Result<(), String> {
    match g {
        None => {
            let _ = std::fs::remove_file(path());
        }
        Some(goal) => {
            let doc = J::o(vec![
                ("id", J::s(&goal.id)),
                ("revision", J::Num(goal.revision as f64)),
                ("objective", J::s(&goal.objective)),
                ("phase", J::s(goal.phase.as_str())),
                ("rounds_started", J::Num(goal.rounds_started as f64)),
                ("max_rounds", J::Num(goal.max_rounds as f64)),
                (
                    "blocked_reason",
                    match &goal.blocked_reason {
                        Some(r) => J::s(r),
                        None => J::Null,
                    },
                ),
                ("created_at", J::Num(goal.created_at as f64)),
                ("updated_at", J::Num(goal.updated_at as f64)),
            ]);
            std::fs::write(path(), doc.dump()).map_err(|e| format!("{}: {e}", path().display()))?;
        }
    }
    Ok(())
}

/// Replace the in-memory goal and persist it. `revision` is bumped by every
/// state-changing operation, so a model working from a stale value is rejected
/// (DSH compare-and-set).
fn commit(mut g: Option<Goal>, bump: bool) -> Result<Option<Goal>, String> {
    if let Some(goal) = g.as_mut() {
        if bump {
            goal.revision = goal.revision.saturating_add(1);
        }
        goal.updated_at = now();
    }
    save(&g)?;
    let mut guard = state().lock().map_err(|_| "goal state poisoned".to_string())?;
    *guard = g.clone();
    Ok(g)
}

fn default_max_rounds() -> u32 {
    crate::config::Config::load().goal_max_rounds
}

/// Create a goal. Rejects while another goal is not complete (DSH rule).
pub fn create(objective: &str, max_rounds: Option<u32>) -> Result<Goal, String> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Err("goal objective must be a non-empty string".into());
    }
    let existing = load();
    if let Some(g) = &existing {
        if g.phase != Phase::Complete {
            return Err(format!(
                "a goal is already {}. Use /goal edit <objective> or /goal clear first.",
                g.phase.as_str()
            ));
        }
    }
    let max = max_rounds.unwrap_or_else(default_max_rounds);
    let goal = Goal {
        id: gen_id(),
        revision: 1,
        objective: objective.to_string(),
        phase: Phase::Active,
        rounds_started: 0,
        max_rounds: max,
        blocked_reason: None,
        armed: true,
        created_at: now(),
        updated_at: now(),
    };
    commit(Some(goal.clone()), false)?;
    Ok(goal)
}

/// Update the persisted objective or round cap (compare-and-set).
pub fn edit(id: &str, revision: u32, objective: Option<&str>, max_rounds: Option<u32>) -> Result<Goal, String> {
    let g = load().ok_or("no goal is currently set")?;
    if g.id != id || g.revision != revision {
        return Err(format!(
            "goal state changed: expected id={} revision={}, current id={} revision={}",
            id, revision, g.id, g.revision
        ));
    }
    if objective.is_none() && max_rounds.is_none() {
        return Err("goal edit requires objective and/or max_goal_rounds".into());
    }
    let mut n = g;
    if let Some(o) = objective {
        let o = o.trim();
        if o.is_empty() {
            return Err("goal objective must be a non-empty string".into());
        }
        n.objective = o.to_string();
    }
    if let Some(m) = max_rounds {
        n.max_rounds = m;
    }
    commit(Some(n.clone()), true)?;
    Ok(n)
}

pub fn pause(id: &str, revision: u32) -> Result<Goal, String> {
    transition(id, revision, |g| {
        if g.phase != Phase::Active {
            return Err(format!("goal is {}, not active", g.phase.as_str()));
        }
        g.phase = Phase::Paused;
        g.armed = false;
        Ok(())
    })
}

/// A human may resume a paused or blocked goal; the model may not.
pub fn resume(id: &str, revision: u32) -> Result<Goal, String> {
    transition(id, revision, |g| {
        match g.phase {
            Phase::Paused | Phase::Blocked => {}
            Phase::Complete => return Err("the goal is complete; create a new one".into()),
            Phase::Active => {}
        }
        if g.max_rounds > 0 && g.rounds_started >= g.max_rounds {
            return Err(format!(
                "goal exhausted {} rounds; increase max_rounds before resuming",
                g.max_rounds
            ));
        }
        g.phase = Phase::Active;
        g.blocked_reason = None;
        g.armed = true;
        Ok(())
    })
}

pub fn complete(id: &str, revision: u32) -> Result<Goal, String> {
    transition(id, revision, |g| {
        if g.phase == Phase::Complete {
            return Err("the goal is already complete".into());
        }
        g.phase = Phase::Complete;
        g.armed = false;
        Ok(())
    })
}

pub fn block(id: &str, revision: u32, reason: &str) -> Result<Goal, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("blocked_reason is required with action blocked".into());
    }
    transition(id, revision, |g| {
        g.phase = Phase::Blocked;
        g.blocked_reason = Some(reason.to_string());
        g.armed = false;
        Ok(())
    })
}

pub fn clear() -> Result<(), String> {
    commit(None, false).map(|_| ())
}

fn transition(
    id: &str,
    revision: u32,
    f: impl FnOnce(&mut Goal) -> Result<(), String>,
) -> Result<Goal, String> {
    let mut g = load().ok_or("no goal is currently set")?;
    if g.id != id || g.revision != revision {
        return Err(format!(
            "goal state changed: expected id={} revision={}, current id={} revision={}",
            id, revision, g.id, g.revision
        ));
    }
    f(&mut g)?;
    commit(Some(g.clone()), true)?;
    Ok(g)
}

/// Arm/disarm continuation while a goal is active (process-local).
pub fn set_armed(armed: bool) -> Option<Goal> {
    let mut g = load();
    if let Some(goal) = g.as_mut() {
        if goal.phase == Phase::Active {
            goal.armed = armed;
            let mut guard = state().lock().ok()?;
            *guard = g.clone();
            return g;
        }
    }
    g
}

/// Start the next automatic round: bump the round counter and persist it.
/// Returns the goal as it was *before* the bump plus the round number, or
/// `None` when continuation is no longer admissible.
pub fn start_round() -> Option<(Goal, u32)> {
    let mut g = load()?;
    if !g.is_active() {
        return None;
    }
    if g.max_rounds > 0 && g.rounds_started >= g.max_rounds {
        g.armed = false;
        if let Ok(mut guard) = state().lock() {
            *guard = Some(g);
        }
        return None;
    }
    let before = g.clone();
    let round = g.rounds_started + 1;
    g.rounds_started = round;
    let _ = commit(Some(g), true);
    Some((before, round))
}

/// DSH `renderGoalRoundPrompt`: the model-visible continuation instruction.
pub fn round_prompt(goal: &Goal, round: u32) -> String {
    let max = if goal.max_rounds == 0 {
        "unlimited".to_string()
    } else {
        goal.max_rounds.to_string()
    };
    format!(
        "<goal_round>\nObjective: {}\nRound: {round}/{max}\n\n\
Continue working toward the objective in this same session. Treat the current \
workspace, tool results, and durable session state as authoritative; inspect \
them instead of assuming earlier narration is still current. Make concrete \
progress and verify the result. Before claiming completion, gather evidence \
that the whole objective is achieved, read the current goal, and mark it \
complete. If work remains, leave the goal active for the next round. Follow \
the configured goal-tool policy before reporting a blocker.\n</goal_round>",
        J::s(&goal.objective).dump()
    )
}

/// DSH `renderWrapupContext`: after an autonomous round reports a terminal
/// state, the model should address the user once before the turn ends.
pub fn wrapup_context(goal: &Goal) -> String {
    let objective = J::s(&goal.objective).dump();
    match goal.phase {
        Phase::Complete => format!(
            "<goal_complete>\nObjective: {objective}\nThe goal is marked complete and this \
autonomous run is ending. Write the closing message to the user now: state the \
outcome, summarize what was done and how it was verified, and point to the \
concrete results (files, commits, or other artifacts). Report only what earlier \
rounds and tool results in this session actually establish; when a detail is \
not in the session, say so instead of inventing it. Note anything the user \
should review or do next. Address the user directly. Do not call any more tools \
in this run; further work waits for the user's next instruction.\n</goal_complete>"
        ),
        Phase::Blocked => format!(
            "<goal_blocked>\nObjective: {objective}\nBlocked: {}\nThe goal is marked blocked \
and this autonomous run is ending. Write the closing message to the user now: \
state what has been completed so far, describe the concrete blocking condition \
and what you tried, and say exactly what you need from the user to continue. \
Report only what earlier rounds and tool results in this session actually \
establish; when a detail is not in the session, say so instead of inventing it. \
Address the user directly. Do not call any more tools in this run; further work \
waits for the user's next instruction.\n</goal_blocked>",
            J::s(goal.blocked_reason.as_deref().unwrap_or("")).dump()
        ),
        _ => String::new(),
    }
}

/// Human-facing status lines for `/goal` (DSH `renderGoal`).
pub fn status_lines() -> Vec<String> {
    match load() {
        None => vec!["No goal is currently set.".into()],
        Some(g) => {
            let mut out = vec![
                "Goal".to_string(),
                format!("Status: {}", g.phase.as_str()),
            ];
            if let Some(r) = &g.blocked_reason {
                out.push(format!("Blocker: {r}"));
            }
            out.push(format!("Objective: {}", g.objective));
            let max = if g.max_rounds == 0 {
                "unlimited".to_string()
            } else {
                g.max_rounds.to_string()
            };
            out.push(format!("Rounds: {}/{}", g.rounds_started, max));
            out.push(format!(
                "Activation: {}",
                if g.armed { "armed" } else { "disarmed" }
            ));
            out.push(String::new());
            out.push(format!(
                "Commands: {}",
                match (g.phase, g.armed) {
                    (Phase::Active, true) => "/goal edit <objective>, /goal pause, /goal clear",
                    (Phase::Active, false) => "/goal edit <objective>, /goal resume, /goal clear",
                    (Phase::Complete, _) => "/goal <objective>, /goal clear",
                    _ => "/goal edit <objective>, /goal resume, /goal clear",
                }
            ));
            out
        }
    }
}

/// Bottom-right status badge: pursuing / paused / blocked / done, with the
/// round budget. This is the persistent goal-status place in the UI.
pub fn badge() -> Option<String> {
    let g = current()?;
    let budget = if g.max_rounds == 0 {
        format!("{}/\u{221e}", g.rounds_started)
    } else {
        format!("{}/{}", g.rounds_started, g.max_rounds)
    };
    Some(match g.phase {
        Phase::Active if g.armed => format!("\u{25ce} Pursuing Goal {budget}"),
        Phase::Active | Phase::Paused => format!("\u{23f8} Goal paused {budget}"),
        Phase::Blocked => "\u{2298} Goal blocked".to_string(),
        Phase::Complete => "\u{2713} Goal done".to_string(),
    })
}

/// One-line banner for the status area while a goal is active.
pub fn banner() -> Option<String> {
    let g = current()?;
    if g.phase == Phase::Complete {
        return None;
    }
    let max = if g.max_rounds == 0 {
        "\u{221e}".to_string()
    } else {
        g.max_rounds.to_string()
    };
    Some(format!(
        "\u{25ce} goal {}/{} {}",
        g.rounds_started,
        max,
        short(&g.objective, 26)
    ))
}

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('\u{2026}');
    }
    out.replace('\n', " ")
}

// ---------------------------------------------------------------------------
// model-facing tools
// ---------------------------------------------------------------------------

fn goal_value(g: Option<&Goal>) -> J {
    let goal = match g {
        None => J::Null,
        Some(g) => J::o(vec![
            ("id", J::s(&g.id)),
            ("revision", J::Num(g.revision as f64)),
            ("objective", J::s(&g.objective)),
            ("phase", J::s(g.phase.as_str())),
            ("rounds_started", J::Num(g.rounds_started as f64)),
            ("max_goal_rounds", J::Num(g.max_rounds as f64)),
            (
                "blocked_reason",
                match &g.blocked_reason {
                    Some(r) => J::s(r),
                    None => J::Null,
                },
            ),
        ]),
    };
    let activation = if g.map(|g| g.armed).unwrap_or(false) {
        "armed"
    } else {
        "disarmed"
    };
    J::o(vec![("goal", goal), ("activation", J::s(activation))])
}

fn goal_render(_args: &J, v: &J) -> String {
    v.dump()
}

fn get_exec(_args: &J) -> Result<J, Fail> {
    Ok(goal_value(load().as_ref()))
}

fn create_exec(args: &J) -> Result<J, Fail> {
    let objective = tool::str_arg(args, "objective").unwrap_or_default();
    let max = tool::num_arg(args, "max_goal_rounds").map(|n| n.max(0.0) as u32);
    let g = create(&objective, max).map_err(Fail::new)?;
    Ok(goal_value(Some(&g)))
}

fn update_exec(args: &J) -> Result<J, Fail> {
    let fail = |m: String| Fail::new(m);
    let id = tool::str_arg(args, "goal_id").unwrap_or_default();
    let revision = tool::num_arg(args, "revision").unwrap_or(0.0) as u32;
    if id.is_empty() || revision == 0 {
        return Err(Fail::new(
            "goal_id must be non-empty and revision must be a positive integer",
        ));
    }
    let action = tool::str_arg(args, "action").unwrap_or_default();
    let objective = tool::str_arg(args, "objective");
    let max = tool::num_arg(args, "max_goal_rounds").map(|n| n.max(0.0) as u32);
    let reason = tool::str_arg(args, "blocked_reason").unwrap_or_default();
    let g = match action.as_str() {
        "edit" => edit(&id, revision, objective.as_deref(), max),
        "pause" => {
            if in_round() {
                Err("pause requires a direct human turn; the user must pause it".into())
            } else {
                pause(&id, revision)
            }
        }
        "resume" => {
            // A human turn may re-arm an active goal or resume a paused/blocked
            // one; an automatic goal round may not.
            if in_round() {
                Err("resume requires a direct human turn; the user must resume it".into())
            } else {
                resume(&id, revision)
            }
        }
        "complete" => complete(&id, revision),
        "blocked" => {
            let cur = match load() {
                Some(c) => c,
                None => return Err(fail("no goal is currently set".into())),
            };
            if in_round() && cur.rounds_started < BLOCKED_AFTER_ROUNDS {
                Err(format!(
                    "blocked requires at least {BLOCKED_AFTER_ROUNDS} consecutive goal rounds; \
current round is {}",
                    cur.rounds_started
                ))
            } else {
                block(&id, revision, &reason)
            }
        }
        other => Err(format!(
            "unknown action '{other}'; expected edit, pause, resume, complete or blocked"
        )),
    }
    .map_err(fail)?;
    Ok(goal_value(Some(&g)))
}

/// post-execute: a terminal autonomous update tells the model to write a
/// closing message for the user (DSH `deferContext`).
fn update_post(_args: &J, value: &J) -> Option<String> {
    let phase = value
        .get("goal")
        .and_then(|g| g.get("phase"))
        .and_then(|p| p.as_str())?;
    if phase != "complete" && phase != "blocked" {
        return None;
    }
    load().map(|g| wrapup_context(&g))
}

const CREATE_DESCRIPTION: &str = "Create a persisted goal that keeps this session working across automatic continuation rounds. Use it when the direct human request is a long-running objective, even if the user did not say \"goal\"; not for single-turn work.";

const GUIDANCE: &str = "create_goal may infer goal intent from a direct human request in any language. After session resume or fork, an active goal is disarmed: when a human asks to continue or resume in any wording or language, use update_goal action resume to rearm it. Mark complete only when the objective is actually achieved. Mark blocked only after the same blocking condition persists for at least 3 consecutive goal rounds, and report that concrete condition in blocked_reason; difficulty, uncertainty, or useful remaining work is not blocked.";

/// System-prompt section describing the goal tools.
pub fn guidance() -> &'static str {
    GUIDANCE
}

/// The three goal tools, appended to the registry at startup.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "get_goal",
            description: "Read the current session goal, including the id and revision that update_goal requires.",
            params: vec![],
            concurrency_safe: false,
            timeout_ms: 5_000,
            execute: get_exec,
            render: goal_render,
            present: |_a| "read current goal".into(),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "create_goal",
            description: CREATE_DESCRIPTION,
            params: vec![
                Param::req(
                    "objective",
                    "string",
                    "The concrete completion objective inferred from the direct human request.",
                ),
                Param::opt(
                    "max_goal_rounds",
                    "integer",
                    "Optional limit on automatic continuation rounds. 0 means unlimited.",
                ),
            ],
            concurrency_safe: false,
            timeout_ms: 5_000,
            execute: create_exec,
            render: goal_render,
            present: |a| format!("create goal: {}", short(&tool::str_arg(a, "objective").unwrap_or_default(), 30)),
            post: None,
            schema_override: None,
            dynamic: None,
        },
        ToolDef {
            name: "update_goal",
            description: "Update the current goal.",
            params: vec![
                Param::req("goal_id", "string", "Exact id returned by get_goal."),
                Param::req("revision", "integer", "Exact positive revision returned by get_goal."),
                Param::one_of(
                    "action",
                    "edit, pause, and resume require a direct top-level human request. complete and blocked are also allowed during an automatic continuation of this goal; blocked is rejected before the configured minimum round count.",
                    &["edit", "pause", "resume", "complete", "blocked"],
                ),
                Param::opt("objective", "string", "Replacement objective; valid only with action edit."),
                Param::opt("max_goal_rounds", "integer", "Replacement cap; valid only with action edit. 0 means unlimited."),
                Param::opt(
                    "blocked_reason",
                    "string",
                    "Required only with action blocked: the concrete condition that persisted across rounds and blocks progress.",
                ),
            ],
            concurrency_safe: false,
            timeout_ms: 5_000,
            execute: update_exec,
            render: goal_render,
            present: |a| format!("update goal: {}", tool::str_arg(a, "action").unwrap_or_default()),
            post: Some(update_post),
            schema_override: None,
            dynamic: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests touch ~/.dst through config::home(); keep them in-memory only.
    #[test]
    fn phase_roundtrip() {
        for p in [Phase::Active, Phase::Paused, Phase::Blocked, Phase::Complete] {
            assert_eq!(Phase::parse(p.as_str()), Some(p));
        }
    }

    #[test]
    fn round_prompt_quotes_objective() {
        let g = Goal {
            id: "g1".into(),
            revision: 1,
            objective: "fix the display".into(),
            phase: Phase::Active,
            rounds_started: 0,
            max_rounds: 0,
            blocked_reason: None,
            armed: true,
            created_at: 0,
            updated_at: 0,
        };
        let p = round_prompt(&g, 2);
        assert!(p.starts_with("<goal_round>"));
        assert!(p.contains("fix the display"));
        assert!(p.contains("Round: 2/unlimited"));
    }

    #[test]
    fn wrapup_mentions_outcome() {
        let g = Goal {
            id: "g1".into(),
            revision: 2,
            objective: "ship it".into(),
            phase: Phase::Complete,
            rounds_started: 3,
            max_rounds: 0,
            blocked_reason: None,
            armed: false,
            created_at: 0,
            updated_at: 0,
        };
        assert!(wrapup_context(&g).contains("<goal_complete>"));
    }
}
