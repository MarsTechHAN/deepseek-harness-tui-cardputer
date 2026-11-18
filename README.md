<div align="center">

<img src="docs/banner.png" alt="dst — a DeepSeek agent harness for the M5Stack Cardputer Zero" width="100%">

# dst

**The whole DeepSeek Harness agent loop — turns, tools, goals, compaction — in one
zero-dependency Rust binary, on a 320×170 handheld.**

*a DeepSeek harness TUI for the M5Stack Cardputer Zero*

<a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-22d3ee?style=flat-square" alt="License: GPL-3.0"></a>
<img src="https://img.shields.io/badge/rust-2021-dea584?style=flat-square&logo=rust&logoColor=white" alt="Rust 2021">
<img src="https://img.shields.io/badge/dependencies-0-2ea043?style=flat-square" alt="Zero dependencies">
<img src="https://img.shields.io/badge/binary-1.0%20MB-2ea043?style=flat-square" alt="1 MB binary">
<img src="https://img.shields.io/badge/panel-320%C3%97170%20%C2%B7%2053%C3%9712%20cells-22d3ee?style=flat-square" alt="320x170 panel, 53x12 cells">
<img src="https://img.shields.io/badge/UI%20languages-6-8b5cf6?style=flat-square" alt="6 UI languages">
<img src="https://img.shields.io/badge/tests-70%20passing-2ea043?style=flat-square" alt="70 unit tests">
<a href="CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square" alt="PRs welcome"></a>

<a href="#quick-start">Quick start</a> ·
<a href="#screenshots">Screenshots</a> ·
<a href="#features">Features</a> ·
<a href="#commands">Commands</a> ·
<a href="#architecture">Architecture</a> ·
<a href="#testing-without-the-device">Simulator</a> ·
<a href="#documentation">Documentation</a>

</div>

---

`dst` is a pocket-sized agent: a terminal UI and an agent harness that run
entirely on a **M5Stack Cardputer Zero** — a Raspberry Pi CM0 handheld with
512 MB of RAM and a 320×170 panel that holds **53 columns × 12 rows** of text.

It is not a chat client with a keyboard bolted on. The loop is the real thing,
ported from the DeepSeek Harness (DSH): a turn owns steps, a step is one model
request plus its tool batch, and every call travels
`tool/call → pre-execute → sandbox gate → execute → post-execute → tool/result`
with `additionalContexts` injected after the batch. Tools return canonical JSON;
a separate `render()` projects it to the text the model reads.

<table>
<tr>
<td width="50%" valign="top">

**Why it exists**

Everything else assumes a desktop: a browser, a big terminal, a fast CPU, a
battery you can drain. `dst` assumes the opposite — 12 rows, a thumb keyboard,
and a 512 MB device that still deserves a real agent loop with permissions,
memory and goals.

</td>
<td width="50%" valign="top">

**What it is not**

No subagents, no remote server, no cloud sandbox, no telemetry. One process,
one binary, `~/.dst` for state, an OpenAI-compatible (or DeepSeek) endpoint for
intelligence, and `curl` for the wire.

</td>
</tr>
</table>

---

## Table of Contents

