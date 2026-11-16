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
        name: "usage",
        description: "token usage, cache hit rate and context pressure",
        input_hint: None,
    },
    Command {
        name: "compact",
        description: "summarise older history now (same method as automatic compaction)",
        input_hint: None,
    },
    Command {
        name: "goal",
        description: "show or manage the session goal (long-running objective)",
        input_hint: Some("[show|<objective>|edit <objective>|pause|resume|clear]"),
    },
    Command {
        name: "resume",
        description: "pick a previous session from a menu (expand to preview)",
        input_hint: None,
    },
    Command {
        name: "rewind",
        description: "restore a checkpoint from an earlier turn (menu picker)",
        input_hint: None,
    },
    Command {
        name: "extensions",
        description: "inspect skills and MCP servers (menu)",
        input_hint: None,
    },
    Command {
        name: "rename",
        description: "name the current saved session (shown in /resume)",
        input_hint: Some("<title>"),
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

/// Commands matching what has been typed after `/` (before any space).
/// Used by the interactive palette.
/// Localized one-line description for a command.
pub fn describe(name: &str) -> String {
    let key = format!("cmd_{name}");
    let t = crate::i18n::t(&key);
    if t.is_empty() {
        COMMANDS
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.description.to_string())
            .unwrap_or_default()
    } else {
        t.to_string()
    }
}

pub fn palette(input: &str) -> Vec<(&'static str, String, Option<&'static str>)> {
    let t = input.trim_start();
    if !t.starts_with('/') {
        return Vec::new();
    }
    let q = &t[1..];
    if q.contains(char::is_whitespace) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(q))
        .map(|c| (c.name, describe(c.name), c.input_hint))
        .collect()
}

pub struct Outcome {
    pub lines: Vec<(String, u8)>,
    pub quit: bool,
    pub clear: bool,
    /// Start the QR login server (main owns the UI and the config write).
    pub login: bool,
    /// Open the interactive configuration wizard.
    pub config_wizard: bool,
    /// Open the menuconfig-style /resume picker (main owns the screen).
    pub resume_menu: bool,
    /// Open the model picker menu (main owns /v1/models fetch and the screen).
    pub model_menu: bool,
    /// Run the manual compaction flow (main owns the transcript).
    pub compact: bool,
    /// Open the checkpoint picker (main owns the transcript).
    pub rewind_menu: bool,
    /// Open the skills/MCP inspection menu.
    pub extensions_menu: bool,
    /// Open the command palette menu (/help).
    pub help_menu: bool,
}

