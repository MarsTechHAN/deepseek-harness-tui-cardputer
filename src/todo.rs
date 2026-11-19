//! Agent-maintained todo list (DSH `dsh-tool-todo` contract).
//!
//! Whole-list replacement: every `todo_write` carries the COMPLETE plan. Items
//! are `{content, status}` with `pending` / `in_progress` / `completed`, exactly
//! like DeepSeek Harness, so a model trained on DSH's tool uses the same shape.
//! The list belongs to the current turn (DSH clears the projection at
//! `turn/start`) and drives the `t3/7` status badge.
//!
//! Legacy shorthand (`items: ["[x] done", "[ ] next"]`, `{text, done}`) is still
//! accepted so old prompts/scripts keep working.

use crate::json::J;
use crate::tool::{Fail, Param, ToolDef};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::InProgress => "in_progress",
            Status::Completed => "completed",
        }
    }

    fn from_str(s: &str) -> Option<Status> {
        match s {
            "pending" => Some(Status::Pending),
            "in_progress" => Some(Status::InProgress),
            "completed" => Some(Status::Completed),
            _ => None,
        }
    }
}

#[derive(Clone)]
struct Item {
    content: String,
    status: Status,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

/// Parse one item from the DSH object shape, or the legacy string/`{text,done}`
/// shape. Returns `None` for an unusable item instead of failing the whole call:
/// one bad entry must not lose a long plan.
fn parse_item(v: &J) -> Option<Item> {
    if let Some(s) = v.as_str() {
        let s = s.trim();
        let status = if s.starts_with("[x]") || s.starts_with("[X]") {
            Status::Completed
        } else if s.starts_with("[~]") || s.starts_with("[-]") {
            Status::InProgress
        } else {
            Status::Pending
        };
        let content = s
            .trim_start_matches("[x]")
            .trim_start_matches("[X]")
            .trim_start_matches("[~]")
            .trim_start_matches("[-]")
            .trim_start_matches("[ ]")
            .trim()
            .to_string();
        return if content.is_empty() {
            None
        } else {
            Some(Item { content, status })
        };
    }
    let content = v
        .get("content")
        .and_then(|t| t.as_str())
        .or_else(|| v.get("text").and_then(|t| t.as_str()))?
        .trim()
        .to_string();
    if content.is_empty() {
        return None;
    }
    let status = v
        .get("status")
        .and_then(|s| s.as_str())
        .and_then(Status::from_str)
        .unwrap_or_else(|| {
            if matches!(v.get("done"), Some(J::Bool(true))) {
                Status::Completed
            } else {
                Status::Pending
            }
        });
    Some(Item { content, status })
}

fn parse(args: &J) -> Vec<Item> {
    let arr = args
        .get("todos")
        .or_else(|| args.get("items"))
        .and_then(|v| v.as_arr())
        .cloned()
        .unwrap_or_default();
    arr.iter().filter_map(parse_item).collect()
}

fn counts(items: &[Item]) -> (usize, usize, usize) {
    let mut pending = 0;
    let in_progress = 0;
    let mut completed = 0;
    for item in items {
        match item.status {
            Status::Pending => pending += 1,
            Status::InProgress => (),
            Status::Completed => completed += 1,
        }
    }
    let _ = in_progress;
    let active = items
        .iter()
        .filter(|i| i.status == Status::InProgress)
        .count();
    (pending, active, completed)
}

fn value() -> J {
    let items = ITEMS.lock().map(|g| g.clone()).unwrap_or_default();
    let (pending, in_progress, completed) = counts(&items);
    J::o(vec![
        (
            "todos",
            J::Arr(
                items
                    .iter()
                    .map(|i| {
                        J::o(vec![
                            ("content", J::s(&i.content)),
                            ("status", J::s(i.status.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "counts",
            J::o(vec![
                ("pending", J::Num(pending as f64)),
                ("inProgress", J::Num(in_progress as f64)),
                ("completed", J::Num(completed as f64)),
            ]),
        ),
    ])
}

fn write_exec(args: &J) -> Result<J, Fail> {
    let items = parse(args);
    if items.is_empty() {
        return Err(Fail::new(
            "todo_write needs a non-empty 'todos' list of {content, status} items",
        ));
    }
    if let Ok(mut g) = ITEMS.lock() {
        *g = items;
    }
    Ok(value())
}

fn read_exec(_a: &J) -> Result<J, Fail> {
    Ok(value())
}

fn render(_a: &J, v: &J) -> String {
    let items = v
        .get("todos")
        .or_else(|| v.get("items"))
        .and_then(|i| i.as_arr())
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return "(todo list empty)".into();
    }
    let (pending, in_progress, completed) = counts(
        &items
            .iter()
            .filter_map(parse_item)
            .collect::<Vec<Item>>(),
    );
    let _ = (pending, in_progress, completed);
    let (mut p, mut ip, mut c) = (0usize, 0usize, 0usize);
    let mut out = String::new();
    for item in &items {
        let parsed = match parse_item(item) {
            Some(x) => x,
            None => continue,
        };
        match parsed.status {
            Status::Pending => p += 1,
            Status::InProgress => ip += 1,
            Status::Completed => c += 1,
        }
        out.push_str(match parsed.status {
            Status::Completed => "[x] ",
            Status::InProgress => "[~] ",
            Status::Pending => "[ ] ",
        });
        out.push_str(&parsed.content);
        out.push('\n');
    }
    format!(
        "Updated todo list: {p} pending, {ip} in progress, {c} completed.\n{}",
        out.trim_end()
    )
}

/// Progress text for the status area (`3/7`).
pub fn progress() -> Option<String> {
    let items = ITEMS.lock().ok()?.clone();
    if items.is_empty() {
        return None;
    }
    let done = items
        .iter()
        .filter(|i| i.status == Status::Completed)
        .count();
    Some(format!("{}/{}", done, items.len()))
}

/// Forget the plan. DSH clears its todo projection at every `turn/start`; the
/// TUI calls this when a turn begins (and on `/clear`), so the list always
/// describes the work in front of the model instead of a stale plan.
pub fn clear() {
    if let Ok(mut g) = ITEMS.lock() {
        g.clear();
    }
}

fn todo_write_access(_a: &J) -> crate::tool::Access {
    crate::tool::Access::Write("todo".into())
}
fn todo_read_access(_a: &J) -> crate::tool::Access {
    crate::tool::Access::Read("todo".into())
}

pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "todo_write",
            description: "Record and update a task list to plan multi-step work and show progress; \
skip it for trivial single-step tasks. Add one todo per concrete step before you start. While \
work remains, keep exactly one todo 'in_progress'. Mark each todo 'completed' as soon as it is \
done. Every call replaces the whole list: send the COMPLETE list every time.",
            params: vec![Param::req(
                "todos",
                "array",
                "The COMPLETE task list, replacing any previous list. Items: {content, status} \
with status 'pending' | 'in_progress' | 'completed'.",
            )],
            concurrency_safe: false,
            access: Some(todo_write_access),
            timeout_ms: 5_000,
            execute: write_exec,
            render,
            present: |_a| "todo_write".into(),
            post: None,
            schema_override: Some(
                r#"{"type":"object","properties":{"todos":{"type":"array","description":"The COMPLETE task list, replacing any previous list.","items":{"type":"object","additionalProperties":false,"properties":{"content":{"type":"string","description":"What the task is - a short imperative line."},"status":{"type":"string","enum":["pending","in_progress","completed"],"description":"pending (not started) | in_progress (now) | completed (done)."}},"required":["content","status"]}}},"required":["todos"],"additionalProperties":false}"#
                    .into(),
            ),
            dynamic: None,
        },
        ToolDef {
            name: "todo_read",
            description: "Read the current turn's todo list (items and statuses).",
            params: vec![],
            concurrency_safe: true,
            access: Some(todo_read_access),
            timeout_ms: 5_000,
            execute: read_exec,
            render,
            present: |_a| "todo_read".into(),
            post: None,
            schema_override: None,
            dynamic: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse as jparse;

    fn items(raw: &str) -> Vec<Item> {
        parse(&jparse(raw).unwrap())
    }

    #[test]
    fn dsh_style_objects_are_parsed_with_statuses() {
        let got = items(
            r#"{"todos":[
                {"content":"read config","status":"completed"},
                {"content":"write answer","status":"in_progress"},
                {"content":"verify","status":"pending"}
            ]}"#,
        );
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].status, Status::Completed);
        assert_eq!(got[1].status, Status::InProgress);
        assert_eq!(got[2].status, Status::Pending);
        assert_eq!(
            counts(&got),
            (1, 1, 1),
            "pending/in-progress/completed counts"
        );
    }

    #[test]
    fn legacy_shorthand_still_works() {
        let got = items(
            r#"{"items":["[x] done step","[ ] next step",{"text":"later","done":false}]}"#,
        );
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].status, Status::Completed);
        assert_eq!(got[1].status, Status::Pending);
        assert_eq!(got[1].content, "next step");
    }