- [Screenshots](#screenshots)
- [Features](#features)
  - [Agent loop](#agent-loop)
  - [Tools](#tools)
  - [Permissions and sandbox](#permissions-and-sandbox)
  - [Goals, context and memory](#goals-context-and-memory)
  - [Interface](#interface)
  - [Creator mode](#creator-mode)
  - [Voice](#voice)
  - [Internationalisation](#internationalisation)
- [Quick start](#quick-start)
- [Commands](#commands)
- [Key bindings](#key-bindings)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Testing without the device](#testing-without-the-device)
- [Deploying to the device](#deploying-to-the-device)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

---

## Screenshots

<table>
<tr>
<td colspan="2"><a href="docs/live-walkthrough.png"><img src="docs/live-walkthrough.png" alt="Twelve-screen live walkthrough: startup, streaming, reply, tool call, push-to-talk, help, config, resume, login"></a><br><sub><b>Live walkthrough</b> — 12 real screens against a local model: streaming, a folded reasoning line, a tool card, push-to-talk, <code>/help</code>, <code>/config</code>, <code>/resume</code>, <code>/login</code>. Click for full size.</sub></td>
</tr>
<tr>
<td width="50%"><a href="docs/live-creator.png"><img src="docs/live-creator.png" alt="Creator mode: the model builds a battery readout as a skill package and /apps lists it"></a><br><sub><b>Creator mode</b> — the model builds a battery readout as a skill package (<code>SKILL.md</code> + <code>run.sh</code> + <code>meta.json</code>); <code>/apps</code> lists it and runs it full-screen.</sub></td>
<td width="50%"><img src="docs/splash.gif" alt="Animated splash: the DeepSeek whale breaches out of a night sea on the device pixel grid"><br><sub><b>Splash</b> — a 14-frame whale breach on the device pixel grid, deterministic so snapshots reproduce.</sub></td>
</tr>
<tr>
<td width="50%"><img src="docs/login-qr-screen.png" alt="WeChat QR and the phone-page URL rendered on the 53x12 panel"><br><sub><b>Sign-in</b> — a real WeChat QR on the panel, a LAN page for the phone, or the credential typed on the device.</sub></td>
<td width="50%"><img src="docs/login-i18n-sheet.png" alt="The login screen rendered in English, Chinese, Spanish, Japanese, German and Italian"><br><sub><b>Six languages</b> — every screen, menu and dialog is translated: en · zh · es · ja · de · it.</sub></td>
</tr>
</table>

<div align="center">
<sub><a href="docs/splash-frames.png">splash frame sheet</a> · <a href="docs/login-page.png">phone login page</a> · <a href="docs/live-walkthrough.png">walkthrough at full size</a></sub>
</div>

---

## Features

### Agent loop

- **DSH turn/step driver.** A turn owns steps; a step is one model request plus
  its tool batch. Tool calls are rendered when they *start* and settle into a
  one-row card with duration and a preview, in `tool/call` → `tool/result` order.
- **Streaming with a live reasoning panel.** Reasoning deltas stream above the
  prompt and fold to one dim line when the answer begins.
- **No step cap by default.** A turn may use as many steps as it needs
  (`agent.max_steps` opts into a cap, `0` = unlimited). Every tool declares a
  deadline; an overrun settles as `tool timed out after Ns` without freezing the
  prompt, and `Esc` still works while a tool runs.
- **Prefix-cache friendly requests.** The system prompt and shadowed history are
  replayed verbatim so DeepSeek's cache keeps hitting across turns.

### Tools

The model sees 32 tools in normal mode (list yours with `dst --tools`);
creator mode adds the execution and skill-writing tools at the bottom of the
table:

| group | tools |
| --- | --- |
| shell & files | `bash`, `read_file`, `write_file`, `edit_file`, `list_dir`, `glob`, `grep` |
| web + vision | `search`, `fetch`, `ocr` |
| orchestration | `artifact`, `speak`, `rename`, `todo_read`, `todo_write`, `skill`, `state_report` |
| background | `job_start`, `job_output`, `job_wait`, `job_kill`, `job_list`, `watcher_start`, `watcher_list`, `watcher_stop` |
| goals | `get_goal`, `create_goal`, `update_goal` |
| memory | `memory_write`, `memory_read`, `memory_search` |
| scheduling | `cron`, `watch` |
| creator mode | `app_run`, `app_test`, `skill_save`, `skill_file`, `skill_rollback` |

Any MCP server in `~/.dst/mcp.json` adds its tools as `mcp__<server>__<tool>`.

### Permissions and sandbox

Every tool call passes a gate with three modes (`sandbox.mode`; switched in
`/config`, chosen once during onboarding):

- **`auto`** (default) — known-safe reads and workspace writes pass; system
  changes (`sudo`, `systemctl start/stop`, package managers, mounts, writes
  under `/etc`, `/boot`, `/usr`, reboot) are refused outright; anything else
  gets one short LLM call to judge it.
- **`strict`** — a full-screen allow/reject dialog for every non-safe call.
- **`full`** — everything runs.

A refusal becomes a failed tool result with the concrete reason, so the model
can explain instead of retrying. HTTPS is enforced: non-TLS endpoints are
refused.

### Goals, context and memory

- **Goals** (`/goal`) are persisted objectives that keep the session working
  across automatic continuation rounds. The model judges itself through
  `update_goal`: `complete` only when achieved, `blocked` only after one concrete
  condition persisted for three rounds. Round and step caps are configurable;
  `Esc` disarms, `/goal resume` re-arms.
- **Automatic compaction** at `agent/pre-step`: prune oversized old tool results,
  keep a verbatim tail on a user-message boundary, then one streamed summary
  call framed as an `<compacted-summary>` checkpoint. A provider overflow forces
  one compaction and retries once. `/compact` runs it on demand.
- **Context injection** — an attributed clock reading
  (`Time sampled while preparing turn N, step M: …`), a repeat-tool reminder
  after the third and fifth identical call, and `additionalContexts` from tools.
- **Memory** — `AGENTS.md` from the workspace and `~/.dst` is loaded into the
  prompt (bounded), and `memory_write`/`memory_read`/`memory_search` keep short
  preferences across sessions.
- **`/usage`** reports cache hit rate, cache read, uncached input and output
  with share bars; a ring (`○ ◔ ◑ ◕ ●`) on the status line shows context pressure.

### Interface

- Markdown-aware rendering (headings, lists, dimmed code blocks), word-aware
  wrapping at the real 53-cell width, UTF-8 and CJK input.
- **Command palette** — type `/` for every command with its description and
  argument hint; menus are menuconfig-style with right-aligned values.
- **Full-screen artifacts** — the model can take over the panel with tables,
  code or checklists; `Esc` dismisses, `PgUp`/`PgDn` scroll.
- **Pinned progress row** — `state_report` pins one short line to the top row
  (bright magenta `»`) while a turn runs; a newer report replaces it and the
  line joins the transcript as history. Reports over 50 display cells are
  rejected, so the row can never wrap on the 53-cell panel.
- **Paging** — `PgUp`/`PgDn` page the transcript and list menus by a screenful,
  with the scroll clamped at the buffer edges; `Up`/`Down` recall input history
  on an empty prompt.
- **Sessions** — every turn is saved with a title the model keeps current via
  the `rename` tool; `/resume` previews sessions grouped by directory, `/rewind`
  restores a checkpoint.
- **Background tasks** — jobs and watchers with a manager (Down on an empty
  prompt), live progress on the status line, and coalesced watcher messages.
- **Tool output viewer** (`CtrlO`) — recent tool calls with their expanded
  arguments and full results in a scrollable panel; it also opens during a turn,
  Enter shows one call + result, `s` saves the text under `~/.dst/tmp/`.
- **Queueing and interrupts** — typing during a turn queues by default;
  `Shift+Enter` steers at the next step; `Esc` stops the whole turn at once and
  text already typed becomes the next message; `Tab` during a turn interrupts,
  starts recording immediately and sends the transcript (`auto_send_voice` on)
  or leaves it in the prompt (off).
- **Animated splash** — a deterministic whale breach, frame-scheduled rather
  than constant-rate; typing pauses it, the first message ends it, `/clear`
  brings it back.

### Creator mode

An opt-in mode where `dst` is the device's only program: everything you use is
something it built and saved. It is skill-centric — one kind of package, and a
package becomes runnable by adding a manifest.

- Every build is a **skill package** under `~/.dst/skills/<name>/`: `SKILL.md`
  (YAML frontmatter `name`/`description` plus instructions) and optional support
  files. The model writes it with `skill_save`/`skill_file`; `skill_save` is
  versioned (`.history/vN.md`) and `skill_rollback` restores an earlier body.
- A package is a **runnable app** when it also has `meta.json`
  (`title`, `kind` = `sh`|`python`|`binary`|`prompt`|`doc`, `entry`, `requires`,
  `default_args`, `capture`). The folder is the only registry: the `SKILLS`
  catalog marks it `[sh:run.sh]` (and ` (!last test: exit 3)` when a run failed),
  and `dst` writes `last_run`/`last_test` back into the same file.
- There is no `~/.dst/apps`, no `app.json`/`APP.md`, no separate app catalog and
  no `app_*` writer. `app_test` (headless, or a real pty with `tty=true`) and
  `app_run` (full-screen takeover, args, timeout) only execute what the package
  declares.
- Launches are bounded: long-press `Esc` always leaves an app, a timeout
  backstop, ulimits, a scrubbed environment, and cwd/`$DST_APP_STATE` inside the
  package.
- `/apps` lists the runnable packages with title, kind, last use and a `!` when
  broken: Enter runs, `→` reads `SKILL.md`, `l` replays captured output, `i`
  asks the model to improve it, `x` deletes, `+ new app` seeds a build request,
  `/` finds. Prompt apps are previewed before they are sent; doc apps open their
  panel.
- A turn that saved a package is announced (`saved skill <name>` /
  `saved app <name> (runnable skill)`); a turn that wrote files without saving
  one is nudged to persist it.

> Full reference — manifest fields, prompt contract, safety and the verification
> matrix: [`docs/CREATOR.md`](docs/CREATOR.md).

### Voice

Push-to-talk (`Tab`) records with `arecord`, posts to `/v1/tools/asr` and
auto-sends the transcript; TTS (`/v1/tools/tts`) plays through `aplay`.
Resampling and WAV framing are done in Rust. Speech tools exist only on
DeepSeek's official API — on a third-party endpoint they are removed from the
registry and the model is told so.

### Internationalisation

`language: en | zh | es | ja | de | it` in the config (switchable in `/config`).
Every user-visible string is translated — menus, palette previews, approval
dialogs, onboarding, login pages, job and watcher states. A unit test fails the
build if any language misses a key. Tool descriptions stay English; the system
prompt adds a short `LANGUAGE` block that pins the UI language as the model's
default answer language (a user request for another language wins).

---

## Quick start

### 1. Requirements

| need | why |
| --- | --- |
| Linux (Debian 13 on the device) | `fbterm`, `alsa-utils` for the panel and audio |
| Rust 2021 on the host | building `dst` and running the simulator (a release build on the 446 MB device OOM-kills it) |
| `zig` on the host | aarch64 cross-linker for the deploy build (`brew install zig`) |
| `curl` | every HTTP request (model, web tools, login) |
| `arecord`/`aplay` (`alsa-utils`) | optional: ASR capture, TTS playback |
| `qrencode`, `python3` + Pillow | optional: phone-page QR, WeChat QR on the panel |

### 2. Build

Zero crates, so the build needs no network at all. Build on the host; the device
never compiles:

```sh
git clone git@github.com:MarsTechHAN/deepseek-harness-tui-cardputer.git
cd deepseek-harness-tui-cardputer

cargo build --release        # host build for the simulator and `cargo test`
```

Release profile: `opt-level="z"`, LTO, `panic=abort`, `strip` → a ~1 MB host
binary. The device gets a cross-compiled aarch64 build — see
[Deploying to the device](#deploying-to-the-device).

### 3. First run

```sh
dst
```

The first launch asks for an endpoint (DeepSeek official or any
OpenAI-compatible one) and a sandbox mode. To sign in to the DeepSeek Platform
properly, use `/login`:

1. **WeChat QR on the panel** — the same nine-step flow as the DSH desktop
   client, with live status and a QR countdown;
2. **Phone page** — `http://<device-ip>:8787/` shows the QR, live steps and a
   manual API-key form, in the device's UI language;
3. **Type it** — a DSH token, or base URL + API key + model.

Credentials live in `~/.dst/config.yml` (mode `600`). A DSH token is sent as
`x-dsh-auth-token`; an API key as `Authorization: Bearer`.

A credential is **tested before it is accepted**: a DSH token is checked with one
tiny completion on the login platform, an API key with `GET /v1/models`. A key
that cannot answer is rejected and any previous credentials are restored, so a
half-configured device never looks ready while being unable to chat.

The last first-run step offers to scan the device's hardware into the shipped
`hardware-info` skill (display, audio, keyboard, battery, network, buses) and
write the measured state into it. That skill is what later hardware work reads
first; `/update-hwinfo` re-runs the same scan at any time (and, in creator mode,
has the model refine the skill afterwards).

### 4. Talk to it

```sh
dst                      # interactive TUI
dst --once "summarise this repo"        # one turn, prints events
dst --tools                             # effective tool set + capability decision
dst --hwinfo-scan                       # run the hardware scan, print the skill summary
dst --call bash '{"command":"uname -a"}'  # run one tool exactly as the model would
dst --voice-selftest 3                  # record, report RMS and transcript
```

---

## Commands

Commands run locally and are never sent to the model.

| command | what it does |
| --- | --- |
| `/help` | list commands, grouped by category, with a live preview |
| `/model [name]` | show models from `/v1/models` or switch to one |
| `/effort [auto, low, high, max]` | reasoning effort |
| `/config [wizard]` | interactive configuration (options first) |
| `/login` | DeepSeek sign-in: WeChat QR, phone page, or typed credential |
| `/key <api-key>` | set the API key directly |
| `/usage` | cache hit rate, tokens in/out, context pressure |
| `/compact` | summarise older history now |
| `/goal [show, <objective>, edit, pause, resume, clear]` | long-running objective |
| `/apps [run <name>]` | browse, run or delete the runnable skill packages dst built (menu) |
| `/update-hwinfo` | rescan this device's hardware and refresh the shipped `hardware-info` skill |
| `/extensions` | inspect skill packages (runnable ones tagged `[app]`) and MCP servers |
| `/resume` | pick a previous session (preview on `→`) |
| `/rewind` | restore a checkpoint from an earlier turn |
| `/rename <title>` | name the current session |
| `/clear` | clear the transcript and restart the splash |
| `/quit` | exit |

## Key bindings

The Cardputer Zero has no dedicated arrow or PageUp/PageDown keys: hold the
sticky `fn` modifier and press `fn`+`F`/`X`/`Z`/`C` for up/down/left/right, and
`fn`+`L`/`M` for PageUp/PageDown (the terminal receives the standard
`ESC[5~`/`ESC[6~`, which dst decodes).

| key | action |
| --- | --- |
| `Enter` | send |
| `Shift+Enter` | steer the running turn at its next step (does not cancel it) |
| `Esc` | stop the whole turn at once (and any playing speech); typed text becomes the next message; long-press leaves an app |
| `Tab` | hold to talk; during a turn it interrupts and starts recording immediately |
| `fn`+`F`/`X`/`Z`/`C` | arrow keys: up / down / left / right |
| `Up`/`Down` | recall input history on an empty prompt (Down with running jobs opens the task manager); scroll the transcript otherwise |
| `PgUp`/`PgDn` (`fn`+`L`/`M`) | page the transcript and list menus; `Home`/`End` jump to the ends |
| `/` | command palette |
| `CtrlO` | recent tool output (Enter opens call + result, `s` saves) |
| `CtrlQ` | edit the queued message |
| `CtrlA`/`CtrlE`, `CtrlW` | cursor to start/end, delete a word |

---

## Configuration

`~/.dst/config.yml` — DSH's YAML patch dialect: plain settings plus `insert:`
rows shaped `{ id, name, config, disabled }` that target a tool by `name`.

```yaml
model: deepseek-flash
effort: high                 # auto | low | high | max
max_tokens: 4096
api:
  base: https://api.deepseek.com/v1
  key: sk-...                # kept in this file, mode 600
tui:
  language: en               # en | zh | es | ja | de | it
  mode: normal               # normal | creator
agent:
  max_steps: 0               # 0 = unlimited
goal:
  max_rounds: 256
  max_steps: 0
compaction:
  enabled: true
  threshold_tokens: 512000
sandbox:
  mode: auto                 # auto | strict | full

insert:
  - id: speak
    name: speak
    disabled: true
```

Every key, including the numeric budgets and the `insert:` rows, is documented
in [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md). `/config` edits the same
values interactively; `/config path` prints the file location.

---

## Architecture

<img src="docs/architecture.svg" alt="dst architecture: device I/O, TUI, harness, tools, sandbox, model, state" width="100%">

| file | role |
| --- | --- |
| `src/main.rs` | TUI: framebuffer terminal, input, panels, wizard, resume |
| `src/harness.rs` | turn/step driver (DSH agent-loop port) |
| `src/tool.rs` | tool contracts and the execution pipeline |
| `src/tools.rs` | built-in tool registry |
| `src/sandbox.rs` | permission gate: safe rules, danger rules, LLM judge |
| `src/api.rs` | streaming chat client over a `curl` subprocess |
| `src/compact.rs` | automatic and manual context compaction |
| `src/context.rs` | in-progress context injection (clock, repeat reminder) |
| `src/goal.rs` | goal state machine, tools and the round driver |
| `src/session.rs` | session titles, `rename` tool, `/resume` previews |
| `src/checkpoint.rs` | turn checkpoints for `/rewind` |
| `src/bg.rs` | background jobs and line watchers |
| `src/jobs.rs` | cron and file-watch scheduler |
| `src/skills.rs` | skill packages: catalog, progressive loader, `skill_save`/`skill_file`/`skill_rollback` |
| `src/apps.rs` | runnable skill packages: `meta.json` loader, `/apps` launcher data, screen takeover |
| `src/hwinfo.rs` | the shipped `hardware-info` skill: bounded read-only hardware scan, scan-section renderer, seed install |
| `src/mcp.rs` | MCP stdio client |
| `src/voice.rs` | TTS/ASR, resampling, WAV framing, recorder |
| `src/config.rs` | `~/.dst` home and the DSH-dialect YAML config |
| `src/cmd.rs` | human commands |
| `src/i18n.rs` | six-language UI catalog |
| `src/login.rs`, `src/dsh_login.rs` | login page + QR, nine-step DSH/WeChat login |
| `src/art.rs` | the animated splash |
| `src/json.rs` | minimal JSON |

Deeper notes on the loop, compaction, goals and the permission model:
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Testing without the device

The TUI is a plain ANSI program, so it runs under a pty anywhere.
[`tools/sim`](tools/sim/README.md) contains a screen simulator (53×12 pty → pyte
→ PNG at the device's native 320×170 with the X11 6×13 bitmap font) and a
scripted OpenAI-compatible mock server. No hardware, no network, no API key.

```sh
cargo build                       # host binary

cd tools/sim
python3 run_demo.py               # splash → thinking → tool call → reply + contact sheet
python3 run_scenarios.py          # feature scenarios, asserts on screens *and* request logs
python3 run_live.py               # visual walkthrough against a real local endpoint
python3 check_memory.py           # samples RSS over 20 turns
```

`run_scenarios.py` covers the feature matrix end to end: thinking panel, tool
call ordering, timeouts, `Esc`, step limits, repeat/time context injection,
automatic and manual compaction, every `/goal` flow, `/config`, the palette,
artifacts, sessions across a restart, onboarding, sandbox modes, jobs, watchers,
searchable resume, push-to-talk with mocked speech, the nine-step WeChat login
chain, provider failures and recovery, CJK input and the creator-mode scenarios.

Unit tests (host, no network): `cargo test` — 70 tests over the harness, tool
pipeline, compaction budgets, goal state machine, context formatting, config
round-trip, skill packages, runnable apps and the i18n catalogue.

---

## Deploying to the device

The device boots `cpzterm.service` (`fbterm` + `dst`). Deployment is
cross-compiled on the host and pushed over SSH — nothing is built on the device,
because a release build on the 446 MB box gets OOM-killed.

```sh
tools/deploy/deploy.sh                     # default target: hanxiao@192.168.43.49
tools/deploy/deploy.sh user@host           # another device
tools/deploy/deploy.sh --build-only        # cross-compile + verify only; no SSH
tools/deploy/deploy.sh --no-restart        # install, leave cpzterm running
```

Host prerequisites (the script detects missing pieces and prints the fix, it
never installs them): `rustup target add aarch64-unknown-linux-gnu`, `zig` for
the aarch64 linker (a wrapper at `/tmp/zigcc/zig-cc` is recreated when missing),
and `sshpass` only when using `DST_SSH_PASSWORD`.

Stages:

1. **host toolchain** — cargo/rustc, the aarch64 Rust target and the zig linker
   wrapper;
2. **cross build** — `cargo build --release --target aarch64-unknown-linux-gnu`
   with `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER` pointing at the zig
   wrapper (LTO + `codegen-units=1`; `DST_JOBS=1` by default);
3. **artifact verification** — an aarch64 ELF64 little-endian PIE checked with
   `file` + `readelf` (ELF header via `od` when binutils is absent), plus size
   and sha256;
4. **copy** — `scp` to `/tmp/dst.new`, sha256 compared on the device;
5. **install** — `sudo install -m755` to `/usr/local/bin/dst`;
6. **restart** — `systemctl restart cpzterm.service`, poll `is-active` for
   `DST_RESTART_WAIT` seconds, dump `journalctl -u` if it fails;
7. **smoke check** — `dst --tools` runs and exits before the TUI, so the panel
   is never taken over.

`DST_SSH_KEY` selects a key; `DST_SSH_PASSWORD` (needs `sshpass`) uses password
auth and the same password for remote `sudo -S` (passwordless sudo works too).
Other overrides: `DST_CARGO_TARGET_DIR`, `DST_ZIG_CC`, `DST_JOBS`,
`DST_REMOTE_TMP`, `DST_REMOTE_BIN`, `DST_SERVICE`, `DST_RESTART_WAIT`;
`DST_BUILD_ONLY=1` / `DST_NO_RESTART=1` mirror the flags.
`tools/deploy/fix-cpzterm-order.sh` repairs an early-boot ordering cycle
(`Before=sysinit.target` deadlocks boot with `Resource deadlock avoided`).

---

## Documentation

| document | contents |
| --- | --- |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | turn/step loop, tool pipeline, compaction, goals, permissions |
| [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) | every config key, `insert:` rows, capabilities per endpoint |
| [`docs/CREATOR.md`](docs/CREATOR.md) | creator mode: skill packages, `meta.json` runnable apps, `/apps`, prompt and takeover contract |
| [`docs/LOGIN.md`](docs/LOGIN.md) | DeepSeek sign-in: states, requirements, troubleshooting — [中文](docs/LOGIN.zh.md) · [ES](docs/LOGIN.es.md) · [日本語](docs/LOGIN.ja.md) · [DE](docs/LOGIN.de.md) · [IT](docs/LOGIN.it.md) |
| [`docs/UX-AUDIT.md`](docs/UX-AUDIT.md) | screen-by-screen UX audit checklist and known limits |
| [`docs/UX-LIVE.md`](docs/UX-LIVE.md) | the common-usage walkthrough behind the screenshots |
| [`docs/SPLASH-STORYBOARD.md`](docs/SPLASH-STORYBOARD.md) | splash frame storyboard and reference analysis |
| [`tools/sim/README.md`](tools/sim/README.md) | simulator, mock API, scenario DSL, screenshot pipeline |

## Contributing

Issues and pull requests are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md)
for the build, test and screenshot workflow. Security reports: see
[`SECURITY.md`](SECURITY.md).

`dst` keeps a few promises that make review easy: **no third-party crates**, no
credentials in the tree, and every UI string in the i18n catalogue.

## License

[GPL-3.0](LICENSE) © MarsTechHAN

## Acknowledgements

- The DeepSeek Harness (DSH) for the agent-loop, compaction, goal and
  instructions architecture this port follows.
- The public-domain X11 6×13 bitmap font, vendored for pixel-accurate
  screenshots.
- The original dsh-TUI pixel whale, reused frame-for-frame in the splash.

<div align="center">
<br>
<a href="https://star-history.com/#MarsTechHAN/deepseek-harness-tui-cardputer&Date">
  <img src="https://api.star-history.com/svg?repos=MarsTechHAN/deepseek-harness-tui-cardputer&type=Date" alt="Star history" width="70%">
</a>
<br><br>
<sub>Built for a 320×170 screen — and for the idea that a pocket device deserves a real agent loop.</sub>
</div>
