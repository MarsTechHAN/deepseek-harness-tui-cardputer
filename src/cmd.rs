//! Human commands — the `/` commands, modelled on DSH's `dsh-commands`
//! registry: a command has a name, a description, an optional free-form input
//! hint, and a handler that runs immediately without creating a model message.
//!
//! Results are UI outcomes (transcript lines), never tool results.

use crate::config::{self, Config};

pub const KIND_NORMAL: u8 = 0;
pub const KIND_TOOL: u8 = 2;
pub const KIND_ERR: u8 = 4;
pub const KIND_DIM: u8 = 5;

/// One registered command, mirroring DSH's `CommandDefinition`.
pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
    /// Placeholder advertised before free-form input, e.g. `[name]`.
    pub input_hint: Option<&'static str>,
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "help",
        description: "list commands",
        input_hint: None,
    },
    Command {
        name: "model",
        description: "show or set the model",
        input_hint: Some("[name]"),
    },
    Command {
        name: "effort",
        description: "show or set reasoning effort (auto|low|high|max)",
        input_hint: Some("[auto|low|high|max]"),
    },
    Command {
        name: "login",
        description: "configure the API key by scanning a QR code",
        input_hint: None,
    },
    Command {
        name: "key",
        description: "set the API key directly",
        input_hint: Some("<api-key>"),
    },
    Command {
        name: "resume",
        description: "list or resume a previous session",
        input_hint: Some("[n]"),
    },
    Command {
        name: "config",
        description: "interactive configuration (options first)",
        input_hint: Some("[wizard]"),
    },
    Command {
        name: "clear",
        description: "clear the transcript",
        input_hint: None,
    },
    Command {
        name: "quit",
        description: "exit dst",
        input_hint: None,
    },
];

pub struct Outcome {
    pub lines: Vec<(String, u8)>,
    pub quit: bool,
    pub clear: bool,
    /// Start the QR login server (main owns the UI and the config write).
    pub login: bool,
    /// Open the interactive configuration wizard.
    pub config_wizard: bool,
    /// Resume flow: `Some(None)` lists sessions, `Some(Some(n))` picks one.
    pub resume: Option<Option<usize>>,
}

impl Outcome {
    fn text(s: impl Into<String>, kind: u8) -> Outcome {
        Outcome {
            lines: vec![(s.into(), kind)],
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume: None,
        }
    }
    fn many(lines: Vec<(String, u8)>) -> Outcome {
        Outcome {
            lines,
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume: None,
        }
    }
}

fn api_cfg_of(c: &Config) -> crate::api::Cfg {
    crate::api::Cfg {
        base: c.base.clone(),
        key: c.api_key.clone(),
        model: c.model.clone(),
        max_tokens: c.max_tokens,
        effort: c.effort.clone(),
    }
}

fn usage(prefix: &str) -> Outcome {
    let mut lines = vec![(format!("{prefix}"), KIND_ERR)];
    lines.push(("try /help".into(), KIND_DIM));
    Outcome::many(lines)
}

/// Dispatch a `/` command. Returns `None` when the input is not a command.
pub fn dispatch(input: &str, cfg: &mut Config) -> Option<Outcome> {
    evaluate(input, cfg, true)
}

