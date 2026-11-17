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
| Startup | splash art + hint, idle hints, first-run tips | fixed: the hint no longer wraps/clips the 9-row whale with a long model name; tips open with the tagline and are cleared by the first prompt |
| Onboarding | endpoint menu, sandbox picker | ok: previews explain tradeoffs, rows full width |
| Chat | live thinking, streaming tail, steer marker, injected context | fixed: reasoning wraps at word boundaries; model-facing context (repeat reminders, goal wrap-up) is a one-line marker instead of a dumped paragraph, so the answer stays on screen |
| Tools | pending card, result row, CtrlO list, artifact viewer | fixed: the result preview is one ellipsised row (long errors used to be cut mid-word); full text stays in CtrlO |
| Permissions | strict approval (bash + edit diff) | fixed: `-`/`+` diff lines are coloured red/green; human summary, keys |
| Commands | palette, help menu, previews, 1/N position | fixed: `/help` dropped a redundant footer row (8 commands visible, was 7) and its hint now reads `Enter run/complete` |
| Config | all 15 rows, numeric prompts, model/judge/edit, extensions | fixed: the third-party capability footer fits 53 cells in all six languages instead of being cut mid-word; menu values keep a 2-cell gap |
| Sessions | resume list/search/expanded/resumed, rewind list/preview | fixed: resume hint shortened so it no longer truncates |
| Background | task list (all/running/watchers), save, full output | fixed: the footer hint fits 53 cells in all six languages (it was cut to `f…`); filter titles and full-width rows |
| Queue | CtrlQ editor, delete, pull back into prompt | ok: full-width selection |
| Goal | pursuing / objective / paused / done badges, wrap-up | fixed: state glyphs are ASCII-safe (paused/done rendered as boxes before); the model-facing wrap-up paragraph is now a one-line `+ goal wrap-up …` marker instead of filling the transcript |
| Login | `/login` menu, direct typing prompts, WeChat QR screen | fixed: choice menus say `select · Enter choose · Esc back` instead of the config-editor wording; three paths, numbered guide left + QR right, live scanned/finishing status, QR validity countdown, `r retry` after repeated failures |
| Login QA | phone page states, i18n, QR scanability | ok: six-language page, countdown/expired/retry/cancel, QR decodes with jsQR (`https://open.weixin.qq.com/connect/confirm?...`), cancel aborts the flow |
| Voice | push-to-talk REC, transcript insertion, `speak` tool card, mocked offline | ok: `DST_MOCK_SPEECH=1` fakes ASR/TTS so the whole voice path is reviewable on the simulator; `auto_send_voice` now actually applies |
| i18n | en, zh, es, ja, de, it menus/help/approval/login pages/command feedback/dynamic labels | ok; every user-visible string is catalog-backed. Beyond the six-language parity test, a unit test now scans `src/*.rs` for literal `i18n::t("…")` keys and fails if the English catalog lacks one (this caught the config-menu hint disappearing when its key was dropped). Note: the simulator's PNG renderer garbles wide glyphs inside reverse-video rows (selected row) — text snapshots and a real terminal are correct |

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
- voice: `auto_send_voice` wired, localized "official API only" error,
  `DST_MOCK_SPEECH` test hooks (ASR/TTS) for the simulator
- (from earlier rounds) full-width menus, right-aligned values, ellipsised
  truncation, counters in long lists, ASCII-safe watcher/status glyphs

## Verification entry points

- `python3 tools/sim/run_all.py` — demo, all simulator scenarios, login page i18n,
  login cancel, QR scanability, six-language login layout, long memory session.
- `python3 tools/sim/run_live.py` — common-usage walkthrough against the real
  local vLLM endpoint (only TTS/ASR mocked) with a 12-screen contact sheet.
- `dst --dsh-login-test` — runs the real nine-step login against
  `tools/sim/mock_login.py` (or the live platform with no mock env).
- `dst --qr-ppm <jpeg> <out.ppm>` + `node tools/sim/qr_scan.js` — decodes the
  exact panel QR image.
- `docs/LOGIN{,.zh,.es,.ja,.de,.it}.md` — user-facing login guides in six
  languages with states, requirements and a troubleshooting table.
- `tools/sim/screenshot_login_page.py` — Chrome-headless screenshot of the LAN
  page (minimum headless viewport is 500px; the CSS is responsive below that).

## Known limitations

- Simulator only: wide CJK glyphs in reverse-video rows can render as boxes in
  PNG captures (pyte + fallback-font interaction); text captures show the real
  characters. Device fonts still need a real-hardware pass.
- Hardware-only checks (physical keyboard/audio/reset double-press, boot time,
  TTS playback interruption) remain for the device regression run.
