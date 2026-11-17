# `dst` splash storyboard — “the whale that comes up for air”

Status: **research + storyboard + phase-2 implementation record** — the design
below is what shipped in `src/art.rs` + `src/main.rs`; §9 is the implementation
record with the verification evidence.
Audience: the phase-2 implementer (and the lead for sign-off).

Repo: `deepseek-tui-cardputer` · target panel: 320×170 px, fbterm, **53×12** glyph
grid, 8 ANSI colours, no emoji / no wide cells, 4× Cortex-A53 / 512 MB.
Reference read for this design: [`ccch1mneyyy/dsh-TUI`](https://github.com/ccch1mneyyy/dsh-TUI)
(cloned at `/tmp/dsh-TUI`, shallow, commit `77f2c51`).

The animation block stays **9 rows × 53 columns fixed** (transcript body rows 0–8);
row 9 of the transcript remains the one-line hint (`splash_hint`). Every frame is
9 rows and exactly 53 cells (stored space-padded; the renderer right-pads if an
editor strips the trailing spaces — see §5).

---

## 1. Reference project: how dsh-TUI opens

dsh-TUI is a React/Ink TUI. Its opening screen is a **header**, not a full-screen
splash, but the animation machinery is exactly the thing this storyboard borrows
from. Files that matter (all paths relative to the reference repo root):

| file | lines | what it does |
| --- | --- | --- |
| `src/components/whaleFrames.ts` | 799 | 22 hand-drawn whale frames, **25 rows × 40 palette chars** each. Palette alphabet: `D` outline · `B` body · `L` belly · `W` mouth · `H` heart · `Z` sleep-Z · `.` transparent. `OpeningStep = { frame, ms }`; three `OPENING_SEQUENCES` (`classic`, `heart`, `sleep`); every sequence opens and closes on the `standard` pose; `pickOpeningSequence()` rolls one at mount. |
| `src/components/Whale.tsx` | 160 | Half-block encoder: two vertical sprite pixels per terminal cell (`▀` upper = fg, `▄` lower = fg, both = `▀` + bg), rendered through **6 true-colour RGB tones** (navy outline, DeepSeek blue, ice belly, white mouth, pink heart, gray Z). Sprite becomes **40 cols × 13 rows of ANSI**; every frame is pre-rendered to strings **at module load**; the box width is pinned to 40 so a 4-column-wider tail-wag frame never shifts the text column; each row ends with SGR reset + erase-to-EOL so no “ghost pixels” survive a frame switch. |
| `src/components/whaleLayers.ts` | 116 | Splits the 22 frames into a still `BASE` (standard body with every motion region cleared) plus per-action **diff regions**: eyes / tail / fins / spout / heart / sleep. `composeWhaleGrid(pose)` paints independent planes on top of each other, so a heart can play over a mid-wag tail. |
| `src/components/whaleIdle.ts` | 258 | Settled-header behaviours, ported from the `dsh-ui-whale` web pet: 120 ms planning tick, event-driven (only the next due transition timer is armed), tail/fin passes, blink, 10 s-then-sleep, sleep-Z loop. |
| `src/components/LogoV2.tsx` | 796 | The **phase machine**. `useState(OPENING_SEQUENCES[intro ?? pickOpeningSequence().id])`; `step`; `settled = step >= sequence.length`; a `setTimeout(sequence[step].ms)` chain walks the steps. The 60 ms shimmer clock (`useAnimationFrame`) is subscribed **only while the intro plays** and unsubscribed at settle, so the settled header holds zero timers. A `whaleFrozen` latch flips on the first `working` turn: after the first task the whale is a static standard pose. |
| `src/components/splashLayout.ts` | 64 | Narrow-terminal degradation ladder: whale + big text → big text only → whale only → one plain line; `WHALE_BOX_WIDTH = 40`, `COLUMN_GAP = 2`. |
| `src/components/bigfont.ts`, `src/components/splashFonts.ts` | 141 / 541 | 5-row block-font wordmark `DEEPSEEK` / `HARNESS`, 8 faces rotated by local date, horizontal colour gradient plus an 8-column highlight sweeping at 60 ms. |
| `src/components/shimmer.ts` | 85 | Blue→ice→pale colour ladder (`#4D6BFE → #93BEFF → #D7E4FF`) and `sweep()` — a travelling highlight over the wordmark/tagline. |
| `src/components/splashEggs.ts` | 117 | Date-window holiday word swap; milestone-gated “star us” line (launch-count / usage-hour ladder in `usageStats.ts`) with an OSC 8 link. Both deterministic and seam-injectable. |

### 1.1 How the opening actually runs

1. **Intro phase** (~2.8–3.5 s, once per mount): a dwell table drives frame swaps.
   `classic` is 18 steps: `standard 400 ms → blink 250 → standard 300 →
   spout1..6 150 ms each → standard 250 → tail1..4 150/150/150/180 →
   tail3/2/1 150 ms each → standard 300`. `heart` is 7 steps (350–500 ms) and
   `sleep` is 11 steps (250–400 ms). Motion steps are **150 ms**, holds are
   **250–500 ms** — a deliberate 1.7–3.3× cadence contrast.
2. **Settled phase**: the same tree frozen at the standard pose, zero timers;
   only the layered idle planner (if enabled) keeps painting at its own cadence.
3. **Freeze latch**: the first agent turn permanently freezes the whale (the idle
   planner and click-hearts are welcome-phase features).
4. Colour motion runs alongside: the 60 ms shimmer sweep and the block-font
   gradient are what make the *text* feel alive while the whale moves.

### 1.2 What makes it feel dynamic (and which parts we can take)

| mechanism | reference implementation | take it for `dst`? |
| --- | --- | --- |
| Per-step dwell table (frame + ms) instead of fixed FPS | `OpeningStep[]` | **Yes** — direct model for `HOLD_MS` / `OPENING` |
| Intro → settled phase split with a timer-free settled state | `settled`, `useAnimationFrame(null)` | **Yes** — adapted as opening loop → sleep loop |
| Cadence contrast (150 ms motion vs 250–500 ms holds) | step table | **Yes** — 300 ms motion vs 600–1800 ms holds (see §3.4) |
| Silhouette-level change, not interior detail | 6 spout frames, 4 tail frames | **Yes** — every beat changes outline/height |
| Geometry pinned so motion never moves the layout | fixed 40-wide box, erase-to-EOL | **Yes** — fixed 53-wide, pre-padded rows |
| Pre-rendered strings, zero per-tick encoding | `RENDERED` at module load | **Yes** — static `&str` tables |
| Never-fully-still idle personality | layered idle planner, sleep after 10 s | **Yes** — sleeping idle loop after 3 opening loops |
| Freeze/stop once work starts | `whaleFrozen` on first turn | **Already there** — `splash_done`/`art_on` end on first submit |
| Table-integrity + render-smoke tests | `scripts/verify-whale-intro.mjs` etc. | **Yes** — Rust unit tests + one sim scenario |
| True-colour gradients + 60 ms shimmer | 6-tone sprite + `shimmer.ts` | **No** — 8 ANSI colours, one fg per row |
| Random intro roll per mount | `pickOpeningSequence()` | **No** — determinism required (§3.5) |
| 120 ms tick | `whaleIdle.ts TICK_MS` | **No** — our idle key poll is 300 ms (§2.3) |
| Layer composition at runtime | `composeWhaleGrid()` | **No** — whole static frames are cheaper and testable here |

Additionally, verification scripts worth reading before writing tests:
`scripts/verify-whale-intro.mjs` (pins the 22 frame names/order, 25×40 shape,
palette alphabet, heart/sleep isolation, sequence validity, standard bookends,
positive dwell, per-intro frame families, random-pick seam, and a render smoke
that proves the pink/gray tones appear only during their intro),
`scripts/verify-whale-idle.mjs`, `scripts/verify-animation-ownership.tsx`,
`scripts/verify-splash-layout.ts`, `scripts/verify-splash-brand.tsx`,
`scripts/verify-splash-eggs.tsx`.

### 1.3 The one thing the reference cannot teach us

The reference leans on **colour motion** (gradient text + travelling shimmer +
6-tone sprite) for a large part of its dynamism. On the Cardputer we have one
foreground tone per row and no truecolour, so the “alive” feeling must come almost
entirely from **silhouette, vertical position, waterline and splash** — which is
why this design is staged as a small scene with a horizon instead of a big static
sprite. That is the main design consequence of the reference study.

---

## 2. Current `dst` splash — what we have

### 2.1 `src/art.rs`

* 154 lines, one table: `FRAMES: &[&[&str]]` — **13 frames × 9 rows**, each row
  currently 28–40 chars (not padded). `ART_ROWS = 9`, `ART_WIDTH = 40`.
  `SPRITE_ROWS = 18` is dead (kept with `#[allow(dead_code)]`).
* The art is the dsh-TUI pixel whale tone-mapped to `█ ▓ ░` and cropped (“spout
  base and the whole belly”): outline `█`, body `▓`, belly `░`, and the white
  mouth becomes a **gap** (spaces). It faces left with the flukes raised at the
  top-right.
* The 13 frames differ by small deltas only (blink/fin/tail/spout); there is no
  scene, no water, no vertical motion. README’s “13 unique frames — blink, fins,
  spout, tail, heart, sleep” is **stale**: there are no heart or sleep frames in
  `art.rs`. Fix the README in the implementation phase.

### 2.2 `src/main.rs` (current line anchors; function names are stable)

| piece | where | behaviour |
| --- | --- | --- |
| row kinds | L130–138 | `K_ART = 6 → \x1b[1;36m`, `K_DIM = 5 → \x1b[90m`; used by `draw()` at L441–457 |
| splash state | L69–71, L92–93 | `art_frame: usize`, `art_on: bool`, `splash_done: bool` |
| `seed_splash(model)` | L327–347 | clears transcript/scroll/think/live/artifact, `art_frame = 0`, pushes **9 art rows as `K_ART`** + **1 `K_DIM` hint row**, `art_on = true`, `draw()` |
| `anim_tick` | L350–362 | guards `!art_on \|\| !input.is_empty() \|\| scroll != 0 \|\| lines.len() != ART_ROWS+1`; advances `(art_frame + 1) % FRAMES.len()`; rewrites the 9 rows as `K_ART`; `draw()` |
| call site | L1412–1420 | only from `Scr::edit()`’s idle branch, right after `read_key_timeout()` returns `None` |
| the clock | L974–981 | `read_key_timeout()` runs `stty min 0 time 3` → **the UI tick is 300 ms** |
| redundant draw | L1393–1398 | `edit()` calls `draw()` at the top of every loop iteration, so at idle the splash is currently painted **twice per 300 ms** |
| `art_frame_lines(frame, cols)` | L2607–2611 | left pad `(cols - 40) / 2`; no truncation for narrow terminals |
| `splash_hint(model, cols)` | L1886–1902 | `"  "` + i18n hints (`{} \| hold Tab to talk \| /help`), model truncated so the line fits `cols` |
| seed points | L4824 (startup), L4918 (`/clear` → `cmd::dispatch` out.clear) | splash returns on `/clear` |
| ends | L5094–5098 | first **submitted message** clears the splash (`splash_done = true`, `art_on = false`) |
| cleared elsewhere | L3352, L3697 (resume/rewind), L4523–4528 (first-run tips) | art removed, `splash_done` kept where needed |

Layout maths: `draw()` uses `body_rows = rows - 2` = **10 rows** at the 12-row
panel; the splash occupies exactly those 10 rows (`9 art + 1 hint`). There is no
separate header row.

Net behaviour today: **13 frames × 300 ms = 3.9 s** loop, one sprite, no scene.
The animation pauses as soon as the input line is non-empty, and the first submit
ends it for good. On a fresh profile the first-run tips screen replaces the art, so
the splash is normally seen on the second launch or after `/clear`.

### 2.3 Constraints this research adds to the brief

1. **The animation clock is the key poll: 300 ms.** Holds can only be whole
   multiples of 300 ms with the current loop; the brief’s “motion frames 90–140 ms”
   is *not renderable* without changing `read_key_timeout` (sub-300 ms timeouts are
   `stty time 1/2`, and each poll already spawns `stty` twice — going from 300 ms
   to 100 ms would triple that process churn on an A53 during the splash).
   §3.4 uses the real 300 ms grid and §6.4 lists the optional fast path.
2. **No random at runtime** — so the reference’s “roll one of three intros” trick
   is replaced by phase variety + a second (sleep) sequence.
3. **One foreground colour per row** (`K_ART` bright cyan, `K_DIM` dark gray).
   A row can only be two colours if the art pre-renders embedded SGR into one
   string — see §3.3 for why this design does **not** need that extension.
4. **53-wide rows hit the terminal edge exactly.** `draw()` already prints exactly
   53-cell `fit()` lines (status/input rows) in the simulator, so 53-char art rows
   are safe — but a 54th cell would wrap and break the panel.
5. **The first-run tips screen hides the art** (`maybe_show_tips` → `art_on=false`),
   so any visual verification needs a temp `HOME` with `tui.tips_shown: true`.

---


## 3. Design: a 53×9 night-sea scene

### 3.1 Goals and non-goals

**Goals**

1. The DeepSeek pixel whale stays the hero, recognisable as the same icon
   (`█` outline / `▓` body / `░` belly / mouth gap).
2. It moves through a *scene*, not a glyph swap: calm sea → spout → gather →
   breach → roll → dive → splash → calm with bubbles → loop.
3. Depth (parallax) with only two row tones: dim background band, bright hero band.
4. The `dst` wordmark appears as a quiet reveal at the end of the loop.
5. After ~20 s of idling the whale settles into a slow sleeping/breathing loop.
6. Deterministic frames, fixed geometry, cheap: same 300 ms tick, fewer draws than
   today, no new timers, no new dependencies.
7. Phase 2 is a small diff: `src/art.rs` tables + ~40 lines in `src/main.rs`.

**Non-goals** — truecolour/256-colour, per-cell colour, runtime sprite composition,
half-block 2:1 pixel packing, randomness, sound, input handling changes.

### 3.2 Scene grammar: three fixed row bands

```
row 0        K_DIM     night sky     — stars (and only stars)
rows 1..=7   K_ART     air           — the moving whale, spout, spray, droplets, Z's
row 8        K_DIM     sea           — wave line, foam, bubbles, `dst` wordmark
```

This fixed split is the whole rendering extension: art rows carry a per-row kind
(`K_DIM` for 0/7/8, `K_ART` for 1–6) instead of every row being `K_ART`. No new
SGR, no embedded escapes, no new `draw()` code — `draw()` already maps those two
kinds.

Why this shape works:

* The **horizon is the drama**: the whale is bright against dark water, and its
  submerged body is *automatically* dim because it is painted on dim rows. Depth
  costs no extra colour.
* The **sky row gives parallax for free**: stars stay put while the whale moves.
* The scene is legible at 6×13 px per cell because each band has one job.

**Glyph vocabulary** (allowed set: ASCII + U+2500–U+259F only; unit-tested):

| use | glyphs |
| --- | --- |
| whale outline / body / belly | `█` `▓` `░` |
| water line / swells | `▁` `▂` `▃` `▄` `▅` `▆` |
| dim submerged hull, foam | `░` `▒` `▓` (dim rows only) |
| spray / spout / droplets / bubbles | `░` `▒` `o` `.` |
| stars | `'` `.` |
| sleep | `z` |
| wordmark | `d` `s` `t` |

Explicitly **not** used: emoji, CJK, box-drawing frames, `·`/`≈`/`…` and any
other non-ASCII, non-block character (they are outside the allowed set and some
are wide in the device font).

### 3.3 Colour and parallax — one colour per row is enough

The brief allows a two-colour row **only if** both parts are pre-rendered into one
line. This design avoids that extension on purpose:

* The whale never needs to be bright *inside* a dim row. Where a row is dim, the
  content is meant to be under water (submerged hull, foam, bubbles, wordmark) or
  night sky (stars). The waterline itself is the occluder: the bright body is cut
  at row 6 and the dim row 7–8 shows its silhouette through the sea. That is not a
  compromise, it is the depth cue.
* Splash spray that crosses the surface lives in the bright rows above it; dim
  foam lives on row 8. No element straddles a band with two tones.
* Cost: zero extra ANSI per line (still 1 SGR prefix + 1 reset + erase-to-EOL),
  zero extra bytes, and no risk of embedded ANSI confusing the 53-cell line width
  (which is measured nowhere for art rows but would matter for a future `fit()`).

If a later polish pass wants a *bright* droplet inside a dim row, the cheap
extension is a per-row kind table plus embedded `\x1b[90m` / `\x1b[1;36m` runs in
that one art row and a `K_ART_RAW` kind that emits the string with no prefix. Do
not do this now: the current scene does not need it.

### 3.4 Timing model: variable holds on a 300 ms grid

The splash scheduler is the same 300 ms key poll that runs the idle UI
(`read_key_timeout` → `stty min 0 time 3`). The design therefore quantises every
hold to whole ticks and spends the budget as **contrast**, not frame rate:

* calm / quiet beats: **600–1800 ms** (2–6 ticks)
* motion beats: **300 ms** (1 tick) — the fastest this loop can paint
* the apex roll: **900 ms** (3 ticks) — the “wow” frame is also a rest frame
* whole opening loop: **22 ticks = 6.6 s**; sleep loop: **21 ticks = 6.3 s**

The reference’s 150 ms motion steps are ~2× faster than ours. Two facts make the
300 ms floor acceptable:

1. Every motion frame changes the silhouette a lot (height, roll, splash), so
   stop-motion at 3.3 fps reads as intentional animation rather than lag.
2. CPU: today the splash paints twice per tick (top-of-loop `draw()` + `anim_tick`’s
   `draw()`) ≈ 6.7 paints/s. The new scheduler paints the top-of-loop draw every
   tick (unchanged) plus one draw **only when a frame changes** (14 frames /
   6.6 s ≈ 2.1/s), i.e. ≈ 5.4 paints/s — about 20 % *fewer* paints than today,
   with the same timer and no extra processes.

Ground truth for the implementer: `TICK_MS = 300` must stay equal to the
`read_key_timeout` VTIME (`time 3`). If that timeout ever changes, every hold
scales with it. The simulator’s `--vtime` must stay at its default 3 for the same
reason.

### 3.5 Determinism

* Frames are static tables; the only state is `(step, hold countdown, loops, sleep)`.
* No `rand`, no `Instant`/`SystemTime`, no environment variable.
* Stars use one fixed pattern for the opening and a second fixed pattern for the
  sleep loop (a cheap twinkle — it flips only when the sleep sequence advances).
* The sleep hand-off is triggered by **completed opening loops** (`IDLE_AFTER_LOOPS
  = 3`), not by wall-clock seconds, so screenshots/scenarios are reproducible at
  the same point in the state machine.
* A seed (`/clear`, startup) always resets to step 0, loop 0, opening.

### 3.6 The wordmark

The brand is thematically a water mark: `dst` sits in the deep row (row 8), right
of the submerged hull, dim, and is **revealed by the water settling**:

* step 12: `d` (row 8, col 45)
* step 13: `ds`
* step 0 of the next loop: `dst`

It is deliberately the quietest element on screen — the bright channel belongs to
the whale. Keep it **contiguous** (`dst`, not `d s t`):

1. it reads as the actual wordmark at 6×13 px;
2. it makes the splash screen contain the string `dst`, which the simulator’s
   multi-phase scenarios use as their “app is up” probe (`wait_for {"text": "dst"}`).
   Today those steps silently time out for 10 s on splash-first launches; with the
   wordmark they match immediately. This is a scenario-timing change: verify
   `history_persist`, `session_menu`, `resume_search`, `visual_content`,
   `login_i18n_*` (all text checks, see §8). If the lead wants literally zero
   timing change, use `d s t` — nothing else in this design depends on it.

### 3.7 Idle variant: “asleep at the surface”

After **3 opening loops (≈19.8 s)** the scheduler switches to the `IDLE` sequence
and stops breaching:

* the whale floats low, only the fluke/back strip above water, dim hull below;
* `z` glyphs drift up-right from the blowhole, growing `z` → `zz`;
* the sky twinkles (fixed alternate star pattern per frame);
* one slow breath (back rises a row, no Z) and one lazy puff (short spout + Z);
* holds are 1.2–1.8 s, so the device spends most of its idle time almost still.

The animation still pauses the moment the input line is non-empty and still ends
on the first submitted message (existing guards). `/clear` restarts the opening
from step 0, including the breach. No click/touch interaction is added — the
Cardputer has a keyboard, and keeping the state machine input-free keeps the diff
small (a future “wake on keypress” would need a new flag in `edit()`; not now).

### 3.8 What changes visually vs today

| | today | this design |
| --- | --- | --- |
| sprite | one 40×9 static-ish whale, small deltas | the **same 40×9 whale**, moved vertically against a water line (plus the original fluke poses); never redrawn per frame |
| colour | all 9 rows bright cyan | 1 dim sky row + 6 bright rows + 2 dim sea rows |
| motion | 13 frames, fixed 300 ms, no vertical motion | 14 opening frames with 300–900 ms holds + vertical emergence, roll, flukes, splash |
| loop | 3.9 s circle | 6.6 s story loop ×3, then 6.3 s sleep loop |
| brand | model/hint only | whale icon + dim `dst` wordmark reveal + hint line unchanged |

---

## 4. Storyboard

### 4.1 Phases

| phase | steps | duration | story beat |
| --- | --- | --- | --- |
| A · night sea | 0–1 | 1.5 s | stars, dim sea, whale back at the surface, `dst` in the deep |
| B · breath | 2–4 | 1.2 s | blowhole, spout grows to a mushroom, droplets fall |
| C · gather | 5 | 0.3 s | whale sinks, water dimples, bubbles rise, wordmark gone |
| D · breach | 6–8 | 1.8 s | body rises out of the sea → full breach with side spray → apex fluke swing (old tail pose) |
| E · dive | 9–10 | 0.6 s | fluke/back farewell, the sea throws a crown |
| F · settle | 11 | 0.6 s | spray falls, bubbles rise, wave line calms |
| G · wordmark | 12–13 (+ step 0 of the loop) | 1.5 s | `d` → `ds` → `dst`, stars, hull — loop |
| H · sleep | 14–17 (after 3 loops) | 6.3 s loop | sleeping whale, Z drift, twinkle, slow breath, lazy puff |

### 4.2 Opening sequence — one loop = 22 ticks = 6.6 s

| step | frame | id | hold | ticks | starts at | story beat |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | 0 | `U00 CALM` | 900 ms | 3 | 0.0 s | whale back afloat, stars, `dst` in the deep |
| 1 | 1 | `U01 BREATHE` | 600 ms | 2 | 0.9 s | back settles one row, exhale |
| 2 | 2 | `U02 SPOUT_A` | 300 ms | 1 | 1.5 s | blowhole puffs |
| 3 | 3 | `U03 SPOUT_B` | 300 ms | 1 | 1.8 s | spout column climbs |
| 4 | 4 | `U04 SPOUT_C` | 600 ms | 2 | 2.1 s | mushroom top, droplets fall |
| 5 | 5 | `U05 ANTICIPATE` | 300 ms | 1 | 2.7 s | sink + dimple + bubbles (gather) |
| 6 | 6 | `U06 RISE` | 300 ms | 1 | 3.0 s | 5 rows out, side spray starts |
| 7 | 7 | `U07 BREACH` | 600 ms | 2 | 3.3 s | full body out, crown, droplets |
| 8 | 8 | `U08 ROLL` | 900 ms | 3 | 3.9 s | apex with the original fluke/tail pose swung out (hold on the wow pose) |
| 9 | 9 | `U09 FLUKES` | 300 ms | 1 | 4.8 s | fluke/back farewell as it dives |
| 10 | 10 | `U10 SPLASH` | 300 ms | 1 | 5.1 s | whale gone, crown falling, foam |
| 11 | 11 | `U11 SETTLE` | 600 ms | 2 | 5.4 s | spray down, bubbles up, wave line calms |
| 12 | 12 | `U12 WORD_d` | 300 ms | 1 | 6.0 s | first letter of the wordmark settles |
| 13 | 13 | `U13 WORD_ds` | 300 ms | 1 | 6.3 s | second letter |
| → | 0 | `U00 CALM` | 900 ms | 3 | 6.6 s | `dst` complete; loop restarts |

### 4.3 Sleep sequence — one loop = 21 ticks = 6.3 s (starts after 3 opening loops, ≈19.8 s)

| step | frame | id | hold | ticks | story beat |
| --- | --- | --- | --- | --- | --- |
| 0 | 14 | `I00 SLEEP_z` | 1800 ms | 6 | low whale, one `z` |
| 1 | 15 | `I01 SLEEP_zz` | 1800 ms | 6 | two `z`s drift, sky twinkles |
| 2 | 16 | `I02 BREATHE` | 1500 ms | 5 | one slow breath (back rises a row) |
| 3 | 17 | `I03 SLEEP_SPOUT` | 1200 ms | 4 | lazy puff + `z`, then loop |

### 4.4 Scheduler rules (for the implementation)

1. `seed_splash` paints `OPENING[0]` and arms `hold_ticks(frame) - 1`.
2. Each `anim_tick` (one per 300 ms poll): if `hold > 0`, decrement and **return
   without drawing** (the top-of-loop `draw()` already repainted the same frame).
   Otherwise advance to the next step, arm its hold, repaint the 9 rows, `draw()`.
3. On wrapping `OPENING`, increment `loops`; when `loops >= IDLE_AFTER_LOOPS`
   switch to `IDLE` at step 0. `IDLE` wraps silently.
4. Any pause (typing, scroll) freezes the countdown; no catch-up is attempted.
   This keeps the animation deterministic in *frame steps*, not wall time.
5. First submitted message still clears the transcript and ends the splash
   (unchanged `splash_done` path); `/clear` re-seeds from step 0.

---


""
## 5. Key frame sketches

All blocks below are **9 rows of exactly 53 cells** (space padded). Monospace
display required. Colour is *not* shown in these blocks — use the band rule:
**row 0 dim (night sky, stars), rows 1–7 bright cyan (whale and spray), row 8 dim
(sea line, foam, wordmark)**. An editor may strip trailing spaces when copying
into `art.rs`; that is harmless because `art_frame_lines` right-pads every row,
but the unit tests reject rows **wider** than 53 (that is the wrap bug).

Width ruler for the first block (do not paste into `art.rs`):

```
         1         2         3         4         5   
12345678901234567890123456789012345678901234567890123
```

### 5.1 The ten key frames

#### 1. frame `0` — U00 CALM  ·  900 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** the brand card. The original 9-row whale sits low: its fluke blade, back and tail stock are the four bright rows above the dim water line (row 8), the body below the line is simply not shown. Stars in the single sky row (row 0); `dst` rests on the water at the right.

#### 2. frame `3` — U03 SPOUT_B  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                  o                                  
                 ░ o                                 
               . ░           █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** the spout column climbs from the blowhole (row 5) toward row 2, one droplet `o` separating; the body has not moved. 300 ms.

#### 3. frame `5` — U05 ANTICIPATE  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▂▂▁▁▂▂▁▁▁▂▂▁▁▁▂▂o▁▂▂o▁▁▂▂▁▁▁▂▂o▁▂▂o▁▁▂▂▁▁▁▁▂▂▁▁▁▁▁▁▁ 
```

**Read this:** gather/anticipation. Same height; the wave line turns choppy and dim bubbles `o` dot it, the wordmark has dissolved. The 300 ms cut into the rise makes the transition snap.

#### 4. frame `6` — U06 RISE  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
  o      ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█            o   
.o       █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓██             o. 
 ░▒░     █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓██            ░▒░  
▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁▁▁▁▁▁▁  
```

**Read this:** the whale is lifted two rows — six bright rows above the water: back, mouth and the raised fluke stock all clear; bright spray `░▒░` at both flanks, stars fixed. Still horizontal: the jump is told by how much whale is out of the water.

#### 5. frame `7` — U07 BREACH  ·  600 ms

```
                                                     
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
   o     █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓██           o    
 .o      █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓██              o. 
░▒       ██▓░           ▓█▓▓▓▓██▓██               ▒░ 
░▒▒░       ███░░░      ░░██▓▓▓▓███              ░▒▒░ 
▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁▁▁▁▁▁▁  
```

**Read this:** full breach — the money pose. The whole body is above the line: fluke (top-right), body, mouth gap and belly shading all readable, only the very tip of the fluke clips at the screen top; flank spray and droplets; the dim wave line is the only background beneath it. Held 600 ms.

#### 6. frame `8` — U08 ROLL  ·  900 ms

```
    '        .         '       .        '      .     
                                 ██▓█     ██         
            ████████████         █▓▓▓█ ███▓██        
o         ██▓▓▓▓▓▓▓▓▓▓▓▓████     ██▓▓▓█▓▓▓██         
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓██████▓▓▓▓▓▓██          .
         █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓███             
.o       █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓▓▓██             o.
░▒░      ██▓░           ▓█▓▓▓▓██▓███            ░▒░  
▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁▁▁▁▁▁▁  
```

**Read this:** apex with the original fluke/tail pose (sampled from the old frame set): the fluke is swung out to the right, spray still hangs at both flanks. The 900 ms hold makes this the “wow” frame of the loop.

#### 7. frame `9` — U09 FLUKES  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                                    ██▓█             
   o        ████████████           ██▓▓█         o   
 .o       ██▓▓▓▓▓▓▓▓▓▓▓▓████       █▓▓▓███████    o. 
  ░▒░    ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓███████▓▓▓▓▓▓▓▓▓██  ░▒░  
▃▄▅▄▃▂▂▃▄▅▆▅▄▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▄▂▁▁▁▂▃▄▃▂▁▁▁▁▁▁
```

**Read this:** the farewell. The same sprite sits four rows up, so only the fluke and the top of the back remain above the water line; spray at both flanks. The iconic “whale is gone” silhouette before the splash.

#### 8. frame `10` — U10 SPLASH  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                          o                          
                  .                 .                
            o          .          o                  
     ▒░          ░▒        ░▒▒░          ▒░          
        ░▒▒░        ░▒░     ░▒▒▒░     ░▒░       ░▒▒░ 
                                                     
▃▄▅▄▃▂▂▃▄▅▆▅▄▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▄▂▁▁▁▂▃▄▃▂▁▁▁▁▁▁
```

**Read this:** no whale. Falling spray (`░▒` clumps and `o`/`.` droplets) over the churned dim wave line; 300 ms — the fastest beat, the crash.

#### 9. frame `13` — U13 WORD_ds  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁ds▂▁▁▁▁▁
```

**Read this:** reveal `ds`; the next loop's step 0 completes `dst` and holds it 900 ms.

#### 10. frame `15` — I01 SLEEP_zz  ·  1800 ms

```
  .        '       .        '      .        '        
               z                                     
                   z                                 
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** sleep, two Zs and the alternate star pattern (the twinkle) — the sky is the only thing that changes between the sleep frames besides the Z height.

### 5.2 The other eight frames (intermediate variants)

Same single 9-row sprite, only its vertical position / water coverage / the
original fluke pose changes — the whale never changes shape between frames.

#### frame `1` — U01 BREATHE  ·  600 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁dst▁▁▁  
```

**Read this:** exhale — the same low pose as CALM, only the wave pattern differs (this is the 600 ms breath before the spout).

#### frame `2` — U02 SPOUT_A  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                 o           █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** first contact — one droplet `o` leaves the blowhole; the body has not moved.

#### frame `4` — U04 SPOUT_C  ·  600 ms

```
    '        .         '       .        '      .     
                ░░░o                                 
                o░                                   
                 ░ o                                 
              .  ░           █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** the mushroom head is two rows wide with four falling droplets at different columns; the 600 ms hold lets the eye read the shape.

#### frame `11` — U11 SETTLE  ·  600 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                                                     
                      o        o                     
                  ░▒░              ░▒░               
                                                     
▁▁▂▁▁▁▁▂▁▁▁▁▁o▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁o▁▁▁▁▁▁▂▁▁▁▁▁
```

**Read this:** aftermath. Spray is down to two dim clumps, two bubbles `o` sit back on the line, the wave pattern is calm again. 600 ms to let the eye exhale.

#### frame `12` — U12 WORD_d  ·  300 ms

```
    '        .         '       .        '      .     
                                                     
                                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁d▁▂▁▁▁▁▁
```

**Read this:** wordmark reveal `d` — one dim letter on the water, right of the whale. Nothing else moves except the swell.

#### frame `14` — I00 SLEEP_z  ·  1800 ms

```
    '        .         '       .        '      .     
                                                     
               z                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** sleep, one Z. The whale floats at the line, `z` rises over the back, stars hold.

#### frame `16` — I02 BREATHE  ·  1500 ms

```
  .        '       .        '      .        '        
                                                     
                                                     
                                                     
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** the sleep breath — same low pose, no Z; the wave line is the only motion.

#### frame `17` — I03 SLEEP_SPOUT  ·  1200 ms

```
    '        .         '       .        '      .     
                  o                                  
                 ░          z                        
                 ░                                   
                             █▓▓▓█████▓▓█            
            ████████████      ██▓▓▓▓▓▓▓█             
          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              
         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                
▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁
```

**Read this:** the lazy puff — short spout from the blowhole plus a Z, then back to step 0.

---

## 6. Implementation plan (phase 2)

### 6.1 `src/art.rs` — new interface

Keep the module a pure data + tiny state machine module (no I/O). Replace the
current `FRAMES`/`SPRITE_ROWS` block with the following shape (18 frames copied
from §5; every row exactly 53 chars):

```rust
//! Animated splash scene, see docs/SPLASH-STORYBOARD.md for the design record.
//! Frames are static: 9 rows, at most 53 cells, ASCII + U+2500..=U+259F.
//! `art_frame_lines` right-pads each row to the panel width (see main.rs).

pub const ART_ROWS: usize = 9;
pub const ART_WIDTH: usize = 53;

/// Row tone bands: row 0 = night sky (dim), rows 1..=7 = air/whale (bright),
/// row 8 = sea (dim). No art row is ever two colours.
pub const DIM_ROWS: [bool; ART_ROWS] =
    [true, false, false, false, false, false, false, false, true];

/// 18 frames, each exactly ART_ROWS rows of ART_WIDTH chars.
pub const FRAMES: &[&[&str]] = &[
    // 0 = U00 CALM, 1 = U01 BREATHE, ... 13 = U13 WORD_ds, 14..=17 = sleep loop
    // Copy the blocks from docs/SPLASH-STORYBOARD.md §5 verbatim.
];

/// Screen time per frame in milliseconds; parallel to FRAMES.
pub const HOLD_MS: &[u16] = &[
    900, 600, 300, 300, 600, 300, 300, 600, 900, 300, 300, 600, 300, 300,
    1800, 1800, 1500, 1200,
];

/// Opening order: plays, then repeats, then hands over to IDLE.
pub const OPENING: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
/// Sleep loop order (entered after `IDLE_AFTER_LOOPS` completed opening loops).
pub const IDLE: &[u8] = &[14, 15, 16, 17];
pub const IDLE_AFTER_LOOPS: u8 = 3;

/// The idle UI key-poll period: MUST match `read_key_timeout`'s `stty time 3`.
pub const TICK_MS: u16 = 300;

/// Round a millisecond hold up to whole ticks (never 0).
pub fn hold_ticks(ms: u16) -> u16 {
    (ms + TICK_MS - 1) / TICK_MS
}

/// Deterministic splash scheduler: frame order + loop counting, no clock.
#[derive(Clone, Copy)]
pub struct Splash {
    step: usize,
    loops: u8,
    sleep: bool,
}

impl Splash {
    pub const fn new() -> Self {
        Self { step: 0, loops: 0, sleep: false }
    }

    /// (frame index to paint, ticks to hold it)
    pub fn current(&self) -> (usize, u16) {
        let seq = if self.sleep { IDLE } else { OPENING };
        let frame = seq[self.step] as usize;
        (frame, hold_ticks(HOLD_MS[frame]))
    }

    /// Advance one frame; switches to the sleep loop after the last opening loop.
    pub fn next(&mut self) -> (usize, u16) {
        self.step += 1;
        if self.step >= (if self.sleep { IDLE } else { OPENING }).len() {
            self.step = 0;
            if !self.sleep {
                self.loops += 1;
                if self.loops >= IDLE_AFTER_LOOPS {
                    self.sleep = true;
                }
            }
        }
        self.current()
    }
}
```

Notes:

* This is the brief’s suggested shape (`HOLD_MS: &[u16]` + a per-frame helper),
  with one addition: the driver works in **whole ticks**, so the helper is
  `hold_ticks(frame)` (equivalently `frame_hold_ms(frame)` = `HOLD_MS[frame]`,
  rounded up by `hold_ticks`). The sequence/loop state lives in `Splash` so it can
  be unit-tested without a terminal or clock; `main.rs` only owns the countdown.
* `SPRITE_ROWS` is dead — delete it (or keep with `#[allow(dead_code)]` only if
  something still reads it; nothing does).
