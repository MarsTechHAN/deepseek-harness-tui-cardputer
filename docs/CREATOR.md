# Creator mode — the device as an app platform

Creator mode turns dst from "a chat window on a handheld" into "the only program
on the device, which builds the device's programs". It is opt-in; normal chat
mode stays the default and is unchanged.

This document is the requirement analysis, the business rules, the data layout,
the prompt contract, the screen-takeover contract and the verification plan.
`tools/sim/scenarios/creator_*` are the executable form of this document.

## 1. Why

The Cardputer Zero is a closed handheld: no app store, no browser, no IDE. The
usefulness of the device is bounded by what the user can put on it. A chat UI
answers a question and forgets it; the answer is gone when the screen scrolls.

Creator mode makes *building* the default interaction: when the user asks for
something ("show the battery level", "a pomodoro timer", "a list of the Wi-Fi
networks"), dst builds it, saves it as a permanent, annotated package under
`~/.dst/apps/`, and can run it full-screen on the panel. The device accumulates
capability over time, and the user can list / reopen / delete what it built.

Two invariants keep this from becoming a dangerous toy:

1. **The user is never trapped.** Long-press Esc always leaves any app and
   returns to the chat. The platform enforces it (the runner watches the
   keyboard, the app is killed), so it cannot be broken by a buggy app.
2. **The chat is always reachable.** Apps run in the foreground of a turn; when
   one exits (or is Esc-killed / times out) the TUI repaints itself. dst itself
   only leaves on Ctrl-C/Ctrl-D or `/quit`.

## 2. Requirements

Functional:

- R1 A mode flag `tui.mode: normal|creator`, default `normal`, switched from the
  `/config` menu (Mode row). Persisted. Deliberately not a slash command: the
  mode changes the whole contract, so it belongs behind the settings screen,
  and a one-word command is too easy to fire accidentally.
- R2 In creator mode the model can persist work two ways:
  - **app**: a runnable package (script, binary, prompt or document) with a
    manifest, short human annotation and files; and
  - **skill**: a systematic, self-contained engineering package (procedure,
    format, checklist, supporting files) written for the model itself, kept in
    the same catalog as hand-installed skills.
  Memory stays the place for short durable facts/preferences; the prompt makes
  that split explicit (memory = fragments, skill = complete procedure, app =
  runnable thing).
- R3 App packages persist across sessions and reboots (plain files under
  `~/.dst/apps/`).
- R4 The model is told which apps already exist, skills-style: a one-line
  catalog is injected into the system prompt each turn; the annotation body is
  read on demand (`read_file`), mirroring the skills catalog/`SKILL.md` split.
- R5 The model can run a saved app on the device screen (`app_run`) — the app
  owns the whole panel and all input while dst is paused — and can verify it
  headlessly first (`app_test`: piped stdio, captured exit/stdout/stderr, full
  log file) the way the developers test dst itself.
- R5b Apps are bounded by a platform contract: platform-enforced long-press Esc
  and timeout, CPU/file-size ulimits, a working directory and `$DST_APP_STATE`
  inside the package, no root; the system prompt states the guidelines (no
  system configuration, installs, services or writes outside the package) and
  the app kinds are `sh`, `python`, `binary`, `prompt` and `doc`.
- R6 The user can browse apps (`/apps`): list, preview the annotation, run
  (Enter), delete (x, with confirmation).
- R7 After a turn that changed files but saved no app, creator mode nudges
  (one dim line) — the "no work is lost" rule is visible, not just prompted.
- R8 Everything user-facing is localized in en/zh/es/ja/de/it.

Constraints:

- C1 The device is 320×170 px, 53×12 cells, 8 ANSI colours, no mouse. Apps must
  fit that; the prompt says so.
- C2 512 MB / 4×A53: no on-device compilation, no package installs in the app
  flow. Apps are `sh` scripts (coreutils, /proc, /sys), `python` scripts when
  the interpreter exists, or doc-only packages.
- C3 App writes are restricted to `~/.dst/apps/<category>/<name>/`; traversal,
  absolute paths and system paths are rejected by the tool, not just the prompt.
- C4 Screen takeover only exists in the interactive TUI. `--once`, background
  jobs and `--call` fail with a clear error instead of hanging.

Non-goals: replacing fbterm/the OS; running services in the background (jobs and
watchers already cover that); grabbing the framebuffer; a GUI toolkit.

## 3. Data layout

```text
~/.dst/apps/
    system/battery_viewer/
        app.json        machine manifest (see below)
        APP.md          short annotation: what it is, how to use it, how it works
        run.sh          entry program (kind=sh)
        state/          runtime state written by the app ($DST_APP_STATE)
    tools/report/
        app.json        kind=prompt
        APP.md          human annotation
        PROMPT.md       the request the model follows when the app is run
    tools/pomodoro/
        app.json
        APP.md
        main.py
```

`app.json`:

```json
{
  "name": "battery_viewer",
  "title": "Battery viewer",
  "category": "system",
  "description": "Shows the charge level and charging state, refreshed live.",
  "kind": "sh",
  "entry": "run.sh",
  "default_args": "--fast",
  "capture": false,
  "version": 1,
  "created": "2026-11-17T15:00:00+08:00",
  "updated": "2026-11-17T15:00:00+08:00"
}
```

Rules:

- `name` is a slug (`[a-z0-9][a-z0-9_-]{0,31}`), unique within the category;
  category is a slug, default `apps`.
- `kind` is `sh` | `python` | `binary` | `prompt` | `doc`; `entry` is a
  relative file inside the app dir and is required except for `doc` and
  `prompt`. `sh`/`python` go through the interpreter, `binary` executes the
  entry directly (it must be executable), `prompt` returns PROMPT.md as a
  request to follow, `doc` opens APP.md in the panel.
- `description` is one line (≤160 chars); `annotation` (APP.md) is a short
  markdown body (≤4000 chars): what it does, how to use it, how it works, the
  files. It is written for the user reading `/apps` later, not for the model.
- Saving an existing name updates in place and bumps `version` (and `updated`);
  files not rewritten are kept, so a one-file fix does not need a full rebuild.
- Runtime state goes to a `state/` directory inside the app (`$DST_APP_STATE`),
  so a counter or cache never pollutes APP.md/app.json.
- `app_export` packs the whole package (manifest, annotation, program, state)
  into `~/.dst/tmp/app-<category>-<name>.tar.gz` for backups or for copying an
  app off the device; it is the model-side export path.
- `default_args` is passed when the user launches the app from `/apps`
  (model `app_run` args override it); useful for CLI-style apps with a fixed
  mode. The annotation panel shows the version and the default args.
- `capture: true` makes `app_run` give the app a **piped** stdout/stderr
  (not a tty) while dst forwards the bytes to the panel and records a cleaned
  copy (`state/last-run.log`, ANSI/CR stripped) for replay. It is for text
  apps; full-screen TUI apps should leave it off.
- `app_import` restores an archive from `~/.dst/tmp`: it extracts into a temp
  dir, requires exactly one `<category>/<name>/` tree with a valid manifest,
  caps the package (200 files / 256 KB) and refuses paths outside the tmp dir;
  `replace=true` overwrites an installed app.
- Model-written skills use the existing skills layout:
  `~/.dst/skills/<name>/{SKILL.md, …supporting files}`; `skill_save` writes the
  frontmatter + body (including `origin: model`), `skill_file` adds support
  files. They appear in the SKILLS catalog (and in `/extensions`) from the next
  turn on, and are loaded with the `skill` tool exactly like hand-installed
  skills.
- `/extensions` marks model-written skills with a localized `[model]` badge and
  lets the user manage them: Enter shows the metadata preview, `→` opens the
  full SKILL.md body in the panel viewer, `x` disables/enables, `d` deletes the
  selected user-owned package after `delete skill <name>? y/n`. Packages under
  `/etc/dst/skills` are read-only (no delete).
- Saving a skill **versions** it: `skill_save` bumps `version` in the
  frontmatter, stamps `updated`, copies the previous body to
  `.history/v<N>.md` (last five kept), and shows `(vN)` in the SKILLS catalog
  from v2 on; `/extensions` details show version/updated.
- `skill_rollback` restores the body of an earlier version (default: previous
  step) as a **new** version, archiving the current one first — so a rollback
  is itself reversible and the version number only goes up. Unknown versions
  fail with the available version list.
- The `skill` tool reloads from disk when the startup catalog misses a name, so
  a skill the model just wrote can be loaded and followed in the same turn (the
  SKILLS catalog itself updates on the next turn).
- The injected catalog names the entry (`[sh:run.sh]`), so the model knows which
  file to rewrite without reading the manifest first. A failed last run/test is
  appended to the catalog line (` (!last test: exit 3)`) so the model knows to
  fix an app before reusing it.
- Each run/test is recorded back into `app.json` as `last_run` / `last_test`
  (`{at, secs, outcome}`, outcome `exit N` / `timeout Ns` / `esc` / `signal`).
  The `/apps` annotation panel shows both with a short time; a manifest update
  by `app_save` preserves them.
- Writes go through `app_save` / `app_file`; each file is capped (32 KB per
  file, 256 KB per app) so a model cannot fill the microSD through the tool.

## 4. Modes: what changes

| | normal | creator |
|---|---|---|
| system prompt | base | base + `CREATOR MODE` block + installed-app catalog |
| tools | base set | + `app_save`, `app_file`, `app_test`, `app_run`,
  `app_export`, `app_import`, `skill_save`, `skill_file`, `skill_rollback` |
| idle hint | `idle_hint` | `creator_hint` (mentions `/apps` and long-Esc) |
| after a turn | nothing | saved-app notice, or a nudge when files changed but no app was saved |
| conversation | unchanged | unchanged (apps are additive) |

Switching mode rebuilds the tool registry and the system prompt (the prompt is
rebuilt at the start of every turn anyway, so a new app shows up immediately in
the next turn's catalog). The switch is reported in the transcript, so it is
never a silent state change.

## 5. Model contract (system prompt block)

Appended in creator mode; the catalog is one line per app, capped at 40 apps:

```text
CREATOR MODE — you own this device
You are the only application on this Cardputer Zero. The panel, the keyboard
and the storage are yours: everything the user sees is something you built or
will build.

APPS — persistent things you own
- Root: <home>/.dst/apps/<category>/<name>/ — app.json (manifest), APP.md
  (annotation), the entry program and any supporting files.
- When the user asks you to build, change or automate something for the device,
  persist it as an app before your final answer: app_save creates the package
  and the annotation, app_file writes the files, app_run runs it full-screen.
- Write APP.md for the user who opens `/apps` next week: what it does, how to
  use it, how it works, which files matter. Keep it short and factual.
- Update an existing app instead of duplicating it: keep its name, bump the
  version, rewrite only the files that changed.
- A pure question ("what is...") stays an answer; a doc-only deliverable is a
  valid app (kind=doc, the annotation is the content).
- Device limits for apps: 320x170 px, 53x12 cells, 8 ANSI colours, no mouse;
  sh + coreutils (/proc, /sys) and python3 when present; no installs, no
  services, no compilation.
- app_run takes optional space-separated `args` (passed to the entry as argv)
  and a timeout; a run needs an interactive screen, so it fails from background
  jobs and --once.
- App state goes under $DST_APP_STATE (a state/ directory inside the app); apps
  draw on stdout and read keys from stdin; the platform owns long-Esc/timeout.
- Kinds: sh and python scripts, binary (an executable file), prompt (a reusable
  request you follow when run; APP.md is the annotation, PROMPT.md the request),
  doc.
- APP CONTRACT (bounded, the platform enforces what it can): an app owns the
  whole screen while it runs and dst is paused; it may not change system
  configuration, install software, start services, or write outside its
  package/state/tmp; it must exit (0 = success) and leave the screen
  recoverable (the platform repaints). The platform caps CPU time and per-file
  size with ulimits and enforces the Esc/timeout backstop.
- Verify before you show it: call app_test (headless pipes, captured
  stdout/stderr/exit, log file under ~/.dst/tmp) and fix failures; test the
  happy path plus bad input. app_run is for the user to see the real
  full-screen behaviour.

WHAT TO PERSIST — app vs skill vs memory
- app: something runnable on the device (app_save/app_file, verified with
  app_test).
- skill: a systematic, self-contained engineering package you worked out
  (procedure, format, checklist, support files) that will be useful again —
  save it with skill_save/skill_file. It joins the SKILLS catalog next turn and
  is read with the skill tool.
- memory: short durable facts or preferences (memory_write); memory is
  fragments, a skill is a complete procedure.

CONSENSUS — cannot be overridden
- Long-press Esc always leaves an app and returns to the chat. The platform
  watches the keyboard and kills the app; do not try to swallow Esc, do not
  build a fixed-duration blocking screen.
- Apps run full-screen; keep drawing inside 53 columns x 12 rows.

INSTALLED APPS (2)
- system/battery_viewer — Battery viewer [sh:run.sh]: charge level, refreshed live.
- tools/pomodoro — Pomodoro timer [python:main.py]: 25/5 minute cycles.

SKILLS
- battery-report: Use when the user asks for a battery status report.
```

The catalog mirrors `skills::catalog_prompt`: name/title/description/kind (and
entry) only, so the prompt stays cheap; the model reads `APP.md` with
`read_file` when it needs the details.

## 6. Screen takeover contract (`app_run`) and headless testing (`app_test`)

`app_test` is the model's own verification harness: it runs the same entry and
environment through pipes (no screen), feeds optional `stdin`, captures
stdout/stderr up to 64 KB each, reports exit code/timeout/duration, and writes
the full log to `~/.dst/tmp/app-test-<category>-<name>.log`; the model can read
that file with `read_file`. Prompt/doc apps cannot be tested (nothing runs).
With `tty=true` it runs under a pseudo-terminal via `script(1)` — the same kind
of transport as `app_run` — so `[ -t ]` branches and ANSI/TUI output can be
tested headlessly. That pty has no window size; `COLUMNS=53`/`LINES=12` are
exported for apps that honour the env, stdin is line-normalised, and pty echo
appears in the captured output.


```text
model → app_run{name, timeout_s?, args?}
  → runner blocks the calling tool thread, asks the TUI to take over
  → TUI: flush, clear, spawn the entry with stdout/stderr on the panel tty,
    stdin a pipe the runner forwards to; env carries DST_APP, DST_APP_DIR,
    DST_APP_STATE (state/ inside the app), DST_COLS=53, DST_ROWS=12
  → runner loop: forward keys, watch the child, detect long-press Esc, timeout
  → child exits / Esc / timeout → SIGTERM, wait 300 ms, SIGKILL
  → TUI: reset the tty, repaint the whole screen, return a one-line result
```

- Long-press Esc = ≥3 ESC bytes while the hold lasts ≥450 ms. A tap (one ESC)
  is forwarded to the app like any other key; key auto-repeat makes a real hold
  produce many bytes, so it is unambiguous. This matches the existing
  hold-detection used for Tab (3 bytes = hold). After a hold, the runner keeps
  swallowing input until the keyboard has been quiet for 300 ms (2 s cap), so
  the rest of the held key's auto-repeat cannot reach the TUI and cancel the
  turn.
- `timeout_s` defaults to 60, max 600. Model-initiated runs are bounded; the
  user's `/apps` Enter has no timeout (long-Esc is the exit).
- `args` is space-separated (max 32 tokens / 200 chars) and appended after the
  entry file, so `run.sh --fast` arrives as `$1`.
- The runner owns the keyboard, so an app cannot trap the user even if it
  ignores input. Apps read keys from stdin; their stdout is the panel.
- The tool result is one line: ran/exit code/Esc/timeout and duration. App
  output is not captured during a normal `app_run` — the screen is the app's;
  with `capture: true` the bytes are forwarded to the panel *and* written to
  `state/last-run.log`, and the result names the path.
- The result also tells the model not to repeat the app's content in the chat.
- Every launch (screen or headless) goes through a bounded wrapper
  (`/bin/sh -c 'ulimit -c 0; ulimit -t 120; ulimit -f 2048; exec "$0" "$@"'`):
  no core dumps, ≤120 s CPU time, ≤1 MB per file written. The environment is **scrubbed**: nothing is inherited from dst except PATH,
  locale and terminal basics; HOME/TMPDIR point inside `$DST_APP_STATE`
  (`state/home`, `state/tmp`), so dst's API keys/DSH tokens and config are
  invisible to the app. Combined with the working directory, per-app state and
  the Esc/timeout backstop this is the enforceable part of the APP CONTRACT.
- `kind=prompt` does not take the screen: `app_run` returns PROMPT.md as
  instructions for the model to follow in the current turn, and choosing it in
  `/apps` submits it as the next request (the transcript shows `> prompt app
  <name>` instead of the whole body). `kind=doc` opens the annotation panel;
  `kind=binary` executes the entry directly (it must be executable).

## 7. UX

- `/config` gains a `Mode` row: `normal` / `creator` (tokens, like
  `auto/strict/full` for the sandbox). Toggling it closes the menu and reports
  `creator mode on · apps: ~/.dst/apps (2)` / `creator mode off · 2 apps stay
  installed`, and the idle hint switches immediately.
- `/apps` is a full-width menu with the app count in the title
  (`Apps · 3 · 1! · skills 2` in creator mode — apps, failing apps and the
  skill catalog count; `1!` only appears when an app needs fixing),
  rows `Battery viewer   system · sh · 18:20` — most recently
  used apps first, with the last-run/test time in the value column (a `!`
  after the kind means its last run/test failed) — footer
  `↑↓·Enter run·→ read·l log·/ find·x del·Esc`. Enter runs (doc apps
  open their APP.md panel instead); `→` opens the full annotation panel
  (metadata: path/kind/entry/updated/files, then APP.md) for any app; `/`
  starts search mode (typing filters live by name/title/category, Enter keeps
  the filter, Esc cancels; the list title becomes `Apps · 1/3`); `x` asks
  `delete <name>? y/n` in the footer before removing the package. Running an
  app from the menu returns to the menu; leaving it prints the run notice in
  the transcript.
- The idle hint in creator mode is `creator_hint`, which shows the installed
  count, a failing-app counter (`creator · 2 apps · 1! · /apps · hold Esc`
  when an app's last run/test failed), `/apps` and the long-Esc rule, e.g.
  `creator · 2 apps · /apps · hold Esc`,
  so the mode is visible without a second status glyph. The count is cached and
  refreshed when the mode changes, a turn ends or `/apps` closes.
- A saved app is announced in the transcript on one line: `saved app
  system/battery_viewer`; a model-written skill is announced as `saved skill
  battery-report` and appears in the SKILLS catalog and `/extensions` next turn.
- `l` in `/apps` replays the selected app's last recorded output
  (`state/last-run.log`, capture apps only) in the panel viewer; `i` asks the
  model to improve the selected app (creator mode): it closes the launcher and
  pre-fills `Improve the app <category>/<name>: ` in the prompt.
- In creator mode the launcher always exists: with zero apps it still opens
  (title `Apps · 0 · skills N`) and ends with a `+ new app` row. Enter on it
  closes the launcher and pre-fills the prompt with the localized
  "Build an app: " seed plus a one-line hint, so the user only types what the
  app should do and Enter sends it to the model to build. In normal mode an
  empty `/apps` stays a one-line text notice.
- Launching a prompt app from `/apps` first opens its request in the panel
  viewer (Enter sends, Esc goes back), then submits it as the next turn marked
  `> prompt app <name>` — an accidental Enter no longer fires a stored prompt.
- The nudge appears only when the turn actually wrote files through a tool
  (`write_file` / `edit_file` / `app_file` / `skill_file`) and did not call
  `app_save`/`skill_save`; a
  read-only shell inspection does not trigger it.

## 8. Safety

- Creator mode is explicit opt-in and is revoked in one keystroke. Apps persist
  but cannot run themselves: they run only when the model calls `app_run` or the
  user presses Enter in `/apps`.
- `app_save` / `app_file` are classified Safe by the sandbox because they can
  only write inside the app root (the tool validates every path); they never
  cause an extra judge request.
- `app_run` executes a file the model wrote. That is the point of the mode; the
  consent is the mode switch plus the platform-enforced long-Esc exit. The
  prompt forbids installs/services; the sandbox still governs everything else
  the model does.
- Files are writable only under `~/.dst/apps`; the app root itself is a normal
  user directory (no root, no systemd units).

## 9. Verification

- Unit tests: slug/path validation, manifest round-trip, version bump, catalog
  formatting, run-plan selection, Esc-hold detection, tool schema registration,
  `/apps` command dispatch, `/config` Mode row persistence.
- Simulator scenarios (`tools/sim/run_scenarios.py`):
  - `creator_mode` — toggle on/off from the `/config` Mode row, idle hint,
    transcript notice; no model request.
  - `creator_skill_rollback` — two saves build v1/v2, `skill_rollback` restores
    the v1 body as v3 with the pre-rollback body archived under `.history/v2.md`.
  - `creator_apps_run` also checks `default_args` (menu launch passes
    `--fast`, panel shows `args: --fast`) and `creator_app_test` checks the
    launcher title failing count plus `i` pre-filling an improvement request.
  - `creator_home` — creator mode with no apps: `/apps` opens the launcher
    (`Apps · 0 · skills 1`), `+ new app` pre-fills `Build an app:`, typing the
    description and Enter sends it to the model.
  - `creator_tty_test` — `app_test` in pipe mode vs `tty=true`: `[ -t 0/1 ]`,
    `COLUMNS/LINES`, fed stdin and the result transport line all differ as
    expected.
  - `creator_capture` — a capture app prints ANSI lines and a CR progress line;
    the panel echoes them, `state/last-run.log` holds cleaned text, the tool
    result names it, and `/apps` `l` replays it.
  - `creator_import` — restores a seeded `~/.dst/tmp/app-tools-greeter.tar.gz`,
    refuses an archive outside the tmp dir, replaces the installed app with
    `replace=true`, keeps the executable bit, and the app is listed by `/apps`
    and in the next turn's catalog.
  - `creator_skill` — the model writes skills (`SKILL.md` with frontmatter and
    `origin: model`), updates one to v2 (`SKILL.md` version bump plus
    `.history/v1.md`), the transcript announces them, the next turn's system
    prompt contains them in the SKILLS catalog (no nudge), `/extensions` marks
    them `[model]`, `→` shows the full body, the `skill` tool loads a skill in
    the same turn it was written, and `d` + `y` deletes the selected package
    (only the selected one; the catalog drops it next turn).
  - `creator_app_test` — two apps tested headlessly: stdout/argv/stdin capture,
    a 0 exit and a 3 exit, stderr capture, the full log file on disk, and
    `last test: exit 3` recorded in the broken app's panel, a `!` flag and a
    last-use time on its `/apps` row, and the idle hint counts it (`· 1!`).
  - `creator_kinds` — a binary-kind app runs on the panel, a prompt app launched
    from `/apps` is first previewed (Enter sends, Esc cancels), then submitted as
    the next request and the model follows it.
  - `creator_build` — model saves `system/battery_viewer` + writes `run.sh`,
    exports the package with `app_export` and checks the tarball, writes files
    with the annotation, a later turn's system prompt
    contains the app in `INSTALLED APPS` (with its entry file), `/apps` lists it
    and `→` shows the annotation panel.
  - `creator_apps_run` — normal mode, user-driven menu: Enter runs a seeded app
    full-screen, the menu comes back, `/` search filters, a doc app opens its
    annotation panel, and leaving the menu shows the run notice (no model).
  - `creator_run_args` — model `app_run` with `args`: argv arrives in the app,
    `$DST_APP_STATE` points at the package's state directory, and a probe
    variable (`DEEPSEEK_API_KEY`) set in dst's environment does not leak into
    the app; its HOME is `state/home`.
  - `creator_run` — seeded app, model calls `app_run`; the panel shows the app's
    output mid-run, then returns to the TUI with a timeout result; the tool
    card shows the run, the `/apps` annotation panel records
    `last run: timeout 2s`, and the launcher row shows the use time and `!`.
  - `creator_run_esc` — long-press Esc kills a long-running app and returns.
  - `creator_apps_zh` — `/apps` menu, empty state and creator hint in Chinese.
- `run_all.py --quick` (demo + all scenarios), then the full suite; the visual
  contact sheets use the device-fidelity renderer (`tools/sim/fonts/`, 320×170
  native pixels, subpixel AA for CJK) so every review is done at panel
  resolution.
