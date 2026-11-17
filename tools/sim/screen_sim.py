#!/usr/bin/env python3
"""screen_sim.py -- offline screen simulator for the `dst` Rust TUI.

Spawns the dst binary inside a pseudo-terminal (pty), feeds its ANSI output into
a pyte screen model, and renders deterministic snapshots of the resulting
framebuffer as:

  * plain text files (one file per snapshot, plus `final.txt`)
  * PNG images via Pillow (Menlo for the 6x13-ish monospace grid, a CJK font
    for East-Asian wide cells, Apple Symbols for odd glyphs), plus `final.png`

It supports scripted input: text sends, named keys, waits, quiet waits,
snapshots and a watch mode.  Scenarios can be given on the command line
(order matters) or loaded from a JSON file.

Why a pty and not pipes: dst calls `stty`/reads raw bytes and draws a
full-screen framebuffer; a pty is the only faithful terminal.  The slave side
is pre-configured to raw mode by this script because dst's own stty calls use
GNU-style `-F /dev/tty`, which macOS's BSD stty does not understand (it wants
`-f`).  See README.md "Limitations".

Examples
--------
    # splash screen only, text + PNG
    python3 screen_sim.py --home /tmp/home --out /tmp/shots \
        --snap splash

    # send a line, wait for a streamed reply, snapshot twice
    python3 screen_sim.py --home /tmp/home --out /tmp/shots \
        --send "hello" --key Enter --wait-ms 700 --snap thinking \
        --wait-quiet 800:12000 --snap reply

    # JSON scenario
    python3 screen_sim.py --home /tmp/home --scenario sample_scenario.json

    # watch the animated splash for 3 s at 250 ms cadence
    python3 screen_sim.py --home /tmp/home --out /tmp/shots \
        --watch 3 --interval-ms 250 --watch-prefix anim

The final text screen is printed to stdout; diagnostics go to stderr.
Exit status: 0 on success, 2 on usage/setup error, 3 on snapshot/render error.
"""

from __future__ import annotations

import argparse
import codecs
import fcntl
import glob
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time
import tty
import unicodedata

try:
    import pyte
except ImportError:  # pragma: no cover
    sys.stderr.write("screen_sim: missing dependency: pyte (python3 -m pip install pyte)\n")
    raise SystemExit(2)

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover
    Image = None  # text-only mode still works


# --------------------------------------------------------------------------- #
# defaults / constants
# --------------------------------------------------------------------------- #

REPO = os.environ.get("DST_REPO") or os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
DEFAULT_BIN = os.environ.get("DST_BIN", os.path.join(REPO, "target", "debug", "dst"))
DEFAULT_COLS = 53          # device: 320 px / 6 px per cell = 53 columns
DEFAULT_ROWS = 12          # device: 12 visible rows of 13 px in a 170 px panel
DEFAULT_SCALE = 3          # PNG: 1 logical cell = 6*scale x 13*scale px
DEFAULT_OUT = "snapshots"
DEFAULT_STARTUP_MS = 650   # let dst paint the splash before the first snapshot
DEFAULT_AFTER_MS = 100     # delay inserted after every send/key step
DEFAULT_QUIT_TIMEOUT_MS = 2000

KEYMAP = {
    "Enter": b"\r",
    "Return": b"\r",
    "Esc": b"\x1b",
    "Escape": b"\x1b",
    "Tab": b"\t",
    "Space": b" ",
    "Backspace": b"\x7f",
    "Delete": b"\x1b[3~",
    "Up": b"\x1b[A",
    "Down": b"\x1b[B",
    "Right": b"\x1b[C",
    "Left": b"\x1b[D",
    "Home": b"\x1b[H",
    "End": b"\x1b[F",
    "PgUp": b"\x1b[5~",
    "PgDn": b"\x1b[6~",
    "ShiftEnter": b"\x1b[13;2u",
    "AltEnter": b"\x1b\r",
    "CtrlA": b"\x01",
    "CtrlC": b"\x03",
    "CtrlD": b"\x04",
    "CtrlE": b"\x05",
    "CtrlL": b"\x0c",
    "CtrlO": b"\x0f",
    "CtrlQ": b"\x11",
    "CtrlU": b"\x15",
    "CtrlW": b"\x17",
}

# Font discovery -------------------------------------------------------------
MONO_FONT_CANDIDATES = [
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Supplemental/Menlo.ttc",
    "/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    "/System/Library/Fonts/Supplemental/Courier New.ttf",
]
CJK_FONT_CANDIDATES = [
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Supplemental/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
]
SYMBOL_FONT_CANDIDATES = [
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
]
FONT_SEARCH_DIRS = ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"]

# Terminal palette (default: a dark terminal; matches what dst assumes).
DEFAULT_FG = (216, 222, 233)
DEFAULT_BG = (14, 16, 20)
CURSOR_COLOR = (120, 200, 255)

ANSI_BASE_COLORS = {
    "black": (28, 28, 28),
    "red": (204, 85, 85),
    "green": (85, 170, 85),
    "brown": (212, 160, 23),       # ANSI 33 "yellow"/brown
    "yellow": (212, 160, 23),
    "blue": (85, 85, 204),
    "magenta": (204, 85, 204),
    "cyan": (85, 204, 204),
    "white": (204, 204, 204),
}
ANSI_BRIGHT_COLORS = {
    "brightblack": (128, 128, 128),
    "brightred": (255, 85, 85),
    "brightgreen": (85, 255, 85),
    "brightbrown": (255, 215, 95),
    "brightyellow": (255, 215, 95),
    "brightblue": (85, 85, 255),
    "brightmagenta": (255, 85, 255),
    "brightcyan": (85, 255, 255),
    "brightwhite": (255, 255, 255),
}


