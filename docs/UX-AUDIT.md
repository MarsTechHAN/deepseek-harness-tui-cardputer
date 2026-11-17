# dst UX audit (12-row panel, 53 columns)

Method: every screen is exercised through the pty simulator, captured as text
+ PNG, and reviewed at the real cell size. Two passes feed this catalogue:
`run_scenarios.py` (scripted mock API, deterministic assertions) and
`run_live.py` (the real local vLLM endpoint, only TTS/ASR mocked) for the
common-usage walkthrough; see `docs/UX-LIVE.md` and
`docs/live-walkthrough.png`. All rows were inspected visually, not only
asserted in code.

## Screens reviewed

| Area | Screen(s) | Result |
| --- | --- | --- |
| Startup | splash breach scene + hint, idle hints, first-run tips | splash is an 18-frame 53×9 scene where the original 9-row pixel whale is moved against a dim water line (spout → breach → apex fluke swing → fluke/back dive → splash → `dst` wordmark reveal) with a dim sky row and a dim sea row framing it; after three loops it settles into a sleeping loop. The hint stays on one 53-cell row with long model names in all six languages, and tips still open with the tagline and are cleared by the first prompt |
| Onboarding | endpoint menu, sandbox picker | ok: previews explain tradeoffs, rows full width |
| Input line | long prompts, editing keys, wide CJK text | fixed: the one-line input window scrolls around the cursor with `…` markers on the hidden sides (both sides for a mid-line cursor); Ctrl-A on a 136-char prompt used to print the whole string and wrap over the status row. Home/End/Ctrl-A/Ctrl-E and forward Delete are covered by the `long_input` scenario |
| Chat | live thinking, streaming tail, steer marker, injected context | fixed: reasoning wraps at word boundaries; model-facing context (repeat reminders, goal wrap-up) is a one-line marker instead of a dumped paragraph, so the answer stays on screen |
| Compaction | automatic + manual compact, notices | fixed: `compacted: 1.3K -> 1.2K tokens` keeps one decimal for four-digit counts (instead of a meaningless `1K -> 1K`); a stale `compacting N messages` live row no longer stays on screen after the compaction finishes |
| Artifact | full-screen panel, scrolled body, save | fixed: the saved-path notice no longer gets cut mid-word (short, whole-word `~/.dst/tmp/<name>.txt` name and the notice replaces the key hints); `artifact_scroll` scenario covers PgDn/search/save |
| Tools | pending card, result row, CtrlO list, artifact viewer | fixed: the result preview is one ellipsised row (long errors used to be cut mid-word); full text stays in CtrlO. Failed calls carry an ASCII `x` marker in the CtrlO list because rows cannot be coloured individually (`tool_log_multi` scenario) |
| Permissions | strict approval (bash + edit diff) | fixed: `-`/`+` diff lines are coloured red/green; human summary, keys |
| Usage | `/usage` report | fixed: the report heading was duplicated (one extra row, now one `usage · N requests` header with a localized request count); labels localize in all six languages (`usage_zh` scenario) |
| Commands | palette, help menu, previews, 1/N position | fixed: `/help` dropped a redundant footer row (8 commands visible, was 7) and its hint now reads `Enter run/complete`. Palette name matches rank above description matches, and an exactly typed command wins over the highlighted row: in Chinese `/resume` also fuzzy-matched `/rename` (whose description mentions `/resume`) and Enter used to complete `/rename` instead of running `/resume` |
| Config | all 15 rows, numeric prompts, model/judge/edit, extensions | fixed: the third-party capability footer fits 53 cells in all six languages instead of being cut mid-word; menu values keep a 2-cell gap |
| Sessions | resume list/search/expanded/resumed, rewind list/preview | fixed: resume hint shortened so it no longer truncates. Chinese preview labels verified (`resume_preview_zh`): `标题/模型/轮次/最近`, counts and colon spacing |
| Background | task list (all/running/watchers), save, full output | fixed: the footer hint fits 53 cells in all six languages (it was cut to `f…`); long commands are ellipsised instead of bare-cut; filter titles and full-width rows |
| Queue | CtrlQ editor, delete, pull back into prompt | fixed: long queued messages are ellipsised instead of cut mid-word (`queue_edit` now queues a 110-char message) |
| Goal | pursuing / objective / paused / done badges, wrap-up | fixed: state glyphs are ASCII-safe (paused/done rendered as boxes before); the model-facing wrap-up paragraph is now a one-line `+ goal wrap-up …` marker instead of filling the transcript |
| Login | `/login` menu, direct typing prompts, WeChat QR screen | fixed: choice menus say `select · Enter choose · Esc back` instead of the config-editor wording; three paths, numbered guide left + QR right, live scanned/finishing status, QR validity countdown, `r retry` after repeated failures |
| Login QA | phone page states, i18n, QR scanability | ok: six-language page, countdown/expired/retry/cancel, QR decodes with jsQR (`https://open.weixin.qq.com/connect/confirm?...`), cancel aborts the flow |
| Empty states | `/goal` with no goal, CtrlO with no output, `/resume` with no sessions | ok: each shows a localized one-liner and the app stays usable (`empty_states_zh`) |
| Creator mode | `/config` Mode row, idle hint, saved-app notice, unsaved-work nudge | ok: the mode is switched in `/config` (never silently) and announced in the transcript; it is visible while idle (`creator · /apps · hold Esc`), the config row shows `normal/creator`, a saved app is announced as `saved app system/battery_viewer`, and a turn that wrote files without an app gets a localized one-line nudge (`creator_mode`, `creator_build`, `creator_nudge`) |
| Apps | `/apps` list, annotation preview, run, delete, doc apps | ok: full-width menu (`Battery viewer   system · sh`), Right shows path/kind/entry/updated plus the APP.md lines, Enter runs the app full-screen (doc apps open the panel instead), `x` asks `delete <name>? y/n` before removing the package; the last delete returns the localized empty state (`creator_apps_zh`) |
| Screen takeover | model `app_run`, app drawing, time limit, long-press Esc | ok: the panel belongs to the app while it runs (`charge 87%`), the TUI repaints when it returns, tool card reports ran/exited/stopped; long-press Esc is platform-enforced and trailing key repeats are swallowed so the turn is not cancelled (`creator_run`, `creator_run_esc`) |
| Prompt/catalog | creator system prompt and installed-app catalog | ok: the prompt gains the CREATOR block and a skills-style `INSTALLED APPS` list each turn; after `app_save` the next turn's system message contains the new app, and the post-tool context tells the model to write the program (`creator_build` asserts on the captured requests) |
| Errors | provider 401 (bad key), 429 (rate limit) + automatic retry | fixed: one red `request failed: …` line, a highlighted yellow `→` recovery hint (`/key`, `/login`, `/model`), localized in all six languages; textual rate limits are retried like HTTP 429; the next prompt works normally |
| Goal status | `/goal` report, badge, commands | fixed: report rows and the bottom-right badge are localized; the command list sits on its own row instead of wrapping its last command |
| Sessions | resume preview metadata | fixed: title/model/turns/recent labels are localized |
| Voice | push-to-talk REC, transcript insertion, `speak` tool card, mocked offline | ok: `DST_MOCK_SPEECH=1` fakes ASR/TTS so the whole voice path is reviewable on the simulator; `auto_send_voice` now actually applies |
| i18n | en, zh, es, ja, de, it menus/help/approval/login pages/command feedback/dynamic labels | ok; every user-visible string is catalog-backed. Beyond the six-language parity test, a unit test now scans `src/*.rs` for literal `i18n::t("…")` keys and fails if the English catalog lacks one (this caught the config-menu hint disappearing when its key was dropped). Runtime notices (request failed, retries, compaction, goal notices, save/mic/speech failures), the `/goal` report/badge and the resume-preview labels are localized too. The simulator's PNG renderer paints all backgrounds before glyphs and fills the continuation cell behind a wide glyph, so selected (reverse-video) menu rows render CJK cleanly (verified in zh/ja/de config and help menus) |

