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
- The opening screen is a 53×9 night-sea scene built from the dsh-TUI pixel whale
  (`art.rs`, tone-mapped to `█ ▓ ░` so it reads on an 8-colour framebuffer): the
  original 9-row whale is a single sprite that is **lifted against a dim water
  line** — calm at the surface, spout, rise, full breach, apex fluke swing,
  fluke/back dive — then spray settles and the dim `dst` wordmark returns on the
  water before the loop restarts. 18 static frames across three colour bands (dim
  night sky, bright air with the whale, dim sea line), driven by a per-frame hold
  table (300–1800 ms, quantised to the 300 ms idle tick) so the scene moves in
  beats instead of a uniform frame rate.
- It animates while idle (typing pauses it on the current frame, the first
  message ends it); after three opening loops it settles into a slow sleeping
  loop, and `/clear` restarts the opening animation. The frames and the schedule
  are deterministic (no randomness), so simulator snapshots are reproducible.
  Frame storyboard and reference analysis: [`docs/SPLASH-STORYBOARD.md`](docs/SPLASH-STORYBOARD.md);
  stills in [`docs/splash-frames.png`](docs/splash-frames.png) and the loop as
  [`docs/splash.gif`](docs/splash.gif) (simulator, device pixel grid).

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
  input only for the API key and a custom base URL), `/login` (real DeepSeek
  Platform sign-in; see below), `/usage`, `/key`, `/resume` (menu picker), `/rename <title>`,
  `/compact`, `/goal`, `/apps`, `/clear`, `/quit`.
- `/goal <objective>` starts a long-running goal; `/goal` shows it, and
  `/goal edit|pause|resume|clear` manages it. `/compact` summarises older
  history immediately with the same method automatic compaction uses.
- `/apps` opens the manager for the apps dst built, where Enter runs one and
  `x` deletes it. Creator mode itself is a row in `/config` (see below).

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
  switchable from `/config`. Every user-visible string is translated — menus,
  help/palette previews, approval dialogs, onboarding, login pages and device
  login screen, jobs/queue/watcher states, thinking/steer/queue/status labels,
  command feedback and tips. A unit test enforces that each language covers the
  full English catalog (missing keys would fall back to English). Only interface text is translated; tool descriptions and the system
  prompt stay English so model behaviour is unchanged.

**Command palette**
- Typing `/` opens an inline palette of every command with its description; Up/Down
  select, Enter runs it (commands that need an argument are completed with a space
  instead). The list scrolls with the selection.

**Creator mode** (`src/apps.rs`, design in [docs/CREATOR.md](docs/CREATOR.md))
- Opt-in mode where dst is the device's only program: everything the user uses
  is something dst built and saved. Off by default; the `Mode` row in
  `/config` switches it (`tui.mode: normal|creator`), and the switch is
  announced in the transcript with a creator idle hint afterwards
  (`creator · 2 apps · /apps · hold Esc`).
- Every build becomes a persistent app package under
  `~/.dst/apps/<category>/<name>/`: `app.json` (manifest), `APP.md` (a short
  annotation written for the user), the entry program, and a `state/` dir for
  runtime data (`$DST_APP_STATE`). Kinds: `sh`, `python`, `binary` (executable
  entry), `prompt` (a reusable request the model follows; PROMPT.md) and `doc`.
  The model creates them with `app_save` / `app_file`, verifies them headlessly
  with `app_test` (piped stdio, captured exit/stdout/stderr, full log under
  `~/.dst/tmp`), and runs them with `app_run` (`args` + `timeout_s`), which
  takes over the panel full-screen and hands it back when the app exits.
  Launches are bounded by the platform: long-Esc/timeout backstop, cwd + state
  dir inside the package, ulimits (no core dumps, CPU time, file size) and a
  scrubbed environment (no credentials inherited; HOME/TMPDIR inside state/).
  Runs and headless tests are recorded back into `app.json` (`last_run` /
  `last_test` with outcome, duration and time), shown in the `/apps` annotation
  panel (a `!` on the list row marks a failing app), and a failed last run/test
  is flagged in the model's app catalog so it fixes the app before reusing it.
  `app_export` packs the package into `~/.dst/tmp/app-*.tar.gz` for backups.
