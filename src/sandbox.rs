//! Tool-call permissions: a small sandbox with three modes.
//!
//! `auto` (default): known-safe operations run directly, system-configuration
//! changes are refused, and everything uncertain is decided by a short LLM
//! call. `strict`: every non-safe operation waits for a human pop-up
//! (allow/reject). `full`: everything runs.
//!
//! Classification is a curated rule set rather than a real syscall sandbox:
//! this device has no namespaces/seccomp and no libc crate. The rules cover:
//! read-only inspection (ls/cat/grep/find/ps/ip/journalctl status/git status…),
//! network reads (fetch/search/curl GET), workspace writes (`write_file` under
//! the current directory, `> relative-file`), and a danger list for anything
//! that changes system configuration (sudo, systemctl start/stop, apt, dd,
//! mounting, writes under /etc /boot /usr, reboot, package installs, …).

use crate::api::{self, Cfg};
use crate::json::J;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Auto,
    Strict,
    Full,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Strict => "strict",
            Mode::Full => "full",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Mode::Auto),
            "strict" => Some(Mode::Strict),
            "full" | "full-access" | "full_access" => Some(Mode::Full),
            _ => None,
        }
    }
    pub fn next(self) -> Mode {
        match self {
            Mode::Auto => Mode::Strict,
            Mode::Strict => Mode::Full,
            Mode::Full => Mode::Auto,
        }
    }
}

#[derive(Clone)]
pub struct Settings {
    pub mode: Mode,
    /// Empty = use the session model for the auto judge.
    pub judge_model: String,
    pub judge_max_tokens: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: Mode::Auto,
            judge_model: String::new(),
            judge_max_tokens: 48,
        }
    }
}

impl Settings {
    pub fn from_config(c: &crate::config::Config) -> Settings {
        Settings {
            mode: Mode::parse(&c.sandbox_mode).unwrap_or(Mode::Auto),
            judge_model: c.sandbox_judge_model.clone(),
            judge_max_tokens: c.sandbox_judge_max_tokens.max(8),
        }
    }
}

/// What the strict-mode pop-up shows.
pub struct Approval {
    pub tool: String,
    pub args: String,
    pub reason: String,
}

pub trait Confirmer {
    fn confirm(&mut self, request: &Approval) -> bool;
}