## Fixes made during the audit

- resume picker footer no longer truncates (`type=search · ↑↓ · →/Tab expand · Enter · Esc`)
- first-run tips clear the splash so they start at the top of the panel
- model picker right-hand value shortened to `auto/low/high/max` so it fits
- splash hint fits one 53-cell row: dropped `/quit`, ellipsise long model
  names
- first-run tips gained the tagline heading and no longer survive the first
  prompt
- `/help` footer trimmed to 3 rows; menu hint footer ellipsised, capability
  hints shortened in all six languages
- login menus use a dedicated choose hint
- reasoning panel wraps at word boundaries
- menu label/value gap fixed (2 cells) so an ellipsised title never touches the
  right-hand value
- menu/status footer hints capped at 53 cells in all six languages (unit test)
  and truncated with an ASCII `...` inside the transcript, where `tidy()`
  expands `…` and used to re-wrap the last word onto its own line
- transcript wrapper no longer strands one or two characters on the next line
  (a saved-file path ended in a lone `t`)
- injected model instructions (repeat-call reminder, goal wrap-up) render as a
  localized one-line marker; the model still receives the full text
- goal badge glyphs ASCII-safe (`|| Goal paused`, `ok Goal done`); strict-mode
  edit diff coloured red/green
- long input: cursor-aware one-line window with `…` markers on either hidden
  side (and cell-width aware for CJK); previously Ctrl-A printed the whole
  string and overwrote the status row
- artifact save: whole-word, shortened file name and a notice that replaces the
  key hints, so the path is never sliced mid-word; footer ellipsised
