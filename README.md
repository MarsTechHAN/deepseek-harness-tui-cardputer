# dst — a DeepSeek harness TUI for the M5Stack Cardputer Zero

A single-binary, zero-dependency Rust agent harness + terminal UI built for a
320×170 handheld: 53 columns × 12 rows of framebuffer terminal.

It speaks the OpenAI-compatible DeepSeek API, and its turn/step driver and tool
pipeline follow the DeepSeek Harness (DSH) architecture: a turn owns steps, a
step is one model request plus its tool batch, and every call traverses
`tool/call → pre-execute → execute → post-execute → tool/result` with
`additionalContexts` injected after the batch. Tool `execute()` returns a
canonical JSON value and a separate `render()` projects it to model-facing text.

## Features

- **TUI**: markdown-aware output (headings, bullets, numbered lists, dimmed code
  blocks), word-aware wrapping, live "thinking" indicator that collapses to one
  dim line, one-row tool calls with durations, UTF-8 input, type-ahead queue
  while a turn runs, `Esc` cancels the running turn.
- **Artifacts**: the model can take over the whole screen with a composed panel
  (tables, code, checklists); `Esc` dismisses it, `PgUp`/`PgDn` scroll.
- **Voice**: push-to-talk ASR (hold `Tab`) via `POST /v1/tools/asr`, and TTS via
  `POST /v1/tools/tts`. Audio capture/playback shells out to `arecord`/`aplay`;
  resampling (48 kHz S32 stereo ↔ 16 kHz s16 mono) and WAV framing are done in
  Rust — no Python, no ffmpeg.
- **Tools**: `bash`, `read_file`, `write_file`, `list_dir`, `speak`, `cron`,
  `watch`, `search`, `fetch`, `ocr`, `artifact`.
- **Background**: cron and file-watch jobs run prompts on the same harness.

## Build

Zero crates, so no network needed for the build:

```sh
cargo build --release        # on the device (aarch64) or any host with rustc
install -m755 target/release/dst /usr/local/bin/dst
```

Release profile is tuned for size/startup: `opt-level="z"`, LTO, `panic=abort`,
`strip`.

## Runtime requirements

| need | package | used by |
|---|---|---|
| `curl` | curl | all HTTP (no TLS crate) |
| `arecord`, `aplay` | alsa-utils | ASR capture, TTS playback |
| API key file | — | `~/.config/dsh-lite/api_key` (DeepSeek key, `chmod 600`) |

Non-interactive use:

```sh
dst --once "<prompt>"            # one turn, prints events
dst --voice-selftest [seconds]   # record, report RMS and transcript
```

## Configuration

`Cfg` (base URL, key, model, max tokens) is read at startup from
`~/.config/dsh-lite/api_key` and environment defaults; see `src/main.rs::read_cfg`.
HTTPS is enforced — non-TLS endpoints are refused.

## Layout

```
src/main.rs      TUI: framebuffer terminal, input, panels, voice trigger
src/harness.rs   turn/step driver (DSH agent-loop port)
src/tool.rs      tool contracts + execution pipeline
src/tools.rs     tool registry (bash/files/voice/jobs/web/ocr/artifact)
src/api.rs       streaming chat client over curl subprocess
src/json.rs      minimal JSON parse/serialize
src/jobs.rs      cron + file-watch scheduler
src/voice.rs     DeepSeek TTS/ASR, resampling, WAV, recorder
```

## Notes

- Built for and tested on a Cardputer Zero (Raspberry Pi CM0, 512 MB, Debian 13,
  fbterm framebuffer terminal at 53×12 cells). The terminal wrap width is pinned
  to 53 cells and the UI clamps to a 13-row panel.
- Keyboard symbols/`sym` layer are a device-side concern, not part of this
  binary; see the companion notes about the vendor keymap.