impl<F: FnMut(&Approval) -> bool> Confirmer for F {
    fn confirm(&mut self, request: &Approval) -> bool {
        self(request)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Safe,
    Danger,
    Unknown,
}

/// Read-only first words. A segment is safe when every pipeline segment starts
/// with one of these and the per-command checks pass.
const READONLY: &[&str] = &[
    "ls", "ll", "la", "pwd", "cat", "head", "tail", "grep", "egrep", "fgrep", "rg",
    "find", "wc", "sort", "uniq", "cut", "tr", "awk", "date", "uname", "uptime",
    "free", "df", "du", "stat", "file", "which", "whereis", "type", "command",
    "echo", "printf", "env", "printenv", "id", "whoami", "hostname", "ps", "top",
    "htop", "pgrep", "lsblk", "blkid", "lscpu", "lsusb", "lspci", "lsmod",
    "sensors", "ip", "ifconfig", "iw", "iwconfig", "iwinfo", "ping", "ping6",
    "traceroute", "dig", "nslookup", "host", "curl", "wget", "git", "journalctl",
    "dmesg", "systemctl", "sed", "test", "[", "true", "false", "sleep",
];

/// Anything matching these is a system-configuration change (auto rejects it,
/// strict asks the human, full runs it).
const DANGER: &[&str] = &[
    r"(^|[[:space:]])sudo([[:space:]]|$)",
    r"(^|[[:space:]])su([[:space:]]|$)",
    r"systemctl[[:space:]]+(start|stop|restart|reload|try-restart|enable|disable|mask|unmask|daemon-reload|set-default)",
    r"service[[:space:]]+[a-z0-9_.-]+[[:space:]]+(start|stop|restart|reload)",
    r"(apt|apt-get|dpkg|dnf|yum|pacman|apk|opkg)[[:space:]]",
    r"(pip|pip3|cargo|npm|yarn|pnpm|gem|go)[[:space:]]+install",
    r"mount[[:space:]]|umount[[:space:]]|modprobe[[:space:]]|insmod[[:space:]]|rmmod[[:space:]]",
    r"(^|[[:space:]])(reboot|shutdown|poweroff|halt|systemd-)([[:space:]]|$)",
    r"(^|[[:space:]])dd[[:space:]]",
    r"(^|[[:space:]])(mkfs|fdisk|parted|wipefs|mkswap|swapon|swapoff)",
    r"iptables|nft[[:space:]]|ufw[[:space:]]",
    r"crontab[[:space:]]",
    r"(^|[[:space:]])(useradd|userdel|usermod|passwd|groupadd|visudo)([[:space:]]|$)",
    r"(^|[[:space:]])(kill|killall|pkill)[[:space:]]",
    r">>?[[:space:]]*/(etc|boot|usr|lib|sys|proc|dev)/",
    r"tee[[:space:]].{0,60}/(etc|boot|usr|lib)",
    r"(chmod|chown|chattr)[[:space:]].{0,60}/(etc|boot|usr|lib)",
    r"(rm|shred)[[:space:]]+-[a-z]*[rf][a-z]*[[:space:]]+/(etc|boot|usr|lib|var|home|root|sys)",
    r"(mv|ln)[[:space:]].{0,80}/(etc|boot|usr|lib)",
    r"find[[:space:]].*(-delete|-exec|-execdir|-ok)",
    r"git[[:space:]]+(push|reset|clean|checkout|rebase|stash[[:space:]]+drop)",
    r"curl[[:space:]].*(-X[[:space:]]*(POST|PUT|DELETE|PATCH)|--data|-d[[:space:]]|--upload|-F[[:space:]]|-T[[:space:]])",
    r"wget[[:space:]].*-O[[:space:]]*/(etc|boot|usr)",
    r"(^|[[:space:]])(insmod|modinfo)[[:space:]]",
];

/// System paths that make a write dangerous even from an otherwise safe tool.
fn path_is_system(path: &str) -> bool {
    let p = path.trim().trim_matches('"').trim_matches('\'');
    [("/etc/", true), ("/boot/", true), ("/usr/", true), ("/lib/", true),
     ("/sys/", true), ("/proc/", true), ("/dev/", true), ("/var/", true)]
        .iter()
        .any(|(prefix, _)| p.starts_with(prefix))
}

/// Run one ERE union through `grep -qE` (the sandbox lives outside the shell;
/// the pattern is ours, the text is the candidate command).
fn regex_hit(patterns: &[&str], text: &str) -> bool {
    let union = patterns
        .iter()
        .map(|p| format!("({p})"))
        .collect::<Vec<_>>()
        .join("|");
    let child = Command::new("grep")
        .args(["-qE", "-e", &union])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        // No grep on PATH: fall back to a conservative substring scan of the
        // most important literals (never fail open).
        Err(_) => {
            let lower = text.to_ascii_lowercase();
            return ["sudo", "systemctl", "apt-get", "apt ", " mount", "reboot",
                    "/etc/", "/boot/", "dd if=", "mkfs"]
                .iter()
                .any(|p| lower.contains(p));
        }
    };
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(text.as_bytes());
    }
    matches!(child.wait(), Ok(s) if s.success())
}

