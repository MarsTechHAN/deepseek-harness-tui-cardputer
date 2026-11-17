//! Animated splash scene: the DeepSeek whale breaching out of a night sea.
//!
//! Design record: `docs/SPLASH-STORYBOARD.md`. The hero is the shipped 9-row
//! pixel whale (the icon from the hand-drawn `whaleFrames.ts` set): every frame
//! reuses that same silhouette and only moves it vertically against the water
//! line, plus the original fluke/tail poses for the roll and the dive.
//!
//! Layout contract: every frame is exactly `ART_ROWS` rows of at most
//! `ART_WIDTH` single-width cells. Row bands are fixed:
//!
//!   row 0      dim    night sky (stars, and the fluke tip when the whale peaks)
//!   rows 1..=7 bright air: the whale, spout, spray, droplets, sleep Zs
//!   row 8      dim    sea surface: wave line, foam, `dst` wordmark
//!
//! The bright/dim split buys parallax on an 8-colour terminal: the whale is
//! bright against the dark band while the sky and sea stay dim, and the water
//! line is drawn last so the whale is clipped exactly at the surface.

pub const ART_WIDTH: usize = 53;
pub const ART_ROWS: usize = 9;

/// Row tone bands: `true` = dim (`K_DIM`), `false` = bright (`K_ART`).
pub const DIM_ROWS: [bool; ART_ROWS] =
    [true, false, false, false, false, false, false, false, true];

