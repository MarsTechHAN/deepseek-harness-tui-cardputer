# dst-sim — offline screen simulator + mock API for the `dst` TUI

Inspect and test the `dst` Rust TUI (repository root is `../..`)
without the Cardputer hardware and without any network access.

Two tools, both pure Python (stdlib + `pyte` + `Pillow`):

| file | purpose |
| --- | --- |
| `screen_sim.py` | runs `dst` in a 53×12 pty, feeds the ANSI stream into pyte, saves text + PNG snapshots, scripts keystrokes |
| `mock_api.py` | local OpenAI-compatible HTTP server: `/v1/models`, streaming/non-streaming `/v1/chat/completions`, scripted thinking/content/tool-call deltas, usage, JSONL request log |
| `sample_scenario.json` | scripted screen_sim run: splash → hello → thinking → reply → tool call → final reply |
| `sample_script.json` | per-request mock responses used by the scenario |
| `sample_home/.dst/config.yml` | ready-made dst config pointing at `http://127.0.0.1:8765/v1` |
| `run_demo.py` | one-command end-to-end demo: starts mock, writes temp HOME, runs simulator, validates, builds contact sheet |
| `run_scenarios.py` | automated feature checks: `/clear`, `/goal` round driver, compaction, voice, login (asserts on screens **and** the request log) |
| `check_multi_instance.py` | starts two real `dst` instances on two ptys with the same HOME and asserts both render (the machine-wide singleton is gone; one instance per terminal remains) |
| `make_demo_webp.py` | rebuilds the committed docs images from the simulator: `docs/demo.webp` (21 s screen recording of a real session: screen_sim records every ~110 ms frame, dedupes, and encodes a timed WebP), `docs/live-walkthrough.png`, `docs/live-creator.png` and the six-language login sheet (real `dst` binary, scripted mock model) |
| `run_live.py` | optional common-usage visual walkthrough against the real local vLLM endpoint from `~/.dsh/settings.yaml`; only TTS/ASR are mocked (`DST_MOCK_SPEECH=1`) |
| `scenarios/<name>/` | per-feature `config.yml` + `scenario.json` + `script.json` for `run_scenarios.py` |
| `screenshots/` | generated evidence from `run_demo.py` (created on first run) |

Everything binds/connects only to `127.0.0.1` except `run_live.py`, which talks
to the local vLLM endpoint configured in the DSH settings.

---

## 1. Build dst once

```bash
cd ../..
cargo build                      # pure-std, host target; binary: target/debug/dst
```

## 2. Quick start: one command, fully automatic

```bash
cd tools/sim
python3 run_demo.py
```

It picks a free localhost port, starts `mock_api.py`, creates a throwaway
`HOME` (`.work/home` with `.dst/config.yml` → mock API, key `test`,
model `deepseek-flash`), runs `screen_sim.py --scenario sample_scenario.json`,
asserts 10 invariants, writes `screenshots/` + `contact_sheet.png`, prints the
final dst screen, then stops the mock. Runtime ≈ 14 s.

Useful flags: `--scale 4` (bigger PNGs), `--out DIR`, `--timeout 240`,
`--skip-validation`, `--keep-work`.

Expected tail of a successful run:

```
[sim] dst exit code=0, 72726 bytes, 6 snapshots in .../screenshots
[demo] check ok: splash: contains "the 'dst' title"
[demo] check ok: thinking: contains 'the live thinking indicator'
[demo] check ok: reply: contains 'streamed content'
[demo] check ok: tool_call: contains 'the list_dir tool line'
[demo] check ok: final_reply: contains 'CJK (wide-character) content'
[demo] check ok: dst exited cleanly on Ctrl-C (code 0)
[demo] check ok: mock request log: 3 chat request(s), 3 response(s)
[demo] check ok: request 2: streamed tool_calls delta (finish_reason='tool_calls')
[demo] check ok: request 3: carried role='tool' result (agent loop closed)
[demo] check ok: Authorization: Bearer test forwarded from config
[demo] validation passed (10 checks)
```

## 2b. Live walkthrough against the real model

`run_live.py` reviews the UI with real generations instead of the scripted
mock: reasoning, streaming, tool calls and latency all come from the local
vLLM endpoint in `~/.dsh/settings.yaml`. Speech is the only mocked part
(no microphone/speaker on the host, and a vLLM endpoint has no DeepSeek
ASR/TTS APIs), so push-to-talk and the `speak` tool are still reviewable.

```bash
cd tools/sim
python3 run_live.py                 # ~20 s -> screenshots/live/ + docs/live-walkthrough.png
python3 run_live.py --quick         # skip the tool and voice turns
python3 run_live.py --base URL --model ID --effort high
```

