# Creator mode — one skill package, runnable when it has `meta.json`

Creator mode turns dst from "a chat window on a handheld" into "the only program
on the device, which builds the device's programs". It is opt-in; normal chat
mode stays the default and is unchanged.

The model is **skill-centric**: there is exactly one unit, the skill package
under `~/.dst/skills/<name>/`. A package becomes a runnable **app** by adding a
`meta.json` next to its `SKILL.md`. There is no separate app store, no second
catalog and no app-writing tool — the package folder is the registry.

This document is the reference for the data layout, the model contract, the
writing/reading tools, the execution contract, the `/apps` UX, safety and the
verification matrix. The executable form is intended to be the
`tools/sim/scenarios/creator_*` scenarios plus the unit tests listed in §11 (the
simulator set is ported alongside the code; see the note at the end of §11).

## 1. Why

The Cardputer Zero is a closed handheld: no app store, no browser, no IDE. The
usefulness of the device is bounded by what the user can put on it. A chat UI
answers a question and forgets it; the answer is gone when the screen scrolls.

Creator mode makes *building* the default interaction: when the user asks for
something ("show the battery level", "a pomodoro timer", "a list of the Wi-Fi
networks"), dst builds it, saves it as a permanent package under
`~/.dst/skills/`, and can run it full-screen on the panel. The device accumulates
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

- **R1** A mode flag `tui.mode: normal|creator`, default `normal`, switched from
  the `/config` menu (Mode row). Persisted. Deliberately not a slash command: the
  mode changes the whole contract, so it belongs behind the settings screen, and
  a one-word command is too easy to fire accidentally.
- **R2** One unit: the *skill package*, a folder with `SKILL.md` (frontmatter
  `name`/`description` plus instructions) and optional supporting files. A
  package is a runnable **app** when the folder also contains `meta.json`.
  Writing either kind of package is `skill_save` + `skill_file`; there is no
  `app_save`/`app_file`/`app_export`/`app_import` and no `app_list`.
  Memory stays the place for short durable facts/preferences; the prompt makes
  the split explicit (memory = fragments, skill = complete procedure, runnable
  package = app).
- **R3** Packages persist across sessions and reboots as plain files under
  `~/.dst/skills/` (`/etc/dst/skills/` is the read-only hand-installed library).
- **R4** The model is told which packages exist through the shared `SKILLS`
  catalog injected into the system prompt each turn: one line per package, with
  runnable packages marked `[kind:entry]` and carrying the failure of their last
  run/test. Bodies load on demand (`skill`, `read_file`), so the catalog stays
  cheap.
- **R5** The model can verify a runnable package headlessly with `app_test`
  (pipes by default, a pseudo-terminal with `tty=true`; captured
  stdout/stderr/exit, full log file) and run it on the device screen with
  `app_run` (the app owns the whole panel and all input while dst is paused).
- **R5b** Apps are bounded by a platform contract: platform-enforced long-press
  Esc and timeout, CPU/file-size ulimits, a working directory and
  `$DST_APP_STATE` inside the package, no root; the system prompt states the
  guidelines (no system configuration, installs, services or writes outside the
  package) and the app kinds are `sh`, `python`, `binary`, `prompt` and `doc`.
- **R6** The user can browse runnable packages (`/apps`): list, read
  `SKILL.md`, run (Enter), replay captured output (`l`), ask the model to
  improve (`i`), delete (`x`, with confirmation), search (`/`) and, in creator
  mode, start a new build (`+ new app`).
- **R7** After a turn creator mode names what was saved — `saved skill <name>`
  and/or `saved app <name> (runnable skill)` — or nudges with one dim line when
  the turn wrote files but saved no package (the "no work is lost" rule is
  visible, not just prompted).
- **R8** Everything user-facing is localized in en/zh/es/ja/de/it.

Constraints:

- **C1** The device is 320×170 px, 53×12 cells, 8 ANSI colours, no mouse. Apps
  and their saved text must fit that; the prompt says so. `state_report` lines
  are capped at 50 display cells so they fit the top row.
- **C2** 512 MB / 4×A53: no on-device compilation, no package installs in the
  app flow. Apps are `sh` scripts (coreutils, /proc, /sys), `python` scripts when
  the interpreter exists, binaries already on the device, or prompt/doc-only
  packages.
- **C3** Package writes are restricted to `~/.dst/skills/<name>/`; traversal,
  absolute paths, backslashes and `SKILL.md` via `skill_file` are rejected by the
  tools, not just the prompt. Deleting refuses anything outside the user skills
  dir.
- **C4** Screen takeover only exists in the interactive TUI. `--once`,
  background jobs and `--call` fail with a clear error instead of hanging.

Non-goals: replacing fbterm/the OS; running services in the background (jobs and
watchers already cover that); grabbing the framebuffer; a GUI toolkit; a
separate app store or registry file.

## 3. Data layout

```text
~/.dst/skills/
    battery-viewer/            app: skill package + meta.json
        SKILL.md               frontmatter name/description + instructions
        meta.json              manifest: makes the package runnable
        run.sh                 entry program (kind=sh)
        state/                 $DST_APP_STATE: HOME, TMPDIR, last-run.log
        .history/v1.md         previous SKILL.md bodies (kept by skill_save)
    report-writer/             prompt app
        SKILL.md
        meta.json              kind=prompt, no entry
        PROMPT.md              the reusable request
    field-notes/               plain skill: not runnable, still in the catalog
        SKILL.md
```

`/etc/dst/skills/<name>/` uses the same layout for hand-installed packages; it is
read-only (`/extensions` cannot delete from it).

### 3.1 `SKILL.md`

```text
---
name: battery-viewer
description: Use when the user asks for the battery level or charging state.
origin: model
version: 2
updated: 2026-11-17T15:00:00+08:00
---

# Battery viewer

Read /sys/class/power_supply/BAT0/... and print two short lines.
```

- `name`/`description` are the catalog entry; the body is what `skill` returns
  and what `/apps` `→` shows. Keep `name` equal to the folder name; `/apps`
  resolves and lists packages by folder name.
- `origin: model`, `version` and `updated` are written by `skill_save` for
  packages dst wrote itself. Hand-installed packages may omit them.

### 3.2 `meta.json`

The presence of this file is the *only* switch that makes a package runnable.
It is written by the model with `skill_file`; dst rewrites it to record runs.

```json
{
  "title": "Battery viewer",
  "kind": "sh",
  "entry": "run.sh",
  "requires": ["sh"],
  "default_args": "--fast",
  "capture": false,
  "version": 1,
  "created": "2026-11-17T15:00:00+08:00",
  "updated": "2026-11-17T15:00:00+08:00",
  "last_run": {"at": "2026-11-17T15:01:00+08:00", "secs": 1.2, "outcome": "exit 0"},
  "last_test": {"at": "2026-11-17T15:01:40+08:00", "secs": 0.4, "outcome": "exit 3"}
}
```

| field | written by | meaning |
| --- | --- | --- |
| `title` | model | human label in `/apps`; defaults to the folder name with `-`/`_` turned into spaces |
| `kind` | model | `sh`, `python`, `binary`, `prompt` or `doc`; any other value makes the package non-runnable |
| `entry` | model | relative file inside the package; required for `sh`/`python`/`binary`, empty for `prompt`/`doc` |
| `requires` | model | binaries the app needs: names on `PATH` or absolute paths, max 8; checked before every run/test |
| `default_args` | model | whitespace-split argv appended to the entry when `/apps` launches it (an `app_run` `args` value wins) |
| `capture` | model | record the app's stdout/stderr to `state/last-run.log`; implies piped output (not a tty) |
| `version` | model | build number shown in the `/apps` panel (default 1); it is independent of the `SKILL.md` version that `skill_save` maintains |
| `created`, `updated` | model | ISO local timestamps shown in the `/apps` panel |
| `last_run`, `last_test` | **dst** | `{at, secs, outcome}` after each run/test; treat as read-only |

Rules:

- The folder name is the app name: a flat slug (`[a-z0-9][a-z0-9_-]{0,47}`), no
  category. A legacy `category/name` string is still accepted when resolving an
  `app_run` name from an old prompt/session, but it never selects a subdirectory.
- A package whose `meta.json` has an unknown `kind`, invalid JSON, or a missing
  `entry` for a program kind is silently **not an app**; it stays a plain skill
  in the catalog.
- dst rewrites `meta.json` when it records a run/test and preserves only the
  fields in the table above. Keep generated data out of it (and out of
  `SKILL.md`); put runtime state in `state/`.
- `description` for `/apps` comes from the `SKILL.md` frontmatter; an optional
  `meta.json` `description` wins when present, but the catalog text belongs in
  `SKILL.md` (it is also what the `skill` tool returns).
- A failed last run/test is the health flag: `exit 0`, `esc` and an empty stamp
  are healthy; everything else marks the package with `!` in `/apps`, counts it
  in the idle hint (`.dst` shows `creator · 2 apps · 1! · /apps · hold Esc`) and
  appends ` (!last test: exit 3)` to its catalog line.

### 3.3 Kinds

| `kind` | `entry` | execution | Enter in `/apps` |
| --- | --- | --- | --- |
| `sh` | script | `/bin/sh <entry> [args]` | full-screen run |
| `python` | script | `python3` (or `python`) `<entry> [args]`; fails if no interpreter | full-screen run |
| `binary` | executable | executes the entry directly; must have the executable bit (`chmod +x` via bash) | full-screen run |
| `prompt` | — | nothing runs: `app_run` returns `PROMPT.md` (else the `SKILL.md` body) as instructions to follow now | request is previewed; Enter sends it as the next turn, Esc goes back |
| `doc` | — | nothing runs; `app_run`/`app_test` return an actionable error | opens the `SKILL.md` panel |

A `prompt`/`doc` package needs no entry file, but still needs `meta.json` to
appear in `/apps`. A deliverable that needs no program does not need `meta.json`
at all: a plain skill is valid.

### 3.4 State, history and limits

- `$DST_APP_STATE` is created by dst before every run/test: it is the package's
  `state/` directory. `HOME` is `state/home`, `TMPDIR` is `state/tmp`, and the
  working directory is the package folder.
- `capture: true` apps get their cleaned output (ANSI/CR stripped, last 64 KB)
  written to `state/last-run.log`; `/apps` `l` replays it and `app_run`'s result
  names the path.
- `skill_save` keeps the previous `SKILL.md` under `.history/vN.md` and prunes to
  the last five snapshots. `skill_rollback` restores an earlier body as a new
  version.
- Tool-enforced limits: `SKILL.md` body ≤12,000 characters, description ≤200,
  package name ≤48 characters, one `skill_file` write ≤32 KB, package total
  ≤256 KB, safe relative paths only (no absolute paths, `..`, backslashes or
  control characters; ≤120 characters). `/apps` loads at most 200 runnable
  packages (`MAX_APPS`).

### 3.5 Migration from the old app layout

The pre-refactor packages under `~/.dst/apps/<category>/<name>/` (`app.json` +
`APP.md` + program) are **not auto-migrated**, and there were none on the shipped
device. To recreate one by hand (or to port one from a development machine):

1. Create `~/.dst/skills/<name>/` (the folder name is the new app name; the old
   category disappears — skill names are flat).
2. Write `SKILL.md` with frontmatter `name` (same as the folder) and
   `description`, and use the old `APP.md` content as the body. The model path is
   `skill_save`, which handles versioning/`.history`.
3. Write `meta.json` with `title`/`kind`/`entry`/`requires`/`default_args`/
   `capture`/`version` from the old `app.json`. The model path is `skill_file`.
4. Keep the entry program (`run.sh`, `main.py`, the binary) and `PROMPT.md` in
   the same folder. Runtime state is not migrated; the app recreates `state/`.
5. There is no `app_export`/`app_import`: back up or copy a package with `bash`
   (`tar -C ~/.dst/skills -czf backup.tgz <name>`) like any other directory.

Old `/apps` labels such as `system/battery_viewer` are no longer displayed; the
launcher shows the folder name and title, and the catalog uses the skill name.

### 3.6 Shipped skills: `hardware-info`

dst ships one skill package, `~/.dst/skills/hardware-info/`, installed from a
seed embedded in the binary on first start (only when the package is missing;
a scanned or refined copy is never overwritten). It is a normal skill — model
readable with the `skill` tool, listed in the catalog, managed by
`/extensions` — but dst itself maintains its measured section:

- the reference body (parts, how dst talks to the panel/keys/audio/battery,
  safe probe commands, hardware-code rules, sources) is the seed/file content;
- between `<!-- hw-scan:begin -->` and `<!-- hw-scan:end -->` the owner is
  `src/hwinfo.rs`: a bounded, read-only scan (every probe has a timeout) writes
  the measured state, the version bumps only when the measured hardware
  changed, the previous copy goes to `.history/` and the raw capture to
  `state/scan-latest.txt`;
- first-run setup offers the scan once (`tui.hw_survey_done` records that it was
  asked); `/update-hwinfo` re-runs it later. In creator mode the command also
  asks the model to read the refreshed skill and refine it with `skill_save`;
  in normal mode the scan alone is applied.

This is the ground truth for hardware tasks ("show the battery", "read the
IMU", "what is on the i2c bus"): answer from the skill before probing blindly,
and prefer the measured current state over the reference assumptions.

## 4. Modes: what changes

| | normal | creator |
|---|---|---|
| system prompt | base | base + `CREATOR MODE` block (the `SKILLS` catalog is shared) |
| tools | 32 base tools | + `app_run`, `app_test`, `skill_save`, `skill_file`, `skill_rollback` |
| writing | hand-installed skills only | + model can create/update/roll back packages |
| `/apps` | browse/run/delete; empty state is a one-line notice | always opens; adds `i` (improve) and `+ new app` |
| idle hint | `idle_hint` (`Up/Down hist · PgUp/PgDn · CtrlO out · /help`) | `creator_hint` (`creator · N apps · /apps · hold Esc`, plus `N!` when packages are broken) |
| after a turn | nothing | saved-skill/saved-app notices, or the unsaved-work nudge |
| conversation | unchanged | unchanged (packages are additive) |

`/config` gains a Mode row (`normal` / `creator`). Toggling it closes the menu
and reports `creator mode on · apps: ~/.dst/skills (N installed)` or
`creator mode off · N apps stay installed`; the registry and the idle hint
switch immediately and the prompt is rebuilt at the start of the next turn
(it is rebuilt every turn anyway, so a package written in turn N is catalogued
in turn N+1). There is deliberately no `/creator` command.

## 5. Model contract (system-prompt block)

The creator block is appended only in creator mode (`src/apps.rs::prompt_section`).
Abridged; the catalog itself is the shared `SKILLS` block:

```text
CREATOR MODE — you own this device
You are the only application on this Cardputer Zero. The panel, the keyboard and
the storage are yours: everything the user sees is something you built or will
build.

ONE UNIT: THE SKILL PACKAGE
- Everything you persist lives in ~/.dst/skills/<name>/: SKILL.md (frontmatter
  name+description, then the instructions), optional supporting files, and
  optional meta.json.
- A skill package is an APP when its folder also has meta.json: that file
  declares kind (sh|python|binary|prompt|doc), entry, requires, default_args and
  capture. Multiplied by nothing else — there is no separate app store and no
  app_* writing tool.
- Create/update a skill with skill_save (SKILL.md + frontmatter, versioned) and
  put every other file — run.sh, PROMPT.md, meta.json — in the folder with
  skill_file. Writing meta.json is the step that makes the package runnable, and
  the platform reads it to list, run and test the app.
- The SKILLS catalog injected below is the single source of truth: a runnable
  skill shows its kind and entry ([sh:run.sh]) and a failed last run/test. Read
  details with read_file (SKILL.md, meta.json) instead of guessing; there is no
  app_list tool because the folder IS the list.
- Update an existing package instead of duplicating it: keep its name, rewrite
  only the files that changed; skill_save bumps the version and keeps the old
  SKILL.md under .history/ (restore it with skill_rollback).
- A pure question ("what is...") stays a chat answer. A deliverable that needs
  no program is still a valid app: kind=doc (SKILL.md is the content) or no
  meta.json at all (a plain skill).
- Device limits for apps: 320x170 px, 53 columns x 12 rows, 8 ANSI colours, no
  mouse. Use sh + coreutils (/proc, /sys); python only when present. No
  installs, no services, no compilation.
- App state: write runtime state under $DST_APP_STATE (a state/ dir inside the
  package); keep SKILL.md and meta.json free of generated data. Apps draw on
  stdout and read keys from stdin; the platform guarantees a long-press of Esc
  and a timeout.
- APP CONTRACT (the platform enforces what it can): an app owns the whole screen
  while it runs and dst is paused; it may not change system configuration,
  install software, start services, or write outside its package/state/tmp; it
  must exit (0 = success) and leave the screen recoverable.
- Verify before you show it: call app_test (captured stdout/stderr/exit, log file
  under ~/.dst/tmp) and fix failures; test the happy path plus bad input.
  app_test is headless by default; with tty=true it runs under a pseudo-terminal
  like app_run (the pty has no window size, so COLUMNS/LINES stand in for the
  real size). app_run is for the user to see the real full-screen behaviour.
- Declare the binaries an app needs in meta.json requires; dst checks them before
  every run/test. Use default_args for fixed arguments from /apps; app_run args
  override them. Set capture=true on a text app you want remembered: dst records
  its output to state/last-run.log (shown in /apps with l) and names the path in
  app_run's result. Capture makes stdout a pipe, so do not use it for TUI apps.

SKILL vs MEMORY
- skill: a systematic, self-contained engineering package ... useful again.
- memory: short durable facts or preferences (memory_write); memory is
  fragments, a skill is a complete procedure.

WHAT THE USER SEES
- /apps lists the runnable packages (skill folders with meta.json), newest
  activity first: Enter runs, Right reads SKILL.md, l replays the last captured
  output, i asks you to improve it, x deletes it. Long-press Esc always leaves a
  running app.
- A saved skill is announced in the transcript; a package that gained meta.json
  is announced as a saved app. After a turn that wrote files but saved no skill,
  you are nudged to persist it.
```

The `SKILLS` catalog (every mode) is one line per enabled package:

```text
SKILLS
These packaged instructions are available on demand. ...

- battery-viewer (v2) [sh:run.sh]: Use when the user asks for the battery level.
- report-writer [prompt]: Use when the user wants a written report drafted.
- broken-clock [sh:run.sh] (!last test: exit 3): Use for a clock on the panel.
- field-notes: Use when the user asks about field observations.
```

`(vN)` appears from `SKILL.md` version 2 on; `[kind:entry]` (or just `[kind]` for
`prompt`/`doc`) marks runnable packages; the health suffix names the most recent
failed stamp and ends the catalog line's marker. The model reads the details with
`read_file` instead of paying for them in every request.

### 5.1 Shared prompt blocks that shape creator work

These are in the base system prompt in every mode; they are what makes a build
turn readable on the panel:

- `LANGUAGE` — the UI language is pinned as the **default answer language**
  (`prose, artifact bodies, state reports, questions and errors`). A user request
  for another language, or a request to match the language they write in, wins
  and stays in effect. The UI language itself is set by `tui.language` /
  `/config`.
- `PROGRESS ROW` — call `state_report` with ONE short line when a long task
  changes stage. It is pinned to the top row while the turn runs, must fit one
  row (at most 50 display cells on the 53-column panel, CJK counts as 2) or the
  call is rejected, and must not contain secrets.
- `STYLE` — concise answers, no narration of steps; on interrupt, stop
  immediately and do not retry the cancelled work.

## 6. Writing and reading packages

All writing goes through two tools registered only in creator mode; reading works
in every mode.

### 6.1 `skill_save`

Parameters: `name`, `description` (max 200 chars, whitespace-normalised),
`instructions` (the complete markdown body, max 12,000 chars).

- Creates `~/.dst/skills/<name>/SKILL.md` with frontmatter
  `name`/`description`/`origin: model`/`version`/`updated` (a title-ish name is
  slugged: lowercase, whitespace → `-`).
- On update: archives the current body to `.history/v<N>.md` (last five kept) and
  bumps `version`; the visible version is `(vN)` in the catalog and in
  `/extensions`.
- Announced in the transcript as `saved skill <name> vN`; `skill_save` alone
  does not make a runnable app.

### 6.2 `skill_file`

Parameters: `name`, `path` (relative path inside the package), `content`
(max 32 KB; the whole package ≤256 KB).

- Requires an existing package (`call skill_save first`); paths are validated
  (no absolute paths, no `..`, no backslashes, no control characters).
- `SKILL.md` is refused here — it belongs to `skill_save`. Everything else is
  fair game: `run.sh`, `main.py`, `PROMPT.md`, templates, reference data and
  `meta.json`.
- Writing `meta.json` is the moment a package becomes runnable; the transcript
  announces `saved app <name> (runnable skill)`. Other support files are not
  announced.

### 6.3 `skill_rollback`

Restores the body of an earlier version as a **new** version (current body is
archived first, so a rollback is itself reversible). Omit `version` to go back
one step. Unknown versions fail with the list of available versions.

### 6.4 Reading

- `skill` loads the body of a package (the result includes the package path so
  relative references resolve); it falls back to a fresh disk scan, so a skill
  written earlier in the same turn can be loaded immediately.
- `read_file` reads raw package files (`SKILL.md`, `meta.json`, entry programs)
  and is the right way to inspect details instead of guessing.
- `/apps` `→` shows the same `SKILL.md` body to the user.
- There is no `app_list`: the `SKILLS` catalog is the list, and the folder is the
  source of truth.

## 7. Execution: `app_test` and `app_run`

Both tools are registered only in creator mode, and both refuse `prompt`/`doc`
packages (nothing to execute; `app_run` on a prompt returns its instructions
instead).

### 7.1 `app_test` — headless verification

Parameters: `name`, `args` (space-separated argv), `stdin` (text piped to stdin,
closed afterwards), `timeout_s` (default 10, max 120), `tty` (boolean).

- Default: pipes, no screen. Captures stdout/stderr up to 64 KB each, reports
  exit code/timeout/duration/bytes, and writes the full log to
  `~/.dst/tmp/app-test-<name>.log` (readable with `read_file`).
- `tty=true`: runs under a pseudo-terminal via `script(1)` — the same transport
  class as `app_run` — so `[ -t ]` branches and ANSI/TUI output can be tested.
  The pty has no window size; `COLUMNS=53` / `LINES=12` are exported for apps
  that honour the environment, and stdin is line-normalised.
- The run is recorded back into the package's `meta.json` as `last_test`
  (`outcome` = `exit N`, `timeout Ns` or `signal`).

### 7.2 `app_run` — full-screen on the panel

Parameters: `name`, `args` (max 200 characters, split on whitespace into at most
32 argv tokens, appended after the entry file), `timeout_s` (default 60, max
600).

Preflight: the package must exist and `requires` must be satisfied (missing
binaries fail with the exact name).

Screen takeover:

```text
model → app_run{name, timeout_s?, args?}
  → the tool thread blocks and asks the TUI to hand over the panel
  → TUI: flush, clear, spawn the entry: stdout/stderr on the panel tty, stdin a
    pipe the runner forwards to; env carries DST_APP, DST_APP_DIR, DST_APP_STATE,
    DST_COLS=53, DST_ROWS=12 (and COLUMNS/LINES=53/12)
  → runner loop: forward keys, watch the child, detect long-press Esc, timeout
  → child exits / Esc / timeout → SIGTERM, wait 300 ms, SIGKILL
  → TUI: reset the tty, repaint the screen, return a one-line result
```

- **Long-press Esc = ≥3 ESC bytes while the hold lasts ≥450 ms.** A tap (one
  ESC) is forwarded to the app like any other key; key auto-repeat makes a real
  hold produce many bytes. After a hold, the runner swallows input until the
  keyboard has been quiet for 300 ms (2 s cap), so the rest of the auto-repeat
  cannot reach the TUI and cancel the turn.
- `timeout_s` defaults to 60 and is capped at 600. A launch from the user's
  `/apps` Enter has a one-hour backstop (`DIRECT_TIMEOUT_S`) because long-press
  Esc is the real exit there.
- The environment is **scrubbed**: nothing is inherited from dst except `PATH`,
  locale (`LANG`/`LC_ALL`/`TZ`) and terminal basics (`TERM`, `COLORTERM`).
  `HOME`/`TMPDIR` point inside `$DST_APP_STATE` (`state/home`, `state/tmp`), so
  dst's API keys/DSH tokens and config are invisible to the app. The working
  directory is the package.
- Every launch goes through a bounded wrapper (`/bin/sh -c 'ulimit -c 0;
  ulimit -t 120; ulimit -f 2048; exec "$0" "$@"'`): no core dumps, capped CPU
  time and file size. The Esc/timeout backstop is the platform guarantee the app
  cannot disable.
- The result is one line: ran and exit code / stopped by Esc / timeout, plus
  duration; with `capture: true` it also names `state/last-run.log`. The model is
  told not to repeat the app's content afterwards.
- The run is recorded back as `last_run` in `meta.json`; `/apps` shows the last
  use time, a `!` when the last run/test failed, and the catalog line carries the
  same health suffix.
- `app_run` needs the interactive screen: from `--once`, background jobs or
  `--call` it fails with an error instead of waiting forever.

## 8. Turn-time behaviour on the 53×12 panel

The following UI changes are part of the creator-mode workflow (they apply in
every mode):

- **Pinned progress row.** `state_report` puts one short line on the top row in
  bright magenta with a `»` icon. A newer report replaces it; when the next
  transcript line appears (or the turn ends) the report is committed to the log
  with the same icon and the row is freed. No tool card is drawn for it — the
  pinned row is the whole presentation. The line is normalised to one row and
  rejected above 50 display cells, so it can never wrap or push the input off
  the panel.
- **Tool cards show their arguments.** When a call starts the transcript gets
  `* <tool>` plus the call's arguments as indented lines (long values folded,
  capped at five rows, then `… more: CtrlO`). When it settles the card gets
  `└ <secs>s ok` / `└ <secs>s x` and a one-line `↳` preview of the result. The
  full arguments and result stay available in `CtrlO`.
- **`CtrlO` opens during a turn.** It lists the last 20 tool calls (full args,
  result up to 6,000 chars; failures marked `x`): `↑↓` selects, Enter opens the
  full call + result in the panel viewer, `s` saves the pair under
  `~/.dst/tmp/tool-<session>-<tool>.txt`, Esc/`q` closes. The turn keeps
  streaming behind it.
- **Paging.** `PgUp`/`PgDn` move the transcript by one screenful; the scroll
  clamps so the top of the transcript stops at the top row instead of scrolling
  into blankness. List menus page the same keys: a page key is replayed as a
  screenful of `↑`/`↓`, so the list window moves and the selection follows. On an
  empty prompt `↑`/`↓` first recall input history (and scroll only when there is
  no history left to recall); with an empty prompt and a scrolled transcript,
  `Home`/`End` jump to top/bottom.
- **One interrupt stops the whole turn.** `Esc` cancels the turn at once — not
  just the tool in flight — and stops any speech. Text already typed becomes the
  next message (queued at the front and announced as
  `⚡ interrupted, sending now: …`); messages queued during the turn are sent
  right after it, in order. An interrupted goal round disarms the goal
  (`/goal resume` re-arms).
- **Tab during a turn is an interrupt-and-talk gesture.** It stops the turn and
  starts recording immediately (the microphone is live before the turn unwinds,
  so the first words are kept). On release the transcript is queued as the next
  message when `tui.auto_send_voice: true` (the default), or left in the prompt
  for editing when it is off. Idle, Tab is hold-to-talk: a tap is ignored; speech
  needs the official DeepSeek API.

On the device there are no dedicated arrow or page keys: hold the sticky `fn`
modifier to press `fn`+`F`/`X`/`Z`/`C` for up/down/left/right and `fn`+`L`/`M` for
PageUp/PageDown. The terminal sees standard cursor sequences (`ESC[5~` /
`ESC[6~`), and dst decodes them in one place.

## 9. `/apps` — the launcher

Bare `/apps` opens the launcher. In normal mode with no runnable packages it
prints a one-line notice (`no apps yet — switch to creator mode in /config`);
creator mode always opens it, even with zero packages, so `+ new app` is always
reachable.

- **Title:** `Apps · N` (normal); in creator mode `Apps · N · B! · skills S` —
  runnable packages, broken packages (the `!` group appears only when B > 0) and
  the size of the whole skill catalog. `/` search shows `Apps · filtered/total`.
- **Rows:** `title` on the left, value on the right = `kind` + ` !` when the last
  run/test failed + ` · HH:MM` of the most recent run or test. Order: newest
  activity first, then name. Up to 10 rows fit on the 12-row panel; longer lists
  show a `sel/total` counter and window around the selection.
- **Keys:**
  - `↑`/`↓` move (PageUp/PageDown page the list).
  - **Enter** — `sh`/`python`/`binary`: run full-screen (the menu returns after
    the app exits or is long-Esc'd, then the transcript says `app <name>
    returned to dst`). `doc`: open the `SKILL.md` panel. `prompt`: preview the
    request first (Enter sends, Esc goes back, `s` saves); sending submits it as
    the next turn marked `> prompt app <name>` and the model follows the body.
  - **`→`** — detail panel: `path`, `kind`, `entry` (when set), `version`,
    `updated`, `default_args`, `requires`, last run/test, the first files with
    sizes, then the `SKILL.md` body. Scrollable; Esc closes.
  - **`l`** — replay `state/last-run.log` (capture apps only) with the last run
    stamp; without a log it says so. Esc closes.
  - **`i`** (creator mode) — close the launcher and pre-fill
    `Improve the app <name>: ` plus a hint; describe the change and press Enter.
  - **`x`** — ask `delete <name>? y/n` in the footer, then remove the whole
    package directory (refused outside the user skills dir).
  - **`+ new app`** (creator mode, last row) — close the launcher and pre-fill
    `Build an app: ` plus a hint, so the user only describes what the app should
    do; Enter sends it to the model.
  - **`/`** — search mode: typing filters live over name/title/description,
    Enter keeps the filter (`filter: … · / edit · Esc clear`), Esc cancels.
  - **Esc/`q`/Ctrl-C** — close the launcher (`Esc` first clears an active
    filter).
- `/apps run <name>` runs one package directly without the menu (same long-press
  Esc exit); an unknown name prints `no app '<name>'`.

## 10. Safety

- Creator mode is explicit opt-in through `/config` and is revoked in one
  keystroke. Packages persist but cannot run themselves: they run only when the
  model calls `app_run` or the user presses Enter in `/apps`.
- `skill_save`, `skill_file`, `skill_rollback`, `app_test` and `app_run` are
  classified Safe by the sandbox (they never trigger the LLM judge): the write
  tools validate every path inside the user skill library, and running a package
  the user asked dst to build is the point of the mode.
- `app_run` executes a file the model wrote. The consent is the mode switch plus
  the platform-enforced long-press Esc; the prompt forbids installs/services and
  the sandbox still governs everything else the model does.
- Files are writable only under `~/.dst/skills/<name>/`; the package is a normal
  user directory (no root, no systemd units, no writes outside it).
- The APP CONTRACT is stated in the prompt and enforced where the platform can:
  bounded CPU/file size, scrubbed environment, per-package state, and the
  Esc/timeout backstop.

## 11. Verification

| Layer | Check | Covers |
| --- | --- | --- |
| unit tests | `cargo test` | `apps.rs`: packages load only with `meta.json`, known kind + entry required, legacy `category/name` lookup, run-plan interpreter/argv, requires preflight, run stamps written back into `meta.json`, Esc-hold detection, tool defs are execution-only (`app_test`/`app_run`). `skills.rs`: frontmatter parsing, save/update versioning + `.history`, rollback semantics, catalog formatting. Plus the `/apps` command dispatch and i18n parity tests |
| registry | `mkdir -p /tmp/h/.dst && printf 'tui:\n  mode: creator\n' > /tmp/h/.dst/config.yml` then `HOME=/tmp/h ./target/debug/dst --tools` | prints `mode=creator` and the effective set: base 32 + `app_run,app_test,skill_file,skill_rollback,skill_save`; no `app_save`/`app_file`/`app_export`/`app_import`/`app_list` |
| simulator | the `creator_*` scenarios in `tools/sim/scenarios/` (`run_scenarios.py`; 92 checked scenarios in total) | mode switch, packages written with `skill_save`/`skill_file` + `meta.json`, catalog line `[sh:run.sh]` and health suffix, `/apps` list/search/run/delete/detail/log/new/improve, skill versioning + rollback, prompt/doc/binary kinds, requires refusal, headless/tty tests, capture replay, run/run+args/long-Esc, zh localization, a real end-to-end build-and-run flow (`creator_app_flow`) |
| images | `python3 tools/sim/make_demo_webp.py` | rebuilds the committed device-fidelity images: `docs/demo.webp` (animated tour), `docs/live-walkthrough.png`, `docs/live-creator.png` (creator sheet: package written, `SKILL.md` in `/apps`, launcher, app running) and the six-language login sheet |
| live (optional) | `tools/sim/live_creator_check.py` | with a real endpoint, the model builds a battery readout package and `/apps` shows it; see `docs/UX-LIVE.md` |
| device | manual pass | long-press Esc key-repeat rate, `/apps` Enter on the panel, prompt-app confirmation, capture log replay, `state/` writes on the real filesystem |

Scenario names map to the contract; if a `creator_*` scenario still asserts
`app.json`/`APP.md`/`~/.dst/apps`, it predates the skill-centric refactor and
must be ported with this document. The unit tests above are the contract that
runs everywhere, with no hardware and no network. The screenshots in `docs/`
are simulator renders rebuilt after the refactor
(`python3 tools/sim/make_demo_webp.py`), so the creator sheet shows the current
`SKILL.md` + `meta.json` workflow.
