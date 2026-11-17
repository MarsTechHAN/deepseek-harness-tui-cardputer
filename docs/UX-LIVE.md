# dst live UX walkthrough (real local vLLM)

`tools/sim/run_live.py` is the "common usage, step by step" visual pass. Unlike
`run_scenarios.py` (scripted mock API, deterministic assertions), this
walkthrough drives dst against the **real local vLLM endpoint** from the DSH
config, so every screen shows a genuine generation: real reasoning, real
streaming, real tool calls and real latency. Only speech is mocked, because the
host has no microphone/speaker and a vLLM endpoint has no DeepSeek ASR/TTS APIs.

```sh
cd tools/sim
python3 run_live.py                 # ~20 s; writes screenshots/live/ + docs/live-walkthrough.png
python3 run_live.py --quick         # skip the tool and voice turns (smoke test)
python3 run_live.py --base URL --model ID --effort high
```

Endpoint discovery: `DST_LIVE_BASE` / `DST_LIVE_MODEL`, otherwise the first
`baseURL` and the `agent-default-model` model in `~/.dsh/settings.yaml`
(`settings.yaml.imported` fallback). Speech mock: `DST_MOCK_SPEECH=1`, ASR text
from `DST_MOCK_ASR_TEXT`, TTS delay from `DST_MOCK_TTS_MS`; it is never set on
the device.

## Screens reviewed (contact sheet: docs/live-walkthrough.png)

| # | Step | What is checked | Result |
| --- | --- | --- | --- |
| 01 | `01_startup` | first-run tips: tagline, three action tips, `/help` footer | fixed: tips now open with the `dst · a DeepSeek harness on a Cardputer Zero` tagline and disappear on the first prompt |
| 02 | `02_streaming` | live status while the first request runs | ok: `| working 0.0s`, no layout jump |
| 03 | `03_reply` | settled answer, folded reasoning line, wrapping, status bar | ok: three-sentence answer wraps at word boundaries; `▸ thinking … (collapsed)` line |
| 04 | `04_tool_running` | tool card while the tool runs | ok: `* bash > sleep 2; printf …` card and `bash running 0.3s` live line |
| 05 | `05_tool_done` | settled tool card, result preview, answer | ok: `* bash | 2.2s ok`, `└ alpha`, final text |
| 06 | `06_recording` | push-to-talk hold: REC indicator with elapsed time | ok: red `● REC 1.0s` above the prompt |
| 07 | `07_voice_stream` | voice prompt auto-sent, thinking panel streaming | ok: magenta reasoning panel + `| thinking 0.3s` |
| 08 | `08_voice_reply` | settled voice answer, user transcript visible | ok: prompt and answer fit the panel |
| 09 | `09_help` | `/help` menu: title, counter, rows, preview, hint | fixed: the redundant "Enter runs it" footer row is gone; 8 commands visible instead of 7 and the hint never cuts a word |
| 10 | `10_config` | `/config` wizard: 15 rows, right-aligned values, help footer | fixed: third-party capability hint shortened to 53 cells (was cut mid-word) |
| 11 | `11_resume` | `/resume` picker: search hint, relative time, title, cwd value | fixed: two-cell gap between the ellipsised title and the right-hand value |
| 12 | `12_login` | `/login` entry menu | fixed: footer says `select · Enter choose · Esc back` (was `Enter change`) |

## Fixes made from this pass

- **Splash clipped with a long model name**: the hint under the 9-row whale
  wrapped to a second row, pushing art off the top (`… | /q` / `uit`). The hint
  lost the optional `/quit`, fits one row, and is ellipsised if the model name
  is still too long.
- **First-run tips**: they now carry the tagline as a heading, and the first
  submitted prompt clears them instead of leaving onboarding text in the
  transcript for the rest of the session.
- **Menu footers**: third-party capability hints were longer than 53 cells and
  were sliced mid-word; all six languages now fit, and the footer is ellipsised
  rather than hard-cut.
- **`/help` usable rows**: dropped one redundant footer line (Enter semantics
  moved into the key hint), so the list shows 8 commands on the 12-row panel.
- **Login menus** used the config-editor wording (`Enter change`); they now use
  a dedicated choose hint in all six languages.
- **Reasoning panel**: thinking text wrapped at the column edge, splitting
  words (`tant replied…`); the streaming wrapper now breaks at the last space.
- **Menu label/value spacing**: long labels ellipsised into the value column;
  two cells are now reserved between them.
- **Voice**: `auto_send_voice` is applied from the config at startup and after
  `/config` (it was previously ignored); the "voice needs the official API"
  error is localized in all six languages; `DST_MOCK_SPEECH=1` fakes ASR/TTS so
  push-to-talk, the transcript insertion and the `speak` tool card can be
  reviewed offline. The mock logs to `~/.dst/logs/voice.log`.


## Creator mode live check (real local vLLM)

`tools/sim/live_creator_check.py` is the creator-mode counterpart of the
walkthrough above: it asks the real model to build a battery app, then opens
`/apps` and verifies the package on disk. It is a *directed* prompt (it names
`app_save`/`app_file` and says not to run the app) because the simulator host is
a Mac without `/sys/class/power_supply` and the point of the pass is the
save-and-inspect path; the model, streaming, tool arguments and rendering are
all real. Sandbox `full` keeps the pass fast (no judge request per probe).

```sh
cd tools/sim
python3 live_creator_check.py     # ~2-4 min; screenshots + contact sheet
```

| # | Step | What is checked | Result |
| --- | --- | --- | --- |
| 01 | `01_startup` | creator mode is on: `创作者 · /apps · 长按 Esc 退出应用` idle hint | ok |
| 02 | `02_built` | real model calls `app_save` + `app_file`; transcript shows the answer and the one-line `已保存应用 system/battery` notice | ok: the model wrote `app.json`, `APP.md` and a sysfs-reading `run.sh`, and warned that the host has no battery node |
| 03 | `03_apps` | `/apps` menu lists the new app (`Battery   system · sh`) with the localized footer | ok |
| 04 | `04_preview` | Right expands the annotation preview: `路径/类型/入口/更新` + APP.md body | ok |

Hard checks: the transcript shows a saved app, a package exists under
`~/.dst/apps` with `app.json`, `APP.md` and `run.sh`, and the preview shows the
package path. Contact sheet: `docs/live-creator.png`.

Evidence and limits: the live model is free to choose a different name,
structure or description than `creator_build`'s mock script; the check accepts
any package. It does not replace the deterministic scenarios, which pin the
exact manifest, catalog injection and screen-takeover behaviour.

## Evidence and limits

- Contact sheet: [`docs/live-walkthrough.png`](live-walkthrough.png).
- Per-step PNG/text: `tools/sim/screenshots/live/` (gitignored, regenerated by
  the command above).
- Deterministic regression coverage for the same states (including the voice
  UI with mocked speech) is in `run_scenarios.py`: `voice_input`, `voice_speak`,
  `tips`, `commands`, `config_menu`, `resume_search`, `login_menu`, plus the
  provider-failure paths `api_error` (401 + recovery) and `api_retry`
  (429 + automatic retry) and `goal_status_zh` for the localized goal report.
- Limits: there is no real microphone, speaker, physical keyboard or panel.
  WeChat login against the live platform still needs a device pass; the
  nine-step flow is covered by `mock_login.py`.
- **Addresses are redacted.** The endpoint the pass runs against is a private
  devbox, so `run_live.py` labels it `local vLLM` in the contact-sheet title and
  the `/config` panel is captured with a neutral endpoint; the committed PNG
  never carries the real address.
