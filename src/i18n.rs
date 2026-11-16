//! Minimal i18n: English (default) and Chinese.
//!
//! Language is stored in `~/.dst/config.yml` as `language: en|zh` and can be
//! changed from `/config`. Only user-facing interface text is translated;
//! tool descriptions and the model-facing system prompt stay English so model
//! behaviour is unchanged by the UI language.

use std::sync::RwLock;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

static LANG: RwLock<Lang> = RwLock::new(Lang::En);

pub fn set(language: &str) {
    let l = if language.trim().to_ascii_lowercase().starts_with("zh") {
        Lang::Zh
    } else {
        Lang::En
    };
    if let Ok(mut g) = LANG.write() {
        *g = l;
    }
}

pub fn lang() -> Lang {
    LANG.read().map(|g| *g).unwrap_or(Lang::En)
}

#[allow(dead_code)] // exposed for callers that persist the language code
pub fn code() -> &'static str {
    match lang() {
        Lang::En => "en",
        Lang::Zh => "zh",
    }
}

const EN: &[(&str, &str)] = &[
    ("tagline", "dst | a DeepSeek harness on a Cardputer Zero"),
    ("hints", "{} | hold Tab to talk | /help | /quit"),
    ("unconfigured", "No API configured - type /login to set up"),
    ("busy", "Enter queues \u{b7} Shift+Enter interrupts \u{b7} Esc aborts"),
    ("queued", "{} queued"),
    ("force_sent", "\u{26a1} interrupted, sending now: {}"),
    ("voice_queued", "\u{1f3a4} recording queued (starts after this turn)"),
    ("scrolled", "\u{2191}{} lines \u{b7} End"),
    ("onboard_welcome", "Welcome to dst"),
    ("onboard_choose", "No API configured yet. Choose how to connect:"),
    ("onboard_opt1", "  1) DeepSeek official (recommended) - scan a QR code"),
    ("onboard_opt2", "  2) Custom / self-hosted OpenAI-compatible API"),
    ("onboard_prompt", "Type 1 or 2 then Enter (Enter alone = 1)"),
    ("onboard_base", "Base URL (e.g. https://api.openai.com/v1):"),
    ("onboard_key", "API key (not echoed while typing):"),
    ("onboard_model", "Model name (Enter = deepseek-flash):"),
    ("onboard_saved", "Saved to ~/.dst/config.yml"),
    ("onboard_cancelled", "Login cancelled"),
    ("onboard_timeout", "Login timed out; run /login again later"),
    ("login_url", "Login URL {}"),
    ("login_no_qr", "QR unavailable ({}); open the URL above"),
    ("login_phone", "Scan with your phone and paste the API key; this completes itself"),
    ("login_done", "Logged in, saved to {}"),
    ("wizard_title", "Configuration - type a number, 0 to go back"),
    ("w1", " 1) Model      {}"),
    ("w2", " 2) Effort     {}"),
    ("w3", " 3) Base URL   {}"),
    ("w4", " 4) API key    {}"),
    ("w5", " 5) TTS voice  {}"),
    ("w6", " 6) Auto-send voice  {}"),
    ("w7", " 7) Queue mode       {}"),
    ("w8", " 8) Language   {}"),
    ("w9", " 9) Save and return"),
    ("w_saved", "Saved to ~/.dst/config.yml"),
    ("w_choose", "Enter a number or model name (0 cancels)"),
    ("w_efforts", " 1) auto   2) low   3) high   4) max"),
    ("w_base", " 1) DeepSeek official   2) Custom (enter URL)"),
    ("w_key_prompt", "Paste the API key (not echoed)"),
    ("w_voice", " 1) stella (female)   2) tide (male)"),
    ("w_lang", " 1) English   2) \u{4e2d}\u{6587}"),
    ("on", "on"),
    ("off", "off"),
    ("set", "configured"),
    ("unset", "not set"),
    ("queued_mode", "queue"),
    ("immediate_mode", "immediate"),
    ("cmd_help_title", "commands"),
    ("cmd_unknown", "unknown command /{}"),
    ("try_help", "try /help"),
    ("usage_title", "usage"),
    ("usage_cache_rate", "cache hit rate"),
    ("usage_cache_read", "cache read"),
    ("usage_uncached", "uncached input"),
    ("usage_output", "output"),
    ("usage_input_total", "input total"),
    ("usage_context", "context  {} tokens (last request)"),
    ("effort_values", "values: auto | low | high | max"),
    ("cmd_help", "list commands"),
    ("cmd_model", "show or set the model"),
    ("cmd_effort", "show or set reasoning effort"),
    ("cmd_login", "configure the API key (QR code)"),
    ("cmd_key", "set the API key directly"),
    ("cmd_usage", "token usage, cache hit rate and context pressure"),
    ("cmd_resume", "list or resume a previous session"),
    ("cmd_config", "interactive configuration menu"),
    ("cmd_clear", "clear the transcript"),
    ("cmd_compact", "summarise older history now"),
    ("cmd_goal", "show or manage the session goal"),
    ("cmd_quit", "exit dst"),
    ("menu_title", "dst configuration"),
    ("menu_model", "Model"),
    ("menu_effort", "Reasoning effort"),
    ("menu_endpoint", "API endpoint"),
    ("menu_key", "API key"),
    ("menu_voice", "TTS voice"),
    ("menu_autosend", "Auto-send voice"),
    ("menu_queue", "Queue while running"),
    ("menu_language", "Language"),
    ("menu_steps", "Max steps/turn"),
    ("menu_compact", "Auto compact"),
    ("menu_goalrounds", "Goal rounds"),
    ("menu_save", "Save & exit"),
    ("menu_hint", "\u{2191}\u{2193} move \u{b7} Enter change \u{b7} Esc exit"),
    ("menu_hint_gated", "third-party API: search/fetch/ocr/voice unavailable \u{b7} \u{2191}\u{2193} Enter Esc"),
    ("menu_model_hint", "\u{2191}\u{2193} select \u{b7} Enter confirm \u{b7} Esc back"),
    ("menu_endpoint_hint", "Enter select \u{b7} Esc back"),
    ("menu_official", "DeepSeek official"),
    ("menu_custom", "Custom / third-party"),
    ("menu_gate_warn", "WARNING: a third-party endpoint has no search/fetch/ocr and no speech (ASR/TTS)."),
    ("menu_gate_ask", "Those tools are removed while it is selected. Continue? (y/N)"),
];