* `FRAMES` rows must carry the trailing spaces of the 53-wide block (a shorter
  row is still fine for the renderer, but the fixed-width contract keeps the
  scene stable and the test simple).
* `HOLD_MS` values are multiples of 300 today; keep that invariant so the table
  means the same thing if `TICK_MS` ever moves (see test 3).

### 6.2 `src/main.rs` — minimal hook

**(a) State** — replace `art_frame: usize` with the scheduler plus a countdown:

```rust
// Scr fields
art: art::Splash,
art_hold: u16,   // ticks this frame still stays
```

`Scr::new()`: `art: art::Splash::new(), art_hold: 0,` (keep `art_on` as-is).

**(b) `seed_splash`** — reset the scheduler and push kinds by row:

```rust
pub fn seed_splash(&mut self, model: &str) {
    self.lines.clear();
    self.scroll = 0;
    self.splash_done = false;
    self.art = art::Splash::new();
    // ... existing clears of think/live/note/artifact ...
    let (frame, ticks) = self.art.current();
    self.art_hold = ticks.saturating_sub(1);
    for (line, kind) in art_frame_lines(frame, self.cols) {
        self.lines.push((line, kind));
    }
    self.lines.push((splash_hint(model, self.cols), K_DIM));
    self.art_on = true;
    self.draw();
}
```

