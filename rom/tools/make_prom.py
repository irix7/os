#!/usr/bin/env python3
"""Build the PROM image from the linked ROM ELF.

The ROM ELF has the boot stub in `.text` and the kernel payload in
`.payload`, both mapped into the PROM window.  The raw PROM image is what
the CPU sees at `0xBFC00000`, so every section has to be placed at its
*virtual* address offset from the base -- concatenating the sections
would silently shift the payload by any alignment gap the linker left
between them.

Usage: make_prom.py <rom.elf> <out.bin>
"""

import struct
import sys

PT_LOAD = 1


def sections(elf):
    """Return [(name, addr, offset, size, flags)] for the ELF's section headers."""
    e_shoff = struct.unpack_from(">I", elf, 0x20)[0]
    e_shentsize = struct.unpack_from(">H", elf, 0x2E)[0]
    e_shnum = struct.unpack_from(">H", elf, 0x30)[0]
    e_shstrndx = struct.unpack_from(">H", elf, 0x32)[0]

    def sh(i):
        base = e_shoff + i * e_shentsize
        name, stype, flags, addr, offset, size = struct.unpack_from(">IIIIII", elf, base)
        return name, stype, flags, addr, offset, size

    _, _, _, _, stroff, strsize = sh(e_shstrndx)
    strtab = elf[stroff:stroff + strsize]

    out = []
    for i in range(e_shnum):
        name, stype, flags, addr, offset, size = sh(i)
        end = strtab.index(b"\0", name)
        out.append((strtab[name:end].decode(), stype, flags, addr, offset, size))
    return out


def main():
    elf_path, out_path = sys.argv[1], sys.argv[2]
    elf = open(elf_path, "rb").read()
    if elf[:4] != b"\x7fELF" or elf[4] != 1 or elf[5] != 2:
        sys.exit("expected a big-endian 32-bit ELF")

    # The sections that make up the PROM image, in virtual-address order.
    want = (".text", ".rodata", ".payload")
    picked = []
    for name, stype, flags, addr, offset, size in sections(elf):
        if name in want and size > 0:
            picked.append((addr, offset, size))
    if not picked:
        sys.exit("no PROM sections found")
    picked.sort()

    base = picked[0][0]
    end = max(addr + size for addr, _, size in picked)
    blob = bytearray(end - base)

    for addr, offset, size in picked:
        at = addr - base
        blob[at:at + size] = elf[offset:offset + size]

    open(out_path, "wb").write(blob)
    print(f"{out_path}: {len(blob)} bytes (base 0x{base:08X})")


if __name__ == "__main__":
    main()
