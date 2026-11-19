# dst UX walkthrough — device-fidelity screenshots (optional live pass)

The committed screenshots in `docs/` are **device-fidelity simulator renders**:
the real `dst` binary running under a pty, replayed through the 53×12 cell
renderer with the panel's bitmap font and an RGB-subpixel model, at the device's
native 320×170 geometry. Model replies are scripted (mock OpenAI-compatible
server), so the images are deterministic and need no API key or hardware.

They are regenerated in one command:

```sh
cd tools/sim
python3 make_demo_webp.py     # docs/demo.webp + docs/live-*.png + login sheet (~2 min)
python3 run_all.py            # every offline check (scenarios, login, memory)
```

An **optional live pass** still exists for real-model verification when an
OpenAI-compatible endpoint is available (`tools/sim/run_live.py`,
`tools/sim/live_creator_check.py`, documented below). The last committed live
images were replaced because they predated the skill-centric creator refactor;
the live scripts themselves are kept and ported so the pass can be re-run
against a real endpoint.

Animated tour: [`docs/demo.webp`](demo.webp) (21 s screen recording). It walks the
whole flow: first run, the pinned progress row, expanded tool calls, CtrlO
during a turn, tool results, transcript paging, a queued message, the Esc
interrupt, voice interrupt + recording, creator mode writing/listing/running a
skill package, the first-run hardware scan, six-language login, `/help`,
`/config` and the conversation continuing.

## Screens reviewed (contact sheet: `docs/live-walkthrough.png`)

| # | Panel | What it shows | State |
| --- | --- | --- | --- |
| 01 | `01 startup` | first-run tips: tagline, three action tips, key hints | ok: tips open with the `dst · a DeepSeek harness on a Cardputer Zero` tagline and clear on the first prompt |
| 02 | `02 progress row` | `state_report` pinned to the top row while tools run | ok: `» reading /proc` in bright magenta above the transcript; it joins the log when the next line arrives |
| 03 | `03 tool expanded` | tool cards while they execute | ok: `* bash` plus one indented row per argument (`command:`, `timeout_s:`), long values folded, full text in CtrlO |
| 04 | `04 tool done` | settled tool card and result | ok: `└ 2.0s ok` and the `↳` preview; no duplicate tool name row |
| 05 | `05 scrollback` | PgUp paging in the transcript | ok: `↑12 lines · End` indicator, top of the buffer stops at the top row instead of scrolling blank |
| 06 | `06 queued message` | typing during a turn | ok: `⏎ queued (1): …` line, the message is sent right after the turn |
| 07 | `07 recording` | push-to-talk / voice interrupt | ok: red `● REC 0.7s`; Tab during a turn stops it at once and starts the take |
| 08 | `08 help` | `/help` command menu | ok: preview pane plus `↑↓ select · Enter run/complete · Esc close`; counter reads `1/17` with `/update-hwinfo` |
| 09 | `09 config` | `/config` wizard | ok: 16 rows, right-aligned values, capability hint fits 53 cells |
| 10 | `10 hardware scan` | `/update-hwinfo` result | ok: `hardware-info updated: 13 subsystems, 23 KB, v2`; normal mode points at creator mode for model refinement |
| 11 | `11 login (zh)` | WeChat login menu in Chinese | ok: CJK renders with the CJK fallback font (the earlier bitmap-only sheet showed tofu) |
| 12 | `12 app running` | a runnable skill package owning the panel | ok: `Battery: 87%` / `state: discharging`; long-press Esc returns to the chat |

## Creator contact sheet: `docs/live-creator.png`

Creator mode has one unit: a **skill package** under `~/.dst/skills/<name>/`.
It becomes a runnable app when the folder also has `meta.json` (kind/entry/
requires/default_args/capture); there is no `~/.dst/apps`, no `app.json`/
`APP.md` and no app-writing tool — `skill_save`/`skill_file` write the package
and `app_test`/`app_run` execute it. The sheet shows:

| # | Panel | What it shows |
| --- | --- | --- |
| 01 | `01 skill package written` | `wrote meta.json`, `wrote run.sh`, `app_test … exit=0`, then `saved skill battery-viewer v1` and `saved app battery-viewer (runnable skill)` |
| 02 | `02 SKILL.md in /apps` | the package detail panel: title, updated, requires, last test, file list and the SKILL.md instructions |
| 03 | `03 /apps launcher` | `Apps · 2 · skills 3`: `Battery viewer   sh · 12:49`, `Status report   prompt`, `+ new app` and the full key footer |
| 04 | `04 app owning the panel` | the package running full-screen (`Battery: 87%`, `state: discharging`) |