Endpoint discovery: `DST_LIVE_BASE` / `DST_LIVE_MODEL`, else the first
`baseURL` plus the `agent-default-model` model in the DSH settings. The
walkthrough captures 12 steps (startup, streaming, reply, tool running/done,
push-to-talk REC, voice streaming/reply, `/help`, `/config`, `/resume`,
`/login`) and fails only on hard layout failures; model wording differences are
warnings. Mocked speech leaves a line in `$HOME/.dst/logs/voice.log`.

## 3. Manual run (two terminals)

**Terminal A — mock API** (keep it in the foreground):

```bash
cd tools/sim
python3 mock_api.py --port 8765 --script sample_script.json --log .work/requests.jsonl
# prints: MOCK_API_URL=http://127.0.0.1:8765/v1
```

**Terminal B — simulator:**

```bash
cd tools/sim
mkdir -p /tmp/dst-home/.dst
cp sample_home/.dst/config.yml /tmp/dst-home/.dst/config.yml   # base: http://127.0.0.1:8765/v1, key: test
python3 screen_sim.py \
    --home /tmp/dst-home \
    --cwd tools/sim \
    --scenario sample_scenario.json \
    --out screenshots
```

The final text screen is printed to stdout; PNGs/text land in `screenshots/`.
Ctrl-C in Terminal A stops the mock (or `kill %1` if backgrounded).

> The config file is what skips dst's onboarding/login screen: `~/.dst/config.yml`
> with a non-empty `api.key` makes dst start configured (`language: en` keeps the
> UI English). See `sample_home/.dst/config.yml`.

---

## 4. `screen_sim.py`

```bash
python3 screen_sim.py --help
```

### Global options

| option | meaning |
| --- | --- |
| `--bin PATH` | dst binary (default: `$DST_BIN` or `<repo>/target/debug/dst`) |
| `--cols N` / `--rows N` | pty winsize in cells (default 53×12; dst clamps to 53 cols / 13 rows) |
| `--home DIR` | `HOME` for dst (put `.dst/config.yml` there) |
| `--cwd DIR` | working directory for dst (also what `list_dir`/`bash` see) |
| `--env K=V` | extra child env var (repeatable); `TERM=xterm-256color`, `LANG/LC_ALL=en_US.UTF-8` are preset |
| `--out DIR` | snapshot directory (default `snapshots`) |
| `--scale N` | output PNG = the 320×170 device panel integer-upscaled by N with nearest-neighbour sampling → exactly `320N × 170N` px (default 3 → 960×510) |
| `--native` | save the 1:1 320×170 panel framebuffer instead of an upscaled PNG (implies scale 1; pixel inspection) |
| `--font-mode auto\|bitmap\|ttf` | `auto`/`bitmap` = vendored X11 6×13 bitmap font, vector+subpixel only for glyphs it lacks; `ttf` = host vector font for every glyph (comparison mode) |
| `--subpixel on\|off` | RGB-stripe subpixel antialiasing for vector glyphs (default `on`; `off` = grayscale AA) |
| `--mono-font/--cjk-font/--symbol-font PATH` | override host font discovery (CJK fallback and `--font-mode ttf`) |
| `--text-only` | no PNGs (text snapshots only) |
| `--scenario FILE` | JSON scenario (`-` reads stdin); its steps run before inline steps |
| `--watch SECONDS --interval-ms MS --watch-prefix NAME` | capture every `MS` for `SECONDS` |
| `--startup-ms N` | settle time after dst's first paint (default 650) |
| `--after-ms N` | delay inserted after every `--send`/`--key` (default 100) |
| `--vtime N` | pty VTIME in 1/10 s for idle reads (default 3) |
| `--raw-log FILE` | dump the raw pty byte stream (debugging) |
| `--quit-timeout-ms N` | wait for clean Ctrl-C exit (default 2000) |

### Ordered step options (executed in command-line order)

| option | meaning |
| --- | --- |
| `--send TEXT` | write TEXT bytes (UTF-8) to dst |
| `--send-line TEXT` | write TEXT then Enter |
| `--key NAME` | `Enter Return Esc Escape Tab Space Backspace Delete Up Down Left Right Home End PgUp PgDn ShiftEnter AltEnter CtrlA CtrlC CtrlD CtrlL CtrlU CtrlW` |
| `--wait-ms N` | sleep N ms while draining output |
| `--wait-stable STABLE_MS[:TIMEOUT_MS]` | wait until the rendered screen is unchanged for STABLE_MS (the reliable "turn finished" detector) |
| `--wait-for TEXT[:TIMEOUT_MS]` | wait until TEXT appears on screen |
| `--wait-quiet QUIET_MS[:TIMEOUT_MS]` | wait until no pty output for QUIET_MS (see Limitations: idle dst always redraws) |
| `--snap NAME` | capture snapshot now |

