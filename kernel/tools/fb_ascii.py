#!/usr/bin/env python3
"""Print a region of the REX3 framebuffer dump as 1:1 ASCII art.

Reads the `rgb.bin` written by `rex fbdump` (2048x1024 x u32 big-endian,
24-bit BGR) and prints one character per pixel so the boot menu can be read
without a display.
"""

import struct
import sys

WIDTH = 2048


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/opencode/rex3dump/rgb.bin"
    x0 = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    y0 = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    x1 = int(sys.argv[4]) if len(sys.argv) > 4 else 200
    y1 = int(sys.argv[5]) if len(sys.argv) > 5 else 40
    # Optional background luminance to treat as "ink" (default: the darkest).
    thresh = int(sys.argv[6]) if len(sys.argv) > 6 else None

    data = open(path, "rb").read()

    if thresh is None:
        # Find the modal luminance so the menu's background is "paper".
        hist = [0] * 64
        for y in range(y0, y1, 2):
            for x in range(x0, x1, 2):
                off = (y * WIDTH + x) * 4
                w = struct.unpack_from(">I", data, off)[0]
                r, g, b = (w >> 16) & 0xFF, (w >> 8) & 0xFF, w & 0xFF
                lum = (r * 77 + g * 151 + b * 28) >> 8
                hist[min(63, lum >> 2)] += 1
        thresh = max(range(64), key=lambda i: hist[i]) * 4 + 8

    for y in range(y0, y1):
        row = []
        for x in range(x0, x1):
            off = (y * WIDTH + x) * 4
            w = struct.unpack_from(">I", data, off)[0]
            r, g, b = (w >> 16) & 0xFF, (w >> 8) & 0xFF, w & 0xFF
            lum = (r * 77 + g * 151 + b * 28) >> 8
            row.append("#" if lum > thresh else ".")
        print("".join(row))


if __name__ == "__main__":
    main()
