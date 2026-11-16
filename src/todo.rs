//! Agent-maintained todo list (DSH `dsh-tool-todo`).

use crate::json::J;
use crate::tool::{Fail, Param, ToolDef};
use std::sync::Mutex;

#[derive(Clone)]
struct Item {
    text: String,
    done: bool,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

fn parse(args: &J) -> Vec<Item> {
    let arr = args.get("items").and_then(|v| v.as_arr()).cloned().unwrap_or_default();
    arr.iter()
        .filter_map(|v| {
            if let Some(s) = v.as_str() {
                let s = s.trim();
                let done = s.starts_with("[x]") || s.starts_with("[X]");
                let text = s.trim_start_matches("[x]").trim_start_matches("[X]")
                    .trim_start_matches("[ ]").trim().to_string();
                if text.is_empty() { None } else { Some(Item { text, done }) }
            } else {
                let text = v.get("text").and_then(|t| t.as_str())?.to_string();
                let done = v.get("done").and_then(|d| d.as_bool()).unwrap_or(false)
                    || v.get("status").and_then(|s| s.as_str()) == Some("completed");
                Some(Item { text, done })
            }
        })
        .collect()
}

fn value() -> J {
    let items = ITEMS.lock().map(|g| g.clone()).unwrap_or_default();
    J::o(vec![
        ("items", J::Arr(items.iter().map(|i| J::o(vec![
            ("text", J::s(&i.text)),
            ("done", J::Bool(i.done)),
        ])).collect())),
        ("done", J::Num(items.iter().filter(|i| i.done).count() as f64)),
        ("total", J::Num(items.len() as f64)),
    ])
}

fn write_exec(args: &J) -> Result<J, Fail> {
    let items = parse(args);
    if let Ok(mut g) = ITEMS.lock() {
        *g = items;
    }
    Ok(value())
}

fn read_exec(_a: &J) -> Result<J, Fail> {
    Ok(value())
}

fn render(_a: &J, v: &J) -> String {
    let items = v.get("items").and_then(|i| i.as_arr()).cloned().unwrap_or_default();
    if items.is_empty() {
        return "(todo list empty)".into();
    }
    let mut out = String::new();
    for i in items {
        let d = i.get("done").map(|b| matches!(b, J::Bool(true))).unwrap_or(false);
        let t = i.get("text").and_then(|x| x.as_str()).unwrap_or("");
        out.push_str(if d { "[x] " } else { "[ ] " });
        out.push_str(t);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Progress text for the status area (`3/7`).
pub fn progress() -> Option<String> {
    let items = ITEMS.lock().ok()?.clone();
    if items.is_empty() {
        return None;
    }
    Some(format!(
        "{}/{}",
        items.iter().filter(|i| i.done).count(),
        items.len()
    ))
}

pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "todo_write",
            description: "Replace the session todo list. Track multi-step work: mark items done as you \
finish them, keep exactly one item in progress if possible. Use [ ] and [x] prefixes or objects with text/done.",
            params: vec![Param::req("items", "array", "List of todo items: strings like '[ ] step' / '[x] step', or {text, done} objects.")],
            concurrency_safe: false,
            timeout_ms: 5_000,
            execute: write_exec,
            render,
            present: |_a| "todo_write".into(),
            post: None,
            schema_override: Some(r#"{"type":"object","properties":{"items":{"type":"array","items":{"type":"string"},"description":"todo items with [ ]/[x] prefixes"}},"required":["items"],"additionalProperties":false}"#.into()),
            dynamic: None,
        },
        ToolDef {
            name: "todo_read",
            description: "Read the current session todo list.",
            params: vec![],
            concurrency_safe: true,
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
