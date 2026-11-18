# Architecture

`dst` is one process, one binary and zero third-party crates. Everything below
lives in `src/`; the device only needs `fbterm`, `curl` and (optionally)
`alsa-utils`.

## Design invariants

1. **No crates.** JSON, YAML, SSE framing, QR encoding, resampling and the
   login crypto are written against `std`; HTTP itself is a `curl` subprocess,
   which is also how TLS is handled. The dependency set is a design
   constraint, not a temporary state.
2. **The loop is the product.** UI code renders events from the harness; it
   never drives the model itself.
3. **Every tool call is a transaction** with a call event, a gate, a deadline,
   a canonical value and a projection to text.
4. **The device is the target.** 53 columns × 12 rows, 320×170 pixels, one core,
   512 MB. Anything that cannot fit is not built.
5. **Deterministic where it matters.** The splash schedule, the compaction
   boundary rule and the mock API are all reproducible, so screenshots and
   tests are stable.

## Process and threads

The UI thread owns the terminal and the framebuffer. Work that can block gets a
thread and reports back through a channel:

| thread | started by | talks back as |
| --- | --- | --- |
| turn driver | `harness::run` | `Ev::…` deltas into the transcript |
| tool executor | `tool::run_batch` | call/result events |
| HTTP client | `api::stream` | streamed reasoning/content deltas |
| login flow | `dsh_login` / `login` | step updates and a QR frame |
| app runner | `apps` | stdout bytes and an exit status |
| MCP client | `mcp` | discovered tools at startup |
| voice | `voice` | REC levels and a transcript |

## Turn / step driver

A **turn** is one user message and everything it causes. A **step** is one model
request plus the tool batch it produced. The driver only ends a turn when a step
produces no tool calls.

```text
user message
  └─ turn
       ├─ agent/pre-step   → compaction? context injection? clock?
       ├─ request          → streamed reasoning + content + tool_calls
       ├─ if tool_calls:
       │    ├─ tool/call      (rendered as a pending card)
       │    ├─ pre-execute    (parse + validate + permission gate)
       │    ├─ execute        (deadline-bounded, may run on a thread)
       │    ├─ post-execute   (policy hook, may attach additionalContexts)
       │    └─ tool/result    (canonical JSON → render() → model text)
       │    └─ additionalContexts injected as one user/message after the batch
       └─ next step (or final answer)
```

There is no step cap by default (`agent.max_steps` opts in). `Esc` aborts the
running turn, including a speech clip that is still playing; queued input stays
queued unless `Shift+Enter` overtakes it.

## Tool contract

A tool is a `ToolDef` (`src/tool.rs`):

| field | role |
| --- | --- |
| `name`, `description`, `params` | what the model sees |
| `timeout_ms` | executor deadline; never sent to the model |
| `execute` | pure `&J -> Result<J, Fail>`: canonical value out, no prose |
| `render` | pure `(&args, &value) -> String`: the text the model reads |
| `present` | one-line pending card title (UI only) |
| `post` | post-execute hook that may attach `additionalContexts` |
| `schema_override`, `dynamic` | MCP and runtime-registered tools |

Batches run **in model order**. Each call settles exactly once: a timeout, a
denial, or a value — never silence. `concurrency_safe` is recorded for a
future parallel dispatcher (the A53 gains nothing from it today).

## Permission gate

`src/sandbox.rs` classifies every call before it executes:

1. **Safe rules** — read-only commands, `read_file`/`list_dir`, web reads, writes
   under the workspace, skill-writing and app-run/test tools. Pass.
2. **Danger rules** — `sudo`, `systemctl start|stop`, package managers, mounts,
   writes under `/etc`, `/boot`, `/usr`, reboot. Refused with a reason.
3. **Everything else** — in `auto` mode one short, cheap LLM call returns a
   verdict; in `strict` mode the user gets a full-screen allow/reject dialog;
   in `full` mode it runs.

A refusal is a *failed tool result* with the concrete reason, which is what lets
the model explain itself instead of retrying the same call.

## Compaction

Automatic compaction runs at `agent/pre-step` when the estimated pressure
reaches

```text
threshold = min(window × 0.8, window − reserved_output − 64K headroom, compact_threshold)
```