impl Outcome {
    fn text(s: impl Into<String>, kind: u8) -> Outcome {
        Outcome {
            lines: vec![(s.into(), kind)],
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            compact: false,
        }
    }
    fn many(lines: Vec<(String, u8)>) -> Outcome {
        Outcome {
            lines,
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            compact: false,
        }
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
        "help" | "?" => Outcome {
            lines: Vec::new(),
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            compact: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: true,
        },
        "model" => {
            if arg.is_empty() {
                Outcome {
                    lines: Vec::new(),
                    quit: false,
                    clear: false,
                    login: false,
                    config_wizard: false,
                    resume_menu: false,
                    model_menu: true,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
                    compact: false,
                }
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
                    (crate::i18n::t("effort_values").to_string(), KIND_DIM),
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
        "usage" => {
            let mut lines = vec![(crate::i18n::t("usage_title").to_string(), KIND_DIM)];
            for l in crate::usage::report() {
                lines.push((l, KIND_NORMAL));
            }
            if !config::is_official(&cfg.base) {
                lines.push((
                    "note: cache accounting is reported by the official DeepSeek API only"
                        .to_string(),
                    KIND_DIM,
                ));
            }
            Outcome::many(lines)
        }
        "resume" => {
            if config::list_sessions().is_empty() {
                Outcome::many(vec![(crate::i18n::t("resume_empty").to_string(), KIND_DIM)])
            } else {
                Outcome {
                    lines: Vec::new(),
                    quit: false,
                    clear: false,
                    login: false,
                    config_wizard: false,
                    resume_menu: true,
                    model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
                    compact: false,
                }
            }
        }
        "rewind" => Outcome {
            lines: vec![("select a checkpoint to restore".to_string(), KIND_DIM)],
            quit: false, clear: false, login: false, config_wizard: false,
            resume_menu: false, model_menu: false, compact: false,
            rewind_menu: true, extensions_menu: false, help_menu: false,
        },
        "extensions" => Outcome {
            lines: Vec::new(), quit: false, clear: false, login: false,
            config_wizard: false, resume_menu: false, model_menu: false,
            compact: false, rewind_menu: false, extensions_menu: true, help_menu: false,
        },
        "rename" => {
            if arg.is_empty() {
                usage("/rename <title>")
            } else {
                match crate::session::rename(arg) {
                    Ok(title) => Outcome::text(format!("session renamed to {title}"), KIND_NORMAL),
                    Err(e) => Outcome::text(format!("rename failed: {e}"), KIND_ERR),
                }
            }
        }
        "compact" => Outcome {
            lines: vec![("compacting older history...".to_string(), KIND_DIM)],
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            compact: true,
        },
        "goal" => {
            let lower = arg.to_ascii_lowercase();
            let result = if arg.is_empty() || lower == "show" || lower == "status" {
                Ok(crate::goal::status_lines())
            } else if lower == "clear" {
                match crate::goal::clear() {
                    Ok(()) => Ok(vec!["Goal cleared.".to_string()]),
                    Err(e) => Err(e),
                }
            } else if lower == "pause" {
                let g = crate::goal::load().ok_or("no goal is currently set".to_string());
                g.and_then(|g| crate::goal::pause(&g.id, g.revision))
                    .map(|g| {
                        let mut l = vec!["Goal paused".to_string()];
                        l.extend(crate::goal::status_lines().into_iter().skip(1));
                        let _ = g;
                        l
                    })
            } else if lower == "resume" {
                let g = crate::goal::load().ok_or("no goal is currently set".to_string());
                g.and_then(|g| crate::goal::resume(&g.id, g.revision))
                    .map(|_| {
                        let mut l = vec!["Goal resumed".to_string()];
                        l.extend(crate::goal::status_lines().into_iter().skip(1));
                        l
                    })
            } else if lower == "edit" {
                Err("Goal editing requires a replacement objective.\nUsage: /goal [<objective>|clear|edit <objective>|pause|resume]".to_string())
            } else if lower.starts_with("edit ") || lower.starts_with("edit\t") {
                let objective = arg[4..].trim().to_string();
                let g = crate::goal::load().ok_or("no goal is currently set".to_string());
                g.and_then(|g| crate::goal::edit(&g.id, g.revision, Some(&objective), None))
                    .map(|_| {
                        let mut l = vec!["Goal updated".to_string()];
                        l.extend(crate::goal::status_lines().into_iter().skip(1));
                        l
                    })
            } else {
                crate::goal::create(arg, None).map(|_| {
                    let mut l = vec!["Goal created".to_string()];
                    l.extend(crate::goal::status_lines().into_iter().skip(1));
                    l
                })
            };
            match result {
                Ok(lines) => Outcome::many(lines.into_iter().map(|l| (l, KIND_NORMAL)).collect()),
                Err(e) => Outcome::many(vec![(e, KIND_ERR)]),
            }
        }
        "clear" => Outcome {
            lines: Vec::new(),
            quit: false,
            clear: true,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            compact: false,
        },
        "quit" | "exit" | "q" => Outcome {
            lines: Vec::new(),
            quit: true,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            compact: false,
        },
        "" => usage("/"),
        other => Outcome::many(vec![
            (crate::i18n::tf("cmd_unknown", &[other]), KIND_ERR),
            (crate::i18n::t("try_help").to_string(), KIND_DIM),
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
        lines.push(("Create a key at platform.deepseek.com, then send it with /key <api-key>".into(), KIND_NORMAL));
        lines.push(("Or use the QR flow: /login starts a login page for your phone".into(), KIND_DIM));
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