- A skills-style catalog (`- system/battery_viewer — Battery viewer [sh:run.sh]:
  …`, one line per app, capped) is injected into the system prompt each turn, so
  the model knows what already exists and which file to rewrite instead of
  duplicating it;
  `APP.md` is read on demand with `read_file`.
- `/apps` is the user's side: a menu with the app count, `/` search mode that
  filters by name/title/category, `→` opens the full annotation panel (metadata +
  APP.md), Enter runs (doc apps open their panel; prompt apps are previewed
  first and Enter sends), `x` deletes after `y/n`.
  Running from the menu comes back to the menu; leaving it prints the run notice.
- Consensus the platform enforces, not the apps: **long-press Esc always leaves
  an app** and gives the screen back to the chat (the runner watches keyboard
  auto-repeat and kills the app; trailing Esc repeats are swallowed). Model-side
  runs have a 60 s default time limit (`timeout_s`), user-started runs have none.
- Creator mode announces saved apps in the transcript and nudges when a turn
  changed files without saving an app, so work is not lost between sessions.
- Creator mode also writes **skills**: `skill_save` / `skill_file` create a
  systematic, self-contained package under `~/.dst/skills/<name>/` (SKILL.md
  frontmatter + body + support files, marked `origin: model`). It joins the
  SKILLS catalog on the next turn and is loaded with the `skill` tool, exactly
  like hand-installed skills and can be loaded with the `skill` tool in the
  same turn they are written (the catalog updates next turn).
  `/extensions` badges model-written skills, opens
  the full SKILL.md body with `→`, and lets the user disable (`x`) or delete
  (`d`, with y/n) them; `/etc/dst/skills` stays read-only.
  The prompt spells out the split: app = runnable device thing, skill = complete
  reusable procedure, memory = short fragments/preferences.
- Only available on the interactive TUI: `app_run` fails cleanly in `--once`,
  background jobs and `--call`. Writes are confined to the app/skill roots by
  the tools (slug/path validation, per-file and per-package size caps).

**Configuration screen**
- `/config` is a menuconfig-style screen: arrows move, Enter edits, Esc leaves. It
  edits model, effort, endpoint, API key, TTS voice, auto-send, queue mode,
  language, UI mode (normal/creator), step/compaction/goal budgets, sandbox
  mode and the auto-judge model, and warns before switching to a third-party
  endpoint.

**Endpoint capabilities**
- Speech and the server-side tools (`search`/`fetch`/`ocr`) only exist on DeepSeek's
  official API. On a third-party endpoint those tools are removed from the registry
  and the model is told they are unavailable; the wizard warns first. `dst --tools`
  prints the effective tool set for diagnosis.

**Interaction polish**
- The `/` palette footer shows the match position (`1/5 ↑↓ Enter Esc`).
- `/help` and the `/` palette group commands by category (session, goal, core,
  setup, tools, info); `/help` previews usage, the palette keeps only short
  argument hints so descriptions stay readable. Enter runs a no-argument
  command or completes one that needs input, Esc closes.
- Up/Down on an empty prompt recall input history (60 entries persisted in
  `~/.dst/history`, mode 600; `/key` never recorded); PgUp/PgDn scroll the
  transcript a page at a time.
- The idle status line shows key hints; running tools report live progress on
  the status line (`bash running 1.2s`).
- CtrlQ while a turn is running opens the queued-message editor: arrows select,
  `x` deletes, Enter moves the message back into the prompt, Esc closes.
- CtrlO opens a recent tool-output list; Enter shows the full result in the
  scrollable panel viewer, `s` saves it under `~/.dst/tmp/`. Ctrl+A/Ctrl+E move the cursor, Ctrl+W deletes a word;
  Home/End jump to the top/bottom of the scrollback when not editing.
- Menus show a `current/total` position in the title when the list is longer
  than the panel (config 1/16, help 1/16, long resume/task lists).
- Menu right-hand text truncates with `…` instead of silently cutting words;
  `/help`, `/rewind`, `/config` and onboarding show a live description/preview
  for the highlighted row.
- The background-task manager opens the full task output in the scrollable
  panel viewer; the viewer supports `s` to save the panel text.
- Numeric config rows (max steps, compaction threshold, goal rounds) open an
  edit prompt instead of only cycling fixed values.