/// The 18 frames: opening story 0..=13, sleeping idle loop 14..=17.
///
/// 0  U00 CALM        whale back and fluke above the water line
/// 1  U01 BREATHE     back settles one row, exhale
/// 2  U02 SPOUT_A     first puff
/// 3  U03 SPOUT_B     spout column climbs
/// 4  U04 SPOUT_C     mushroom + falling droplets
/// 5  U05 ANTICIPATE  sink, bubbles at the water line
/// 6  U06 RISE        whale lifted, spray at the flanks
/// 7  U07 BREACH      full whale above the water line
/// 8  U08 ROLL        apex with the fluke swung (sampled tail pose)
/// 9  U09 FLUKES      fluke/back farewell as it dives
/// 10 U10 SPLASH      whale gone, spray falling onto the line
/// 11 U11 SETTLE      spray down, bubbles at the line
/// 12 U12 WORD_d      wordmark reveal: `d`
/// 13 U13 WORD_ds     wordmark reveal: `ds` (next loop shows `dst`)
/// 14 I00 SLEEP_z     asleep at the surface, one Z
/// 15 I01 SLEEP_zz    two Zs drift, sky twinkles
/// 16 I02 BREATHE     one slow sleeping breath
/// 17 I03 SLEEP_SPOUT lazy puff + Z
pub const FRAMES: &[&[&str]] = &[
    // 0 U00 CALM
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 1 U01 BREATHE
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁dst▁▁▁  ",
    ],
    // 2 U02 SPOUT_A
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                 o           █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 3 U03 SPOUT_B
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                  o                                  ",
        "                 ░ o                                 ",
        "               . ░           █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 4 U04 SPOUT_C
    &[
        "    '        .         '       .        '      .     ",
        "                ░░░o                                 ",
        "                o░                                   ",
        "                 ░ o                                 ",
        "              .  ░           █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 5 U05 ANTICIPATE
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▂▂▁▁▂▂▁▁▁▂▂▁▁▁▂▂o▁▂▂o▁▁▂▂▁▁▁▂▂o▁▂▂o▁▁▂▂▁▁▁▁▂▂▁▁▁▁▁▁▁ ",
    ],
    // 6 U06 RISE
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "  o      ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█            o   ",
        ".o       █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓██             o. ",
        " ░▒░     █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓██            ░▒░  ",
        "▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁▁▁▁▁▁▁  ",
    ],
    // 7 U07 BREACH
    &[
        "                                                     ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "   o     █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓██           o    ",
        " .o      █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓██              o. ",
        "░▒       ██▓░           ▓█▓▓▓▓██▓██               ▒░ ",
        "░▒▒░       ███░░░      ░░██▓▓▓▓███              ░▒▒░ ",
        "▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁▁▁▁▁▁▁  ",
    ],
    // 8 U08 ROLL
    &[
        "    '        .         '       .        '      .     ",
        "                                 ██▓█     ██         ",
        "            ████████████         █▓▓▓█ ███▓██        ",
        "o         ██▓▓▓▓▓▓▓▓▓▓▓▓████     ██▓▓▓█▓▓▓██         ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓██████▓▓▓▓▓▓██          .",
        "         █▓▓▓█▓▓▓▓▓▓█▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓███             ",
        ".o       █▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓█▓▓▓▓▓▓██             o.",
        "░▒░      ██▓░           ▓█▓▓▓▓██▓███            ░▒░  ",
        "▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁▂▂▃▄▂▁▁▁▁▁▁▁  ",
    ],
    // 9 U09 FLUKES
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                                    ██▓█             ",
        "   o        ████████████           ██▓▓█         o   ",
        " .o       ██▓▓▓▓▓▓▓▓▓▓▓▓████       █▓▓▓███████    o. ",
        "  ░▒░    ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓███████▓▓▓▓▓▓▓▓▓██  ░▒░  ",
        "▃▄▅▄▃▂▂▃▄▅▆▅▄▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▄▂▁▁▁▂▃▄▃▂▁▁▁▁▁▁",
    ],
    // 10 U10 SPLASH
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                          o                          ",
        "                  .                 .                ",
        "            o          .          o                  ",
        "     ▒░          ░▒        ░▒▒░          ▒░          ",
        "        ░▒▒░        ░▒░     ░▒▒▒░     ░▒░       ░▒▒░ ",
        "                                                     ",
        "▃▄▅▄▃▂▂▃▄▅▆▅▄▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▄▂▁▁▁▂▃▄▃▂▁▁▁▁▁▁",
    ],
    // 11 U11 SETTLE
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                      o        o                     ",
        "                  ░▒░              ░▒░               ",
        "                                                     ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁o▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁o▁▁▁▁▁▁▂▁▁▁▁▁",
    ],
    // 12 U12 WORD_d
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁d▁▂▁▁▁▁▁",
    ],
    // 13 U13 WORD_ds
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁ds▂▁▁▁▁▁",
    ],
    // 14 I00 SLEEP_z
    &[
        "    '        .         '       .        '      .     ",
        "                                                     ",
        "               z                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 15 I01 SLEEP_zz
    &[
        "  .        '       .        '      .        '        ",
        "               z                                     ",
        "                   z                                 ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 16 I02 BREATHE
    &[
        "  .        '       .        '      .        '        ",
        "                                                     ",
        "                                                     ",
        "                                                     ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
    // 17 I03 SLEEP_SPOUT
    &[
        "    '        .         '       .        '      .     ",
        "                  o                                  ",
        "                 ░          z                        ",
        "                 ░                                   ",
        "                             █▓▓▓█████▓▓█            ",
        "            ████████████      ██▓▓▓▓▓▓▓█             ",
        "          ██▓▓▓▓▓▓▓▓▓▓▓▓████   ██▓▓▓███              ",
        "         ██▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓████▓▓▓▓█                ",
        "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁dst▁▁▁▁▁",
    ],
];

/// Screen time per frame in milliseconds; parallel to `FRAMES`.
///
/// Multiples of `TICK_MS`: the splash is driven by the same 300 ms idle key
/// poll as the rest of the UI, so a hold shorter than a tick cannot be shown
/// without changing `read_key_timeout`. 300 ms motion frames against
/// 600-1800 ms holds is the cadence contrast the reference intro uses; calm
/// and sleeping frames hold longer so the idle screen stays cheap.
pub const HOLD_MS: &[u16] = &[
    900, 600, 300, 300, 600, 300, 300, 600, 900, 300, 300, 600, 300, 300, // 0..=13 opening
    1800, 1800, 1500, 1200, // 14..=17 sleep loop
];

/// Opening order: plays, repeats, then hands over to `IDLE`.
pub const OPENING: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
/// Sleeping idle order: entered after `IDLE_AFTER_LOOPS` completed opening loops.
pub const IDLE: &[u8] = &[14, 15, 16, 17];
/// How many opening loops play before the splash settles into the sleep loop.
pub const IDLE_AFTER_LOOPS: u8 = 3;

/// Idle UI key-poll period: must match `read_key_timeout`'s `stty time 3`.
pub const TICK_MS: u16 = 300;

/// Hold for `frame` in milliseconds (wraps on the table length).
pub fn hold_ms(frame: usize) -> u16 {
    HOLD_MS[frame % HOLD_MS.len()]
}

/// Hold for `frame` rounded up to whole idle ticks (never 0).
pub fn hold_ticks(frame: usize) -> u16 {
    let ms = hold_ms(frame);
    (ms + TICK_MS - 1) / TICK_MS
}

/// Whether the row is painted with the dim tone (`K_DIM`).
pub fn row_is_dim(row: usize) -> bool {
    DIM_ROWS[row.min(ART_ROWS - 1)]
}

/// Deterministic splash scheduler: frame order + loop counting, no clock, no
/// RNG, no environment. `main.rs` owns only the tick countdown.
#[derive(Clone, Copy, Debug)]
pub struct Splash {
    step: usize,
    loops: u8,
    sleep: bool,
}

impl Splash {
    pub const fn new() -> Self {
        Self { step: 0, loops: 0, sleep: false }
    }

    /// Frame to paint now and how many ticks to hold it.
    pub fn current(&self) -> (usize, u16) {
        let seq = if self.sleep { IDLE } else { OPENING };
        let frame = seq[self.step] as usize;
        (frame, hold_ticks(frame))
    }

    /// Advance one step; the opening loop hands over to `IDLE` after
    /// `IDLE_AFTER_LOOPS` completed loops, `IDLE` wraps silently.
    pub fn next(&mut self) -> (usize, u16) {
        self.step += 1;
        if self.step >= (if self.sleep { IDLE } else { OPENING }).len() {
            self.step = 0;
            if !self.sleep {
                self.loops += 1;
                if self.loops >= IDLE_AFTER_LOOPS { self.sleep = true; }
            }
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_width(ch: char) -> bool {
        ch.is_ascii() || ('\u{2500}'..='\u{259f}').contains(&ch)
    }

    #[test]
    fn frames_are_fixed_grids_of_single_width_glyphs() {
        assert_eq!(FRAMES.len(), HOLD_MS.len(), "FRAMES/HOLD_MS length drift");
        for (index, frame) in FRAMES.iter().enumerate() {
            assert_eq!(frame.len(), ART_ROWS, "frame {index}: row count");
            for row in *frame {
                assert!(row.chars().count() <= ART_WIDTH,
                    "frame {index}: row wider than ART_WIDTH would wrap: {row:?}");
                assert!(row.chars().all(single_width),
                    "frame {index}: wide/emoji glyph outside the allowlist: {row:?}");
            }
        }
    }

    #[test]
    fn holds_are_whole_ticks() {
        for (index, ms) in HOLD_MS.iter().enumerate() {
            assert!(*ms > 0, "frame {index}: zero hold");
            assert_eq!(*ms % TICK_MS, 0, "frame {index}: hold not a tick multiple");
        }
        assert_eq!(hold_ticks(0), 3);
        assert_eq!(hold_ticks(2), 1);
        assert_eq!(hold_ticks(14), 6);
    }

    #[test]
    fn sequences_are_valid_and_bounded() {
        for &frame in OPENING.iter().chain(IDLE.iter()) {
            assert!((frame as usize) < FRAMES.len(), "sequence points past FRAMES");
        }
        let opening_ms: u32 = OPENING.iter().map(|&f| hold_ms(f as usize) as u32).sum();
        let idle_ms: u32 = IDLE.iter().map(|&f| hold_ms(f as usize) as u32).sum();
        assert_eq!(opening_ms, 6_600, "opening loop duration");
        assert_eq!(idle_ms, 6_300, "sleep loop duration");
    }

    #[test]
    fn scheduler_plays_opening_three_times_then_sleeps() {
        let mut splash = Splash::new();
        let opening: Vec<usize> = OPENING.iter().map(|&f| f as usize).collect();
        for _loop in 0..IDLE_AFTER_LOOPS {
            let mut seen = Vec::new();
            for _ in 0..OPENING.len() {
                seen.push(splash.current().0);
                splash.next();
            }
            assert_eq!(seen, opening);
        }
        assert_eq!(splash.current().0, IDLE[0] as usize, "sleep loop after 3 opening loops");
    }

    #[test]
    fn band_invariants_hold() {
        for (index, frame) in FRAMES.iter().enumerate() {
            // Row 0 is the night sky: stars only (the fluke clips below it).
            assert!(
                frame[0].chars().all(|c| c == ' ' || c == '\'' || c == '.'),
                "frame {index}: row 0 must stay a star row"
            );
            // The sea line stays soft: no hard blocks on the water row.
            assert!(
                !frame[8].contains('█') && !frame[8].contains('▓'),
                "frame {index}: hard block on the sea line"
            );
            // The water line never leaks into the bright band.
            for row in &frame[1..=7] {
                assert!(
                    !row.chars().any(|c| ('\u{2581}'..='\u{2586}').contains(&c)),
                    "frame {index}: wave glyph in the air band"
                );
            }
        }
    }

    #[test]
    fn wordmark_is_revealed_and_kept() {
        assert!(FRAMES[0][8].contains("dst"), "frame 0 carries the full wordmark");
        assert!(FRAMES[12][8].contains('d') && !FRAMES[12][8].contains("ds"));
        assert!(FRAMES[13][8].contains("ds"));
        for index in IDLE {
            assert!(FRAMES[*index as usize][8].contains("dst"),
                "sleep frame {index} keeps the wordmark");
        }
    }
}