Examples:

```bash
# splash only
python3 screen_sim.py --home /tmp/dst-home --snap splash

# hello + Enter, snapshot mid-thinking, then wait for the turn to settle and snapshot
python3 screen_sim.py --home /tmp/dst-home --out /tmp/shots \
  --send "hello" --key Enter --wait-ms 1100 --snap thinking \
  --wait-stable 900:15000 --snap reply

# watch the animated splash for 2 s at 400 ms cadence
python3 screen_sim.py --home /tmp/dst-home --out /tmp/shots \
  --watch 2 --interval-ms 400 --watch-prefix anim
```

### JSON scenarios

```json
{
  "steps": [
    {"wait_for": {"text": "dst", "timeout_ms": 10000}},
    {"snap": "splash"},
    {"send": "hello"},
    {"key": "Enter"},
    {"wait_ms": 1100, "snap": "thinking"},
    {"wait_stable": {"stable_ms": 800, "timeout_ms": 15000}, "snap": "reply"},
    {"send": "list the files here"},
    {"key": "Enter"},
    {"wait_ms": 1200, "snap": "tool_call"},
    {"wait_stable": {"stable_ms": 800, "timeout_ms": 15000}, "snap": "final_reply"}
  ]
}
```

A step object is processed in this fixed order: `send` / `send_line`, `key`
(string or list), `wait_ms` / `delay_ms`, `wait_quiet` / `wait_quiet_ms`,
`wait_stable` / `wait_stable_ms`, `wait_for`, then `snap` / `snaps` (list),
then `watch` (`{"seconds":…, "interval_ms":…, "prefix":…}`). The dict-key order
in the JSON file does not matter.

### Snapshots and exit

For every snapshot the simulator writes `<NN>_<name>.txt` and
`<NN>_<name>.png` (unless `--text-only`). At the end it writes `final.txt` /
`final.png` from the last screen, `manifest.json` (run settings, fonts, input
timeline, snapshot list, dst exit code, byte count), and prints the final text
screen to stdout. dst is then shut down with Ctrl-C (clean exit expected; falls
back to SIGTERM/SIGKILL). Exit status: 0 ok, 2 usage/setup error, 130 Ctrl-C.
A font or glyph that cannot be rendered degrades only the PNG: the failure is
logged and the text snapshot is still written.

### PNG renderer: the device panel model

The PNG is a model of the Cardputer Zero panel pixels, not a generic terminal
screenshot. The exact geometry:

| quantity | value |
| --- | --- |
| panel framebuffer | 320 × 170 RGB px |
| terminal grid | 53 cols × 12 rows |
| cell pitch | 6 px wide × 14 px tall |
| glyph box | 6 × 13 px at the top of the cell, then a 1 px line gap |
| wide (CJK) glyph box | 12 × 13 px = two cells |
| text area | 53·6 × 12·14 = 318 × 168 px, centred at origin (1,1) |
| panel margins | 1 px of black on every side |

`screen_sim.py` builds that 320×170 framebuffer natively and then produces the
output PNG by *integer nearest-neighbour upscaling*: `--scale N` gives exactly
`320N × 170N` px (default 3 → 960×510, 6 → 1920×1020) and no font is
re-rasterised at the larger size, so an upscaled PNG shows the exact device
pixels. `--native` writes the 1:1 320×170 framebuffer for pixel inspection.

**Fonts**
* `fonts/6x13.bdf.gz` / `fonts/6x13B.bdf.gz` are the public-domain X11 "Fixed"
  6×13 bitmap font (regular/bold). Every glyph they cover is drawn as exactly
  6×13 device pixels, no antialiasing: a bitmap pixel is either the cell
  foreground colour or the background. This covers Latin, Greek, Cyrillic, most
  punctuation/arrows, box drawing and the block elements (`█▓░▀▄`).
* Characters the bitmap font does not cover (CJK/kana, emoji, odd symbols) are
  rasterised from a host vector font (`--cjk-font` / discovery; Apple Symbols
  for narrow symbol fallback) at exactly the device box size — 12×13 px for a
  wide character, 6×13 px for a narrow fallback — with the subpixel compositor
  below.
* `--font-mode auto` (default) and `--font-mode bitmap` use the bitmap font
  first and fall back to the vector font only when the glyph is missing.
  `--font-mode ttf` uses the host vector font for every glyph, so a reviewer can
  compare against a TTF-backed fbterm: Latin will not be pixel-crisp and block
  art will no longer tile, because no host font is designed for a 6×14 cell.
