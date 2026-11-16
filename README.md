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

**Animated splash**
- The opening screen is the dsh-TUI pixel whale (`art.rs`, ported from
  `whaleFrames.ts`): 13 unique frames — blink, fins, spout, tail, heart, sleep —
  tone-mapped to `█ ▓ ░` so it reads on an 8-colour framebuffer terminal.
- It animates while idle (any typing pauses it, the first message ends it), and
  `/clear` brings the animated splash back.

**Terminal UI**
- Markdown-aware rendering (headings, bullets, numbered lists, dimmed code
  blocks), word-aware wrapping at the real 53-cell width, UTF-8 input.
- Live **reasoning panel**: the model's thinking text streams into the space
  above the prompt and collapses to one dim line when the answer starts.
- Tool calls are shown when they *start* (before execution), then settle into a
  one-row card with duration and a one-line result preview (`tool/call` before
  `tool/result`, as in DSH).
- Full-screen **artifacts**: the model can take over the display with a panel it
  composes (tables, code, checklists); `Esc` dismisses, `PgUp`/`PgDn` scroll.
- Runtime control: `Esc` aborts the running turn (including a `speak` clip that
  is still playing); typing while busy is **queued** by default and
  **Shift+Enter** interrupts and sends immediately; push-to-talk (`Tab`) during
  a turn is queued too. A queued `/command` is dispatched as a command when the
  turn ends, never sent to the model.

**Human commands** (execute locally, never sent to the model)
- `/help`, `/model [name]` (lists models from `/v1/models`), `/effort
  [auto|low|high|max]`, `/config` (interactive wizard: options everywhere, text
  input only for the API key and a custom base URL), `/login` (full-screen QR code for a
  phone), `/usage`, `/key`, `/resume` (menu picker), `/rename <title>`,
  `/compact`, `/goal`, `/clear`, `/quit`.
- `/goal <objective>` starts a long-running goal; `/goal` shows it, and
  `/goal edit|pause|resume|clear` manages it. `/compact` summarises older
  history immediately with the same method automatic compaction uses.

**Usage and context**
- `/usage` reports cumulative totals in Claude Code's style: cache hit rate,
  cache read, uncached input and output, each with a share bar. Totals persist in
  `~/.dst/usage.json`.
- A small ring (`○ ◔ ◑ ◕ ●` + percentage) on the status line shows how much of the
  model's context window the last request used (denominator read from `/models`).

**Goals** (`src/goal.rs`, ported from DSH `dsh-goal` + `dsh-goal-round-driver`)
- A goal is a persisted objective that keeps the session working across
  automatic continuation rounds. The model judges progress itself through
  `get_goal` / `create_goal` / `update_goal`: it marks the goal `complete` only
  when the objective is achieved, and `blocked` only when one concrete
  condition has persisted for at least 3 rounds.
- Between rounds the driver injects DSH's `<goal_round>` prompt verbatim; the
  status line shows `◎ goal <round>/<cap> <objective>`. `Esc` disarms the goal,
  `/goal resume` re-arms it. After a restart an active goal is disarmed until
  the user resumes it (same activation rule as DSH).
- Round cap: `goal.max_rounds` (DSH default 256; `0` = unlimited). Per-round step cap:
  `goal.max_steps` (`0` = unlimited). Stopping a goal is always possible with
  `/goal pause` or `/goal clear`.

**Context management** (`src/compact.rs`, ported from DSH `dsh-compaction-basic`)
- Automatic compaction runs at `agent/pre-step`, i.e. before the next request is
  built, once request pressure reaches the budget:
  `threshold = min(window × 0.8, window − reserved_output − 64K headroom, compact_threshold)`
  (default absolute cap **512K tokens**, `compaction.threshold_tokens`).
- The algorithm is DSH's: prune oversized old tool-result middles first
  (model-free), keep the newest tail worth 16% of the message budget on a
  user-message boundary, then make **one** streamed summary call that replays
  the conversation's own system prompt and shadowed messages and appends DSH's
  compaction instruction as the final user message (prefix-cache friendly).
  The result is framed as `This is an automatically generated checkpoint…` +
  `<compacted-summary>…</compacted-summary>` in one user message; a summary
  that is not smaller than the region it replaces is rejected.
- A provider-reported context overflow bypasses the threshold and forces one
  compaction, then retries the request once.
- Manual `/compact` skips the threshold and keeps no verbatim tail.
- The ring in the corner shows context pressure, and `/usage` reports usage.