**(c) `anim_tick`** — hold countdown, then advance:

```rust
fn anim_tick(&mut self) {
    if !self.art_on
        || !self.input.is_empty()
        || self.scroll != 0
        || self.lines.len() != art::ART_ROWS + 1
    {
        return;
    }
    if self.art_hold > 0 {
        self.art_hold -= 1;   // same frame: the top-of-loop draw() already paints it
        return;
    }
    let (frame, ticks) = self.art.next();
    self.art_hold = ticks.saturating_sub(1);
    for (i, (line, kind)) in art_frame_lines(frame, self.cols).into_iter().enumerate() {
        self.lines[i] = (line, kind);
    }
    self.draw();
}
```

**(d) `art_frame_lines`** — carry the per-row kind and survive narrow terminals:

```rust
fn art_frame_lines(frame: usize, cols: usize) -> Vec<(String, u8)> {
    let f = &art::FRAMES[frame % art::FRAMES.len()];
    let pad = " ".repeat(cols.saturating_sub(art::ART_WIDTH) / 2);
    f.iter()
        .enumerate()
        .map(|(row, line)| {
            // Truncate on narrow terminals, then right-pad so every row occupies
            // the same number of cells even if an editor stripped trailing spaces.
            let body: String = line.chars().take(cols).collect();
            let body = format!("{body:<width$}", width = cols.min(art::ART_WIDTH));
            (format!("{pad}{body}"), if art::DIM_ROWS[row] { K_DIM } else { K_ART })
        })
        .collect()
}
```