/// Split a shell command into pipeline segments (enough for classification).
fn segments(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let bytes: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '\'' || c == '"' {
            // keep quoted text intact (spaces are not separators inside)
            cur.push(c);
            i += 1;
            while i < bytes.len() {
                cur.push(bytes[i]);
                if bytes[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '|' || c == ';' || c == '\n' {
            out.push(cur.trim().to_string());
            cur.clear();
        } else if c == '&' {
            let next = bytes.get(i + 1).copied();
            if next == Some('&') {
                out.push(cur.trim().to_string());
                cur.clear();
                i += 1;
            } else {
                out.push(cur.trim().to_string());
                cur.clear();
            }
        } else {
            cur.push(c);
        }
        i += 1;
    }
    out.push(cur.trim().to_string());
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

fn safe_segment(seg: &str) -> bool {
    let mut words = seg.split_whitespace();
    let mut first = match words.next() {
        Some(w) => w.trim_matches(|c| c == '(' || c == ')'),
        None => return true,
    };
    // Skip `VAR=x` prefixes.
    while first.contains('=') && !first.starts_with('-') {
        first = match words.next() {
            Some(w) => w,
            None => return true,
        };
    }
    if !READONLY.contains(&first) {
        return false;
    }
    let rest: Vec<&str> = words.collect();
    match first {
        "systemctl" => rest
            .first()
            .map(|a| matches!(*a, "status" | "show" | "list-units" | "list-unit-files"
                | "is-active" | "is-enabled" | "cat" | "help" | "--version"))
            .unwrap_or(false),
        "journalctl" => !rest.iter().any(|a| a.starts_with("--vacuum") || *a == "--rotate" || *a == "--flush"),
        "git" => rest
            .first()
            .map(|a| matches!(*a, "status" | "log" | "diff" | "show" | "branch" | "remote"
                | "rev-parse" | "ls-files" | "blame" | "describe" | "symbolic-ref"))
            .unwrap_or(false),
        "sed" => !rest.iter().any(|a| *a == "-i" || a.starts_with("-i")),
        "curl" | "wget" => !rest.iter().any(|a| a.starts_with("-X") || a.starts_with("--data")
            || a.starts_with("-d") || a.starts_with("-F") || a.starts_with("-T")
            || (a.starts_with("-O") && path_is_system(a))),
        "find" => !rest.iter().any(|a| *a == "-delete" || a.starts_with("-exec")),
        "kill" | "pkill" | "killall" => false,
        _ => true,
    }
}

/// True when every output redirection targets a non-system path.
fn redirects_ok(cmd: &str) -> bool {
    let mut rest = cmd;
    while let Some(pos) = rest.find('>') {
        let mut tail = &rest[pos + 1..];
        if tail.starts_with('>') {
            tail = &tail[1..];
        }
        let target = tail.trim_start().split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            return false;
        }
        if path_is_system(target) {
            return false;
        }
        if target.starts_with('/') && target != "/dev/null" {
            // Absolute writes outside the workspace are judged, not trusted.
            return false;
        }
        rest = tail;
    }
    true
}

/// Classify a shell command line.
pub fn classify_command(cmd: &str) -> Class {
    if cmd.trim().is_empty() {
        return Class::Unknown;
    }
    if regex_hit(DANGER, cmd) {
        return Class::Danger;
    }
    // Command substitution hides arbitrary commands from this classifier.
    if cmd.contains("$(") || cmd.contains('`') {
        return Class::Unknown;
    }
    if !redirects_ok(cmd) {
        return Class::Unknown;
    }
    let segs = segments(cmd);
    if segs.is_empty() {
        return Class::Unknown;
    }
    if segs.iter().all(|s| safe_segment(s)) {
        Class::Safe
    } else {
        Class::Unknown
    }
}

/// Classify one tool call.
pub fn classify(name: &str, args: &J) -> Class {
    match name {
        "read_file" | "list_dir" | "glob" | "grep" | "search" | "fetch" | "ocr" | "skill" | "speak"
        | "memory_read" | "memory_search" | "todo_read" | "todo_write"
        | "artifact" | "rename" | "get_goal" | "create_goal" | "update_goal"
        | "job_list" | "job_output" | "job_wait" | "job_kill"
        | "watcher_list" | "watcher_stop" => Class::Safe,
        // Creator mode: app_save/app_file can only write inside the app root
        // (the tools validate every path), and app_run runs a package the user
        // explicitly asked dst to build. Registered only in creator mode.
        "app_save" | "app_file" | "app_export" | "app_import" | "app_run" | "app_test"
        | "skill_save" | "skill_file" | "skill_rollback" => Class::Safe,
        "edit_file" => {
            let path = crate::tool::str_arg(args, "path").unwrap_or_default();
            if path_is_system(&path) { Class::Danger }
            else if workspace_path(&path) { Class::Safe }
            else { Class::Unknown }
        }
        "memory_write" => {
            let scope = crate::tool::str_arg(args, "scope").unwrap_or_else(|| "project".into());
            if scope == "project" { Class::Safe } else { Class::Unknown }
        }
        "write_file" => {
            let path = crate::tool::str_arg(args, "path").unwrap_or_default();
            if path_is_system(&path) {
                Class::Danger
            } else if workspace_path(&path) {
                Class::Safe
            } else {
                Class::Unknown
            }
        }
        "bash" => classify_command(&crate::tool::str_arg(args, "command").unwrap_or_default()),
        "job_start" | "watcher_start" => {
            classify_command(&crate::tool::str_arg(args, "command").unwrap_or_default())
        }
        _ => Class::Unknown,
    }
}

fn cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Lexically normalise a path and decide whether it stays inside the current
/// workspace (default working directory).
fn workspace_path(path: &str) -> bool {
    use std::path::{Component, Path, PathBuf};
    if path.trim().is_empty() {
        return false;
    }
    let base = cwd();
    let candidate = Path::new(path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
    let mut norm = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    norm.starts_with(&base)
}

/// Human-readable approval detail: show the meaningful fields, not raw JSON.
fn human_detail(name: &str, args: &J) -> String {
    let arg = |k: &str| crate::tool::str_arg(args, k).unwrap_or_default();
    match name {
        "bash" | "job_start" | "watcher_start" => format!("$ {}", arg("command")),
        "edit_file" => {
            let old = arg("old_string");
            let new = arg("new_string");
            let old_line = old.lines().next().unwrap_or("");
            let new_line = new.lines().next().unwrap_or("");
            format!(
                "path: {}\n- {}\n+ {}",
                arg("path"),
                old_line.chars().take(46).collect::<String>(),
                new_line.chars().take(46).collect::<String>()
            )
        }
        "write_file" => {
            let content = arg("content");
            let head: Vec<&str> = content.lines().take(3).collect();
            format!("path: {}\n{}", arg("path"), head.join("\n"))
        }
        "memory_write" => {
            format!("{} memory\n{}", arg("scope"), arg("content").chars().take(160).collect::<String>())
        }
        _ => {
            let dump = args.dump();
            dump.chars().take(300).collect()
        }
    }
}

/// Authorise one call. `Err(reason)` becomes a failed tool result.
pub fn authorize(
    name: &str,
    args: &J,
    settings: &Settings,
    api_cfg: &Cfg,
    confirmer: &mut dyn Confirmer,
) -> Result<(), String> {
    let class = classify(name, args);
    if class == Class::Safe || settings.mode == Mode::Full {
        return Ok(());
    }
    let pretty = human_detail(name, args);
    match settings.mode {
        Mode::Full => Ok(()),
        Mode::Auto => {
            if class == Class::Danger {
                return Err(format!(
                    "blocked by sandbox: system-configuration change ({name}); \
use full mode or ask the user"
                ));
            }
            match judge(api_cfg, settings, name, &pretty) {
                Ok(true) => Ok(()),
                Ok(false) => Err("rejected by the auto permission judge".to_string()),
                Err(e) => Err(format!("permission judge unavailable ({e}); fail closed")),
            }
        }
        Mode::Strict => {
            let reason = if class == Class::Danger {
                crate::i18n::t("sandbox_reason_danger")
            } else {
                crate::i18n::t("sandbox_reason_unsafe")
            };
            let req = Approval {
                tool: name.to_string(),
                args: pretty,
                reason: reason.to_string(),
            };
            if confirmer.confirm(&req) {
                Ok(())
            } else {
                Err("rejected by the user".to_string())
            }
        }
    }
}

/// The short LLM decision used by auto mode. One line answer, tiny budget.
fn judge(api_cfg: &Cfg, settings: &Settings, name: &str, args: &str) -> Result<bool, String> {
    let mut cfg = api_cfg.clone();
    if !settings.judge_model.is_empty() {
        cfg.model = settings.judge_model.clone();
    }
    cfg.max_tokens = settings.judge_max_tokens;
    let cwd = cwd().to_string_lossy().to_string();
    let system = "You are a permission judge for a coding agent on a handheld Linux device. \
Answer in one short line: either \"SAFE: <reason>\" or \"UNSAFE: <reason>\". \
SAFE means read-only inspection, fetching/searching the web, harness-internal tools, or \
writing inside the current workspace. UNSAFE means changing system configuration, installing \
software, controlling services, deleting outside the workspace, touching devices, or anything \
irreversible. Be strict but do not block ordinary development work inside the workspace.";
    let user = format!(
        "Workspace (cwd): {cwd}\nTool: {name}\nArguments: {args}\n\
Decide whether this tool call is safe to run without human confirmation."
    );
    let messages = J::Arr(vec![
        crate::harness::msg("system", system),
        crate::harness::msg("user", &user),
    ]);
    let tools = J::Arr(Vec::new());
    let out = api::chat_stream(&cfg, &messages, &tools, |_| {})?;
    let text = out.text.trim().to_ascii_uppercase();
    let reason = out
        .text
        .trim()
        .split_once(':')
        .map(|(_, r)| r.trim().to_string())
        .unwrap_or_else(|| out.text.trim().chars().take(80).collect());
    if text.contains("UNSAFE") || text.contains("DENY") {
        Err(format!("permission judge: {reason}"))
    } else if text.contains("SAFE") || text.contains("ALLOW") {
        Ok(true)
    } else {
        Err("permission judge returned an unusable answer".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &str) -> J {
        J::o(vec![("command", J::s(cmd))])
    }

    #[test]
    fn readonly_commands_are_safe() {
        for c in ["ls -la", "cat /etc/hostname", "grep -r foo src", "find . -name '*.rs'",
                  "ps aux | grep dst", "git status", "journalctl -n 20",
                  "systemctl status cpzterm", "ip -4 addr", "curl https://example.com"] {
            assert_eq!(classify_command(c), Class::Safe, "expected safe: {c}");
        }
    }

    #[test]
    fn system_changes_are_danger() {
        for c in ["sudo ls", "systemctl restart cpzterm", "apt-get install curl",
                  "dd if=/dev/zero of=/dev/sda", "> /etc/hosts", "rm -rf /etc",
                  "mount /dev/sda1 /mnt", "reboot", "kill 1",
                  "chmod 777 /etc/passwd", "echo x > /etc/motd"] {
            assert_eq!(classify_command(c), Class::Danger, "expected danger: {c}");
        }
    }

    #[test]
    fn unknown_commands_are_judged() {
        for c in ["./deploy.sh", "systemctl freeze foo", "sed -i s/a/b/ f", "echo $(cat /etc/shadow)"] {
            assert_eq!(classify_command(c), Class::Unknown, "expected unknown: {c}");
        }
    }

    #[test]
    fn creator_app_tools_are_safe() {
        // app_save/app_file validate every path inside the app root, and
        // app_run is the explicit creator-mode action; none should trigger an
        // extra judge request.
        let args = crate::json::parse(r#"{"name":"x","path":"run.sh","content":"y"}"#).unwrap();
        assert_eq!(classify("app_save", &args), Class::Safe);
        assert_eq!(classify("app_file", &args), Class::Safe);
        assert_eq!(classify("app_run", &args), Class::Safe);
    }

    #[test]
    fn redirection_inside_workspace_is_safe() {
        assert_eq!(classify_command("echo hi > out.txt"), Class::Safe);
        assert_eq!(classify_command("echo hi > /tmp/out.txt"), Class::Unknown);
        assert_eq!(classify_command("echo hi > /etc/motd"), Class::Danger);
    }

    #[test]
    fn write_file_outside_workspace_is_not_safe() {
        let a = J::o(vec![("path", J::s("/etc/passwd"))]);
        assert_eq!(classify("write_file", &a), Class::Danger);
        let b = J::o(vec![("path", J::s("out.txt"))]);
        assert_eq!(classify("write_file", &b), Class::Safe);
        let c = J::o(vec![("path", J::s("../escape.txt"))]);
        assert_eq!(classify("write_file", &c), Class::Unknown);
    }

    #[test]
    fn modes_cycle() {
        assert_eq!(Mode::Auto.next(), Mode::Strict);
        assert_eq!(Mode::Strict.next(), Mode::Full);
        assert_eq!(Mode::Full.next(), Mode::Auto);
    }
}