**In-progress context injection** (`src/context.rs`, ported from DSH `dsh-time-context`)
- Before each request the harness may append an attributed clock reading
  (DSH verbatim format: `Time sampled while preparing turn N, step M: …`,
  browser/process time zone, elapsed time). Rate-limited by
  `context.time_refresh_secs` (default 600 s; `0` = every step).
- Repeated identical tool calls inject DSH's `repeat-tool-reminder` text after
  the third and fifth repetition, so the model sees that it is looping.
- Tool results can attach `additionalContexts`; they are injected as a
  `user/message` after the batch's tool results, exactly as DSH orders it.

**Internationalisation**
- UI language via `language: en|zh|es|ja|de|it` in the config (default `en`),
  switchable from `/config`; missing translations fall back to English. Only interface text is translated; tool descriptions and the system
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

**Interaction polish**
- `/help` is a command menu: arrows select, Enter runs a no-argument command or
  completes one that needs input, Esc closes.
- Up/Down on an empty prompt recall input history (60 entries, `/key` never
  recorded); PgUp/PgDn scroll the transcript a page at a time.
- The idle status line shows key hints; running tools report live progress on
  the status line (`bash running 1.2s`).
- Menu right-hand text truncates with `…` instead of silently cutting words.

**No title/header row**: every row belongs to the transcript, status line and
prompt; the session title appears in `/resume`, and goal state is shown as a
bottom-right badge (`◎ Pursuing Goal 2/10`, `⏸ Goal paused`, `✓ Goal done`).

**Background-task manager**: press Down on an empty prompt while jobs or
watchers exist to open a list of tasks (`Job 1`, `Watcher 1`, …) with command,
status and age; Enter shows the output tail, `x` kills the task, Esc closes.

**Menus** are menuconfig-style with full-width rows: labels left-aligned,
values right-aligned at the panel edge.

**Permissions / sandbox**
- Every tool call passes a permission gate. `auto` (default) allows known-safe
  operations (read-only commands, `read_file`/`list_dir`, web reads, writes under
  the current workspace), refuses system-configuration changes (sudo, systemctl
  start/stop, package managers, mounts, writes under /etc /boot /usr, reboot),
  and asks a short LLM call to judge anything else. `strict` shows a full-screen
  allow/reject pop-up for every non-safe call. `full` runs everything.
- Switch mode in `/config` (`sandbox.mode`); rejected calls become failed tool
  results with the concrete reason, so the model can explain instead of retrying.

**Background jobs and watchers**
- `job_start` launches a shell command in the background (multiple at once),
  `job_output` peeks, `job_wait` blocks up to a timeout, `job_kill` stops it.
- `watcher_start` runs a command and delivers every line it prints to the agent
  as a `[watcher <id>]` user message on the next idle tick; `watcher_list` /
  `watcher_stop` manage them. Jobs/watchers are killed when dst exits.

**Sessions and task titles**
- Every turn is saved to `~/.dst/sessions/<timestamp>.json` with a `title`.
- The model keeps that title current through the `rename` tool (the system
  prompt tells it to call `rename` as soon as it understands the task and when
  the task changes); `/rename <title>` lets the human set it too. The title is
  shown in the header and is what `/resume` lists.
- `/resume` is a menuconfig-style picker grouped by directory: sessions from
  the current folder come first, then the rest, each sorted by last talk time
  with a relative age (`now`, `5m`, `3h`, `2d`, `1w`) at the row start. Typing
  filters by session name; Right/Tab expands a preview (title, model, turns,
  last user/reply); Enter resumes; Esc clears the search then exits.

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
`ocr`, `speak`, `skill`, `artifact`, `cron`, `watch`, `get_goal`, `create_goal`,
`update_goal`, plus any MCP tools.

**Timeouts and limits**
- Every tool declares an executor deadline (`ToolDef.timeout_ms`; 60 s default
  for tools that omit one). A call that overruns settles as
  `tool timed out after Ns` without freezing the prompt; `Esc` still works
  while a tool runs.
- There is **no step limit by default** (a turn may use as many model steps as
  it needs, as in DSH). `agent.max_steps` opts into a cap; `0` = unlimited.
  Goal rounds use `goal.max_steps` the same way.

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
agent:
  max_steps: 0          # 0 = unlimited (default)
goal:
  max_rounds: 256       # DSH default; 0 = unlimited
  max_steps: 0          # steps per goal round; 0 = unlimited
  judge: true           # model self-judges via update_goal complete/blocked