At the 53-column panel `pad` is empty; on a narrower terminal the rows are
truncated to `cols` cells (all glyphs are single-width) instead of wrapping.

Everything else is deliberately untouched: the guards, `splash_done`, the
`/clear` seed call, the submit-time clear, `splash_hint`, the `draw()` colour map
and the i18n strings.

**(e) Pause/end semantics are unchanged.** The animation freezes when the input
line becomes non-empty (any key that edits the prompt) and ends for good on the
first submitted message; keys that leave the input empty (e.g. `Up` with no
history) still do not pause it — identical to today's guard, per the brief's
“see `anim_tick` guards”.

### 6.3 Unit tests to add (`art.rs`, `#[cfg(test)]`)

1. **Geometry/palette**: `FRAMES.len() == HOLD_MS.len()`; every frame has
   `ART_ROWS` rows; every row is **at most** `ART_WIDTH` chars (wider = wrap bug;
   blank rows are legal); every char is ASCII or in `U+2500..=U+259F`
   (single-width allowlist; catches stray emoji/CJK/tabs). `art_frame_lines`
   (main.rs) right-pads so the on-screen block is always exactly 53 cells.
2. **Holds**: every hold is `> 0` and a multiple of `TICK_MS`; `hold_ticks`
   rounds up (e.g. 300→1, 301→2, 900→3).
