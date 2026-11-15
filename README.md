# dst — a DeepSeek harness TUI for the M5Stack Cardputer Zero

A single-binary, zero-crate Rust agent harness + terminal UI for a 320×170
handheld (53 columns × 12 rows of framebuffer terminal).

The turn/step driver and tool pipeline follow the DeepSeek Harness (DSH)
architecture: a turn owns steps, a step is one model request plus its tool
batch, and every call traverses
`tool/call → pre-execute → execute → post-execute → tool/result` with
`additionalContexts` injected after the batch. Tool `execute()` returns a
canonical JSON value; a separate `render()` projects it to model-facing text.

No API keys, credentials, or third-party speech services are present in this
repository.

## Features

**Terminal UI**
- Markdown-aware rendering (headings, bullets, numbered lists, dimmed code
  blocks), word-aware wrapping at the real 53-cell width, UTF-8 input.
- Live "thinking" indicator that collapses to one dim line; one-row tool calls
  with durations.
- Full-screen **artifacts**: the model can take over the display with a panel it
  composes (tables, code, checklists); `Esc` dismisses, `PgUp`/`PgDn` scroll.
- Runtime control: `Esc` aborts the running turn; typing while busy is
  **queued** by default and **Shift+Enter** interrupts and sends immediately;
  push-to-talk (`Tab`) during a turn is queued too.

**Human commands** (execute locally, never sent to the model)
- `/help`, `/model [name]` (lists models from `/v1/models`), `/effort
  [auto|low|high|max]`, `/config` (interactive wizard: options everywhere, text
  input only for the API key and a custom base URL), `/login` (full-screen QR code for a
  phone), `/usage`, `/key`, `/resume [n]`, `/clear`, `/quit`.

**Usage and context**
- `/usage` reports cumulative totals in Claude Code's style: cache hit rate,
  cache read, uncached input and output, each with a share bar. Totals persist in
  `~/.dst/usage.json`.
- A small ring (`○ ◔ ◑ ◕ ●` + percentage) on the status line shows how much of the
  model's context window the last request used (denominator read from `/models`).

**Internationalisation**
- UI language via `language: en|zh` in the config (default `en`), switchable from
  `/config`. Only interface text is translated; tool descriptions and the system
  prompt stay English so model behaviour is unchanged.

**Command palette**
- Typing `/` opens an inline palette of every command with its description; Up/Down
  select, Enter runs it (commands that need an argument are completed with a space
  instead). The list scrolls with the selection.

**Configuration screen**
- `/config` is a menuconfig-style screen: arrows move, Enter edits, Esc leaves. It
  edits model, effort, endpoint, API key, TTS voice, auto-send, queue mode and
  language, and warns before switching to a third-party endpoint.

**Endpoint capabilities**
- Speech and the server-side tools (`search`/`fetch`/`ocr`) only exist on DeepSeek's
  official API. On a third-party endpoint those tools are removed from the registry
  and the model is told they are unavailable; the wizard warns first. `dst --tools`
  prints the effective tool set for diagnosis.

**Sessions**
- Every turn is saved to `~/.dst/sessions/<timestamp>.json`; `/resume` lists and
  restores them.

**Skills** (DSH-style progressive disclosure)
- `~/.dst/skills/<name>/SKILL.md` with YAML frontmatter (`name`,
  `description`). Only the catalog enters the prompt; the `skill` tool returns
  the body on demand.

**MCP**
- `~/.dst/mcp.json` (`mcpServers` map, same shape as other MCP clients). Servers
  are spawned over stdio, `initialize` + `tools/list` run at startup, and each
  discovered tool is exposed as `mcp__<server>__<tool>`.

**Voice**
- Push-to-talk ASR (`POST /v1/tools/asr`), TTS (`POST /v1/tools/tts`). Capture
  and playback use `arecord`/`aplay`; resampling and WAV framing are in Rust.
  TTS is only used when the user explicitly asks to be told aloud.

**Tools**: `bash`, `read_file`, `write_file`, `list_dir`, `search`, `fetch`,
`ocr`, `speak`, `skill`, `artifact`, `cron`, `watch`, plus any MCP tools.

## Home layout (`~/.dst`)

```
~/.dst/
  config.yml     all settings, including api.key (mode 600)
  sessions/      resumable conversation logs
  skills/        skill packages (SKILL.md per folder)
  mcp.json       MCP server definitions
  jobs.json      cron / file-watch jobs
  cache/         transient audio and scratch files
  logs/          voice.log, mcp.log
  tmp/           per-run temporary files
```

The first launch with no configuration shows a short guide: choose DeepSeek
official (QR login from a phone) or a custom OpenAI-compatible endpoint.

## Configuration (`~/.dst/config.yml`)

The file is YAML and follows DSH's patch dialect: plain settings plus `insert:`
rows shaped `{ id, name, config, disabled }` (the row fields DSH uses to compose
plugin lists). Here a row targets a tool by `name`.

```yaml
model: deepseek-flash
effort: high            # auto | low | high | max
max_tokens: 4096
api:
  base: https://api.deepseek.com/v1
  key: sk-...           # kept in this same file, mode 600
voice:
  tts_voice: stella     # stella | tide
tui:
  queued_default: true
  auto_send_voice: true
  language: en          # en | zh

# DSH-style patch rows: disable a tool or pass it config
insert:
  - id: speak
    name: speak
    disabled: true
```

`/config` edits the same values interactively, and `/config path` prints the
file location.

## Build

Zero crates, so no network is needed to build:

```sh
# The device only has 512 MB; LTO can be OOM-killed. Build single-threaded there.
CARGO_BUILD_JOBS=1 cargo build --release
install -m755 target/release/dst /usr/local/bin/dst
```

Release profile: `opt-level="z"`, LTO, `panic=abort`, `strip`.

## Runtime requirements

| need | package | used by |
|---|---|---|
| `curl` | curl | all HTTP |
| `arecord`, `aplay` | alsa-utils | ASR capture, TTS playback |
| `qrencode` | qrencode | optional; the QR code for `/login` (falls back to printing the URL) |

Non-interactive entry points:

```sh
dst --once "<prompt>"            # one turn, prints events
dst --voice-selftest [seconds]   # record, report RMS and transcript
```

HTTPS is enforced: non-TLS endpoints are refused.

## Layout

```
src/main.rs      TUI: framebuffer terminal, input, panels, wizard, resume
src/harness.rs   turn/step driver (DSH agent-loop port)
src/tool.rs      tool contracts + execution pipeline
src/tools.rs     built-in tool registry
src/api.rs       streaming chat client over a curl subprocess
src/config.rs    ~/.dst home, DSH-dialect YAML config
src/cmd.rs       human commands (/model, /config, /resume, ...)
src/login.rs     QR login HTTP server + QR rendering
src/skills.rs    skill catalog and loader
src/mcp.rs       MCP stdio client
src/jobs.rs      cron + file-watch scheduler
src/voice.rs     TTS/ASR, resampling, WAV framing, recorder
src/json.rs      minimal JSON
```

## Notes

- Built for the Cardputer Zero (Raspberry Pi CM0, 512 MB, Debian 13, fbterm at
  53×12 cells). Wrap width is pinned to 53 cells and the UI clamps to 13 rows.
- Keyboard layers (`sym`/`fn`) and the vendor keymap are device concerns, not
  part of this binary.
