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
- R2 In creator mode the model can persist work as an app package: manifest,
  short human annotation, entry program and supporting files.
- R3 App packages persist across sessions and reboots (plain files under
  `~/.dst/apps/`).
- R4 The model is told which apps already exist, skills-style: a one-line
  catalog is injected into the system prompt each turn; the annotation body is
  read on demand (`read_file`), mirroring the skills catalog/`SKILL.md` split.
- R5 The model can run a saved app on the device screen (`app_run`). The TUI
  returns when the app exits, is long-Esc-killed, or times out.
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
  "version": 1,
  "created": "2026-11-17T15:00:00+08:00",
  "updated": "2026-11-17T15:00:00+08:00"
}
```

Rules:

- `name` is a slug (`[a-z0-9][a-z0-9_-]{0,31}`), unique within the category;
  category is a slug, default `apps`.
- `kind` is `sh` | `python` | `doc`; `entry` is a relative file inside the app
  dir and is required unless `kind=doc` (Enter in `/apps` opens APP.md in the
  artifact panel instead).
- `description` is one line (≤160 chars); `annotation` (APP.md) is a short
  markdown body (≤4000 chars): what it does, how to use it, how it works, the
  files. It is written for the user reading `/apps` later, not for the model.
- Saving an existing name updates in place and bumps `version` (and `updated`);
  files not rewritten are kept, so a one-file fix does not need a full rebuild.
- Writes go through `app_save` / `app_file`; each file is capped (32 KB per
  file, 256 KB per app) so a model cannot fill the microSD through the tool.

## 4. Modes: what changes

| | normal | creator |
|---|---|---|
| system prompt | base | base + `CREATOR MODE` block + installed-app catalog |
| tools | base set | + `app_save`, `app_file`, `app_run` |
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

CONSENSUS — cannot be overridden
- Long-press Esc always leaves an app and returns to the chat. The platform
  watches the keyboard and kills the app; do not try to swallow Esc, do not
  build a fixed-duration blocking screen.
- Apps run full-screen; keep drawing inside 53 columns x 12 rows.

INSTALLED APPS (2)
- system/battery_viewer — Battery viewer [sh]: charge level, refreshed live.
- tools/pomodoro — Pomodoro timer [python]: 25/5 minute cycles.
```

The catalog mirrors `skills::catalog_prompt`: name/title/description/kind only,
so the prompt stays cheap; the model reads `APP.md` with `read_file` when it
needs the details.

## 6. Screen takeover contract (`app_run`)

```text
model → app_run{name, timeout_s?}
  → runner blocks the calling tool thread, asks the TUI to take over
  → TUI: flush, clear, spawn the entry with stdout/stderr on the panel tty,
    stdin a pipe the runner forwards to
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
- The runner owns the keyboard, so an app cannot trap the user even if it
  ignores input. Apps read keys from stdin; their stdout is the panel.
- The tool result is one line: ran/exit code/Esc/timeout and duration. App
  output is not captured — the screen is the app's during the run.
- The result also tells the model not to repeat the app's content in the chat.

## 7. UX

- `/config` gains a `Mode` row: `normal` / `creator` (tokens, like
  `auto/strict/full` for the sandbox). Toggling it closes the menu and reports
  `creator mode on · apps: ~/.dst/apps (2)` / `creator mode off · 2 apps stay
  installed`, and the idle hint switches immediately.
- `/apps` is a full-width menu: `Battery viewer   system · sh`, footer
  `↑↓ · Enter run · → open · x delete · Esc`. Right expands a compact preview
  (path, kind, entry, updated, two annotation lines; the full APP.md fits in
  `/apps`' panel for kind=doc). Enter runs; for `doc` apps Enter opens the
  panel instead. `x` asks `delete <name>? y/n` in the footer before removing
  the directory.
- The idle hint in creator mode is `creator_hint`, which names `/apps` and the
  long-Esc rule, so the mode is visible without a second status glyph.
- A saved app is announced in the transcript on one line: `saved app
  system/battery_viewer`.
- The nudge appears only when the turn actually wrote files through a tool
  (`write_file` / `edit_file` / `app_file`) and did not call `app_save`; a
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
  - `creator_build` — model saves `system/battery_viewer` + writes `run.sh`,
    files appear on disk with the annotation, a later turn's system prompt
    contains the app in `INSTALLED APPS`, `/apps` lists it and previews APP.md.
  - `creator_run` — seeded app, model calls `app_run`; the panel shows the app's
    output mid-run, then returns to the TUI with a timeout result; the tool
    card shows the run.
  - `creator_run_esc` — long-press Esc kills a long-running app and returns.
  - `creator_apps_zh` — `/apps` menu, empty state and creator hint in Chinese.
- `run_all.py --quick` (demo + all scenarios), then the full suite; the visual
  contact sheets use the device-fidelity renderer (`tools/sim/fonts/`, 320×170
  native pixels, subpixel AA for CJK) so every review is done at panel
  resolution.