3. **Sequence integrity**: every index in `OPENING`/`IDLE` is in range; the
   opening and idle loops both change silhouette (at least e.g. 8 distinct
   frames); total opening duration is ~6.6 s and idle ~6.3 s; and the `FRAMES`
   row count / hold-count relationship matches the tables in §4.
4. **Scheduler determinism**: drive `Splash::next()` for `OPENING.len() * 3`
   steps and assert the visited order is the opening order three times, then
   `IDLE[0]`; assert `sleep == true` only after the third loop and never before.
5. **Wordmark**: frame 0 row 8 contains `dst`; frames 12/13 contain `d`/`ds`;
   the sleep frames keep `dst` (so the scenario probe keeps matching).
6. **Band invariants**: row 0 contains only `'` and `.` (stars); row 8 never
   contains `█` or `▓` (the sea line stays soft); rows 1–7 never contain the
   water-line glyphs `▁▂▃▄▅▆`; frame 0 and the sleep frames have at least one star
   in row 0.

### 6.4 Optional fast path (do **not** ship it in the first implementation)

If the lead wants reference-level cadence (100–150 ms motion holds), the minimal
enabling change is:

1. splash-only shorter key poll: a `read_key_timeout_splash()` using
   `stty min 0 time 1` while `art_on && input.is_empty()`;
