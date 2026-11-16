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
    Es,
    Ja,
    De,
    It,
}

static LANG: RwLock<Lang> = RwLock::new(Lang::En);

pub fn set(language: &str) {
    let code = language.trim().to_ascii_lowercase();
    let l = if code.starts_with("zh") {
        Lang::Zh
    } else if code.starts_with("es") {
        Lang::Es
    } else if code.starts_with("ja") {
        Lang::Ja
    } else if code.starts_with("de") {
        Lang::De
    } else if code.starts_with("it") {
        Lang::It
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
        Lang::Es => "es",
        Lang::Ja => "ja",
        Lang::De => "de",
        Lang::It => "it",
    }
}

/// Language codes in the order `/config` cycles them.
pub const CODES: &[&str] = &["en", "zh", "es", "ja", "de", "it"];

const EN: &[(&str, &str)] = &[
    ("tagline", "dst | a DeepSeek harness on a Cardputer Zero"),
    ("hints", "{} | hold Tab to talk | /help | /quit"),
    ("unconfigured", "No API configured - type /login to set up"),
    ("busy", "Enter queues \u{b7} Shift+Enter steers \u{b7} Esc interrupts"),
    ("queued", "{} queued"),
    ("force_sent", "\u{26a1} interrupted, sending now: {}"),
    ("voice_queued", "\u{1f3a4} recording queued (starts after this turn)"),
    ("scrolled", "\u{2191}{} lines \u{b7} End"),
    ("onboard_welcome", "Welcome to dst"),
    ("onboard_choose", "No API configured yet. Choose how to connect:"),
    ("onboard_opt1", "DeepSeek official (recommended) - scan a QR code"),
    ("onboard_opt2", "Custom / self-hosted OpenAI-compatible API"),
    ("onboard_prompt", "(select with arrows)"),
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
    ("cmd_resume", "pick a previous session from a menu"),
    ("cmd_rename", "name the current saved session"),
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
    ("menu_sandbox", "Sandbox mode"),
    ("jobs_title", "Background tasks"),
    ("cmd_rewind", "restore a checkpoint from an earlier turn"),
    ("cmd_memory", "show project and global memory files"),
    ("cmd_extensions", "inspect skills and MCP servers"),
    ("rewind_title", "Rewind to checkpoint"),
    ("rewind_hint", "\u{2191}\u{2193} select \u{b7} Enter restore \u{b7} Esc close"),
    ("rewind_empty", "No checkpoints yet (created before each turn)"),
    ("memory_title", "Memory files"),
    ("ext_title", "Skills & MCP"),
    ("ext_hint", "\u{2191}\u{2193} select \u{b7} Enter details \u{b7} Esc close"),
    ("menu_extensions", "Skills & MCP"),
    ("jobs_hint", "\u{2191}\u{2193} select \u{b7} Enter output \u{b7} x kill \u{b7} Esc close"),
    ("jobs_none", "No background jobs or watchers"),
    ("menu_save", "Save & exit"),
    ("menu_hint", "\u{2191}\u{2193} move \u{b7} Enter change \u{b7} Esc exit"),
    ("menu_hint_gated", "third-party API: search/fetch/ocr/voice unavailable \u{b7} \u{2191}\u{2193} Enter Esc"),
    ("menu_model_hint", "\u{2191}\u{2193} select \u{b7} Enter confirm \u{b7} Esc back"),
    ("resume_title", "Resume session"),
    ("resume_hint", "type to search \u{b7} \u{2191}\u{2193} select \u{b7} \u{2192}/Tab expand \u{b7} Enter resume \u{b7} Esc clear"),
    ("resume_no_match", "No session names match \u{b7} Backspace clears the search"),
    ("resume_empty", "No saved sessions yet"),
    ("model_none", "This endpoint advertises no models"),
    ("onboard_hint", "\u{2191}\u{2193} select \u{b7} Enter confirm \u{b7} Esc cancel"),
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
    ("onboard_opt1", "DeepSeek 官方（推荐）— 手机扫码"),
    ("onboard_opt2", "自有 / 自建 OpenAI 兼容 API"),
    ("onboard_prompt", "（方向键选择）"),
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
    ("cmd_resume", "菜单式选择历史会话"),
    ("cmd_rename", "给当前会话起名（/resume 显示的名字）"),
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
    ("menu_sandbox", "沙箱权限模式"),
    ("jobs_title", "后台任务"),
    ("cmd_rewind", "回到之前某一轮的检查点"),
    ("cmd_memory", "查看项目/全局记忆文件"),
    ("cmd_extensions", "查看技能与 MCP 服务器"),
    ("rewind_title", "回退到检查点"),
    ("rewind_hint", "\u{2191}\u{2193} 选择 \u{b7} Enter 恢复 \u{b7} Esc 关闭"),
    ("rewind_empty", "还没有检查点（每轮开始前创建）"),
    ("memory_title", "记忆文件"),
    ("ext_title", "技能与 MCP"),
    ("ext_hint", "\u{2191}\u{2193} 选择 \u{b7} Enter 详情 \u{b7} Esc 关闭"),
    ("menu_extensions", "技能与 MCP"),
    ("jobs_hint", "\u{2191}\u{2193} 选择 \u{b7} Enter 输出 \u{b7} x 结束 \u{b7} Esc 关闭"),
    ("jobs_none", "没有后台任务或监视器"),
    ("menu_save", "保存并退出"),
    ("menu_hint", "\u{2191}\u{2193} 移动 \u{b7} Enter 修改 \u{b7} Esc 退出"),
    ("menu_hint_gated", "第三方 API：search/fetch/ocr/语音不可用 \u{b7} \u{2191}\u{2193} Enter Esc"),
    ("menu_model_hint", "\u{2191}\u{2193} 选择 \u{b7} Enter 确认 \u{b7} Esc 返回"),
    ("resume_title", "恢复会话"),
    ("resume_hint", "输入搜索 \u{b7} \u{2191}\u{2193} 选择 \u{b7} \u{2192}/Tab 展开 \u{b7} Enter 恢复 \u{b7} Esc 清除"),
    ("resume_no_match", "没有匹配的会话名 \u{b7} Backspace 清除搜索"),
    ("resume_empty", "还没有保存的会话"),
    ("model_none", "该 endpoint 没有提供模型列表"),
    ("onboard_hint", "\u{2191}\u{2193} 选择 \u{b7} Enter 确认 \u{b7} Esc 取消"),
    ("menu_endpoint_hint", "Enter 选择 \u{b7} Esc 返回"),
    ("menu_official", "DeepSeek 官方"),
    ("menu_custom", "自定义 / 第三方"),
    ("menu_gate_warn", "警告：第三方 API 不支持 search/fetch/ocr，也不支持语音（ASR/TTS）。"),
    ("menu_gate_ask", "选中期间这些工具会被移除。继续？(y/N)"),
];
const ES: &[(&str, &str)] = &[
    ("tagline", "dst | un harness DeepSeek en un Cardputer Zero"),
    ("hints", "{} | mantén Tab para hablar | /help | /quit"),
    ("unconfigured", "API no configurada: escribe /login"),
    ("busy", "Enter encola · Shift+Enter interrumpe · Esc aborta"),
    ("queued", "{} en cola"),
    ("force_sent", "⚡ interrumpido, enviando: {}"),
    ("voice_queued", "🎤 grabación en cola"),
    ("scrolled", "↑{} líneas · End"),
    ("on", "sí"),
    ("off", "no"),
    ("set", "configurada"),
    ("unset", "sin configurar"),
    ("queued_mode", "cola"),
    ("immediate_mode", "inmediato"),
    ("onboard_choose", "Elige cómo conectar:"),
    ("onboard_opt1", "DeepSeek official (recomendado) - código QR"),
    ("onboard_opt2", "API compatible OpenAI propia"),
    ("onboard_saved", "Guardado en ~/.dst/config.yml"),
    ("cmd_help_title", "Comandos"),
    ("cmd_unknown", "Comando desconocido /{}"),
    ("try_help", "prueba /help"),
    ("usage_title", "Uso"),
    ("usage_cache_rate", "tasa de caché"),
    ("usage_output", "salida"),
    ("usage_context", "contexto {} tokens"),
    ("menu_title", "Configuración"),
    ("menu_model", "Modelo"),
    ("menu_effort", "Esfuerzo"),
    ("menu_endpoint", "Endpoint"),
    ("menu_key", "API key"),
    ("menu_voice", "Voz TTS"),
    ("menu_language", "Idioma"),
    ("menu_steps", "Pasos máx"),
    ("menu_compact", "Compactar auto"),
    ("menu_goalrounds", "Rondas de objetivo"),
    ("menu_sandbox", "Modo sandbox"),
    ("menu_save", "Guardar y salir"),
    ("resume_title", "Reanudar sesión"),
    ("resume_hint", "escribe para buscar · ↑↓ elegir · →/Tab expandir · Enter reanudar · Esc borrar"),
    ("resume_empty", "No hay sesiones guardadas"),
    ("resume_no_match", "Ningún nombre coincide · Backspace borra"),
    ("cmd_resume", "elegir una sesión anterior"),
    ("cmd_rename", "nombrar la sesión actual"),
    ("cmd_compact", "compactar el historial ahora"),
    ("cmd_goal", "gestionar el objetivo"),
    ("cmd_config", "configuración interactiva"),
];