* Bold cells use the `6x13B` face (falling back to regular when the bold face
  lacks a glyph, e.g. `█`); bold CJK uses the weighted face of the CJK
  collection when it has one (Hiragino Sans GB W6 on this machine).

**Subpixel compositor (vector glyphs only)** — an approximation of LCD RGB
stripe rendering, applied in this order:
1. rasterise the glyph into an 8-bit `L` mask at 3× horizontal and 3× vertical
   resolution of the device box (12×13 → 36×39), with the host font sized so
   the advance fills the cell: a 36 px em for a 12 px wide (CJK) box and a
   30 px em for a 6 px wide cell (12 / 10 device px per em), baseline at the
   bitmap font's 11 px ascent;
2. per device pixel, average each subpixel *column* over the 3×3 block
   → c_r, c_g, c_b (3 horizontal subpixels per pixel, vertical box-filtered);
3. apply FreeType's `[1,2,1]/4` horizontal FIR across the subpixel samples in
   RGB order, per device row;
4. composite per channel: `out = bg*(1-c) + fg*c`.
`--subpixel off` uses the average of the three coverages for all channels
(grayscale antialiasing, no coloured fringes).

**How faithful is it?** The geometry is exact (320×170 panel, 6×14 pitch, 6×13
glyph box, 2-cell CJK, 1 px margin), the LCD stripe order is real (R,G,B), and
the bitmap glyphs are drawn with zero antialiasing exactly like a bitmap
framebuffer font. It is still a *model*: the glyph shapes for Latin come from
the canonical X11 6×13 bitmap font, not from the exact font on the board — that
font cannot be read back without device access, so the family is an assumption
(the geometry is not). The subpixel filter is a `[1,2,1]/4` approximation of
FreeType's LCD filter (FreeType's default is a 5-tap FIR), there is no panel
gamma/harmony model, so the coloured fringes are somewhat stronger and more
saturated than a real panel's, and the ANSI palette is an approximation of the
panels' colours. CJK/vector glyphs are therefore legible but visibly
rainbow-fringed at 1:1; that is the compositor, not a capture artefact.

The renderer keeps the two-pass drawing (all cell backgrounds first, then
glyphs), so the continuation cell of a wide character inherits its leading
cell's colours and CJK in a reverse-video row is never clipped. The ANSI
8/256-colour mapping, bold brightening and the cursor rectangle are unchanged.

`manifest.json` → `renderer` records the model and the fonts actually used:
`native_size` `[320,170]`, `output_size`, `grid`, `cell_pitch` `[6,14]`,
`glyph_box` `[6,13]`, `text_area`/`text_origin`, `font_mode`, `subpixel`,
`bitmap_font`/`bitmap_bold_font`, `mono_font`/`cjk_font`/`symbol_font` and
`fonts_used` (the fonts rasterised during the run).

---

## 5. `mock_api.py`

```bash
python3 mock_api.py --port 8765 --script sample_script.json --log .work/requests.jsonl
```

| endpoint | behaviour |
| --- | --- |
| `GET /v1/models` | `{"object":"list","data":[…]}`; each model has `id`, `name`, `context_window` (what `api.rs::context_window` parses) and `effort.supported_levels` (what `list_models` parses) |
| `POST /v1/chat/completions` | streaming SSE when `"stream": true` (dst always streams), plain JSON otherwise; `Authorization` accepted as-is |
| `GET /health` | request counters / remaining script entries |
| `GET /` | endpoint + script summary |

dst request bodies (`model`, `messages`, `tools`, `stream`, `max_tokens`,
`tool_choice`, `stream_options`, `reasoning_effort`) are accepted as-is; the
full body is written to the JSONL log. `--port 0` binds an ephemeral port and
prints `MOCK_API_PORT=<n>` / `MOCK_API_URL=…` lines (used by `run_demo.py`).
`--auto-tool` (or `"auto_tool": true` in the script) enables the
"tool call then final reply" mode when the script runs out.

### Request script format

Per-request responses are consumed in order: request N uses entry N. When the
script is exhausted the server answers with a canned text reply (or the
auto-tool flow). Each entry is a shorthand:

```json
{
  "initial_delay_ms": 150,
  "reasoning": "The user says hello, keep the answer short.",
  "reasoning_chunk_chars": 11,
  "reasoning_delay_ms": 120,
  "content": "Hi! streamed from mock_api.py",
  "content_chunk_chars": 8,
  "content_delay_ms": 80,
  "tool_calls": [
    {"id": "call_1", "name": "list_dir", "arguments": {"path": "."},
     "tool_chunk_chars": 6, "tool_delay_ms": 45}
  ],
  "finish_reason": "tool_calls",
  "usage": {"prompt_tokens": 262, "completion_tokens": 31,
            "prompt_cache_hit_tokens": 224, "prompt_cache_miss_tokens": 38,
            "total_tokens": 293}
}
```

* `reasoning` is streamed as `delta.reasoning_content` (dst's thinking field;
  `reasoning` is the fallback name), `content` as `delta.content`.
* Tool calls are streamed as OpenAI-style `delta.tool_calls` chunks: the first
  chunk carries `id` + `function.name` + the first slice of `function.arguments`,
  later chunks carry argument slices only. dst reassembles them per `index`.
* `finish_reason` defaults to `tool_calls` when tool calls exist, else `stop`.
  `usage` (all five fields incl. cache hit/miss) rides on the finish chunk — dst
  only reads usage from a chunk that has `choices[0]` — and again on a
  spec-shaped usage-only chunk.
* Strings are split into `*_chunk_chars` deltas with a `*_delay_ms` pause, so
  the UI can be snapshotted mid-stream.

For exact control, use `events` instead of the shorthand:

```json
{"events": [
  {"type": "delay", "ms": 100},
  {"type": "reasoning", "text": "think", "delay_ms": 80},
  {"type": "content", "text": "Hi", "delay_ms": 40},
  {"type": "tool_call", "index": 0, "id": "call_1", "name": "list_dir"},
  {"type": "tool_call", "index": 0, "arguments": "{\"path\":\".\"}"},
  {"type": "finish", "reason": "tool_calls"},
  {"type": "usage", "usage": {"prompt_tokens": 10, "completion_tokens": 5}},
  {"type": "raw", "chunk": {"choices": []}}
]}
```

Top level may be `{"models": [...], "auto_tool": true, "responses": [...]}` or
just `[...]` (list of response entries).

### JSONL log

Every `POST /v1/chat/completions` appends two records to `--log`:
`{"type":"request","request":N,"time":…,"authorization":…,"stream":…,
"script_source":"script|fallback","script_entry":N,"body":{…}}` and
`{"type":"response","request":N,"summary":{reasoning_chars, content_chars,
tool_calls, finish_reason, usage, chunks}}` (a disconnect adds
`"aborted":true`).

### Auto-tool mode (no script needed)

```bash
python3 mock_api.py --port 0 --auto-tool
# point ~/.dst/config.yml at the printed MOCK_API_URL, then:
python3 screen_sim.py --home /tmp/auto-home --cwd . --send hello --key Enter \
  --wait-stable 900:20000 --snap tool_then_final
```

First request → `list_dir` tool call; dst executes it; second request → final
text. Demonstrated working; the final screen contains the `list_dir` tool line
and "Done: … this is the follow-up reply generated by mock_api.py".

---

## 6. How the simulator works

1. `pty.openpty()`; `TIOCSWINSZ` to 53×12; slave termios set to raw with
   `VMIN=0, VTIME=3` **before** `fork`; child does `setsid()` + `TIOCSCTTY`,
   `dup2` slave to 0/1/2, then `execve(dst)` with `TERM=xterm-256color` and the
   requested `HOME`/cwd/env. A controlling tty is required because dst probes
   `stty size` / `/dev/tty`.
2. Output is drained with `select()`, decoded with an incremental UTF-8 decoder,
   and fed to `pyte.Stream(pyte.Screen(cols, rows))`. All waits/snapshots run in
   the same thread, so snapshots are deterministic (no reader race).
3. Each snapshot is rendered from the pyte screen onto a native 320×170 RGB
   panel image (53×12 cells of 6×14 px, 1 px black margin; see §4 "PNG renderer").
   The vendored 6×13 bitmap font supplies exact, unantialiased Latin/Greek/
   Cyrillic/punctuation/arrow/box/block glyphs; CJK (pyte puts the char in cell
   N and a `""` continuation in cell N+1, which is honoured) and glyphs the
   bitmap font lacks are rasterised from a host vector font at the device box
   size with the RGB-subpixel compositor; then the panel is integer-upscaled
   for the output PNG. ANSI colours (incl. 256-colour, bold, reverse) map to a
   fixed dark terminal palette; pyte's cursor is drawn as a 1 px blue rectangle.
4. dst is stopped with a Ctrl-C byte (raw mode turns it into a normal key) so it
   restores the terminal and prints `bye`; SIGTERM/SIGKILL are fallbacks.

---

## 7. Adding scenarios

