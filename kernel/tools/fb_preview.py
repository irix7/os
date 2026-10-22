#!/usr/bin/env python3
"""Print a region of the REX3 framebuffer dump as downscaled ASCII art.

Reads the `rgb.bin` written by `rex fbdump` (2048x1024 x u32 big-endian,
24-bit BGR) and prints one character per NxN block, so the boot menu can be
read without a display.

Usage: fb_preview.py <rgb.bin> [x0 y0 x1 y1] [block]
"""

import struct
import sys

WIDTH = 2048
RAMP = " .:-=+*#%@"


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/opencode/rex3dump/rgb.bin"
    x0 = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    y0 = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    x1 = int(sys.argv[4]) if len(sys.argv) > 4 else 1280
    y1 = int(sys.argv[5]) if len(sys.argv) > 5 else 1024
    block = int(sys.argv[6]) if len(sys.argv) > 6 else 2

    data = open(path, "rb").read()
    if len(data) < WIDTH * 1024 * 4:
        sys.exit(f"framebuffer dump too short: {len(data)} bytes")

    for by in range(y0, y1, block):
        row = []
        for bx in range(x0, x1, block):
            best = 0
            for dy in range(block):
                y = by + dy
                if y >= 1024:
                    break
                for dx in range(block):
                    x = bx + dx
                    if x >= WIDTH:
                        break
                    off = (y * WIDTH + x) * 4
                    w = struct.unpack_from(">I", data, off)[0]
                    r, g, b = (w >> 16) & 0xFF, (w >> 8) & 0xFF, w & 0xFF
                    lum = (r * 77 + g * 151 + b * 28) >> 8
                    if lum > best:
                        best = lum
            row.append(RAMP[min(len(RAMP) - 1, best * len(RAMP) // 256)])
        print("".join(row))


if __name__ == "__main__":
    main()