compaction:
  enabled: true
  threshold_tokens: 512000   # absolute pressure cap; window ratio/headroom still clamp it
  keep_messages: 2           # minimum trailing messages kept verbatim (token tail is authoritative)
  max_tokens: 8192           # output budget of the summary call
  # model: deepseek-flash    # optional separate summariser model
tools:
  call_timeout_ms: 60000
context:
  time: true
  time_refresh_secs: 600     # 0 = inject the clock on every step

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
dst --once "<prompt>"                    # one turn, prints events
dst --tools                              # effective tool set + capability decision
dst --call <name> '<json args>'          # run one tool exactly as the model would
dst --voice-selftest [seconds]           # record, report RMS and transcript
```

## Testing without the device (`tools/sim`)

The TUI is a pure ANSI terminal program, so it can run under a pty on a
development machine. `tools/sim` contains a screen simulator and a scripted
OpenAI-compatible mock server; no network or API key is involved.

```sh
cargo build                       # host binary

cd tools/sim
python3 run_demo.py               # splash / thinking / tool call / reply + PNG contact sheet
python3 run_scenarios.py          # /clear, /goal round driver, automatic compaction checks
```

`run_demo.py` writes snapshots and a contact sheet under `screenshots/`;
`run_scenarios.py` runs 26 feature scenarios (thinking panel, tool call ordering,
tool timeout, Esc interrupt, step limits, repeat/time context injection,
auto+manual compaction, all `/goal` flows, `/clear`, `/config`, command palette,
artifact panel, `rename`+`/resume` session menu across a restart, onboarding
menu, sandbox auto/strict/full, background jobs, watchers, searchable resume)
and asserts on captured screens *and* on the request log (e.g. that a
`<goal_round>` prompt was sent, or that a checkpoint message reached the next
request). `check_memory.py` samples RSS across a 20-turn session, and
`run_all.py` runs the three together. Everything binds the mock API to
127.0.0.1 only.

Unit tests (host, no network): `cargo test` — harness/tool pipeline, compaction
budgets, goal state machine, context formatting, config round-trip.

HTTPS is enforced: non-TLS endpoints are refused.

## Layout

```
src/main.rs      TUI: framebuffer terminal, input, panels, wizard, resume
src/harness.rs   turn/step driver (DSH agent-loop port)
src/tool.rs      tool contracts + execution pipeline (timeouts, call/result)
src/tools.rs     built-in tool registry
src/compact.rs   automatic/manual context compaction (DSH compaction-basic port)
src/context.rs   in-progress context injection (clock, repeat-call reminder)
src/goal.rs      goals: state machine, tools, round driver prompt (DSH goal port)
src/session.rs   saved-session title: rename tool, /resume menu previews
src/sandbox.rs   tool permission gate: safe rules, danger rules, LLM judge
src/bg.rs        background jobs + line watchers (job_*/watcher_* tools)
src/api.rs       streaming chat client over a curl subprocess
src/config.rs    ~/.dst home, DSH-dialect YAML config
src/cmd.rs       human commands (/model, /compact, /goal, /resume, ...)
src/login.rs     QR login HTTP server + QR rendering
src/skills.rs    skill catalog and loader
src/mcp.rs       MCP stdio client
src/jobs.rs      cron + file-watch scheduler
src/voice.rs     TTS/ASR, resampling, WAV framing, recorder
src/json.rs      minimal JSON
tools/sim/       pty screen simulator + scripted mock API (host testing)
tools/deploy/    scp/build/install and systemd-ordering fix for the device
```

## Deploying to the device

The device boots `cpzterm.service`, which runs `fbterm` + `dst` on the panel.
`tools/deploy/deploy.sh` copies `src/`, builds on the device with
`CARGO_BUILD_JOBS=1` (512 MB is not enough for parallel LTO), installs
`/usr/local/bin/dst`, and restarts the service:

```sh
tools/deploy/deploy.sh hanxiao@192.168.88.234
```

`tools/deploy/fix-cpzterm-order.sh` repairs an early-boot ordering cycle: the
base unit must not declare `Before=sysinit.target` (that combined with the
keymap unit deadlocks boot with `Resource deadlock avoided`). Run it on the
device once; it removes the ordering edge, reloads systemd and verifies with
`systemd-analyze verify`.

## Notes

- Built for the Cardputer Zero (Raspberry Pi CM0, 512 MB, Debian 13, fbterm at
  53×12 cells). Wrap width is pinned to 53 cells and the UI clamps to 13 rows.
- Keyboard layers (`sym`/`fn`) and the vendor keymap are device concerns, not
  part of this binary.