## Optional live passes (real endpoint)

```sh
cd tools/sim
python3 run_live.py                # ~20 s; 12-panel contact sheet from a real model
python3 run_live.py --quick        # skip the tool and voice turns (smoke test)
python3 run_live.py --base URL --model ID --effort high
python3 live_creator_check.py      # ~2-4 min; creator mode with the real model
```

Endpoint discovery: `DST_LIVE_BASE` / `DST_LIVE_MODEL`, otherwise the first
`baseURL` and the `agent-default-model` in `~/.dsh/settings.yaml`
(`settings.yaml.imported` fallback). Speech is mocked on the host
(`DST_MOCK_SPEECH=1`, `DST_MOCK_ASR_TEXT`, `DST_MOCK_TTS_MS`) because there is no
microphone/speaker and a vLLM endpoint has no DeepSeek ASR/TTS APIs.

`live_creator_check.py` asks the real model to build a battery readout package,
then opens `/apps` and verifies the package on disk. It is a *directed* prompt
(the simulator host has no `/sys/class/power_supply`; the point is the
save-and-inspect path) while the model, streaming, tool arguments and rendering
are real. Hard checks: the transcript announces a saved skill and a saved app,
`~/.dst/skills/<name>/{SKILL.md,meta.json,entry}` exists and the `/apps` preview
shows the package path. Sandbox `full` keeps the pass fast (no judge request per
probe).

Image input is covered deterministically by two scenarios: `read_image_tool`
(the agent opens a local image with the `read_image` tool) and `image_input`
(the human attaches one with `/image`) — both produce the DeepSeek vision block
and keep the history free of base64. The live passes are text-only for now, so a
real vision-model answer is a manual check.

Live evidence and limits:

- per-step PNG/text: `tools/sim/screenshots/live/` (gitignored, regenerated by
  the commands above);
- deterministic regression coverage for the same states is in
  `run_scenarios.py` (103 checked scenarios), and the committed contact sheets
  are built from those by `make_demo_webp.py`;
- **addresses are redacted**: the endpoint a live pass runs against is private,
  so `run_live.py` labels it `local vLLM` and captures `/config` with a neutral
  endpoint; committed images never carry the real address.

## Fixes from earlier real-model passes (kept for context)

- **Splash clipped with a long model name**: the hint under the 9-row whale
  wrapped to a second row, pushing art off the top. The hint lost the optional
  `/quit`, fits one row and is ellipsised if the model name is too long.
- **First-run tips**: they carry the tagline as a heading, and the first
  submitted prompt clears them instead of leaving onboarding text behind.
- **Menu footers**: third-party capability hints were longer than 53 cells and
  sliced mid-word; all six languages now fit and the footer is ellipsised.
- **`/help` usable rows**: a redundant footer row was dropped (Enter semantics
  moved into the key hint), so the list shows more commands per screen.
- **Login menus** used the config-editor wording (`Enter change`); they use a
  dedicated choose hint in all six languages.
- **Reasoning panel**: thinking text wrapped at the column edge, splitting
  words; the streaming wrapper breaks at the last space.
- **Menu label/value spacing**: long labels ellipsise into the value column and
  two cells are reserved between label and value.
- **Voice**: `auto_send_voice` is applied from the config at startup and after
  `/config`; the "voice needs the official API" error is localized;
  `DST_MOCK_SPEECH=1` fakes ASR/TTS so push-to-talk, transcript insertion and the
  `speak` card can be reviewed offline (the mock logs to `~/.dst/logs/voice.log`).

## Evidence and limits

- Simulator PNGs use the panel's native pixel grid (320×170, 53×12 cells of
  6×14 px, 6×13 glyph box) with the public-domain X11 6×13 bitmap font and an
  RGB-subpixel model for CJK/vector fallbacks (`tools/sim/README.md`). Geometry
  is exact; the board's exact font family and panel gamma are still assumptions,
  so coloured CJK fringes look stronger than on real hardware and block art is
  dithered at the bitmap pixel level. A real-hardware visual pass is still worth
  doing for font shapes and colour.
- There is no real microphone, speaker, physical keyboard or panel in the
  simulator. The WeChat login against the live platform still needs a device
  pass; the nine-step flow itself is covered by `mock_login.py` scenarios.
- Hardware-only checks (physical keyboard/audio/reset double-press, boot time,
  TTS playback interruption, long-press Esc key repeat rate) remain for the
  device regression run.