# --------------------------------------------------------------------------- #
# small helpers
# --------------------------------------------------------------------------- #

def log(msg: str) -> None:
    sys.stderr.write(f"[sim] {msg}\n")
    sys.stderr.flush()


def safe_name(name: str) -> str:
    s = re.sub(r"[^A-Za-z0-9_.-]+", "_", name.strip())
    return s.strip("_") or "snap"


def is_wide(ch: str) -> bool:
    """East-Asian wide/fullwidth, matching what terminals draw in two cells."""
    if not ch:
        return False
    return unicodedata.east_asian_width(ch) in ("W", "F")


def xterm_256_color(n: int):
    """Map an xterm-256 palette index to RGB."""
    if 0 <= n < 16:
        names = [
            "black", "red", "green", "brown", "blue", "magenta", "cyan", "white",
        ]
        base = [ANSI_BASE_COLORS[k] for k in names]
        bright = [
            ANSI_BRIGHT_COLORS[k]
            for k in [
                "brightblack", "brightred", "brightgreen", "brightyellow",
                "brightblue", "brightmagenta", "brightcyan", "brightwhite",
            ]
        ]
        return (base + bright)[n]
    if 16 <= n <= 231:
        n -= 16
        r, g, b = n // 36, (n % 36) // 6, n % 6
        steps = [0, 95, 135, 175, 215, 255]
        return (steps[r], steps[g], steps[b])
    if 232 <= n <= 255:
        v = 8 + (n - 232) * 10
        return (v, v, v)
    return DEFAULT_FG


def resolve_color(value, default, bold=False, brighten=True):
    """Resolve a pyte color (None/'default'/name/int) to an RGB tuple."""
    if value is None or value == "default":
        rgb = default
        return rgb
    if isinstance(value, int):
        return xterm_256_color(value)
    name = str(value).lower()
    if name.startswith("bright") and name in ANSI_BRIGHT_COLORS:
        return ANSI_BRIGHT_COLORS[name]
    if name in ANSI_BASE_COLORS:
        base = ANSI_BASE_COLORS[name]
        if bold and brighten:
            bright_name = "bright" + name
            if bright_name in ANSI_BRIGHT_COLORS:
                return ANSI_BRIGHT_COLORS[bright_name]
        return base
    return default


def find_font(candidates, dirs, patterns):
    for c in candidates:
        if os.path.exists(c):
            return c
    for d in dirs:
        if not os.path.isdir(d):
            continue
        for pat in patterns:
            hits = sorted(glob.glob(os.path.join(d, pat)))
            if hits:
                return hits[0]
    return None


# --------------------------------------------------------------------------- #
# PNG renderer
# --------------------------------------------------------------------------- #

