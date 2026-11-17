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
   under the workspace, creator app tools. Pass.
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

`/resume` groups sessions by directory and previews the last exchange;
`/rewind` restores a checkpoint from an earlier turn.

## Skills and apps

Both are directories with a manifest and a body, discovered on startup and
refreshed on change:

```text
~/.dst/skills/<name>/SKILL.md          YAML frontmatter + body, .history/vN.md
~/.dst/apps/<category>/<name>/app.json manifest (kind, entry, requires, history)
                             APP.md    user-facing annotation
                             run.sh    entry program
                             state/    $DST_APP_STATE
```

Only the catalog (one line per item) enters the prompt; bodies load on demand
through the `skill` tool or `/apps`. Creator-mode runtime constraints — screen
takeover, long-press `Esc`, timeouts, ulimits, scrubbed environment — are
documented in `docs/CREATOR.md`.

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

## Test seams

The simulator (`tools/sim`) drives the real binary through a pty and asserts on
both the captured screen *and* the mock server's request log — which is how
"a `<goal_round>` prompt was really sent" or "a checkpoint reached the next
request" are proven. `DST_MOCK_SPEECH` exercises the voice UI without hardware.
See `tools/sim/README.md` and `docs/CONFIGURATION.md` for the full list.
