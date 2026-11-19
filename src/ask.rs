//! Interactive user-choice tool (`ask_user`), modelled on Cursor's AskQuestion
//! and Claude Code's AskUserQuestion, sized for the 53x12 Cardputer Zero panel.
//!
//! A worker thread blocks on `request_answers()` while the TUI's tool-pump
//! services `service_prompt(ui)` on the main thread, renders the interactive
//! selection screen (single-select, multi-select, and custom "Other..." text),
//! and sends back the structured answers.

use crate::json::J;
use crate::tool::{obj, str_arg, Fail, ToolDef};
use std::sync::Mutex;

const MAX_QUESTIONS: usize = 6;
const MAX_OPTIONS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskQuestion {
    pub id: String,
    pub prompt: String,
    pub allow_multiple: bool,
    pub options: Vec<AskOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskRequest {
    pub title: Option<String>,
    pub questions: Vec<AskQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskAnswer {
    pub id: String,
    pub prompt: String,
    pub selected: Vec<AskOption>,
    pub custom_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskResponse {
    pub cancelled: bool,
    pub answers: Vec<AskAnswer>,
}

/// Parse and validate an `ask_user` call's JSON arguments.
pub fn parse_request(args: &J) -> Result<AskRequest, String> {
    let title = str_arg(args, "title")
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty());
    let q_arr = args
        .get("questions")
        .and_then(|v| v.as_arr())
        .ok_or_else(|| "missing required 'questions' array".to_string())?;
    if q_arr.is_empty() {
        return Err("'questions' must contain at least 1 question".to_string());
    }
    if q_arr.len() > MAX_QUESTIONS {
        return Err(format!("at most {MAX_QUESTIONS} questions per ask_user call"));
    }
    let mut questions = Vec::with_capacity(q_arr.len());
    let mut seen_q = std::collections::HashSet::new();
    for (qi, q) in q_arr.iter().enumerate() {
        let id = str_arg(q, "id")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("questions[{qi}] missing non-empty 'id'"))?;
        if !seen_q.insert(id.clone()) {
            return Err(format!("duplicate question id '{id}'"));
        }
        let prompt = str_arg(q, "prompt")
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("question '{id}' missing non-empty 'prompt'"))?;
        let allow_multiple = matches!(q.get("allow_multiple"), Some(J::Bool(true)));
        let opt_arr = q
            .get("options")
            .and_then(|v| v.as_arr())
            .ok_or_else(|| format!("question '{id}' missing 'options' array"))?;
        if opt_arr.len() < 2 {
            return Err(format!("question '{id}' needs at least 2 options"));
        }
        if opt_arr.len() > MAX_OPTIONS {
            return Err(format!("question '{id}' has more than {MAX_OPTIONS} options"));
        }
        let mut options = Vec::with_capacity(opt_arr.len());
        let mut seen_o = std::collections::HashSet::new();
        for (oi, o) in opt_arr.iter().enumerate() {
            let (oid, label) = if let Some(s) = o.as_str() {
                let lbl = s.split_whitespace().collect::<Vec<_>>().join(" ");
                if lbl.is_empty() {
                    return Err(format!("question '{id}' options[{oi}] must not be empty"));
                }
                (format!("{}", oi + 1), lbl)
            } else {
                let oid = str_arg(o, "id")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("question '{id}' options[{oi}] missing 'id'"))?;
                let lbl = str_arg(o, "label")
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("question '{id}' options[{oi}] missing 'label'"))?;
                (oid, lbl)
            };
            if !seen_o.insert(oid.clone()) {
                return Err(format!("question '{id}' has duplicate option id '{oid}'"));
            }
            options.push(AskOption { id: oid, label });
        }
        questions.push(AskQuestion {
            id,
            prompt,
            allow_multiple,
            options,
        });
    }
    Ok(AskRequest { title, questions })
}

// ---------------------------------------------------------------------------
// screen handoff bridge between tool worker thread and interactive TUI
// ---------------------------------------------------------------------------

enum PromptMsg {
    Picked,
    Done(Result<AskResponse, String>),
}

struct PendingPrompt {
    req: AskRequest,
    tx: std::sync::mpsc::Sender<PromptMsg>,
}

static PENDING: Mutex<Option<PendingPrompt>> = Mutex::new(None);
static SCREEN_AVAILABLE: Mutex<bool> = Mutex::new(false);

/// Called by the interactive TUI at startup/teardown.
pub fn set_screen_available(on: bool) {
    if let Ok(mut g) = SCREEN_AVAILABLE.lock() {
        *g = on;
    }
}

fn screen_available() -> bool {
    SCREEN_AVAILABLE.lock().map(|g| *g).unwrap_or(false)
}