const JA: &[(&str, &str)] = &[
    ("tagline", "dst | Cardputer Zero 上の DeepSeek harness"),
    ("hints", "{} | Tab 長押しで音声 | /help | /quit"),
    ("unconfigured", "API 未設定: /login で設定"),
    ("busy", "Enter キュー · Shift+Enter 中断 · Esc 中止"),
    ("queued", "{} 件キュー"),
    ("force_sent", "⚡ 中断して送信: {}"),
    ("voice_queued", "🎤 録音をキュー"),
    ("scrolled", "↑{} 行 · End"),
    ("on", "オン"),
    ("off", "オフ"),
    ("set", "設定済み"),
    ("unset", "未設定"),
    ("queued_mode", "キュー"),
    ("immediate_mode", "即時"),
    ("onboard_choose", "接続方法を選択:"),
    ("onboard_opt1", "DeepSeek 公式（推奨）— QR コード"),
    ("onboard_opt2", "独自の OpenAI 互換 API"),
    ("onboard_saved", "~/.dst/config.yml に保存"),
    ("cmd_help_title", "コマンド"),
    ("cmd_unknown", "不明なコマンド /{}"),
    ("try_help", "/help を見てください"),
    ("usage_title", "使用量"),
    ("usage_cache_rate", "キャッシュ率"),
    ("usage_output", "出力"),
    ("usage_context", "コンテキスト {} tokens"),
    ("menu_title", "設定"),
    ("menu_model", "モデル"),
    ("menu_effort", "推論強度"),
    ("menu_endpoint", "エンドポイント"),
    ("menu_key", "APIキー"),
    ("menu_voice", "TTS 音声"),
    ("menu_language", "言語"),
    ("menu_steps", "最大ステップ"),
    ("menu_compact", "自動圧縮"),
    ("menu_goalrounds", "ゴール回数"),
    ("menu_sandbox", "サンドボックス"),
    ("menu_save", "保存して終了"),
    ("resume_title", "セッション再開"),
    ("resume_hint", "入力で検索 · ↑↓ 選択 · →/Tab 展開 · Enter 再開 · Esc クリア"),
    ("resume_empty", "保存済みセッションなし"),
    ("resume_no_match", "一致なし · Backspace でクリア"),
    ("cmd_resume", "過去セッションを選ぶ"),
    ("cmd_rename", "現在セッションに名前"),
    ("cmd_compact", "今すぐ履歴を圧縮"),
    ("cmd_goal", "ゴール管理"),
    ("cmd_config", "対話設定"),
];