2. skip the redundant top-of-loop `draw()` when the `anim_tick` guard holds
   (nothing changed since the last paint);
3. re-quantise `HOLD_MS` to 100 ms (motion 100–150, holds 300–1200); `TICK_MS = 100`.

Cost: `read_key_timeout` spawns `stty` twice per call; at 300 ms that is ~6.7
process spawns/s, at 100 ms it becomes ~20/s on the A53 during the welcome phase,
plus up to 10 paints/s in motion frames instead of 3.3. The 300 ms/stop-motion
version already reads as animation because the poses change hard; take the fast
path only if the device is measured to have headroom. Verify with `--raw-log` /
`manifest.json` `bytes_seen` before/after.

### 6.5 Work order and file scope

| # | file | change |
| --- | --- | --- |
| 1 | `src/art.rs` | replace frame table with the 18 frames + `HOLD_MS`/`OPENING`/`IDLE`/`DIM_ROWS`/`TICK_MS`/`hold_ticks`/`Splash` + tests |
| 2 | `src/main.rs` | state fields (a), `seed_splash` (b), `anim_tick` (c), `art_frame_lines` (d); `Scr::new` init |
| 3 | `README.md` | “Animated splash” bullet: new frame set/behaviour, drop the stale “heart/sleep” wording |
| 4 | `docs/UX-AUDIT.md` | startup row wording if the visual changed (optional) |
| 5 | `tools/sim/scenarios/splash/` | optional new scenario: `tips_shown: true` config + `watch` + assertions on the hint/wordmark/frame count |

Not touched: `splash_hint`/i18n, `draw()` colour map, `K_*` constants, config,
`cmd.rs`, dependencies (still zero crates).

---

## 7. Visual verification through the simulator

### 7.1 Build + unit tests (after the lead's commit)

```bash
cd /Users/bytedance/Works/m5stack-tools/deepseek-tui-cardputer
cargo build                      # target/debug/dst
cargo test                       # new art.rs tests
```

### 7.2 A HOME where the splash is actually visible

The first-run tips screen replaces the art (`maybe_show_tips`), so the whale is
only on screen after `tui.tips_shown: true` (or after `/clear`). For a direct
capture:

```bash
rm -rf /tmp/dst-splash-home && mkdir -p /tmp/dst-splash-home/.dst
cat > /tmp/dst-splash-home/.dst/config.yml <<'YAML'
model: deepseek-flash
effort: auto
max_tokens: 1024
api:
  base: http://127.0.0.1:9/v1   # refuse instantly; no mock needed for splash-only capture
  key: test
tui:
  tips_shown: true
  language: en
context:
  time: false
YAML
chmod 600 /tmp/dst-splash-home/.dst/config.yml
```

Two practical notes:

* point `api.base` at a **closed localhost port** (or start `tools/sim/mock_api.py`):
  `refresh_ctx_window` shells out to `curl --max-time 15 {base}/models` after the
  first splash paint, and a real/unreachable remote endpoint can stall the process
  for up to 15 s before the animation loop starts. A refused localhost connection
  returns immediately;
* keep `--vtime` at its default 3 (the splash holds are quantised to that tick).

### 7.3 Frame-order capture (text only, cheap)

```bash
python3 tools/sim/screen_sim.py --home /tmp/dst-splash-home \
  --out /tmp/dst-splash-seq --startup-ms 50 \
  --watch 28 --interval-ms 100 --watch-prefix anim --text-only
```

28 s covers three opening loops (19.8 s) plus one full sleep loop (6.3 s).
Deduplicate the captured screens (100 ms sampling catches every 300 ms frame at
least twice):

```python
import glob, hashlib, os
seen, uniq = set(), []
for p in sorted(glob.glob('/tmp/dst-splash-seq/*_anim_*.txt')):
    t = open(p).read()
    h = hashlib.sha256(t.encode()).hexdigest()
    if h in seen:
        os.remove(p)
        continue
    seen.add(h)
    uniq.append(p)
print(len(uniq), 'unique screens')       # expect 18 (14 opening + 4 sleep);
                                        # ignore a partial first paint without the hint row
```

A cheap order check without parsing the art: assert the unique screens keep the
hint row `hold Tab to talk` present in all of them, and that the *first* unique
screen has the star row; then `--watch` PNGs for the eyeball review.

### 7.4 PNG review (device panel look)

```bash
# whole loop at 3x (960x510 PNGs) for a contact sheet
python3 tools/sim/screen_sim.py --home /tmp/dst-splash-home \
  --out /tmp/dst-splash-png --startup-ms 50 \
  --watch 8 --interval-ms 150 --watch-prefix opening --scale 3

# hero frames at 6x (1920x1020) — pick by eye from the dedupe list, e.g.
python3 tools/sim/screen_sim.py --home /tmp/dst-splash-home \
  --out /tmp/dst-splash-hero --startup-ms 50 \
  --watch 7 --interval-ms 150 --watch-prefix opening --scale 6

# 1:1 pixel inspection of a single frame
python3 tools/sim/screen_sim.py --home /tmp/dst-splash-home \
  --out /tmp/dst-splash-native --startup-ms 50 \
  --watch 2 --interval-ms 300 --watch-prefix opening --native
```