/// Blocking request from a tool worker thread: "show this question form".
pub fn request_answers(req: AskRequest) -> Result<AskResponse, String> {
    if !screen_available() {
        return Err(
            "ask_user needs the interactive screen; it is not available in --once, background jobs or --call"
                .to_string(),
        );
    }
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let mut g = PENDING.lock().map_err(|_| "ask_user state is poisoned")?;
        if g.is_some() {
            return Err("another question prompt is already open".to_string());
        }
        *g = Some(PendingPrompt { req, tx });
    }
    match rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(PromptMsg::Picked) => {}
        Ok(PromptMsg::Done(r)) => return r,
        Err(_) => {
            if let Ok(mut g) = PENDING.lock() {
                *g = None;
            }
            return Err("the TUI did not pick up the ask_user request".to_string());
        }
    }
    // Wait generously while the user reads and responds on the panel.
    match rx.recv_timeout(std::time::Duration::from_secs(600)) {
        Ok(PromptMsg::Done(r)) => r,
        _ => Err("timed out waiting for user answer".to_string()),
    }
}

/// Called from the tool executor's UI pump (every ~15 ms while tools run).
pub fn service_prompt(ui: &mut dyn crate::harness::Ui) {
    if !screen_available() {
        return;
    }
    let req = match PENDING.lock() {
        Ok(mut g) => g.take(),
        Err(_) => return,
    };
    let req = match req {
        Some(r) => r,
        None => return,
    };
    let _ = req.tx.send(PromptMsg::Picked);
    let result = ui.ask_user(&req.req);
    let _ = req.tx.send(PromptMsg::Done(result));
}

// ---------------------------------------------------------------------------
// tool execution & rendering
// ---------------------------------------------------------------------------

fn response_to_json(resp: &AskResponse) -> J {
    let answers = resp
        .answers
        .iter()
        .map(|a| {
            let selected = J::Arr(
                a.selected
                    .iter()
                    .map(|o| obj(vec![("id", J::s(&o.id)), ("label", J::s(&o.label))]))
                    .collect(),
            );
            let custom = match &a.custom_text {
                Some(t) => J::s(t),
                None => J::Null,
            };
            obj(vec![
                ("id", J::s(&a.id)),
                ("prompt", J::s(&a.prompt)),
                ("selected", selected),
                ("custom_text", custom),
            ])
        })
        .collect();
    obj(vec![
        ("cancelled", J::Bool(resp.cancelled)),
        ("answers", J::Arr(answers)),
    ])
}

fn ask_exec(args: &J) -> Result<J, Fail> {
    let req = parse_request(args).map_err(Fail::new)?;
    let resp = request_answers(req).map_err(Fail::new)?;
    Ok(response_to_json(&resp))
}

fn ask_render(_args: &J, v: &J) -> String {
    if matches!(v.get("cancelled"), Some(J::Bool(true))) {
        return "User cancelled the question without selecting an option.".to_string();
    }
    let arr = match v.get("answers").and_then(|a| a.as_arr()) {
        Some(a) if !a.is_empty() => a,
        _ => return "User submitted no answers.".to_string(),
    };
    let mut lines = vec!["User answered:".to_string()];
    for a in arr {
        let id = a.get("id").and_then(|x| x.as_str()).unwrap_or("q");
        let mut parts: Vec<String> = Vec::new();
        if let Some(sel) = a.get("selected").and_then(|x| x.as_arr()) {
            for o in sel {
                let oid = o.get("id").and_then(|x| x.as_str()).unwrap_or("");
                let lbl = o.get("label").and_then(|x| x.as_str()).unwrap_or("");
                if !lbl.is_empty() {
                    parts.push(format!("{lbl} (id: {oid})"));
                }
            }
        }
        if let Some(custom) = a.get("custom_text").and_then(|x| x.as_str()) {
            if !custom.trim().is_empty() {
                parts.push(format!("Other: \"{custom}\""));
            }
        }
        let summary = if parts.is_empty() {
            "(no selection)".to_string()
        } else {
            parts.join(", ")
        };
        lines.push(format!("- [{id}] {summary}"));
    }
    lines.join("\n")
}

fn ask_present(args: &J) -> String {
    if let Some(t) = str_arg(args, "title").filter(|s| !s.trim().is_empty()) {
        let short: String = t.chars().take(30).collect();
        return format!("ask: {short}");
    }
    let count = args
        .get("questions")
        .and_then(|q| q.as_arr())
        .map(|a| a.len())
        .unwrap_or(1);
    if count == 1 {
        if let Some(p) = args
            .get("questions")
            .and_then(|q| q.as_arr())
            .and_then(|a| a.first())
            .and_then(|q| q.get("prompt"))
            .and_then(|s| s.as_str())
        {
            let short: String = p.chars().take(30).collect();
            return format!("ask: {short}");
        }
    }
    format!("ask_user ({count} questions)")
}