1. Add response entries to a mock script (or use `--auto-tool`). Entry N is the
   response to request N; a plain chat turn is 1 request, a tool round trip is 2
   (tool_call → after the tool result → final reply).
2. Write a scenario JSON with the steps you want (see §4). For streaming states,
   put a `wait_ms` in the middle of the streamed reasoning/content and snap
   there; for settled states use `wait_stable`.
3. Run:
   ```bash
   python3 screen_sim.py --home /tmp/dst-home --cwd . \
     --scenario my_scenario.json --out my-shots
   ```
4. Inspect `my-shots/*.png` (and `my-shots/*.txt` when you only need the grid).

---

## 8. Verified evidence

Captured by `python3 run_demo.py` against the current `cargo build` output
(53×12, PNGs are 960×510 px: the 320×170 device panel at scale 3):

| screenshot | state |
| --- | --- |
| `screenshots/01_splash.png` | animated whale splash + hint line `deepseek-flash \| hold Tab to talk \| /help \| /quit` |
| `screenshots/02_thinking.png` | `> hello` sent, magenta `reasoning_content` deltas streaming, `\ thinking 0.9s` live indicator |
| `screenshots/03_reply.png` | streamed content + collapsed `▸ thinking 2.2s \| 167 chars (collapsed)` |
| `screenshots/04_tool_call.png` | yellow `list_dir` tool lines while the follow-up request is in flight |
| `screenshots/05_final_reply.png` | final markdown reply, CJK line `中文测试：宽字符单元格渲染正常。` |
| `screenshots/06_final.png`, `final.png` | final framebuffer (same as `final.txt`, printed to stdout) |
| `screenshots/contact_sheet.png` | 2×2 labelled sheet of splash / thinking / tool_call / final_reply |

Text versions of every snapshot (`*.txt`), `manifest.json` (fonts, input
timeline, byte counts, dst exit code) and `.work/requests.jsonl` (request /
response evidence, including the tool-call turn and the `role:"tool"` follow-up
request) are produced too. `run_demo.py` fails with a non-zero status if any of
the 10 checks regress.

---

## 9. Limitations / notes

* **macOS vs dst's stty calls.** dst invokes `stty -F /dev/tty …` (GNU syntax);
  BSD/macOS stty only knows `-f`, so dst's own raw/poll-mode setup silently fails
  here. The simulator pre-configures the pty slave itself (raw, `VMIN=0`,
  `VTIME=3`) before `exec`, which reproduces dst's intended Linux behaviour well
  enough for inspection. Consequences: dst's idle `edit()` loop redraws the
  screen roughly every 300 ms, so **pty output is never quiet while dst idles**
  — use `--wait-stable` / `wait_stable` to detect a finished turn;
  `--wait-quiet` is only useful for genuinely quiet processes (blocking call,
  after exit).
* **PingFang SC is not installed on this machine.** `/System/Library/Fonts`,
  `/System/Library/Fonts/Supplemental` (and `/Library/Fonts`) contain Hiragino
  Sans GB, STHeiti, Songti and Arial Unicode but no PingFang; the renderer falls
  back to **Hiragino Sans GB**. Discovery checks PingFang first, so a machine
  that has it will use it automatically; override with `--cjk-font`.
* **pyte 0.8.2** models the character grid, SGR colours/bold/reverse and the
  cursor; it does not emulate every DEC private mode (e.g. no blinking,
  italic/strikethrough rendering, limited origin/resize handling). The cursor is
  drawn as an outline at pyte's tracked position, so a few applications that
  position the cursor outside the grid would look clamped.
* **CJK/wide cells**: wide characters occupy exactly two cells = 12×13 px (one
  char + pyte's `""` continuation cell, which the renderer skips for glyphs but
  paints with the leading cell's background, so reverse-video CJK rows are
  whole). Ambiguous-width characters (e.g. `·`, `→`) are drawn as one cell. A
  wide character in the last column would be clipped at the panel edge; pyte's
  wrapping normally prevents that. Block glyphs (█▓░) fill their 6×13 glyph box
  exactly; the 1 px line gap below each line is background colour (that is the
  device model, so a multi-row block sprite shows thin horizontal gaps).
* **The PNG is a model, not a capture.** Geometry is exact, glyph shapes for
  Latin come from the canonical X11 6×13 bitmap font (the board's exact font
  cannot be read without device access), and vector glyphs get a `[1,2,1]/4`
  LCD approximation: coloured fringes are stronger than a real panel's. See
  §4 "PNG renderer" for the full model.
* **Colours** are an approximation of a dark xterm-256colour palette, not the
  exact Cardputer panel gamma. Bold base colours are brightened; 256-colour
  cells are mapped through the standard cube.