Then, in order:

1. prune the middles of oversized old tool results (model-free);
2. keep the newest tail worth ~16 % of the message budget on a user-message
   boundary;
3. make **one** streamed summary call that replays the conversation's own system
   prompt and shadowed messages, appending the compaction instruction as the
   final user message (prefix-cache friendly);
4. frame the result as `This is an automatically generated checkpoint…` plus
   `<compacted-summary>…</compacted-summary>` in a single user message. A
   summary that is not smaller than the region it replaces is rejected.

A provider-reported context overflow bypasses the threshold, forces one
compaction and retries the request once. `/compact` skips the threshold and
keeps no verbatim tail.

## Goals

`src/goal.rs` ports DSH's goal driver. A goal is a persisted objective with a
round counter; between rounds the driver injects DSH's `<goal_round>` prompt
verbatim. The model drives its own state machine:

```text
active ──update_goal complete──▶ complete
   │
   ├──update_goal blocked (one concrete condition for ≥ 3 rounds)──▶ blocked
   ├──Esc / /goal pause──▶ paused ──/goal resume──▶ active
   └──/goal clear──▶ (removed)
```

The status line shows `◎ goal <round>/<cap> <objective>`, `⏸ goal paused` or
`✓ goal done`. Caps come from `goal.max_rounds` and `goal.max_steps`
(`0` = unlimited). After a restart an active goal is *disarmed* until the user
resumes it — the same activation rule as DSH.

## Context injection and instructions

Before each request the harness may append:

- an attributed clock reading in DSH's exact format
  (`Time sampled while preparing turn N, step M: …`, with the process time
  zone and elapsed time), rate-limited by `context.time_refresh_secs`;
- a repeat-tool reminder after the third and fifth identical call;
- any `additionalContexts` a tool attached, injected as a single
  `user/message` after the batch's tool results.

`src/instructions.rs` loads `AGENTS.md` from the workspace and from
`~/.dst` (bounded to 16 KB) into the system prompt, so a repository can describe
its own conventions the way CLAUDE.md / AGENTS.md users expect.

## Sessions, checkpoints and memory

| data | where | written by |
| --- | --- | --- |
| transcript + title | `~/.dst/sessions/<timestamp>.json` | every turn |
| turn checkpoints | `~/.dst/checkpoints/` | each completed turn |
| input history | `~/.dst/history` (mode 600) | on submit, never `/key` |
| usage totals | `~/.dst/usage.json` | after each response |
| memory fragments | `~/.dst` (via `memory_*` tools) | the model |

`/resume` groups sessions by directory and previews the last exchange.
`/rewind` restores the transcript to **just before one of your earlier
messages**: each checkpoint is saved with the prompt the turn is about to run,
the picker lists those prompts (newest first, with the context before each), and
choosing one cuts the history back and puts that prompt into the input line for
editing or re-sending. Checkpoints are non-destructive (the abandoned future
files remain until the retention cap), and the rewound session is saved
immediately so quitting cannot restore the old future.

## Side questions (`/btw` subagents)

`/btw <question>` runs a **one-off read-only subagent** without touching the
main conversation:

1. **Context** — `btw_context` builds a request from the live system prompt
   (with a `SIDE QUESTION MODE` contract appended), a bounded tail of the
   current history (96 KB, starting at a user/assistant boundary so no orphan
   tool result is replayed) and the question as the final user message.
2. **Tools** — `tools::view_registry` filters the normal registry to an
   allow-list of pure viewing tools (`read_file`, `read_image`, `list_dir`,
   `glob`, `grep`, `search`, `fetch`, `ocr`, `skill`, `memory_read/search`,
   `todo_read`, `get_goal`, `job_list/output`, `watcher_list`). There is no
   `write_file`/`edit_file`/`bash`/job/skill/app mutation, no `artifact` and no
   `speak`; the sandbox confirmer always rejects, and the step budget is capped
   at 8.
3. **UI** — a dedicated `BtwUi` shows only a live status line (thinking / which
   tool is being read) and treats `Esc` as cancel. It writes nothing to the
   transcript.
4. **Answer** — the settled text is wrapped and shown as a normal artifact
   panel (`btw · <question>`), dismissed with Esc or saved with `s`. The main
   `msgs`, the session file and the checkpoint list are not modified, so the
   side question cannot pollute (or be re-sent with) the real conversation.