- compaction: precise four-digit token counts and no stale "compacting" row
- prompt editor: forward Delete (ESC [ 3 ~), Home/End/Ctrl-A/Ctrl-E and
  mid-line `…` markers; all exercised by `long_input`
- CtrlO: failed entries marked with `x`; multi-entry list + error expansion
  covered by `tool_log_multi`
- `/usage`: single localized header (duplicate title line removed), localized
  request count; Chinese report verified visually
- command palette: typed-name matches outrank description matches, and exact
  commands run even when a fuzzy row is highlighted (found via the Chinese
  resume-preview scenario: `/resume` was being rewritten to `/rename`)
- Chinese labels use `: ` after labels (`标题: …`, `状态: …`) instead of the
  full-width colon, which `tidy()` had squashed to a cramped half-width
- resume preview labels localized and covered by `resume_preview_zh`
- simulator PNG renderer: two-pass drawing plus continuation-cell background,
  so reverse-video (selected) rows no longer clip or slice CJK glyphs; this
  finally allows visual review of localized selected rows
- long queue entries (and job commands) are ellipsised instead of hard-cut
- localized empty states for `/goal`, CtrlO and `/resume` covered by
  `empty_states_zh`
- creator mode: persistent app packages (`app.json` + `APP.md` + program) under
  `~/.dst/apps`, a skills-style catalog injected into the prompt each turn, and
  `/apps` to browse/preview/run/delete them; a saved-app notice and an
  unsaved-work nudge keep the "work is persisted" promise visible
- screen takeover: `app_run` spawns the app on the panel, forwards the keyboard
  over a pipe, enforces long-press Esc (trailing repeats swallowed) and time
  limits, then repaints the TUI; the app never traps the user
- the mode is visible everywhere it matters: `/config` Mode row, creator idle
  hint, transcript notice on switch; there is deliberately no `/creator`
  command (too easy to hit by accident for a whole-contract switch)
- provider errors: localized `request failed` line plus a yellow `→` recovery
  hint, retry notice localized, textual rate limits treated as transient;
  `mock_api.py` can now serve scripted HTTP errors and `api_error`/`api_retry`
  scenarios pin the flow down
- `/goal` status report, goal badge and resume-preview labels localized in six
  languages; the goal command list is one row (`Commands:` label above it)
- voice: `auto_send_voice` wired, localized "official API only" error,
  `DST_MOCK_SPEECH` test hooks (ASR/TTS) for the simulator
- (from earlier rounds) full-width menus, right-aligned values, ellipsised
  truncation, counters in long lists, ASCII-safe watcher/status glyphs

## Verification entry points

- `python3 tools/sim/run_all.py` — demo, all simulator scenarios (including the
  six `creator_*` ones), login page i18n, login cancel, QR scanability,
  six-language login layout, long memory session.
- `python3 tools/sim/run_live.py` — common-usage walkthrough against the real
  local vLLM endpoint (only TTS/ASR mocked) with a 12-screen contact sheet.
- `python3 tools/sim/live_creator_check.py` — creator mode with the real model:
  it must save a package and `/apps` must show it (`docs/live-creator.png`).
- `dst --dsh-login-test` — runs the real nine-step login against
  `tools/sim/mock_login.py` (or the live platform with no mock env).
- `dst --qr-ppm <jpeg> <out.ppm>` + `node tools/sim/qr_scan.js` — decodes the
  exact panel QR image.
- `docs/LOGIN{,.zh,.es,.ja,.de,.it}.md` — user-facing login guides in six
  languages with states, requirements and a troubleshooting table.
- `tools/sim/screenshot_login_page.py` — Chrome-headless screenshot of the LAN
  page (minimum headless viewport is 500px; the CSS is responsive below that).

## Known limitations

- Simulator PNGs are now rendered at the panel's native pixel grid (320x170,
  53x12 cells of 6x14 px, 6x13 glyph box) with the public-domain X11 6x13
  bitmap font and an RGB-subpixel model for CJK/vector fallbacks, then
  integer-upscaled (`tools/sim/README.md`). Geometry is exact; the board's
  exact font family and panel gamma are still assumptions, so the coloured
  CJK fringes look somewhat stronger than on real hardware and block art is
  dithered at the bitmap pixel level. A real-hardware visual pass is still
  needed for font shapes and colour.
- Inline Markdown emphasis (`**bold**`, backticks) is stripped rather than
  styled: the transcript stores one colour per row, so bold/inline-code would
  need embedded SGR handling in the wrapper. Headings, lists and fenced code
  blocks are styled by row kind and verified against a real vLLM answer.
- Creator-mode limits by design: apps are shell/python scripts or documents
  (no on-device compilation, no services, no GUI toolkit), `app_run` captures no
  output (the screen belongs to the app while it runs) and only works in the
  interactive TUI; app packages live under `~/.dst/apps` and are not sandboxed
  beyond the tool's own path validation.
- Hardware-only checks (physical keyboard/audio/reset double-press, boot time,
  TTS playback interruption, long-press Esc key repeat rate) remain for the
  device regression run.
