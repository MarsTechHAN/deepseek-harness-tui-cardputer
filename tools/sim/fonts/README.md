# Simulator fonts

The device panel is 320x170 px and the terminal grid is 53x12 cells: one cell
is 6x14 px (a 6x13 glyph plus a 1 px line gap), and the text area (318x168) is
centered in the panel (1 px margin). The simulator renders that framebuffer
1:1 and then integer-upscales it with nearest-neighbour sampling for the output
PNG (`--scale N` -> exactly 320*N x 170*N px); `--native` saves the 1:1
320x170 image, so a PNG always shows the same device pixels the LCD gets.

`6x13.bdf.gz` / `6x13B.bdf.gz`
- The classic X11 "Fixed" 6x13 bitmap font (regular and bold), from the X.Org
  `font/misc-misc` package: https://gitlab.freedesktop.org/xorg/font/misc-misc
- Public domain: `COPYRIGHT "Public domain font. Share and enjoy."`
- It is bit-identical in geometry to a 6x13 framebuffer terminal cell, which is
  what the panel's 53 columns imply. Glyphs are drawn as exact 6x13 pixels (no
  antialiasing). The bold face covers 1282 glyphs (notably no `█▓░` block
  elements); bold cells fall back to the regular face when it lacks a glyph.
  CJK has no glyphs here; the renderer rasterizes those from the host CJK font
  at 12x13 px with subpixel (RGB-stripe) antialiasing, which is what a
  TTF-backed fbterm would show.
- Regenerate with: `gzip -9 6x13.bdf` / `gzip -9 6x13B.bdf` from the files at
  the URL above.

This is a *simulation* of the device font: the exact font on the board could
not be read back (no device access), so the geometry (6x14 pitch, 318x168
centered area) is exact and the glyph shapes are the canonical 6x13 terminal
font. `--font-mode ttf` renders the same grid with a host vector font and the
same subpixel compositor for comparison; `--subpixel off` switches the
compositor to grayscale antialiasing. The compositor itself is an
approximation (a `[1,2,1]/4` FIR LCD filter, no panel gamma), so vector glyphs
show stronger coloured fringes than a real panel would.
