#!/usr/bin/env python3
"""Reference LZ4 block decoder, for checking the ROM's build-time compressor.

Decodes the payload format the ROM uses:

    [4 bytes] magic 0x184D2204
    [4 bytes] uncompressed size, big-endian
    [n bytes] LZ4 block (RFC 7857)

Usage: lz4_check.py <payload.bin> <expected.bin>
"""

import struct
import sys

MAGIC = 0x184D2204


def decode_block(data):
    """Decode an LZ4 block; returns the decompressed bytes."""
    out = bytearray()
    i = 0
    n = len(data)
    while i < n:
        token = data[i]
        i += 1

        # Literal run length.
        lit_len = token >> 4
        if lit_len == 15:
            while True:
                b = data[i]
                i += 1
                lit_len += b
                if b != 255:
                    break
        out += data[i:i + lit_len]
        i += lit_len

        if i >= n:
            # Last sequence: literals only.
            break

        # Match distance, little-endian.
        offset = data[i] | (data[i + 1] << 8)
        i += 2
        if offset == 0:
            raise ValueError("zero match distance")

        # Match length.
        match_len = token & 0x0F
        if match_len == 15:
            while True:
                b = data[i]
                i += 1
                match_len += b
                if b != 255:
                    break
        match_len += 4

        start = len(out) - offset
        if start < 0:
            raise ValueError("match distance before the start of the output")
        for k in range(match_len):
            out.append(out[start + k])
    return bytes(out)


def main():
    payload = open(sys.argv[1], "rb").read()
    expected = open(sys.argv[2], "rb").read()

    magic, size = struct.unpack_from(">II", payload, 0)
    print(f"magic 0x{magic:08X} (want 0x{MAGIC:08X}), size {size}, payload {len(payload)}")
    if magic != MAGIC:
        sys.exit("bad magic")

    try:
        got = decode_block(payload[8:])
    except Exception as e:
        sys.exit(f"decode failed: {e}")

    print(f"decoded {len(got)} bytes, expected {len(expected)}")
    if got == expected:
        print("MATCH")
    else:
        for i, (a, b) in enumerate(zip(got, expected)):
            if a != b:
                print(f"first difference at {i}: got 0x{a:02X} want 0x{b:02X}")
                break
        sys.exit("MISMATCH")


if __name__ == "__main__":
    main()
