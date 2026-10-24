#!/usr/bin/env python3
"""Convert the SGI PROM's IRIS 10 raster font into a Rust table.

Source: stand/arcs/include/fonts/iris10i.h in the IRIX 6.5 source tree.
Format (see stand/arcs/include/stand_htport.h):

    struct fontinfo {
        short offset;    /* word index into the data array */
        char  xsize;     /* glyph width in pixels */
        char  ysize;     /* glyph height in pixels */
        signed char xorig;
        signed char yorig;
        short  xmove;    /* pen advance */
    };

Each 16-bit data word is one glyph row; the pixel for column x is bit
(15 - x).  The source stores rows bottom-up, so `gfx::draw_char` reverses
them while rasterising.  The font height is 16 with a 3 pixel descender, so
the baseline sits at row 13 and a glyph's first row is at
(13 - ysize - yorig).

The data block carries the character number in `/* char 0xNN */` comments.
Those are comments, not glyph words; strip them before collecting the data.
"""

import re
import sys

FONT_H = 16
FONT_DS = 3
BASELINE = FONT_H - FONT_DS  # 13


def parse(path):
    text = open(path).read()

    # Data array: unsigned short iris10iData[] = { 0x...., ... };
    m = re.search(r"iris10iData\[\]\s*=\s*\{(.*?)\};", text, re.S)
    if not m:
        sys.exit("iris10iData[] not found")
    # The data block is annotated with `/* char 0xNN */` comments; those hex
    # values are not glyph rows.  Strip comments before collecting words.
    body = re.sub(r"/\*.*?\*/", "", m.group(1), flags=re.S)
    data = [int(w, 16) for w in re.findall(r"0x([0-9a-fA-F]+)", body)]

    # Info table: struct fontinfo iris10iInfo[] = { {off, xs, ys, xo, yo, xm}, ... };
    m = re.search(r"iris10iInfo\[\]\s*=\s*\{(.*?)\};", text, re.S)
    if not m:
        sys.exit("iris10iInfo[] not found")
    body = m.group(1)
    # The standalone build has no leading NUL entry.
    body = re.sub(r"#ifndef\s+_STANDALONE.*?#endif", "", body, flags=re.S)
    entries = re.findall(
        r"\{\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*\}",
        body,
    )
    info = [tuple(int(x) for x in e) for e in entries]
    return data, info


def main():
    src, dst = sys.argv[1], sys.argv[2]
    data, info = parse(src)

    # Drop the NUL entry (standalone builds start at char 0x20).
    if info and info[0] == (0, 0, 0, 0, 0, 0):
        info = info[1:]

    out = []
    out.append("// SPDX-License-Identifier: GPL-3.0-only")
    out.append("//! The SGI PROM's IRIS 10 raster font, converted from")
    out.append("//! `stand/arcs/include/fonts/iris10i.h` in the IRIX 6.5 source tree.")
    out.append("//!")
    out.append("//! Each `FONT_DATA` entry is one glyph row: the pixel for column `x` is")
    out.append(f"//! bit `15 - x`.  `FONT_INFO` holds the per-character metrics")
    out.append("//! `(word offset, width, height, x origin, y origin, pen advance)`.")
    out.append("")
    out.append("/// Font height in pixels.")
    out.append(f"pub const FONT_HEIGHT: u8 = {FONT_H};")
    out.append("/// Descender depth in pixels; the baseline sits at row 13.")
    out.append(f"pub const FONT_DESCENDER: u8 = {FONT_DS};")
    out.append("/// Row of the baseline within a character cell.")
    out.append(f"pub const FONT_BASELINE: u8 = {BASELINE};")
    out.append("")
    out.append("/// Glyph row bitmaps, indexed by `FONT_INFO[i].0`.")
    out.append("#[rustfmt::skip]")
    out.append(f"pub static FONT_DATA: [u16; {len(data)}] = [")
    for i in range(0, len(data), 8):
        chunk = data[i:i + 8]
        out.append("    " + ", ".join(f"0x{w:04X}" for w in chunk) + ",")
    out.append("];")
    out.append("")
    out.append("/// Per-character metrics: (offset, xsize, ysize, xorig, yorig, xmove).")
    out.append("#[rustfmt::skip]")
    out.append(f"pub static FONT_INFO: [(u16, u8, u8, i8, i8, u8); {len(info)}] = [")
    for (off, xs, ys, xo, yo, xm) in info:
        out.append(f"    ({off}, {xs}, {ys}, {xo}, {yo}, {xm}),")
    out.append("];")
    out.append("")

    open(dst, "w").write("\n".join(out))
    print(f"wrote {dst}: {len(data)} words, {len(info)} glyphs (0x20..0x{0x20 + len(info) - 1:02x})")


if __name__ == "__main__":
    main()