const ZH: &[(&str, &str)] = &[
    ("tagline", "dst | 运行在 Cardputer Zero 上的 DeepSeek harness"),
    ("hints", "{} | 长按 Tab 说话 | /help | /quit"),
    ("unconfigured", "还没有配置 API，输入 /login 开始设置"),
    ("busy", "Enter 排队 \u{b7} Shift+Enter 立即打断 \u{b7} Esc 中止"),
    ("queued", "{} 条排队"),
    ("force_sent", "\u{26a1} 已打断，立即发送: {}"),
    ("voice_queued", "\u{1f3a4} 已排队录音（本轮结束后开始）"),
    ("scrolled", "\u{2191}{} 行 \u{b7} End 回底"),
    ("onboard_welcome", "欢迎使用 dst"),
    ("onboard_choose", "还没有配置 API，请选择接入方式："),
    ("onboard_opt1", "  1) DeepSeek 官方（推荐）— 手机扫码"),
    ("onboard_opt2", "  2) 自有 / 自建 OpenAI 兼容 API"),
    ("onboard_prompt", "输入 1 或 2 后回车（直接回车 = 1）"),
    ("onboard_base", "Base URL（例如 https://api.openai.com/v1）:"),
    ("onboard_key", "API key（输入时不回显）:"),
    ("onboard_model", "模型名（回车 = deepseek-flash）:"),
    ("onboard_saved", "已保存到 ~/.dst/config.yml"),
    ("onboard_cancelled", "已取消登录"),
    ("onboard_timeout", "登录超时，稍后可再输入 /login"),
    ("login_url", "登录地址 {}"),
    ("login_no_qr", "二维码不可用（{}），请手动打开上面的地址"),
    ("login_phone", "用手机扫码并粘贴 API key，这里会自动完成"),
    ("login_done", "已登录，保存到 {}"),
    ("wizard_title", "配置 — 输入编号，0 返回"),
    ("w1", " 1) 模型       {}"),
    ("w2", " 2) 推理强度   {}"),
    ("w3", " 3) 接口地址   {}"),
    ("w4", " 4) API key    {}"),
    ("w5", " 5) TTS 音色   {}"),
    ("w6", " 6) 语音自动发送  {}"),
    ("w7", " 7) 消息排队模式  {}"),
    ("w8", " 8) 语言       {}"),
    ("w9", " 9) 保存并返回"),
    ("w_saved", "已保存到 ~/.dst/config.yml"),
    ("w_choose", "输入编号或模型名（0 取消）"),
    ("w_efforts", " 1) auto   2) low   3) high   4) max"),
    ("w_base", " 1) DeepSeek 官方   2) 自定义（输入 URL）"),
    ("w_key_prompt", "粘贴 API key（输入不回显）"),
    ("w_voice", " 1) stella（女声）   2) tide（男声）"),
    ("w_lang", " 1) English   2) \u{4e2d}\u{6587}"),
    ("on", "开"),
    ("off", "关"),
    ("set", "已配置"),
    ("unset", "未配置"),
    ("queued_mode", "排队"),
    ("immediate_mode", "立即"),
    ("cmd_help_title", "命令"),
    ("cmd_unknown", "未知命令 /{}"),
    ("try_help", "试试 /help"),
    ("usage_title", "用量"),
    ("usage_cache_rate", "缓存命中率"),
    ("usage_cache_read", "缓存读取"),
    ("usage_uncached", "未缓存输入"),
    ("usage_output", "输出"),
    ("usage_input_total", "输入合计"),
    ("usage_context", "上下文  {} tokens（上次请求）"),
    ("effort_values", "取值: auto | low | high | max"),
    ("cmd_help", "列出命令"),
    ("cmd_model", "查看或设置模型"),
    ("cmd_effort", "查看或设置推理强度"),
    ("cmd_login", "配置 API key（扫码）"),
    ("cmd_key", "直接设置 API key"),
    ("cmd_usage", "用量统计、缓存命中率与上下文压力"),
    ("cmd_resume", "列出或恢复历史会话"),
    ("cmd_config", "打开配置菜单"),
    ("cmd_clear", "清空对话记录"),
    ("cmd_compact", "立即压缩较早的上下文"),
    ("cmd_goal", "查看/管理目标（长任务）"),
    ("cmd_quit", "退出 dst"),
    ("menu_title", "dst 配置"),
    ("menu_model", "模型"),
    ("menu_effort", "推理强度"),
    ("menu_endpoint", "接口地址"),
    ("menu_key", "API key"),
    ("menu_voice", "TTS 音色"),
    ("menu_autosend", "语音自动发送"),
    ("menu_queue", "运行时排队"),
    ("menu_language", "语言"),
    ("menu_steps", "每轮最大步数"),
    ("menu_compact", "自动压缩"),
    ("menu_goalrounds", "目标轮数"),
    ("menu_save", "保存并退出"),
    ("menu_hint", "\u{2191}\u{2193} 移动 \u{b7} Enter 修改 \u{b7} Esc 退出"),
    ("menu_hint_gated", "第三方 API：search/fetch/ocr/语音不可用 \u{b7} \u{2191}\u{2193} Enter Esc"),
    ("menu_model_hint", "\u{2191}\u{2193} 选择 \u{b7} Enter 确认 \u{b7} Esc 返回"),
    ("menu_endpoint_hint", "Enter 选择 \u{b7} Esc 返回"),
    ("menu_official", "DeepSeek 官方"),
    ("menu_custom", "自定义 / 第三方"),
    ("menu_gate_warn", "警告：第三方 API 不支持 search/fetch/ocr，也不支持语音（ASR/TTS）。"),
    ("menu_gate_ask", "选中期间这些工具会被移除。继续？(y/N)"),
];

/// Look up one interface string; unknown keys return the key itself.
pub fn t(key: &str) -> &'static str {
    let table = match lang() {
        Lang::En => EN,
        Lang::Zh => ZH,
    };
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or("")
}

/// Same, substituting `{}` placeholders in order.
pub fn tf(key: &str, args: &[&str]) -> String {
    let mut out = t(key).to_string();
    for a in args {
        if let Some(i) = out.find("{}") {
            out.replace_range(i..i + 2, a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_default() {
        set("en");
        assert_eq!(t("on"), "on");
        assert!(tf("force_sent", &["hi"]).contains("hi"));
    }

    #[test]
    fn chinese_switch() {
        set("zh");
        assert_eq!(t("on"), "开");
        assert!(tf("queued", &["2"]).contains('2'));
        set("en");
    }
}