- Failed requests print an actionable hint (auth → `/key`, rate limit → wait or
  `/model`, network → check Wi-Fi).
- `/resume` previews include the recent exchange, not just a summary line.

**No title/header row**: every row belongs to the transcript, status line and
prompt; the session title appears in `/resume`, and goal state is shown as a
bottom-right badge (`◎ Pursuing Goal 2/10`, `⏸ Goal paused`, `✓ Goal done`).

**Background-task manager**: press Down on an empty prompt while jobs or
watchers exist to open a list of tasks (`Job 1`, `Watcher 1`, …) with command,
status and age; Enter shows the output tail, `f` cycles all/running/watchers, `x` kills the task, Esc closes.

**Menus** are menuconfig-style with full-width rows: labels left-aligned,
values right-aligned at the panel edge.

**Permissions / sandbox**

Onboarding explains the default `auto` sandbox on the endpoint-choice preview,
then offers a first-run picker (`auto` / `strict` / `full`) after the endpoint is
configured; Esc keeps `auto`, and `/config` changes it later.
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
- `watcher_start` runs a command and delivers its lines to the agent as
  `[watcher <id>]` user messages on the next idle tick; a chatty watcher is
  coalesced (last 5 lines + "+N more lines") and auto-turns are rate-limited to
  at most one per 1.2 s. `watcher_list` / `watcher_stop` manage them.
  Jobs/watchers are killed when dst exits.

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
  apps/          creator-mode app packages (<category>/<name>/{app.json,APP.md,…})
  mcp.json       MCP server definitions
  jobs.json      cron / file-watch jobs
  cache/         transient audio and scratch files
  logs/          voice.log, mcp.log
  tmp/           per-run temporary files
```

The first launch with no configuration shows a short guide: choose DeepSeek
official or a custom OpenAI-compatible endpoint, then pick the sandbox mode.

### `/login`: real DeepSeek Platform sign-in (DSH token)

Full guides: [English](docs/LOGIN.md) · [中文](docs/LOGIN.zh.md) ·
[Español](docs/LOGIN.es.md) · [日本語](docs/LOGIN.ja.md) ·
[Deutsch](docs/LOGIN.de.md) · [Italiano](docs/LOGIN.it.md) — each includes
states, requirements and troubleshooting.

`/login` offers three paths, all saving credentials to `~/.dst/config.yml`
(mode 600):

1. **WeChat QR on this screen** — dst runs the same nine-step flow as the DSH
   desktop client (WeChat QR from `open.weixin.qq.com`, callback nonce, web
   session token, PKCE `auth_init`/`auth_approve`/`auth_exchange`) and shows the
   real WeChat QR on the panel: a compact numbered text guide on the left
   (title / steps / page / Esc) and the QR on the right, with a live status
   line that updates to "scanned — confirm on the phone" and "finishing login"
   as the phone proceeds, plus a `QR valid for m:ss` countdown while waiting. An expired QR refreshes automatically once; after a
   second failure the screen offers `r retry · Esc cancel`. The DSH token is stored under `api.dsh_token` and sent to the model
   API as `x-dsh-auth-token` (no Bearer). Requires `python3` + Pillow only to
   convert the WeChat JPEG QR into panel modules; without a framebuffer the same
   QR and live progress are shown on the phone page.
2. **Phone page** — a LAN page with the WeChat QR, live steps, a Cancel button
   that aborts an in-flight flow, a QR expiry countdown, a Retry label after a
   failure, and a manual API-key form (`http://<device-ip>:8787/`). The page
   follows the device UI language (en/zh/es/ja/de/it).
3. **Type directly** — menuconfig-style typing for either a DSH token or an
   OpenAI-compatible base URL + API key + model.