    #[test]
    fn an_empty_list_is_rejected_instead_of_clearing_the_plan() {
        let empty = jparse(r#"{"todos":[]}"#).unwrap();
        assert!(write_exec(&empty).is_err());
    }

    #[test]
    fn render_matches_the_dsh_result_line() {
        let args = jparse(
            r#"{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"in_progress"}]}"#,
        )
        .unwrap();
        let v = match write_exec(&args) {
            Ok(v) => v,
            Err(e) => panic!("write failed: {}", e.message),
        };
        let text = render(&jparse("{}").unwrap(), &v);
        assert!(
            text.starts_with("Updated todo list: 0 pending, 1 in progress, 1 completed."),
            "{text}"
        );
        assert!(text.contains("[x] a") && text.contains("[~] b"), "{text}");
        clear();
    }

    #[test]
    fn the_tool_description_carries_the_dsh_guidance() {
        let defs = tool_defs();
        let write = defs.iter().find(|d| d.name == "todo_write").unwrap();
        for needle in [
            "multi-step work",
            "skip it for trivial single-step tasks",
            "one todo per concrete step",
            "exactly one todo 'in_progress'",
            "as soon as it is done",
        ] {
            assert!(
                write.description.to_lowercase().contains(&needle.to_lowercase()),
                "todo_write description misses {needle:?}"
            );
        }
    }
}