const DE: &[(&str, &str)] = &[
    ("tagline", "dst | ein DeepSeek-Harness auf dem Cardputer Zero"),
    ("hints", "{} | Tab halten zum Sprechen | /help | /quit"),
    ("unconfigured", "Keine API konfiguriert: /login"),
    ("busy", "Enter stellt ein · Shift+Enter unterbricht · Esc bricht ab"),
    ("queued", "{} eingereiht"),
    ("force_sent", "⚡ unterbrochen, sende: {}"),
    ("voice_queued", "🎤 Aufnahme eingereiht"),
    ("scrolled", "↑{} Zeilen · End"),
    ("on", "an"),
    ("off", "aus"),
    ("set", "konfiguriert"),
    ("unset", "nicht gesetzt"),
    ("queued_mode", "Warteschlange"),
    ("immediate_mode", "sofort"),
    ("onboard_choose", "Verbindung wählen:"),
    ("onboard_opt1", "DeepSeek official (empfohlen) - QR-Code"),
    ("onboard_opt2", "Eigene OpenAI-kompatible API"),
    ("onboard_saved", "Gespeichert in ~/.dst/config.yml"),
    ("cmd_help_title", "Befehle"),
    ("cmd_unknown", "Unbekannter Befehl /{}"),
    ("try_help", "versuche /help"),
    ("usage_title", "Nutzung"),
    ("usage_cache_rate", "Cache-Rate"),
    ("usage_output", "Ausgabe"),
    ("usage_context", "Kontext {} tokens"),
    ("menu_title", "Konfiguration"),
    ("menu_model", "Modell"),
    ("menu_effort", "Aufwand"),
    ("menu_endpoint", "Endpunkt"),
    ("menu_key", "API-Schlüssel"),
    ("menu_voice", "TTS-Stimme"),
    ("menu_language", "Sprache"),
    ("menu_steps", "Max. Schritte"),
    ("menu_compact", "Auto-Verdichten"),
    ("menu_goalrounds", "Ziel-Runden"),
    ("menu_sandbox", "Sandbox-Modus"),
    ("menu_save", "Speichern & beenden"),
    ("resume_title", "Sitzung fortsetzen"),
    ("resume_hint", "tippen zum Suchen · ↑↓ wählen · →/Tab Details · Enter fortsetzen · Esc löschen"),
    ("resume_empty", "Keine gespeicherten Sitzungen"),
    ("resume_no_match", "Kein Name passt · Backspace löscht"),
    ("cmd_resume", "frühere Sitzung wählen"),
    ("cmd_rename", "aktuelle Sitzung benennen"),
    ("cmd_compact", "Verlauf jetzt verdichten"),
    ("cmd_goal", "Ziel verwalten"),
    ("cmd_config", "interaktive Konfiguration"),
];