Model requests and voice/OCR pick the auth header automatically: DSH token →
`x-dsh-auth-token`, API key → `Authorization: Bearer`. `/config` shows
`API key / token` as the credential row, shows `DSH token` as the value when one
is stored, and setting an API key (`/key` or the config prompt) replaces the
token so the new key is not shadowed.

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
| `qrencode` | qrencode | optional; QR for the phone-page URL (falls back to printing the URL) |
| `python3` + Pillow | Python 3 + Pillow | optional; converts the WeChat JPEG QR to panel modules (falls back to the phone page) |

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
python3 run_scenarios.py          # feature + voice + login scenario checks
python3 run_live.py               # UI walkthrough against the real local vLLM model
```

`run_demo.py` writes snapshots and a contact sheet under `screenshots/`;
`run_scenarios.py` runs the feature scenarios (thinking panel, tool call
ordering, tool timeout, Esc interrupt, step limits, repeat/time context
injection, auto+manual compaction, all `/goal` flows, `/clear`, `/config`,
command palette, artifact panel, `rename`+`/resume` session menu across a
restart, onboarding menu, sandbox auto/strict/full, background jobs, watchers,
searchable resume, push-to-talk with mocked speech, `speak`, the nine-step
WeChat login chain, provider auth/rate-limit failures and their recovery, long
input scrolling/editing (including CJK), the multi-entry tool-output viewer,
the scrolling artifact panel, and creator mode `creator_mode`, `creator_build`,
`creator_run`, `creator_run_esc`, `creator_run_args`, `creator_apps_run`,
`creator_apps_zh`, `creator_skill`, `creator_app_test`, `creator_kinds`,
`creator_nudge`; app run/test history and skill management are asserted there)
and
asserts on captured screens *and* on the request log
(e.g. that a `<goal_round>` prompt was sent, or that a checkpoint message
reached the next request). `check_memory.py` samples RSS across a 20-turn
session, and `run_all.py` runs them together.

PNG captures are rendered at the panel's native resolution: a 320×170
framebuffer with a 53×12 grid of 6×14 px cells (6×13 glyph box), the public
domain X11 6×13 bitmap font (`tools/sim/fonts/`) for Latin and a 12×13
subpixel-antialiased CJK fallback, then integer-upscaled for viewing — see
`tools/sim/README.md`. `live_creator_check.py` proves creator mode end-to-end with the same live
endpoint: the model must call `app_save`/`app_file`, a package must appear under
`~/.dst/apps` and `/apps` must show it (`docs/live-creator.png`).
`run_live.py` is the common-usage visual pass: it uses the **real local vLLM
endpoint** from `~/.dsh/settings.yaml` (real reasoning/streaming/tool calls)
and mocks only TTS/ASR (`DST_MOCK_SPEECH=1`), capturing startup, streaming,
tool running/done, push-to-talk, `/help`, `/config`, `/resume` and `/login` into
`docs/live-walkthrough.png`. See `docs/UX-LIVE.md`. Everything else binds the
mock API to 127.0.0.1 only.

Voice test hooks (never set on the device): `DST_MOCK_SPEECH=1` fakes the
recorder/ASR (text from `DST_MOCK_ASR_TEXT`) and logs TTS instead of playing
(`DST_MOCK_TTS_MS`); `~/.dst/logs/voice.log` records both paths.

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
src/dsh_login.rs real DSH/WeChat nine-step login (curl + std crypto)
src/login.rs     login HTTP page (WeChat QR + API key) and QR rendering
src/skills.rs    skill catalog and loader
src/apps.rs      creator mode: app packages, catalog, screen takeover runtime
src/mcp.rs       MCP stdio client
src/jobs.rs      cron + file-watch scheduler
src/voice.rs     TTS/ASR, resampling, WAV framing, recorder
src/json.rs      minimal JSON
tools/sim/       pty screen simulator + scripted mock API + live-API walkthrough
docs/UX-AUDIT.md 12-row screen-by-screen UX audit checklist and known limits
docs/UX-LIVE.md  common-usage walkthrough against the real local vLLM model
docs/CREATOR.md  creator mode: requirements, data layout, prompt + takeover contract
tools/sim/mock_login.py  mock of the nine-step DSH login chain (host tests)
tools/sim/qr_scan.js     jsQR decoder for the panel QR image
tools/sim/check_qr_scan.py  renders the panel QR and proves it decodes
tools/sim/check_login_page_i18n.py  starts the login page in all six languages
tools/sim/check_login_cancel.py  proves the page's Cancel stops the flow
tools/sim/render_login_i18n.py  renders the QR login screen in all six languages
                               and asserts the text never overlaps the QR
tools/sim/run_live.py  real-vLLM UX walkthrough (speech mocked) + contact sheet
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