## Planning (`todo_write`) and the prompt contract

The model-facing todo tool follows DeepSeek Harness's `dsh-tool-todo` contract
rather than a bespoke list API: parameter `todos` is the **complete** list of
`{content, status}` items (`pending` / `in_progress` / `completed`), every call
replaces the whole list, exactly one item is `in_progress` while work remains,
and the tool result is the stable DSH line `Updated todo list: N pending, N in
progress, N completed.` (`src/todo.rs`). Legacy shorthand (`items: ["[x] …"]`,
`{text, done}`) is accepted for old scripts but never advertised.

The list is a per-turn plan, not storage: DSH clears its todo projection at
`turn/start`, so dst calls `todo::clear()` before every turn (and on `/clear`);
the status area shows the `done/total` badge meanwhile, and `todo_read` returns
the current list. The system prompt has a matching `PLANNING` section (plan
before multi-step work, skip trivial requests, keep one item in progress, mark
completed immediately) and explains that `state_report` complements it — the
pinned row is *now*, the todo list is *the plan*. The prompt also requires one
`state_report` at the start of every working step. Both requirements are
asserted by unit tests, so the prompt cannot silently lose them again.

## Images (vision input)

`deepseek-flash` accepts images in Chat Completions when `content` is an array of
blocks. dst has two entry points that produce the same block — the agent-facing
**`read_image` tool** (same name/parameter as DeepSeek Harness:
`{"file_path": "..."}`, result `{path, image}`) and the human `/image <path>`
shortcut (`src/vision.rs`):

1. **Load** — the path is resolved (`~`, relative, absolute), the file is capped
   at 8 MB, and the format is sniffed from the magic bytes (JPEG / PNG / GIF /
   WebP, exactly like the API which ignores the file name). Header parsing keeps
   the pixel size for the one-line label.
2. **Encode** — `vision::b64` (RFC 4648, std only) builds the documented
   `data:image/<type>;base64,<payload>` URL.
3. **Send** — for `/image`, `harness::user_msg_with_images` turns the user
   message into `content: [{type:text},{type:image_url,image_url:{url:data:…}}]`.
   For `read_image`, the tool's canonical value keeps the data URL aside from
   the rendered text, and the harness builds the same array for the **tool
   message** (`role: "tool"`; DeepSeek accepts images in user/system/tool
   messages). The API resizes images to about 1300×1300 (~1024 tokens each), so
   the payload is the expensive part, not the question.
4. **Forget** — as soon as the next response arrives, the message is rewritten
   to a text placeholder (`[image attached: …]` / `[image read: …]`). Follow-up
   steps, session files, checkpoints and compaction therefore never carry the
   base64 again, and the 446 MB device never holds a multi-MB history. The
   compactor additionally flattens any image blocks to `[image attached]` before
   replaying history to the summariser.

Limits are local caps on top of the API's (32 MiB per inline image, 48 MiB body,
600 images): at most 2 images per message and 8 MB per file. A text-only
endpoint/model rejects the block with a 400; the error path maps it to an
actionable hint (`/ocr` for text in images, or a vision model). Images are never
persisted: after the turn the history is text again, so `/resume`, `/rewind` and
the transcript stay small.

## Skill packages (and runnable apps)

Creator mode has one unit: a directory with a manifest and a body, discovered on
startup and re-read before every turn. A package becomes a runnable app when it
also carries `meta.json`; there is no second directory or catalog.

```text
~/.dst/skills/<name>/SKILL.md          YAML frontmatter + body, .history/vN.md
                     meta.json         optional: makes the package a runnable app
                     run.sh            entry program (kind=sh)
                     state/            $DST_APP_STATE
/etc/dst/skills/                       read-only hand-installed packages
```

Only the catalog (one line per package) enters the prompt; bodies load on demand
through the `skill` tool or `/apps`, and runnable packages are marked
`[kind:entry]` with the outcome of their last run/test. `skill_save`,
`skill_file` and `skill_rollback` write user packages and validate every path
inside `~/.dst/skills/<name>/`; `app_test` and `app_run` only execute a package
whose `meta.json` declares a known kind and entry. Creator-mode runtime
constraints — screen takeover, long-press `Esc`, timeouts, ulimits, scrubbed
environment — are documented in `docs/CREATOR.md`.