const ASK_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["questions"],"properties":{"title":{"type":"string","description":"Optional short header for the form (max 40 chars)."},"questions":{"type":"array","minItems":1,"maxItems":6,"description":"Questions to present to the user, one screen per question.","items":{"type":"object","additionalProperties":false,"required":["id","prompt","options"],"properties":{"id":{"type":"string","description":"Unique identifier for this question."},"prompt":{"type":"string","description":"Short question text shown above the options (fits 1-2 lines on the 53-col panel)."},"allow_multiple":{"type":"boolean","default":false,"description":"If true, user can toggle multiple options with Space and confirm with Enter."},"options":{"type":"array","minItems":2,"maxItems":8,"description":"Selectable choices. Put a recommended choice first with '(Recommended)'. An 'Other...' custom text option is always added automatically.","items":{"type":"object","additionalProperties":false,"required":["id","label"],"properties":{"id":{"type":"string","description":"Option identifier returned in answers."},"label":{"type":"string","description":"Concise option label shown on one row."}}}}}}}}}"#;

pub fn tool_defs() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "ask_user",
        description: "Ask the user one or more structured multiple-choice questions on the \
device panel and wait for their selection. Use it ONLY when genuinely blocked on a user \
decision (choosing an approach, clarifying ambiguous requirements, selecting options) \
instead of listing choices in chat prose. Supports single-select and multi-select \
(allow_multiple=true), and automatically includes an 'Other...' custom text option. \
Only available in the interactive TUI.",
        params: vec![],
        concurrency_safe: false,
        access: None,
        timeout_ms: 600_000,
        execute: ask_exec,
        render: ask_render,
        present: ask_present,
        post: None,
        schema_override: Some(ASK_SCHEMA.to_string()),
        dynamic: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_single_and_multi_questions() {
        let raw = r#"{
            "title": "Build setup",
            "questions": [
                {
                    "id": "mode",
                    "prompt": "Which build mode?",
                    "options": [
                        {"id": "release", "label": "Release (Recommended)"},
                        {"id": "debug", "label": "Debug"}
                    ]
                },
                {
                    "id": "extras",
                    "prompt": "Include extras?",
                    "allow_multiple": true,
                    "options": ["Tests", "Benchmarks"]
                }
            ]
        }"#;
        let args = crate::json::parse(raw).unwrap();
        let req = parse_request(&args).unwrap();
        assert_eq!(req.title.as_deref(), Some("Build setup"));
        assert_eq!(req.questions.len(), 2);
        assert!(!req.questions[0].allow_multiple);
        assert!(req.questions[1].allow_multiple);
        assert_eq!(req.questions[1].options[0].label, "Tests");
    }

    #[test]
    fn rejects_invalid_question_payloads() {
        let too_few = crate::json::parse(
            r#"{"questions":[{"id":"q1","prompt":"Pick","options":[{"id":"a","label":"Only one"}]}]}"#,
        )
        .unwrap();
        assert!(parse_request(&too_few).unwrap_err().contains("at least 2"));

        let dup_id = crate::json::parse(
            r#"{"questions":[
                {"id":"q1","prompt":"A?","options":[{"id":"1","label":"x"},{"id":"2","label":"y"}]},
                {"id":"q1","prompt":"B?","options":[{"id":"1","label":"x"},{"id":"2","label":"y"}]}
            ]}"#,
        )
        .unwrap();
        assert!(parse_request(&dup_id).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn renders_selected_and_custom_answers_cleanly() {
        let resp = AskResponse {
            cancelled: false,
            answers: vec![
                AskAnswer {
                    id: "lang".into(),
                    prompt: "Language?".into(),
                    selected: vec![AskOption {
                        id: "rs".into(),
                        label: "Rust (Recommended)".into(),
                    }],
                    custom_text: None,
                },
                AskAnswer {
                    id: "notes".into(),
                    prompt: "Notes?".into(),
                    selected: vec![],
                    custom_text: Some("keep it tiny".into()),
                },
            ],
        };
        let v = response_to_json(&resp);
        let text = ask_render(&J::Null, &v);
        assert!(text.contains("- [lang] Rust (Recommended) (id: rs)"), "{text}");
        assert!(text.contains("- [notes] Other: \"keep it tiny\""), "{text}");

        let cancelled = response_to_json(&AskResponse {
            cancelled: true,
            answers: vec![],
        });
        assert!(ask_render(&J::Null, &cancelled).contains("cancelled"));
    }

    #[test]
    fn schema_is_valid_json_object() {
        let v = crate::json::parse(ASK_SCHEMA).expect("valid JSON schema");
        assert_eq!(v.get("type").and_then(|s| s.as_str()), Some("object"));
    }
}