* **dst is under active development** (the repo's `src/` was being edited while
  these screenshots were generated). The simulator is content-agnostic: it
  captures whatever the current build draws. Snapshot text therefore reflects
  the current UI wording (e.g. dst strips `_`/`*` markdown markers, so
  `mock_api.py` shows as `mockapi.py`; tool lines may show both an in-progress
  and a settled row).
* The pty is fixed at the requested size; dst clamps to its panel (max 53×13).
  The device model is 53×12 (12×14 = 168 px high); `--rows 12` is the default
  and device-faithful size. Non-default `--cols/--rows` grow the model canvas
  just enough to fit the requested grid (so they are inspectable, not clipped),
  which means the PNG is no longer exactly 320×170 px before scaling; the
  default run is always exactly 320×170 → `320N × 170N` PNG.


---

## 8. Feature checks (`run_scenarios.py`, `check_memory.py`)

`run_demo.py` is the visual smoke test. The other two scripts assert behaviour
for every feature that is easy to regress, on screens **and** on the mock
request log:

| scenario | feature | what it verifies |
| --- | --- | --- |
| `thinking` | reasoning panel | reasoning streams as full-width wrapped lines (not one line per chunk) and folds to one line when the answer starts |
| `tool_order` | tool pipeline | `tool/call` line appears while the tool is still running, no settled result yet; after return the result row + preview appear and the result reaches the model |
| `tool_timeout` | tool timeouts | a tool overrunning `tools.call_timeout_ms` settles as `timed out`, the turn continues, the model sees the error result |
| `esc_cancel` | Esc interrupt | Esc cancels the stream, late content is discarded, the voice layer receives a stop request, and the prompt is usable again |
| `step_limit` | step limit | `agent.max_steps: 1` stops after one step with the limit notice |
| `unlimited_steps` | no step cap | 13 tool steps run to completion (14 requests) with no hidden 12-step cap |
| `repeat_reminder` | context injection | the third identical tool call injects DSH's repeat reminder into a later request |
| `time_context` | context injection | exactly one clock reading per request, turn numbers advance, no accumulation |
| `compact` | auto compaction | summariser instruction sent, `<compacted-summary>` checkpoint reaches the next request, conversation continues |
| `compact_manual` | manual `/compact` | `/compact` summarises immediately (no threshold), notice shown, next turn carries the checkpoint |
| `goal` | goal round driver | `/goal` creates a goal, round 1 starts automatically with `<goal_round>`, banner shows `◎ goal 1/<cap>`, cap stops continuation |
| `goal_manage` | goal commands | queued `/goal pause` stops the loop, `/goal resume` runs the next round, `/goal edit` changes the objective, `/goal clear` removes goal + banner |
| `goal_complete` | goal completion | model self-judges via `update_goal complete`, closing message renders, banner disappears, goal persists as complete, no further round |
| `goal_blocked` | blocked policy | premature `blocked` report is rejected with DSH's 3-round threshold, goal stays active, Esc disarms the run |
| `clear` | `/clear` | splash returns, live/thinking text and panels are gone, a `/clear` typed while busy is dispatched as a command (not sent to the model) |
| `config_menu` | `/config` | new settings (max steps, auto compact, goal rounds) are reachable, Enter cycles the compaction threshold, the value is persisted to `config.yml` |
| `commands` | commands/palette | help lists `/goal` `/compact`, palette completes `/g` to `/goal`, `/usage` renders, `/model` lists endpoint models |
| `artifact` | artifact panel | the model panel replaces the screen, Esc dismisses it and the conversation continues |
| `session_menu` | rename + `/resume` | the `rename` tool persists a session title, it appears in the header, `/resume` lists titles in a menu, Right expands a preview, Enter resumes the transcript, `/model` is a menu, and no numbered option lists appear |
| `sandbox_auto` | permissions | safe read-only command runs; system-change rule rejects; unknown command triggers the short judge LLM and is denied with its reason |
| `sandbox_strict` | permissions | pop-up shows tool/args and allow/reject; `n` rejects, tool fails with "rejected by the user", conversation continues |
| `sandbox_full` | permissions | dangerous command executes without a prompt |
| `jobs` | background jobs | job_start x2, job_wait returns output, job_kill stops the long job, ids visible |
| `watcher` | watchers | watcher lines arrive as `[watcher w1]` user messages, trigger an autonomous turn, watcher_stop stops it |
| `resume_search` | resume UX | sessions from two directories, current folder first, relative age labels, type-to-filter by name only |
| `i18n_es` | localisation | Spanish splash/hints, translated config menu and command descriptions |
| `job_manager` | manual task manager | Down opens bullets for jobs/watchers, Enter shows output, x kills |
| `onboarding` | first-run flow | an unconfigured start shows a menu (official vs custom endpoint), arrows move, no "type 1 or 2" prompt, Esc cancels |
| `rewind_prompt` | `/rewind` to an earlier message | the picker lists previous user inputs, restoring cuts the transcript back to before the chosen message, that message is pre-filled in the prompt, and pressing Enter re-sends it (the abandoned future is gone from the next request) |
| `btw_side_question` | read-only side-question subagent | `/btw` runs its own request with the conversation context and only read-only tools (no write tools in the schema), shows the answer in an artifact panel, and leaves the main request history and session untouched |
| `busy_commands` | the three input classes during a turn | `/usage` runs immediately, `/model` is refused, `/btw` runs as a separate read-only request with its answer in a panel, plain chat queues, and no command ever enters the model context |
| `state_report_steps` | one report per working step | each tool step starts with `state_report`, the pinned row survives tool cards/text and is replaced by the next report, then joins the transcript when the turn ends |
| `usage_ring_reset` | fresh-start context ring | a `usage.json` carrying a large previous-run context starts at `○ 0%`, not the old value |
| `todo_plan` | DSH-style todo plan | `todo_write` sends the whole `{content,status}` list, the `done/total` badge moves 0/2 → 1/2, the result line is DSH's, and the list is cleared when the next turn starts |
| `new_session` | `/clear` and `/new` | both reset the conversation (the next request is system+user only), restart the splash and zero the ring |
| `read_image_tool` | agent opens an image | the model calls `read_image {"file_path": "~/photo.png"}`, the card shows the path, the tool result carries the DeepSeek `image_url` block so the model describes the picture, and the following request's history has only the `[image read: …]` placeholder |
| `image_input` | local image attachments | `/image ~/photo.png` attaches a local PNG, one line per attachment plus the live indicator, the chat request carries the DeepSeek `image_url` content block (data URL), the model answers, and the next request's history has only the `[image attached: …]` placeholder (no base64 re-sent) |
| `onboarding_hw` / `onboarding_reject` | first-run hardware survey + credential test | after a verified configuration the hardware-info scan is offered once (Y writes the measured scan into the shipped skill, `tui.hw_survey_done`); an unusable key is rejected, the device stays unconfigured and no survey runs |
| `update_hwinfo` / `update_hwinfo_creator` | `/update-hwinfo` | normal mode rescans locally and reports subsystems/size/version; creator mode also queues one model turn that reads the refreshed skill and refines it |
| `progress` | progress row, tool expansion, CtrlO, interrupts | `state_report` pinned to the top row, expanded tool-call arguments, CtrlO during a turn, PgUp while busy, Esc cancelling the whole turn and the next message being sent |
| `paging` | transcript paging | PgUp/PgDn move a screenful with the scroll clamped (never scrolls into blankness), End returns to the newest lines |
| `esc_queue` | interrupt + queued message | typing during a turn queues the message; Esc stops the turn and the queued text is sent next (exactly two agent requests) |
| `voice_interrupt` / `voice_interrupt_send` | voice interrupt | Tab during a turn stops it at once and starts recording; the take is left in the prompt (`auto_send_voice: false`) or auto-sent (`true`) |
| `creator_app_flow` | real build-and-run flow | scripted model writes a sh package (battery viewer) plus a prompt package, `app_test` runs it, `/apps` lists both, Enter runs the viewer full-screen (`Battery: 87%`), the prompt package is previewed and submitted |

```bash
cd tools/sim
python3 run_scenarios.py               # all 101 checked scenarios (~13 min)
python3 run_scenarios.py goal compact  # just some
python3 check_memory.py                # 20-turn session, samples dst RSS
python3 run_all.py                     # demo + scenarios + memory in one go
```

Each scenario runs against an ephemeral mock port and a temporary `HOME`, so
`~/.dst` is untouched; `--quick` on `run_all.py` skips the long memory check.
Add a directory under `scenarios/` (with `config.yml`, `scenario.json`,
`script.json`, optionally a `home/` overlay) and register a check function in
`run_scenarios.py` to cover more behaviour.

`check_memory.py` drives a 20-turn session with large replies and samples the
`dst` child's RSS: the transcript is capped, so the process must stay far below
the device's 512 MB budget (tens of MB on the debug build) while still
rendering the last turn.

Note: `mock_api.py` serves plain HTTP on `127.0.0.1`. `dst` only relaxes its
HTTPS requirement for loopback endpoints, so `/model` listing works against the
mock while remote endpoints still require TLS.
