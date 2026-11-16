# dst UX audit (12-row panel, 53 columns)

Method: every screen is exercised through the pty simulator with the scripted
mock API, captured as text + PNG, and reviewed at the real cell size. The
catalogue below is the checklist used for the final pass; all rows were
inspected visually, not only asserted in code.

## Screens reviewed

| Area | Screen(s) | Result |
| --- | --- | --- |
| Startup | splash, idle hints, first-run tips | fixed: tips now replace the splash instead of starting mid-art |
| Onboarding | endpoint menu, sandbox picker | ok: previews explain tradeoffs, rows full width |
| Chat | live thinking, streaming tail, steer marker | ok: reasoning wraps, folds, steer label visible |
| Tools | pending card, result row, CtrlO list, artifact viewer | ok: one row per card, full output scrolls/saves |
| Permissions | strict approval (bash + edit diff) | ok: human summary, -old/+new preview, keys |
| Commands | palette, help menu, previews, 1/N position | ok: grouped categories, footer position |
| Config | all 15 rows, numeric prompts, model/judge/edit, extensions | ok: right-aligned values, preview help, counters |
| Sessions | resume list/search/expanded/resumed, rewind list/preview | fixed: resume hint shortened so it no longer truncates |
| Background | task list (all/running/watchers), save, full output | ok: filter titles, full-width rows |
| Queue | CtrlQ editor, delete, pull back into prompt | ok: full-width selection |
| Goal | pursuing / objective / paused / done badges | ok: badge and todo progress fit the right slot |
| Login | `/login` menu, direct typing prompts, WeChat QR screen | ok: three paths, numbered guide left + QR right, live scanned/finishing status, QR validity countdown, `r retry` after repeated failures |
| Login QA | phone page states, i18n, QR scanability | ok: six-language page, countdown/expired/retry/cancel, QR decodes with jsQR (`https://open.weixin.qq.com/connect/confirm?...`), cancel aborts the flow |
| i18n | en, zh, es, ja, de, it menus/help/approval/login pages/command feedback/dynamic labels | ok; every user-visible string is catalog-backed and the parity unit test keeps all six languages complete. Note: the simulator's PNG renderer garbles wide glyphs inside reverse-video rows (selected row) — text snapshots and a real terminal are correct |

## Fixes made during the audit

- resume picker footer no longer truncates (`type=search · ↑↓ · →/Tab expand · Enter · Esc`)
- first-run tips clear the splash so they start at the top of the panel
- model picker right-hand value shortened to `auto/low/high/max` so it fits
- (from earlier rounds) full-width menus, right-aligned values, ellipsised
  truncation, counters in long lists, ASCII-safe watcher/status glyphs

## Verification entry points

- `python3 tools/sim/run_all.py` — demo, all simulator scenarios, login page i18n,
  login cancel, QR scanability, six-language login layout, long memory session.
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