## MCP

`~/.dst/mcp.json` uses the common `mcpServers` shape. Servers are spawned over
stdio, `initialize` + `tools/list` run at startup, and each discovered tool is
exposed as `mcp__<server>__<tool>` with a verbatim JSON schema.

## Login

`src/dsh_login.rs` reimplements the DSH desktop client's nine-step WeChat flow
(QR from `open.weixin.qq.com`, callback nonce, web session token, PKCE
`auth_init` / `auth_approve` / `auth_exchange`) with `curl` and std crypto. The
resulting token is stored as `api.dsh_token` and sent as `x-dsh-auth-token`.
`src/login.rs` serves the LAN page (`:8787`) and renders the QR into panel
modules. Both are documented per language in `docs/LOGIN*.md`.

## Failure handling

| failure | behaviour |
| --- | --- |
| tool deadline | `tool timed out after Ns`, turn continues |
| permission denial | failed result with the reason |
| auth error | hint to `/key` or `/login` |
| rate limit | wait, or pick another model |
| network | check Wi-Fi, retry with `api.retries` / `fallback_models` |
| context overflow | force one compaction, retry once |
| non-TLS endpoint | refused |

## Bounded memory and resources

The device has 446 MB of visible RAM, a slow microSD and no swap worth using,
and dst is meant to stay up for days. Every unbounded-looking structure has an
explicit bound; the caps below are enforced in code, not only in the prompt.

| Where | Bound | Why |
| --- | --- | --- |
| transcript (`Scr::lines`) | 400 rows | a long turn must not retain hundreds of MB of tool output |
| tool log (CtrlO) | 20 entries, 6000 chars each | full args/result for recent calls only |
| tool capture | `bash`/`grep`: 64 KB stdout + 16 KB stderr, drained concurrently; `read_file` streams the requested range; `edit_file` refuses files >8 MB | `Command::output()` on `cat big.log` would allocate the whole stream before truncation |
| history (`msgs`) | compaction (token budget) **plus** an absolute 16 MB cap that trims the oldest tail | compaction can be disabled/failing and some endpoints report no context window |
| scheduler turns (cron/watchers) | same compaction settings + a 600-message cap | unattended turns write into the shared history with no human in the loop |
| prompt/queue | 4 KB text, 16 queued messages | paste floods and endless queueing |
| voice take | 60 s (48 kHz mono, ~6 MB) | an interrupt take can stay open for a long turn |
| background jobs | 24 retained (finished ones pruned), 8 KB per line, own process group, reaped on the idle tick | `job_start` in a long session must not keep every command and tail |
| MCP servers | killed and reaped on every exit path (`ShutdownOnDrop`) | a dropped `Child` is not waited for and would outlive dst |
| app run/test | own process group, bounded reader joins (2 s), group kill | a daemonized helper must not block the UI on a pipe |
| sessions / checkpoints | newest 40 session files, newest 20 checkpoints, atomic writes; `/resume` refuses >24 MB and moves instead of clones | one full-history file per launch and two writes per turn would grow the SD card and peak at ~10x a large session |
| logs | `voice.log`/`mcp.log` rotate at 256 KB | append-only logs written per keypress/take |
| scratch | `~/.dst/tmp` swept at startup (7 days) | artifact exports, job dumps and app-test logs accumulate |
| hardware scan | 20 s total, 2.5 s per probe, 64 KB per stream, raw capture capped; `.history/` keeps 5 versions | onboarding must never hang on a wedged bus |

`tools/sim/check_memory.py` drives a 20-turn session and asserts the peak RSS
stays far below the device budget; `run_all.py` runs it together with the 92
scenarios.

## Test seams

The simulator (`tools/sim`) drives the real binary through a pty and asserts on
both the captured screen *and* the mock server's request log — which is how
"a `<goal_round>` prompt was really sent" or "a checkpoint reached the next
request" are proven. `DST_MOCK_SPEECH` exercises the voice UI without hardware.
See `tools/sim/README.md` and `docs/CONFIGURATION.md` for the full list.