class ScreenRenderer:
    """Renders a pyte.Screen to PNG using a cell grid.

    The device grid is 6x13 px per cell (53x12 cells => 318x156 px).  We render
    at `scale`x: each cell is `6*scale` x `13*scale` px.  Menlo is sized so its
    advance is exactly one cell wide (`font.getlength('M') == 6*scale`); CJK
    glyphs are drawn into a two-cell box with a CJK font; block glyphs
    (U+2588/2593/2591) use Menlo, which covers them.
    """

    def __init__(self, cols, rows, scale=DEFAULT_SCALE,
                 mono_path=None, cjk_path=None, symbol_path=None):
        self.cols = cols
        self.rows = rows
        self.scale = max(1, int(scale))
        self.cell_w = 6 * self.scale
        self.cell_h = 13 * self.scale
        self.mono_path = mono_path or find_font(
            MONO_FONT_CANDIDATES, FONT_SEARCH_DIRS, ["Menlo*", "Monaco*", "Courier*"])
        self.cjk_path = cjk_path or find_font(
            CJK_FONT_CANDIDATES, FONT_SEARCH_DIRS,
            ["PingFang*", "Hiragino*", "STHeiti*", "Songti*", "*Unicode*"])
        self.symbol_path = symbol_path or find_font(
            SYMBOL_FONT_CANDIDATES, FONT_SEARCH_DIRS, ["Apple Symbols*", "Symbol*"])
        self._fonts = {}
        self.mono = self._load_mono(bold=False)
        self.mono_bold = self._load_mono(bold=True)
        self.cjk = self._load_cjk()
        self.symbol = self._load_symbol()
        if self.mono is None:
            raise RuntimeError(
                "no monospace font found; pass --mono-font PATH (looked in: "
                + ", ".join(MONO_FONT_CANDIDATES) + ")")

    # -- font loading ------------------------------------------------------ #
    def _try_font(self, path, size, index=0):
        if not path or not os.path.exists(path):
            return None
        try:
            return ImageFont.truetype(path, size, index=index)
        except Exception:
            return None

    def _load_mono(self, bold: bool):
        if Image is None or not self.mono_path:
            return None
        # Find the right face inside a .ttc: try regular then scan for "Bold".
        indices = range(0, 4)
        regular = bold_candidate = None
        for idx in indices:
            f = self._try_font(self.mono_path, 12, idx)
            if f is None:
                continue
            try:
                name = " ".join(f.getname())
            except Exception:
                name = ""
            if regular is None:
                regular = f
            if "bold" in name.lower():
                bold_candidate = f
                break
        base = bold_candidate if (bold and bold_candidate is not None) else regular
        if base is None:
            return None
        # Size the font so one advance == one cell width.
        size = max(6, int(round(self.cell_w / 0.60)))
        font = None
        for _ in range(8):
            font = self._try_font(self.mono_path,
                                  size,
                                  getattr(base, "index", 0) if hasattr(base, "index") else 0)
            if font is None:
                break
            try:
                adv = font.getlength("M")
            except Exception:
                adv = self.cell_w
            if abs(adv - self.cell_w) <= 0.75:
                break
            if adv <= 0:
                break
            size = max(6, int(round(size * self.cell_w / adv)))
        self._mono_size = size
        return font or base

    def _load_cjk(self):
        if Image is None or not self.cjk_path:
            return None
        size = max(8, int(round(self.cell_w * 2 * 0.95)))
        for idx in range(0, 4):
            f = self._try_font(self.cjk_path, size, idx)
            if f is not None:
                self._cjk_size = size
                return f
        return None

    def _load_symbol(self):
        if Image is None or not self.symbol_path:
            return None
        size = max(8, int(round(self.cell_h * 0.80)))
        f = self._try_font(self.symbol_path, size, 0)
        self._symbol_size = size
        return f

    # -- glyph choice ------------------------------------------------------ #
    def _glyph_font(self, ch: str):
        """Pick (font, cell_span) for one character."""
        if is_wide(ch):
            if self.cjk is not None:
                return self.cjk, 2
            return self.mono, 2
        cp = ord(ch)
        # Block elements and box drawing: Menlo covers them.
        if 0x2580 <= cp <= 0x259F or 0x2500 <= cp <= 0x257F:
            return self.mono, 1
        # Latin/Greek/Cyrillic/punctuation: Menlo.
        if cp < 0x2000 or 0x2000 <= cp <= 0x206F:
            return self.mono, 1
        # Symbols/arrows/geometric shapes: prefer the symbol font, fall back.
        if self.symbol is not None:
            return self.symbol, 1
        return self.mono, 1

    # -- text -------------------------------------------------------------- #
    def text(self, screen) -> str:
        lines = []
        for r in range(self.rows):
            row = screen.buffer[r]
            chars = []
            for c in range(self.cols):
                data = row[c].data
                chars.append(data if data else "")
            lines.append("".join(chars).rstrip())
        return "\n".join(lines)

    # -- png --------------------------------------------------------------- #
    def png(self, screen, path: str) -> None:
        if Image is None:
            raise RuntimeError("Pillow is not installed; cannot render PNG")
        img = Image.new("RGB", (self.cols * self.cell_w, self.rows * self.cell_h), DEFAULT_BG)
        draw = ImageDraw.Draw(img)
        cw, ch = self.cell_w, self.cell_h

        def colours(cell):
            fg = resolve_color(cell.fg, DEFAULT_FG, bold=cell.bold)
            bg = resolve_color(cell.bg, DEFAULT_BG)
            if cell.reverse:
                # Swap foreground/background. With default colours that
                # means a light fill with dark text, not dark-on-dark.
                if bg == DEFAULT_BG:
                    fg, bg = DEFAULT_BG, DEFAULT_FG
                elif fg == DEFAULT_BG:
                    fg, bg = DEFAULT_FG, bg
                else:
                    fg, bg = bg, fg
            return fg, bg

        # Two passes: paint every cell background first, then draw glyphs.
        # Wide glyphs overflow their 2-cell box slightly; drawing per-cell in a
        # single pass let the next cell's background clip the previous glyph
        # (visible as sliced CJK inside reverse-video rows).
        for r in range(self.rows):
            row = screen.buffer[r]
            for c in range(self.cols):
                cell = row[c]
                if cell.data == "":
                    prev = row[c - 1] if c > 0 else None
                    if prev is not None and prev.data and is_wide(prev.data[0]):
                        _fg, bg = colours(prev)
                    else:
                        _fg, bg = colours(cell)
                else:
                    _fg, bg = colours(cell)
                if bg != DEFAULT_BG:
                    x0, y0 = c * cw, r * ch
                    draw.rectangle([x0, y0, x0 + cw - 1, y0 + ch - 1], fill=bg)

        for r in range(self.rows):
            row = screen.buffer[r]
            for c in range(self.cols):
                cell = row[c]
                data = cell.data
                if data == "":
                    continue  # continuation cell of a wide character
                fg, _bg = colours(cell)
                x0, y0 = c * cw, r * ch
                if not data.strip():
                    continue
                font, span = self._glyph_font(data[0] if len(data) == 1 else data)
                if font is None:
                    font = self.mono
                bold = bool(cell.bold)
                if bold and font is self.mono and self.mono_bold is not None:
                    font = self.mono_bold
                box_w = span * cw
                try:
                    ascent, descent = font.getmetrics()
                    adv = font.getlength(data)
                except Exception:
                    ascent, descent, adv = ch - 4, 4, cw
                base_y = y0 + max(0, (ch - (ascent + descent)) // 2 + ascent)
                if span == 2 and adv > 0:
                    base_x = x0 + max(0, (box_w - adv) / 2.0)
                else:
                    base_x = x0
                kwargs = dict(font=font, fill=fg, anchor="ls")
                if bold and font is self.mono and self.mono_bold is None:
                    kwargs["stroke_width"] = max(1, self.scale // 3)
                    kwargs["stroke_fill"] = fg
                try:
                    draw.text((base_x, base_y), data, **kwargs)
                except Exception:
                    # Unknown anchor/font quirk: fall back to a plain draw.
                    draw.text((x0, y0), data, font=font, fill=fg)

        # Cursor: pyte tracks it; draw a thin outline so the input caret shows.
        try:
            cur = screen.cursor
            if not cur.hidden and 0 <= cur.x < self.cols and 0 <= cur.y < self.rows:
                x0, y0 = cur.x * cw, cur.y * ch
                draw.rectangle(
                    [x0 + 1, y0 + 1, x0 + cw - 2, y0 + ch - 2],
                    outline=CURSOR_COLOR, width=max(1, self.scale // 2))
        except Exception:
            pass

        img.save(path, "PNG")

    # -- diagnostics ------------------------------------------------------- #
    def describe(self) -> dict:
        return {
            "cell_px": [self.cell_w, self.cell_h],
            "scale": self.scale,
            "mono_font": self.mono_path,
            "cjk_font": self.cjk_path,
            "symbol_font": self.symbol_path,
        }


# --------------------------------------------------------------------------- #
# pty session
# --------------------------------------------------------------------------- #

class PtySim:
    def __init__(self, binary, cols=DEFAULT_COLS, rows=DEFAULT_ROWS,
                 home=None, cwd=None, env=None, vtime=3, startup_ms=DEFAULT_STARTUP_MS,
                 after_ms=DEFAULT_AFTER_MS, raw_log=None, out_dir=DEFAULT_OUT,
                 renderer=None, text_only=False, startup_timeout_ms=20000,
                 pid_file=None):
        self.binary = binary
        self.cols = int(cols)
        self.rows = int(rows)
        self.home = home
        self.cwd = cwd or os.getcwd()
        self.extra_env = dict(env or {})
        self.vtime = int(vtime)
        self.startup_ms = int(startup_ms)
        self.after_ms = int(after_ms)
        self.startup_timeout_ms = int(startup_timeout_ms)
        self.raw_log = raw_log
        self.out_dir = out_dir
        self.text_only = text_only
        self.renderer = renderer

        self.master = None
        self.slave = None
        self.pid = None
        self.returncode = None
        self.exited = False
        self.total_bytes = 0
        self.last_output = time.time()
        self.screen = pyte.Screen(self.cols, self.rows)
        self.stream = pyte.Stream(self.screen)
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self._raw_fh = None
        self._start_time = 0.0
        self.pid_file = pid_file
        self._snapshots = []
        self._snap_idx = 0
        self._input_log = []

    # -- lifecycle --------------------------------------------------------- #
    def start(self):
        if not os.path.exists(self.binary):
            raise SystemExit(
                f"screen_sim: dst binary not found: {self.binary}\n"
                f"  build it with: cd {REPO} && cargo build")
        os.makedirs(self.out_dir, exist_ok=True)
        if self.raw_log:
            os.makedirs(os.path.dirname(os.path.abspath(self.raw_log)), exist_ok=True)
            self._raw_fh = open(self.raw_log, "wb")

        master, slave = pty.openpty()
        self.master, self.slave = master, slave
        # Window size first: dst probes `stty size` at startup and clamps to 53x13.
        fcntl.ioctl(slave, termios.TIOCSWINSZ,
                    struct.pack("HHHH", self.rows, self.cols, 0, 0))
        # Put the slave in raw mode ourselves (dst's `stty -F /dev/tty` is
        # GNU-syntax and silently fails on macOS).  VMIN=0/VTIME=vtime makes
        # reads behave like dst's Linux poll mode (0.1s ticks by default).
        tty.setraw(slave)
        attrs = termios.tcgetattr(slave)
        attrs[6][termios.VMIN] = 0
        attrs[6][termios.VTIME] = self.vtime
        termios.tcsetattr(slave, termios.TCSANOW, attrs)

        env = dict(os.environ)
        env.update({
            "TERM": "xterm-256color",
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "COLORTERM": "truecolor",
        })
        env.pop("TMUX", None)  # dst hides its header row when TMUX is set
        if self.home:
            env["HOME"] = os.path.abspath(self.home)
        env.update(self.extra_env)
        cwd = os.path.abspath(self.cwd)

        pid = os.fork()
        if pid == 0:  # child
            try:
                os.close(master)
                os.setsid()
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
                os.dup2(slave, 0)
                os.dup2(slave, 1)
                os.dup2(slave, 2)
                if slave > 2:
                    os.close(slave)
                try:
                    os.chdir(cwd)
                except OSError:
                    os._exit(126)
                os.execve(self.binary, [self.binary] + list(self.child_args), env)
            except BaseException:
                os._exit(127)
        # parent
        self.pid = pid
        if self.pid_file:
            try:
                with open(self.pid_file, "w", encoding="utf-8") as fh:
                    fh.write(str(pid))
            except OSError:
                pass
        os.close(slave)
        self.slave = None
        os.set_blocking(master, False)
        self._start_time = time.time()

        # Wait for the first paint, then let the splash settle a moment.
        if not self._wait_for_first_paint(self.startup_timeout_ms):
            self._reap_nonblocking()
            raise SystemExit(
                "screen_sim: dst produced no output in "
                f"{self.startup_timeout_ms} ms (exit={self.returncode}); "
                "is the binary runnable?")
        self.pump(self.startup_ms)
        if self.exited:
            raise SystemExit(
                f"screen_sim: dst exited during startup (code={self.returncode})")

    # Extra argv for the child; subclasses/tests may override.
    child_args = ()

    def _wait_for_first_paint(self, timeout_ms):
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            self.pump(50)
            if self.total_bytes > 0:
                return True
            if self.exited:
                return False
        return False

    # -- io ---------------------------------------------------------------- #
    def pump(self, ms):
        """Drain pty output for up to `ms` milliseconds."""
        deadline = time.time() + ms / 1000.0
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                return
            if self.exited:
                time.sleep(min(remaining, 0.01))
                return
            try:
                r, _, _ = select.select([self.master], [], [], min(remaining, 0.05))
            except (OSError, ValueError):
                self._mark_exited()
                return
            if not r:
                self._reap_nonblocking()
                continue
            try:
                data = os.read(self.master, 65536)
            except OSError:
                self._mark_exited()
                return
            if not data:
                self._mark_exited()
                return
            self._feed(data)

    def _feed(self, data: bytes):
        self.total_bytes += len(data)
        self.last_output = time.time()
        if self._raw_fh is not None:
            try:
                self._raw_fh.write(data)
                self._raw_fh.flush()
            except OSError:
                pass
        try:
            self.stream.feed(self._decoder.decode(data))
        except Exception as exc:  # pragma: no cover - pyte bug guard
            log(f"pyte feed error (ignored): {exc!r}")

    def pump_until_quiet(self, quiet_ms, timeout_ms):
        """Drain until no output has arrived for quiet_ms.

        NOTE: dst repaints its idle screen every ~300 ms (its edit loop redraws
        after each VTIME tick), so on a live dst this normally never goes quiet.
        Prefer `pump_until_stable` to detect "the turn finished"; this one is
        useful for a process that has genuinely stopped drawing (e.g. during a
        blocking tool call, or after exit)."""
        quiet = quiet_ms / 1000.0
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            self.pump(40)
            if self.exited:
                return True
            if time.time() - self.last_output >= quiet:
                return True
        return False

    def screen_text(self) -> str:
        """Current framebuffer as plain text (the same view snapshots save)."""
        lines = []
        for r in range(self.rows):
            row = self.screen.buffer[r]
            lines.append("".join(row[c].data for c in range(self.cols)).rstrip())
        return "\n".join(lines)

    def pump_until_stable(self, stable_ms, timeout_ms):
        """Drain until the *rendered screen* has not changed for stable_ms.

        This is the reliable "turn is over" detector for dst: while a reply
        streams (or the splash animates) the screen text changes; once the turn
        settles, the periodic idle repaint produces identical frames."""
        stable = stable_ms / 1000.0
        deadline = time.time() + timeout_ms / 1000.0
        last = None
        last_change = time.time()
        while time.time() < deadline:
            self.pump(50)
            cur = self.screen_text()
            if cur != last:
                last = cur
                last_change = time.time()
            if self.exited:
                return True
            if time.time() - last_change >= stable:
                return True
        return False

    def pump_until_text(self, needle, timeout_ms):
        """Drain until `needle` appears in the rendered screen text."""
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            self.pump(50)
            if needle in self.screen_text():
                return True
            if self.exited:
                break
        return False

    def write(self, data: bytes):
        if isinstance(data, str):
            data = data.encode("utf-8")
        self._input_log.append(
            {"t": round(time.time() - self._start_time, 3), "bytes": data.hex(),
             "text": data.decode("utf-8", "replace")})
        if self.exited or self.master is None:
            raise SystemExit("screen_sim: cannot send input, dst has exited")
        view = memoryview(data)
        while view:
            try:
                n = os.write(self.master, view)
            except OSError as exc:
                raise SystemExit(f"screen_sim: write to pty failed: {exc}")
            view = view[n:]

    def send_text(self, text: str):
        self.write(text.encode("utf-8"))

    def send_key(self, name: str):
        key = name.strip()
        if key not in KEYMAP:
            raise SystemExit(
                f"screen_sim: unknown key {name!r}; known: {', '.join(sorted(KEYMAP))}")
        self.write(KEYMAP[key])

    # -- snapshots --------------------------------------------------------- #
    def capture(self, name="snap", final=False) -> str:
        """Write a text and (unless text-only) PNG snapshot.  Returns text."""
        self.pump(max(40, self.after_ms // 2))  # let pending bytes land
        text = self.screen_text()
        self._snap_idx += 1
        os.makedirs(self.out_dir, exist_ok=True)
        prefix = f"{self._snap_idx:02d}_"
        base = os.path.join(self.out_dir, prefix + safe_name(name))
        with open(base + ".txt", "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
        png_path = None
        if not self.text_only:
            png_path = base + ".png"
            self.renderer.png(self.screen, png_path)
        entry = {
            "index": self._snap_idx,
            "name": name,
            "text": base + ".txt",
            "png": png_path,
            "elapsed_ms": int((time.time() - self._start_time) * 1000),
            "bytes_seen": self.total_bytes,
        }
        self._snapshots.append(entry)
        if final:
            with open(os.path.join(self.out_dir, "final.txt"), "w", encoding="utf-8") as fh:
                fh.write(text + "\n")
            if not self.text_only:
                self.renderer.png(self.screen, os.path.join(self.out_dir, "final.png"))
            entry["final"] = True
        log(f"snapshot {self._snap_idx:02d} {name!r} -> {base}.txt"
            + ("" if self.text_only else f" / {base}.png"))
        return text

    # -- shutdown ---------------------------------------------------------- #
    def shutdown(self, timeout_ms=DEFAULT_QUIT_TIMEOUT_MS):
        if self.master is None:
            return
        if not self.exited:
            try:
                # Ctrl-C is a normal key in dst's raw mode and exits cleanly.
                self.write(b"\x03")
            except SystemExit:
                pass
            deadline = time.time() + timeout_ms / 1000.0
            while time.time() < deadline and not self.exited:
                self.pump(50)
        if not self.exited:
            self._reap_nonblocking()
        if not self.exited and self.pid:
            log("dst did not exit after Ctrl-C; sending SIGTERM")
            try:
                os.kill(self.pid, signal.SIGTERM)
            except OSError:
                pass
            deadline = time.time() + 1.0
            while time.time() < deadline and not self.exited:
                self.pump(50)
        if not self.exited and self.pid:
            try:
                os.kill(self.pid, signal.SIGKILL)
            except OSError:
                pass
        self._reap_blocking()
        try:
            os.close(self.master)
        except OSError:
            pass
        self.master = None
        if self._raw_fh is not None:
            self._raw_fh.close()
            self._raw_fh = None

    def _record_status(self, status):
        self.exited = True
        if os.WIFEXITED(status):
            self.returncode = os.WEXITSTATUS(status)
        elif os.WIFSIGNALED(status):
            self.returncode = -os.WTERMSIG(status)

    def _reap_blocking(self, timeout_ms=1000):
        """Reap the child for real so manifest exit codes are accurate."""
        if not self.pid:
            return
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            try:
                pid, status = os.waitpid(self.pid, os.WNOHANG)
            except ChildProcessError:
                self.exited = True
                return
            except OSError:
                return
            if pid == self.pid:
                self._record_status(status)
                return
            time.sleep(0.02)
        try:
            os.kill(self.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            _, status = os.waitpid(self.pid, 0)
            self._record_status(status)
        except (ChildProcessError, OSError):
            self.exited = True

    def _reap_nonblocking(self, force=False):
        if self.exited and not force:
            return
        try:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError:
            self.exited = True
            return
        except OSError:
            return
        if pid == self.pid:
            self._record_status(status)

    def _mark_exited(self):
        self.exited = True
        self._reap_nonblocking(force=True)

    # -- manifest ---------------------------------------------------------- #
    def write_manifest(self, path=None):
        path = path or os.path.join(self.out_dir, "manifest.json")
        doc = {
            "binary": self.binary,
            "cols": self.cols,
            "rows": self.rows,
            "home": self.home,
            "cwd": self.cwd,
            "env_overrides": self.extra_env,
            "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "duration_ms": int((time.time() - self._start_time) * 1000) if self._start_time else 0,
            "bytes_seen": self.total_bytes,
            "exit_code": self.returncode,
            "renderer": self.renderer.describe() if self.renderer else None,
            "snapshots": self._snapshots,
            "input": self._input_log,
            "final_text": os.path.join(self.out_dir, "final.txt"),
            "final_png": None if self.text_only else os.path.join(self.out_dir, "final.png"),
        }
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(doc, fh, indent=2, ensure_ascii=False)
            fh.write("\n")


# --------------------------------------------------------------------------- #
# scenario ops
# --------------------------------------------------------------------------- #

def normalize_steps(steps):
    """Validate/normalize JSON scenario steps into an op list."""
    ops = []
    if not isinstance(steps, list):
        raise SystemExit("scenario: 'steps' must be a list")
    for i, step in enumerate(steps):
        if not isinstance(step, dict):
            raise SystemExit(f"scenario step {i}: must be an object")
        produced = False
        if "send" in step:
            ops.append({"op": "send", "text": str(step["send"])})
            produced = True
        if "send_line" in step:
            ops.append({"op": "send_line", "text": str(step["send_line"])})
            produced = True
        if "key" in step:
            keys = step["key"]
            if isinstance(keys, list):
                for k in keys:
                    ops.append({"op": "key", "key": str(k)})
            else:
                ops.append({"op": "key", "key": str(keys)})
            produced = True
        if "wait_ms" in step:
            ops.append({"op": "wait", "ms": int(step["wait_ms"])})
            produced = True
        if "delay_ms" in step:
            ops.append({"op": "wait", "ms": int(step["delay_ms"])})
            produced = True
        if "wait_quiet" in step:
            wq = step["wait_quiet"]
            if isinstance(wq, dict):
                ops.append({
                    "op": "wait_quiet",
                    "quiet_ms": int(wq.get("quiet_ms", 700)),
                    "timeout_ms": int(wq.get("timeout_ms", 15000)),
                })
            else:
                ops.append({"op": "wait_quiet", "quiet_ms": int(wq), "timeout_ms": 15000})
            produced = True
        if "wait_quiet_ms" in step:
            ops.append({
                "op": "wait_quiet",
                "quiet_ms": int(step["wait_quiet_ms"]),
                "timeout_ms": int(step.get("timeout_ms", 15000)),
            })
            produced = True
        if "wait_stable" in step:
            ws = step["wait_stable"]
            if isinstance(ws, dict):
                ops.append({
                    "op": "wait_stable",
                    "stable_ms": int(ws.get("stable_ms", 800)),
                    "timeout_ms": int(ws.get("timeout_ms", 15000)),
                })
            else:
                ops.append({"op": "wait_stable", "stable_ms": int(ws),
                            "timeout_ms": int(step.get("timeout_ms", 15000))})
            produced = True
        if "wait_stable_ms" in step:
            ops.append({
                "op": "wait_stable",
                "stable_ms": int(step["wait_stable_ms"]),
                "timeout_ms": int(step.get("timeout_ms", 15000)),
            })
            produced = True
        if "wait_for" in step:
            wf = step["wait_for"]
            if isinstance(wf, dict):
                ops.append({"op": "wait_for", "text": str(wf.get("text", "")),
                            "timeout_ms": int(wf.get("timeout_ms", 15000))})
            else:
                ops.append({"op": "wait_for", "text": str(wf),
                            "timeout_ms": int(step.get("timeout_ms", 15000))})
            produced = True
        snaps = []
        if "snap" in step:
            snaps.append(str(step["snap"]))
        if "snaps" in step:
            snaps.extend(str(s) for s in step["snaps"])
        for s in snaps:
            ops.append({"op": "snap", "name": s})
            produced = True
        if "watch" in step:
            w = step["watch"]
            ops.append({
                "op": "watch",
                "seconds": float(w.get("seconds", 3.0)),
                "interval_ms": int(w.get("interval_ms", 300)),
                "prefix": str(w.get("prefix", "watch")),
            })
            produced = True
        if not produced:
            raise SystemExit(f"scenario step {i}: empty/unknown step {step!r}")
    return ops


def load_scenario(path):
    if path == "-":
        doc = json.load(sys.stdin)
    else:
        with open(path, "r", encoding="utf-8") as fh:
            doc = json.load(fh)
    if isinstance(doc, list):
        return normalize_steps(doc)
    if not isinstance(doc, dict) or "steps" not in doc:
        raise SystemExit(f"scenario {path}: expected {{'steps': [...]}} or a list")
    return normalize_steps(doc["steps"])


def run_ops(sim: PtySim, ops):
    for op in ops:
        kind = op["op"]
        if kind == "send":
            sim.send_text(op["text"])
            sim.pump(sim.after_ms)
        elif kind == "send_line":
            sim.send_text(op["text"])
            sim.pump(max(30, sim.after_ms // 2))
            sim.write(b"\r")
            sim.pump(sim.after_ms)
        elif kind == "key":
            sim.send_key(op["key"])
            sim.pump(sim.after_ms)
        elif kind == "wait":
            sim.pump(op["ms"])
        elif kind == "wait_quiet":
            ok = sim.pump_until_quiet(op["quiet_ms"], op["timeout_ms"])
            if not ok:
                log(f"wait_quiet timed out after {op['timeout_ms']} ms; snapshotting anyway")
        elif kind == "wait_stable":
            ok = sim.pump_until_stable(op["stable_ms"], op["timeout_ms"])
            if not ok:
                log(f"wait_stable timed out after {op['timeout_ms']} ms; "
                    "the screen never settled (still animating/streaming?)")
        elif kind == "wait_for":
            ok = sim.pump_until_text(op["text"], op["timeout_ms"])
            if not ok:
                log(f"wait_for {op['text']!r} not found within {op['timeout_ms']} ms")
        elif kind == "snap":
            sim.capture(op["name"])
        elif kind == "watch":
            run_watch(sim, op["seconds"], op["interval_ms"], op["prefix"])
        else:  # pragma: no cover
            raise SystemExit(f"internal: unknown op {kind}")


def run_watch(sim: PtySim, seconds: float, interval_ms: int, prefix: str):
    log(f"watch: {seconds:.1f}s at {interval_ms} ms")
    start = time.time()
    n = 0
    while time.time() - start < seconds:
        sim.pump(interval_ms)
        sim.capture(f"{prefix}_{n:03d}")
        n += 1
    return n


# --------------------------------------------------------------------------- #
# command line
# --------------------------------------------------------------------------- #

STEP_OPTS = {
    "--send", "--send-line", "--key", "--wait-ms", "--delay-ms",
    "--snap", "--wait-quiet", "--wait-stable", "--wait-for",
}


def build_parser():
    p = argparse.ArgumentParser(
        prog="screen_sim.py",
        description="Run the dst TUI in a pty and capture its screen as text/PNG.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Scenario steps are executed in command-line order (interleaving is\n"
            "preserved).  JSON scenario files use {\"steps\": [...]} with keys\n"
            "send/send_line/key/wait_ms/wait_quiet/snap/watch.\n\n"
            "Examples:\n"
            "  %(prog)s --home /tmp/dsthome --snap splash\n"
            "  %(prog)s --home /tmp/dsthome --send hi --key Enter --wait-ms 700\n"
            "           --snap thinking --wait-quiet 800:12000 --snap reply\n"
            "  %(prog)s --scenario sample_scenario.json --out screenshots\n"
            "  %(prog)s --watch 3 --interval-ms 250 --watch-prefix anim\n"
        ),
    )
    p.add_argument("--bin", default=DEFAULT_BIN, help="dst binary (default: %(default)s)")
    p.add_argument("--cols", type=int, default=DEFAULT_COLS, help="pty width in cells (default 53)")
    p.add_argument("--rows", type=int, default=DEFAULT_ROWS, help="pty height in cells (default 12)")
    p.add_argument("--home", help="HOME for the child (point it at a dir holding .dst/config.yml)")
    p.add_argument("--cwd", help="working directory for dst (default: current directory)")
    p.add_argument("--env", action="append", default=[], metavar="K=V",
                   help="extra environment variable for the child (repeatable)")
    p.add_argument("--out", default=DEFAULT_OUT, help="snapshot output directory (default: %(default)s)")
    p.add_argument("--scenario", help="JSON scenario file ('-' for stdin)")
    p.add_argument("--watch", type=float, default=0.0, metavar="SECONDS",
                   help="watch mode: snapshot every --interval-ms for SECONDS")
    p.add_argument("--interval-ms", type=int, default=300,
                   help="watch mode snapshot interval (default 300)")
    p.add_argument("--watch-prefix", default="watch", help="watch snapshot name prefix")
    p.add_argument("--startup-ms", type=int, default=DEFAULT_STARTUP_MS,
                   help="settle time after dst's first paint (default 650)")
    p.add_argument("--after-ms", type=int, default=DEFAULT_AFTER_MS,
                   help="delay after each send/key step (default 100)")
    p.add_argument("--pid-file", default=None,
                   help="write the dst child PID here (for RSS sampling)")
    p.add_argument("--vtime", type=int, default=3,
                   help="terminal VTIME in tenths of a second for idle reads (default 3)")
    p.add_argument("--scale", type=int, default=DEFAULT_SCALE,
                   help="PNG scale: cell = 6*scale x 13*scale px (default 3)")
    p.add_argument("--mono-font", help="override monospace font path (Menlo)")
    p.add_argument("--cjk-font", help="override CJK font path (PingFang SC etc.)")
    p.add_argument("--symbol-font", help="override symbol font path (Apple Symbols)")
    p.add_argument("--text-only", action="store_true", help="skip PNG rendering")
    p.add_argument("--raw-log", help="also write the raw pty byte stream here")
    p.add_argument("--quit-timeout-ms", type=int, default=DEFAULT_QUIT_TIMEOUT_MS,
                   help="how long to wait for a clean Ctrl-C exit (default 2000)")
    # ordered step options (handled by the scanner below, listed for --help)
    p.add_argument("--key", action="append", metavar="NAME",
                   help="send a named key: " + "|".join(sorted(KEYMAP)))
    p.add_argument("--wait-quiet", action="append", metavar="QUIET_MS[:TIMEOUT_MS]",
                   help="wait until output is quiet (default timeout 15000)")
    p.add_argument("--snap", action="append", metavar="NAME",
                   help="capture a snapshot at this point in the sequence")
    p.add_argument("--wait-stable", action="append", metavar="STABLE_MS[:TIMEOUT_MS]",
                   help="wait until the rendered screen has not changed for "
                        "STABLE_MS (turn-finished detector; default timeout 15000)")
    p.add_argument("--wait-for", action="append", metavar="TEXT[:TIMEOUT_MS]",
                   help="wait until TEXT appears on screen (default timeout 15000)")
    return p


def scan_ordered(argv):
    """Split argv into (global argv, ordered ops).

    argparse cannot preserve interleaving of repeatable options, so the ordered
    step options are extracted manually first.  Everything else is passed on to
    argparse unchanged.
    """
    ops = []
    rest = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-h", "--help"):
            rest.append(a)
            i += 1
            continue
        if a in STEP_OPTS:
            if i + 1 >= len(argv):
                raise SystemExit(f"screen_sim: {a} needs a value")
            val = argv[i + 1]
            if a == "--send":
                ops.append({"op": "send", "text": val})
            elif a == "--send-line":
                ops.append({"op": "send_line", "text": val})
            elif a == "--key":
                ops.append({"op": "key", "key": val})
            elif a in ("--wait-ms", "--delay-ms"):
                ops.append({"op": "wait", "ms": int(val)})
            elif a == "--snap":
                ops.append({"op": "snap", "name": val})
            elif a == "--wait-quiet":
                if ":" in val:
                    q, t = val.split(":", 1)
                    ops.append({"op": "wait_quiet", "quiet_ms": int(q), "timeout_ms": int(t)})
                else:
                    ops.append({"op": "wait_quiet", "quiet_ms": int(val), "timeout_ms": 15000})
            elif a == "--wait-stable":
                if ":" in val:
                    q, t = val.split(":", 1)
                    ops.append({"op": "wait_stable", "stable_ms": int(q),
                                "timeout_ms": int(t)})
                else:
                    ops.append({"op": "wait_stable", "stable_ms": int(val),
                                "timeout_ms": 15000})
            elif a == "--wait-for":
                if ":" in val and not val.startswith(":"):
                    text, t = val.rsplit(":", 1)
                    if t.isdigit():
                        ops.append({"op": "wait_for", "text": text,
                                    "timeout_ms": int(t)})
                    else:
                        ops.append({"op": "wait_for", "text": val,
                                    "timeout_ms": 15000})
                else:
                    ops.append({"op": "wait_for", "text": val, "timeout_ms": 15000})
            i += 2
            continue
        # Also catch --opt=value forms of global options.
        rest.append(a)
        i += 1
    return rest, ops


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = build_parser()
    rest, ops = scan_ordered(argv)
    args = parser.parse_args(rest)

    extra_env = {}
    for pair in args.env:
        if "=" not in pair:
            raise SystemExit(f"screen_sim: --env expects K=V, got {pair!r}")
        k, v = pair.split("=", 1)
        extra_env[k] = v

    if args.scenario:
        ops = load_scenario(args.scenario) + ops

    renderer = None
    if not args.text_only:
        if Image is None:
            raise SystemExit("screen_sim: Pillow not installed; rerun with --text-only")
        renderer = ScreenRenderer(
            args.cols, args.rows, scale=args.scale,
            mono_path=args.mono_font, cjk_path=args.cjk_font,
            symbol_path=args.symbol_font)
        if renderer.cjk is None:
            log("warning: no CJK font found; CJK glyphs will not render")
        if not renderer.mono_path.endswith("Menlo.ttc") and "Menlo" not in (
                renderer.mono_path or ""):
            log(f"warning: Menlo not found, using {renderer.mono_path}")

    sim = PtySim(
        binary=os.path.abspath(os.path.expanduser(args.bin)),
        cols=args.cols, rows=args.rows, pid_file=args.pid_file,
        home=args.home, cwd=args.cwd, env=extra_env,
        vtime=args.vtime, startup_ms=args.startup_ms, after_ms=args.after_ms,
        raw_log=args.raw_log, out_dir=args.out, renderer=renderer,
        text_only=args.text_only)
    sim.start()
    log(f"spawned dst pid={sim.pid} pty={args.cols}x{args.rows} "
        f"HOME={sim.home or '(inherited)'} cwd={sim.cwd}")

    final_text = ""
    try:
        if args.watch > 0:
            run_watch(sim, args.watch, args.interval_ms, args.watch_prefix)
        if ops:
            run_ops(sim, ops)
        final_text = sim.capture("final", final=True)
    except SystemExit:
        raise
    except BaseException as exc:
        log(f"error during scenario: {exc!r}")
        try:
            final_text = sim.capture("final", final=True)
        except Exception:
            pass
        raise
    finally:
        sim.shutdown(args.quit_timeout_ms)
        sim.write_manifest()

    log(f"dst exit code={sim.returncode}, {sim.total_bytes} bytes, "
        f"{len(sim._snapshots)} snapshots in {args.out}")
    # The final text screen goes to stdout (pipes/CI friendly).
    sys.stdout.write(final_text + "\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        sys.stderr.write("\nscreen_sim: interrupted\n")
        raise SystemExit(130)