const IT: &[(&str, &str)] = &[
    ("tagline", "dst | un harness DeepSeek su Cardputer Zero"),
    ("hints", "{} | tieni Tab per parlare | /help | /quit"),
    ("unconfigured", "Nessuna API configurata: /login"),
    ("busy", "Enter accoda · Shift+Enter interrompe · Esc annulla"),
    ("queued", "{} in coda"),
    ("force_sent", "⚡ interrotto, invio: {}"),
    ("voice_queued", "🎤 registrazione in coda"),
    ("scrolled", "↑{} righe · End"),
    ("on", "on"),
    ("off", "off"),
    ("set", "configurata"),
    ("unset", "non impostata"),
    ("queued_mode", "coda"),
    ("immediate_mode", "immediato"),
    ("onboard_choose", "Scegli come collegarti:"),
    ("onboard_opt1", "DeepSeek official (consigliato) - codice QR"),
    ("onboard_opt2", "API OpenAI-compatibile propria"),
    ("onboard_saved", "Salvato in ~/.dst/config.yml"),
    ("cmd_help_title", "Comandi"),
    ("cmd_unknown", "Comando sconosciuto /{}"),
    ("try_help", "prova /help"),
    ("usage_title", "Utilizzo"),
    ("usage_cache_rate", "tasso cache"),
    ("usage_output", "output"),
    ("usage_context", "contesto {} tokens"),
    ("menu_title", "Configurazione"),
    ("menu_model", "Modello"),
    ("menu_effort", "Sforzo"),
    ("menu_endpoint", "Endpoint"),
    ("menu_key", "API key"),
    ("menu_voice", "Voce TTS"),
    ("menu_language", "Lingua"),
    ("menu_steps", "Passi max"),
    ("menu_compact", "Compatta auto"),
    ("menu_goalrounds", "Round obiettivo"),
    ("menu_sandbox", "Modalità sandbox"),
    ("menu_save", "Salva ed esci"),
    ("resume_title", "Riprendi sessione"),
    ("resume_hint", "digita per cercare · ↑↓ scegli · →/Tab dettagli · Enter riprendi · Esc cancella"),
    ("resume_empty", "Nessuna sessione salvata"),
    ("resume_no_match", "Nessun nome corrisponde · Backspace cancella"),
    ("cmd_resume", "scegli una sessione precedente"),
    ("cmd_rename", "nomina la sessione corrente"),
    ("cmd_compact", "comprimi subito la cronologia"),
    ("cmd_goal", "gestisci l'obiettivo"),
    ("cmd_config", "configurazione interattiva"),
];


/// Look up one interface string; unknown keys return the key itself.
fn table(l: Lang) -> &'static [(&'static str, &'static str)] {
    match l {
        Lang::En => EN,
        Lang::Zh => ZH,
        Lang::Es => ES,
        Lang::Ja => JA,
        Lang::De => DE,
        Lang::It => IT,
    }
}

pub fn t(key: &str) -> &'static str {
    let l = lang();
    if let Some(v) = table(l).iter().find(|(k, _)| *k == key).map(|(_, v)| *v) {
        return v;
    }
    // Missing translations fall back to English, then to the key itself.
    table(Lang::En)
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

    #[test]
    fn new_languages_switch_and_fallback() {
        for (code, hint) in [
            ("es", "mantén Tab"),
            ("ja", "Tab 長押し"),
            ("de", "Tab halten"),
            ("it", "tieni Tab"),
        ] {
            set(code);
            assert_eq!(self::code(), code);
            assert!(t("hints").contains(hint), "{code} hints: {}", t("hints"));
            // Keys missing from a translation table fall back to English.
            assert!(!t("menu_custom").is_empty(), "{code} fallback missing");
        }
        set("en");
        assert_eq!(t("on"), "on");
    }
}