Contact sheet (same idea as `tools/sim/run_demo.py::make_contact_sheet`, but over
deduplicated watch frames):

```python
import glob, hashlib, os
from PIL import Image, ImageDraw, ImageFont
seen, panels = set(), []
for p in sorted(glob.glob('/tmp/dst-splash-png/*_opening_*.png')):
    t = open(p[:-4] + '.txt').read()
    h = hashlib.sha256(t.encode()).hexdigest()
    if h in seen:
        continue
    seen.add(h)
    panels.append((os.path.basename(p), Image.open(p).convert('RGB')))

cols = 4
w, h = panels[0][1].size
sheet = Image.new('RGB', (cols * w + (cols + 1) * 8,
                          ((len(panels) + cols - 1) // cols) * (h + 22) + 8), (14, 16, 20))
d = ImageDraw.Draw(sheet)
font = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 16)
for i, (name, im) in enumerate(panels):
    x = 8 + (i % cols) * (w + 8)
    y = 8 + (i // cols) * (h + 22)
    sheet.paste(im, (x, y + 18))
    d.text((x, y), name, font=font, fill=(255, 220, 120))
sheet.save('/tmp/dst-splash-png/contact.png')
print(len(panels), 'panels ->', '/tmp/dst-splash-png/contact.png', sheet.size)
```

What to look for at 3×/6×:

* row 0 stars are visible but subordinate; row 8 wave line reads as a horizon;
* the whale silhouette is recognisable in `U00`/`U07`/`U08`; no glyph falls back
  to a hollow box (all glyphs are in the vendored 6×13 bitmap font);
* bright vs dim contrast survives nearest-neighbour scaling (2 tones only);
* no row wraps: the art never paints a 54th cell (the frame's 9 rows must be
  exactly 53 cells; the status/input rows below must stay on their own lines).

### 7.5 Behaviour assertions

```bash
# pause on typing: art rows must be identical across two snapshots while input is non-empty
python3 tools/sim/screen_sim.py --home /tmp/dst-splash-home \
  --out /tmp/dst-pause --startup-ms 1200 \
  --send "x" --wait-ms 900 --snap p1 --wait-ms 900 --snap p2 --text-only
# compare rows 0..8 of 01_p1.txt and 02_p2.txt: equal

# end on submit: after Enter the art + hint are gone, the prompt line is present
python3 tools/sim/screen_sim.py --home /tmp/dst-splash-home \
  --out /tmp/dst-submit --startup-ms 1200 \
  --send "hi" --key Enter --wait-ms 900 --snap after --text-only

# /clear brings it back, starting from step 0 (also covered by run_scenarios)
```

### 7.6 Verification existing checks must survive (do not “fix” these)

| check | command | what it asserts | why it must keep passing |
| --- | --- | --- | --- |
| demo splash | `python3 tools/sim/run_demo.py` | first `splash` snapshot contains `hold Tab` (“the splash hint line”) | first-run tips screen and/or hint row still there |
| clear scenario | `python3 tools/sim/run_scenarios.py clear` | after `/clear` the final screen contains `hold Tab to talk` (“screen reset to splash”), no “thinking/reasoning” text survives | `seed_splash` re-seeds 9 art rows + hint, art has no forbidden words |
| hint width | unit/visual | `splash_hint` ≤ 53 cells (existing `fit_dots` logic) | unchanged function |
| multi-phase restarts | `python3 tools/sim/run_scenarios.py history_persist session_menu resume_search visual_content login_i18n_en` | text-only checks; these are the scenarios whose “app is up” probe is `wait_for {"text":"dst"}` | the contiguous wordmark makes the splash satisfy that probe; see §8 risk 7 |
| full suite | `python3 tools/sim/run_all.py --quick` | everything above + login + memory | final gate |

### 7.7 Cost check (optional but cheap)

`screen_sim.py` writes `manifest.json` with `bytes_seen` and `elapsed_ms` per
snapshot. Compare the bytes/s of the new splash against the frozen binary's
baseline over the same 20 s watch: the new scheduler should emit **no more** bytes
than today (fewer frame draws; the top-of-loop draw is unchanged). If it is
higher, something is drawing twice per tick.

---

## 8. Risks and regression matrix

| # | risk | mitigation / check |
| --- | --- | --- |
| 1 | hint row lost or moved → `run_demo`/`clear` checks fail | `seed_splash` still pushes 9 art rows + `splash_hint(...)` `K_DIM`; `anim_tick` guard still `lines.len() == ART_ROWS + 1` |
| 2 | a frame is 10 rows or a row is 54 cells → animation stops silently (guard) or the panel wraps | unit test 1 (9 rows, rows ≤ 53, allowlist); `art_frame_lines` truncates + right-pads; sim review |
| 3 | stale scheduler state after `/clear` → splash resumes mid-sleep | `seed_splash` must reset the whole scheduler (`Splash::new`); test by `/clear` twice and watch from step 0 |
| 4 | first submitted message no longer ends the splash | unchanged `splash_done`/`art_on` path (L5094); manual check §7.5 |
| 5 | typing no longer pauses (or pauses too late) | `anim_tick` guard untouched; pause check §7.5 |
| 6 | CPU regression | same 300 ms tick, no new timers; draws per second decrease (~5.4/s vs ~6.7/s); §7.7 bytes check |
| 7 | contiguous `dst` wordmark changes scenario timing: multi-phase `wait_for {"text": "dst"}` steps now match the splash instead of timing out for 10 s | run `history_persist session_menu resume_search visual_content login_i18n_en` after implementing; all their assertions are text-based. Preferred default; if the lead wants zero timing change use `d s t` |
| 8 | narrow terminal: 53-wide rows wrap on `< 53` columns (tmux/sim variants) | `art_frame_lines` truncates to `cols` cells; spot-check `--cols 40` |
| 9 | dim rows make the screen too dark on the device | intended parallax; review at `--scale 3/6`; if unreadable, move the wave line to its own bright row only if the brief’s “dim water line” is abandoned (not recommended) |
| 10 | simulator `--vtime` ≠ 3 stretches holds | documented in §3.4; keep default 3 |
| 11 | README/UX docs drift (currently claims heart/sleep frames) | update in the same implementation PR |
| 12 | binary size | +18 frames × 9 × ~54 B ≈ 9 KB of static strings, no deps; negligible on 512 MB and `opt-level = z` |

**Definition of done for phase 2**

- [ ] `cargo build && cargo test` green.
- [ ] 18 unique screens in the §7.3 capture, hint row present on all, wordmark
      `d → ds → dst` visible at the loop seam, sleep loop reached after 3 loops.
- [ ] pause-on-typing and end-on-submit checks from §7.5 pass.
- [ ] `run_demo.py` and `run_scenarios.py clear` pass, then `run_all.py --quick`.
- [ ] one 3× contact sheet and one 6× hero PNG attached to the implementation PR.
- [ ] README animated-splash bullet matches the shipped behaviour.

---

## Appendix A — timing recap

| constant | value |
| --- | --- |
| `TICK_MS` | 300 ms (must equal `read_key_timeout` `stty time 3`) |
| opening loop | 22 ticks = 6.6 s |
| sleep loop | 21 ticks = 6.3 s |
| sleep hand-off | after 3 opening loops (`IDLE_AFTER_LOOPS = 3`) ≈ 19.8 s |
| fastest hold | 300 ms (motion) |
| longest hold | 1800 ms (sleep) |
| opening frames / sleep frames | 14 / 4 (18 unique static frames) |
| art block | 9 rows × 53 cells, single colour per row (row 0 / 8 dim) |

## Appendix B — cross-references

* Reference project files quoted in §1: `src/components/whaleFrames.ts`,
  `Whale.tsx`, `whaleLayers.ts`, `whaleIdle.ts`, `LogoV2.tsx`, `splashLayout.ts`,
  `bigfont.ts`, `splashFonts.ts`, `shimmer.ts`, `splashEggs.ts`,
  `scripts/verify-whale-intro.mjs`.