/// `persist=false` keeps the state change in memory only (tests).
pub fn evaluate(input: &str, cfg: &mut Config, persist: bool) -> Option<Outcome> {
    let t = input.trim();
    if !t.starts_with('/') {
        return None;
    }
    let body = &t[1..];
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    let arg = parts.next().unwrap_or("").trim();
    let name = name.to_ascii_lowercase();

    let out = match name.as_str() {
        "help" | "?" => {
            let mut lines = vec![("commands".to_string(), KIND_DIM)];
            for c in COMMANDS {
                let hint = c
                    .input_hint
                    .map(|h| format!(" {h}"))
                    .unwrap_or_default();
                lines.push((format!("/{}{hint}", c.name), KIND_TOOL));
                lines.push((format!("   {}", c.description), KIND_DIM));
            }
            lines.push(("Enter queues while busy; Shift+Enter interrupts".into(), KIND_DIM));
            Outcome::many(lines)
        }
        "model" => {
            if arg.is_empty() {
                let mut lines = vec![(format!("model  {}", cfg.model), KIND_NORMAL)];
                match crate::api::list_models(&api_cfg_of(cfg)) {
                    Ok(models) if !models.is_empty() => {
                        lines.push(("可选（/model <name>）:".into(), KIND_DIM));
                        for (id, name, efforts) in models {
                            lines.push((
                                format!("  {id}  {name}{}", if efforts.is_empty() { String::new() } else { format!("  effort: {}", efforts.join("/")) }),
                                KIND_TOOL,
                            ));
                        }
                    }
                    Ok(_) => lines.push(("未取得模型列表".into(), KIND_DIM)),
                    Err(e) => lines.push((format!("模型列表不可用: {e}"), KIND_DIM)),
                }
                Outcome::many(lines)
            } else if arg.len() > 120 {
                usage("/model <name>")
            } else {
                cfg.model = arg.to_string();
                let saved = if persist { cfg.save() } else { Ok(()) };
                match saved {
                    Ok(()) => Outcome::text(format!("model set to {}", cfg.model), KIND_NORMAL),
                    Err(e) => Outcome::text(format!("model set, but saving failed: {e}"), KIND_ERR),
                }
            }
        }
        "effort" => {
            if arg.is_empty() {
                Outcome::many(vec![
                    (format!("effort  {}", cfg.effort), KIND_NORMAL),
                    ("values: auto | low | high | max".into(), KIND_DIM),
                    ("usage: /effort high".into(), KIND_DIM),
                ])
            } else {
                let v = arg.to_ascii_lowercase();
                if !matches!(v.as_str(), "auto" | "low" | "high" | "max") {
                    usage("/effort [auto|low|high|max]")
                } else {
                    cfg.effort = v.clone();
                    let saved = if persist { cfg.save() } else { Ok(()) };
                    match saved {
                        Ok(()) => Outcome::text(format!("effort set to {v}"), KIND_NORMAL),
                        Err(e) => Outcome::text(format!("effort set, but saving failed: {e}"), KIND_ERR),
                    }
                }
            }
        }
        "key" => {
            if arg.is_empty() {
                usage("/key <api-key>")
            } else {
                // Never echo the key back.
                cfg.api_key = arg.to_string();
                cfg.configured = true;
                let saved = if persist { cfg.save() } else { Ok(()) };
                match saved {
                    Ok(()) => Outcome::text(
                        format!("API key saved to {}", config::key_path().display()),
                        KIND_NORMAL,
                    ),
                    Err(e) => Outcome::text(format!("saving the key failed: {e}"), KIND_ERR),
                }
            }
        }
        "login" => {
            let mut o = login_hint(cfg);
            o.login = true;
            o
        }
        "config" => {
            if arg == "path" {
                Outcome::text(config::config_path().display().to_string(), KIND_NORMAL)
            } else if arg.is_empty() || arg == "wizard" {
                let mut o = Outcome::many(vec![
                    ("打开配置向导…".to_string(), KIND_DIM),
                ]);
                o.config_wizard = true;
                o
            } else {
                let mut lines = vec![("effective config".to_string(), KIND_DIM)];
                for l in cfg.summary() {
                    lines.push((l, KIND_NORMAL));
                }
                lines.push(("edit: ~/.dst/config.yml  (DSH patch dialect)".into(), KIND_DIM));
                Outcome::many(lines)
            }
        }
        "resume" => {
            let sessions = config::list_sessions();
            if sessions.is_empty() {
                Outcome::many(vec![("还没有可恢复的会话".into(), KIND_DIM)])
            } else if arg.is_empty() {
                let mut lines = vec![("最近的会话（/resume <n> 恢复）:".to_string(), KIND_DIM)];
                for (i, (_p, label, _t)) in sessions.iter().take(5).enumerate() {
                    lines.push((format!("  {}) {}", i + 1, label), KIND_TOOL));
                }
                let mut o = Outcome::many(lines);
                o.resume = Some(None);
                o
            } else {
                match arg.parse::<usize>() {
                    Ok(n) if n >= 1 && n <= sessions.len() => {
                        let mut o = Outcome::many(vec![(
                            format!("恢复会话 {}", sessions[n - 1].1),
                            KIND_DIM,
                        )]);
                        o.resume = Some(Some(n - 1));
                        o
                    }
                    _ => usage("/resume <n>"),
                }
            }
        }
        "clear" => Outcome {
            lines: Vec::new(),
            quit: false,
            clear: true,
            login: false,
            config_wizard: false,
            resume: None,
        },
        "quit" | "exit" | "q" => Outcome {
            lines: Vec::new(),
            quit: true,
            clear: false,
            login: false,
            config_wizard: false,
            resume: None,
        },
        "" => usage("/"),
        other => Outcome::many(vec![
            (format!("unknown command /{other}"), KIND_ERR),
            ("try /help".into(), KIND_DIM),
        ]),
    };
    Some(out)
}

/// `/login`: when the key is missing this starts the guided setup; when a key
/// is present it shows how to replace it.
fn login_hint(cfg: &mut Config) -> Outcome {
    let mut lines = vec![("login".to_string(), KIND_DIM)];
    if cfg.configured {
        lines.push(("an API key is already configured".into(), KIND_NORMAL));
        lines.push(("replace it: /key <api-key>".into(), KIND_DIM));
        lines.push(("re-run guided setup: delete ~/.dst/api_key then /login".into(), KIND_DIM));
    } else {
        lines.push(("no API key configured yet".into(), KIND_ERR));
        lines.push(("1) DeepSeek official: create a key at platform.deepseek.com".into(), KIND_NORMAL));
        lines.push(("2) send it here with: /key <api-key>".into(), KIND_NORMAL));
        lines.push(("QR login (scan from your phone) is available via /login qr".into(), KIND_DIM));
    }
    Outcome::many(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn non_command_passes_through() {
        assert!(dispatch("hello", &mut cfg()).is_none());
    }

    #[test]
    fn model_sets_value() {
        let mut c = cfg();
        let o = evaluate("/model deepseek-v4", &mut c, false).unwrap();
        assert_eq!(c.model, "deepseek-v4");
        assert!(o.lines[0].0.contains("deepseek-v4"));
    }

    #[test]
    fn effort_validates() {
        let mut c = cfg();
        let bad = evaluate("/effort banana", &mut c, false).unwrap();
        assert!(bad.lines[0].0.starts_with("/effort"), "usage hint: {:?}", bad.lines);
        assert_eq!(c.effort, "auto");
        assert!(evaluate("/effort high", &mut c, false).is_some());
        assert_eq!(c.effort, "high");
    }

    #[test]
    fn key_is_never_echoed() {
        let mut c = cfg();
        let o = evaluate("/key sk-secret-value", &mut c, false).unwrap();
        let all: String = o.lines.iter().map(|(s, _)| s.clone()).collect();
        assert!(!all.contains("sk-secret-value"));
        assert_eq!(c.api_key, "sk-secret-value");
    }

    #[test]
    fn quit_and_clear_flags() {
        assert!(dispatch("/quit", &mut cfg()).unwrap().quit);
        assert!(dispatch("/clear", &mut cfg()).unwrap().clear);
    }
}