* Current code quoted in §2: `src/art.rs`, `src/main.rs`
  (`seed_splash`, `anim_tick`, `art_frame_lines`, `splash_hint`,
  `read_key_timeout`, `draw`).
* Simulator docs: `tools/sim/README.md` (§4 `screen_sim.py`, §9 Limitations),
  `tools/sim/run_demo.py` (splash check), `tools/sim/run_scenarios.py`
  (`check_clear`), `tools/sim/screen_sim.py` (`--watch`, `manifest.json`).
* Brief constraints: 53×12 grid, 6×13 cells, 8 ANSI colours, 9-row art block.

---

## 9. Implementation record (phase 2, shipped)

### 9.1 Files changed

| file | change |
| --- | --- |
| `src/art.rs` | 18-frame scene built around the **original 9-row whale as one moving sprite**; `HOLD_MS`, `hold_ms()`, `hold_ticks()`, `OPENING`, `IDLE`, `IDLE_AFTER_LOOPS`, `TICK_MS`, `DIM_ROWS`, `row_is_dim()`, deterministic `Splash` scheduler + 6 unit tests (geometry/palette, holds, sequence bounds, scheduler order, band invariants, wordmark). |
| `src/main.rs` | splash-only patch: `art: art::Splash` + `art_hold: u16` replace `art_frame`; `seed_splash` resets the scheduler; `anim_tick` is a hold countdown that repaints only when a frame changes; `art_frame_lines` returns `(line, kind)` and right-pads/truncates. Nothing else touched. |
| `README.md` | “Animated splash” bullet describes the moving-whale scene, the bands and the sleep loop. |
| `docs/UX-AUDIT.md` | startup row describes the scene and the one-row hint guarantee. |
| `docs/SPLASH-STORYBOARD.md` | this document (phase table, final art §5, interface §6, implementation record §9). |

Not committed — the lead owns the commit.

### 9.2 Shipped art model: one whale, moved against a water line

The hero is the **original 9-row 40-wide pixel whale from `cd64b87:src/art.rs`**
(old frame 0), used verbatim and never redrawn per frame:

| beat | what moves |
| --- | --- |
| CALM / BREATHE / SPOUT / ANTICIPATE / WORD / SLEEP | the sprite is **4 rows above the water line** → its top 4 rows (fluke blade, back, tail stock, tail/body join) are above water. Resting at 4 instead of 3 rows keeps the **fluke stock joined to the hull above the line** (the source art only joins them at that row). |
| RISE | lifted 2 rows: six bright rows clear (back, mouth, fluke stock). |
| BREACH | lifted to the top: seven bright rows clear — fluke, body, mouth gap and belly all readable; only the fluke **tip clips in the dim sky row** (it is not drawn, rather than drawn dim). |
| ROLL | one row down with the **old fluke/tail pose** (old frame 7): the fluke is swung out at the apex. |
| FLUKES | four rows up with the **old fluke/tail pose** (old frame 8): only the fluke and the top of the back remain above the line. |
| SPLASH / SETTLE | no whale — spray and bubbles over the water line only. |

Row bands changed to **`[sky, bright ×7, sea]`**: the previous two-row sea (rows
7–8) cannot host the 9-row whale fully bright at the peak, so the sea is now a
single dim water row (row 8) that paints the wave line, foam, bubbles and the
`dst` wordmark; row 0 stays the dim star row. Every frame is 53×9; the whale’s
shape is identical across frames — only its vertical position, the original
fluke pose and the water coverage change.

### 9.3 Cadence and per-tick CPU cost (unchanged from phase 1)

* `anim_tick` = one **300 ms** idle key-poll tick; no new timer, thread or `stty` call.
* Hold tick (majority): one integer decrement + return — no allocation, no I/O, no paint.
* Frame-change tick: 9 rows formatted (~486 B) + one 12-row `draw()`; changes ≈
  **2.1/s opening**, **0.6/s sleep**; ≈ **5.4 paints/s vs 6.7/s before** (~18 % fewer).
* Measured pty output during an 8 s capture: **39.5 KB / 7.9 s ≈ 4.9 KiB/s**.
* 90–140 ms motion holds are not used: the UI only wakes on the 300 ms poll
  (optional fast path in §6.4, deliberately not shipped).

### 9.4 Visual revision history

1. **v1 (phase-2 first cut)**: 30×6 hand crop → read as a blocky barge; rejected in review.
2. **v2**: 36×6 max-downsample of the same icon — still blocky, and the raised tail
   had a black seam against the hull in rise/breach frames (rejected in review).
3. **v3 (shipped)**: the original 9-row whale is a single sprite moved against the
   water line. Defects fixed during the revision: tail/hull seam (resting frames
   raised to 4 rows so the join stays above water); the fluke was briefly drawn
   dim in the sky row (read as a grey bar) → the tip is now clipped instead of
   dimmed; spout origin moved down to the blowhole row so the plume touches the
   back; flank spray/droplets rebalanced for the 40-wide body.

### 9.5 Old-vs-new comparison (for the reviewer)

* Old splash captured from a `git worktree add /tmp/dst-old cd64b87` build with the
  same simulator/HOME: `/tmp/dst-old-shots/01_old_000.png`.
* Final 14-frame sheet (`--scale 3`, real device-fidelity renderer):
  `/tmp/splash-final3/contact_sheet.png`
* Side-by-side (old splash | new CALM | new BREACH): `/tmp/splash-final3/old_vs_new.png`
* **Verdict**: at BREACH the whale is the old silhouette — fluke top-right, body,
  mouth gap and belly at the same relative positions, tail attached — it reads at
  least as clearly as the old splash (the only differences are the clipped fluke
  tip at the screen edge and the bottom line hidden by the water row). At CALM the
  fluke/back/tail-stock above the water line read as the same whale at the
  surface; stars and `dst` frame it. The tail is joined to the hull in all 14
  inspected frames.

### 9.6 Verification evidence (all re-runnable)

```bash
cd /Users/bytedance/Works/m5stack-tools/deepseek-tui-cardputer
cargo test                       # 67 passed, incl. the 6 art tests
cargo build                      # debug binary for the simulator
cd tools/sim
python3 run_demo.py              # 10 checks passed, contact_sheet.png written
python3 run_scenarios.py clear ux   # clear: screen reset to splash; ux: PASS
```

Splash-only recipe (closed `api.base`, `tui.tips_shown: true` → `--home /tmp/dst-splash-home`):

```bash
python3 screen_sim.py --home /tmp/dst-splash-home --out /tmp/splash-final3/seq \
  --startup-ms 50 --watch 8 --interval-ms 100 --watch-prefix v --scale 3
# 48 PNGs → 14 unique consecutive frames (0..=13), reviewed at 960x510
# frame order/timing matches HOLD_MS within the 100 ms capture grid
```

Behaviour checks (same HOME):

* **pause on typing**: `--send x --wait-ms 900 --snap p1 --wait-ms 900 --snap p2`
  → art rows 0–8 identical (frozen), hint row still present, input shows `x`.
* **end on first message**: `hi` + Enter → art and hint gone, `> hi` in the transcript.
* **`/clear` re-seeds**: `hi` + Enter, `/clear` + Enter → 9 art rows + hint return at
  the start of the opening loop.
* **six languages** (en/zh/es/ja/de/it, short + 48-char model): splash hint row
  measured through the simulator is 40–53 cells, art rows 53, no wrapping.
* **narrow terminal** (`--cols 40`): rows truncate to 40 cells, max line 40, no wrap.

### 9.7 Known follow-ups

* The sea band is now **one dim row**; if a two-row sea is wanted later, the
  clean way is a per-frame row-tone map (so the peak breach can paint the whale
  bright on what are normally sea rows) — that is an interface change and was
  explicitly out of scope for this revision.
* A “wake on keypress” for the sleep loop would need a flag in `edit()`; out of scope.
* The 100–150 ms motion-hold fast path (§6.4) remains optional/unshipped.
