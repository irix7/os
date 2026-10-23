#!/usr/bin/env python3
"""Run the ROM's LZ4 decompressor on the host, to debug it.

Interprets the MIPS instructions the decompressor uses (the subset in
rom/src/boot/reset.S) so the exact instruction sequence can be traced
against a payload, without booting the machine.  Branch delay slots are
modelled, because MIPS branches always execute the following instruction.

Usage: lz4_run.py <rom.elf> <payload.bin>
"""

import struct
import sys


def sign16(v):
    return v - 0x10000 if v & 0x8000 else v


def section_headers(elf):
    e_shoff = struct.unpack_from(">I", elf, 0x20)[0]
    e_shentsize = struct.unpack_from(">H", elf, 0x2E)[0]
    e_shnum = struct.unpack_from(">H", elf, 0x30)[0]
    e_shstrndx = struct.unpack_from(">H", elf, 0x32)[0]

    def raw(i):
        base = e_shoff + i * e_shentsize
        return struct.unpack_from(">IIIIIIII", elf, base)

    headers = [raw(i) for i in range(e_shnum)]
    stroff, strsize = headers[e_shstrndx][4], headers[e_shstrndx][5]
    strtab = elf[stroff:stroff + strsize]
    out = {}
    for h in headers:
        name_off, stype, flags, addr, offset, size = h[0], h[1], h[2], h[3], h[4], h[5]
        link = h[6]
        end = strtab.index(b"\0", name_off)
        name = strtab[name_off:end].decode()
        out[name] = (stype, addr, offset, size, link)
    return out


def symbol_table(elf, secs):
    out = {}
    for name, (stype, addr, offset, size, link) in secs.items():
        if stype != 2:  # SHT_SYMTAB
            continue
        stroff, strsize = secs[".strtab"][2], secs[".strtab"][3]
        strtab = elf[stroff:stroff + strsize]
        for k in range(size // 16):
            base = offset + k * 16
            nm, value, _sz, _info, _other, _shndx = struct.unpack_from(">IIIBBH", elf, base)
            if nm == 0:
                continue
            end = strtab.index(b"\0", nm)
            out[strtab[nm:end].decode()] = value
    return out


def read_word(elf, secs, addr):
    for name, (stype, sec_addr, off, size, _link) in secs.items():
        if sec_addr and sec_addr <= addr < sec_addr + size:
            return struct.unpack_from(">I", elf, off + (addr - sec_addr))[0]
    raise KeyError(hex(addr))


class Cpu:
    """A CPU with two memory windows: the PROM image and the RAM destination.

    The destination is below the PROM in the address space, so the two cannot
    share one flat buffer without the offsets going negative.
    """

    def __init__(self, rom, rom_base, ram, ram_base):
        self.rom = rom
        self.rom_base = rom_base
        self.ram = ram
        self.ram_base = ram_base
        self.r = [0] * 32
        self.pending = None
        self.trace = False
        self.trace_log = []

    def load(self, addr, n):
        if self.rom_base <= addr < self.rom_base + len(self.rom):
            off = addr - self.rom_base
            return bytes(self.rom[off:off + n])
        off = addr - self.ram_base
        if 0 <= off and off + n <= len(self.ram):
            return bytes(self.ram[off:off + n])
        raise RuntimeError(f"load from unmapped address {addr:#x}")

    def store(self, addr, data):
        off = addr - self.ram_base
        if 0 <= off and off + len(data) <= len(self.ram):
            self.ram[off:off + len(data)] = data
            return
        raise RuntimeError(f"store to unmapped address {addr:#x}")

    def run(self, pc, stop_at=None, limit=100_000_000):
        steps = 0
        while True:
            if stop_at is not None and pc == stop_at:
                return
            steps += 1
            if steps > limit:
                raise RuntimeError(f"instruction limit at {pc:#x}")
            w = struct.unpack(">I", self.load(pc, 4))[0]
            if self.trace:
                self.trace_log.append((pc, w, self.load(pc, 4).hex()))
            if self.pending is not None:
                target = self.pending
                self.pending = None
                self.step(w, pc)
                pc = target
                continue
            pc = self.step(w, pc)

    def step(self, w, pc):
        op = w >> 26
        rs = (w >> 21) & 0x1F
        rt = (w >> 16) & 0x1F
        rd = (w >> 11) & 0x1F
        shamt = (w >> 6) & 0x1F
        funct = w & 0x3F
        imm = w & 0xFFFF
        simm = sign16(imm)
        target = ((pc + 4) & 0xF0000000) | ((w & 0x3FFFFFF) << 2)
        nxt = pc + 4

        if op == 0:  # SPECIAL
            if funct == 0x25:      # or
                self.r[rd] = self.r[rs] | self.r[rt]
            elif funct == 0x21:    # addu
                self.r[rd] = (self.r[rs] + self.r[rt]) & 0xFFFFFFFF
            elif funct == 0x23:    # subu
                self.r[rd] = (self.r[rs] - self.r[rt]) & 0xFFFFFFFF
            elif funct == 0x2B:    # sltu
                self.r[rd] = 1 if self.r[rs] < self.r[rt] else 0
            elif funct == 0x00:    # sll
                self.r[rd] = (self.r[rt] << shamt) & 0xFFFFFFFF
            elif funct == 0x02:    # srl
                self.r[rd] = (self.r[rt] & 0xFFFFFFFF) >> shamt
            elif funct == 0x08:    # jr
                nxt = self.r[rs]
            elif funct == 0x09:    # jalr
                self.r[rd] = pc + 8
                nxt = self.r[rs]
            else:
                raise RuntimeError(f"unsupported SPECIAL funct {funct:#x} at {pc:#x}")
        elif op == 0x09:    # addiu
            self.r[rt] = (self.r[rs] + simm) & 0xFFFFFFFF
        elif op == 0x0F:    # lui
            self.r[rt] = (imm << 16) & 0xFFFFFFFF
        elif op == 0x0C:    # andi
            self.r[rt] = self.r[rs] & imm
        elif op == 0x0D:    # ori
            self.r[rt] = self.r[rs] | imm
        elif op == 0x23:    # lw
            self.r[rt] = struct.unpack(">I", self.load(self.r[rs] + simm, 4))[0]
        elif op == 0x24:    # lbu
            self.r[rt] = self.load(self.r[rs] + simm, 1)[0]
        elif op == 0x28:    # sb
            self.store(self.r[rs] + simm, bytes([self.r[rt] & 0xFF]))
        elif op == 0x04:    # beq
            if self.r[rs] == self.r[rt]:
                self.pending = pc + 4 + simm * 4
        elif op == 0x05:    # bne
            if self.r[rs] != self.r[rt]:
                self.pending = pc + 4 + simm * 4
        elif op == 0x02:    # j
            self.pending = target
        elif op == 0x03:    # jal
            self.r[31] = pc + 8
            self.pending = target
        else:
            raise RuntimeError(f"unsupported opcode {op:#x} at {pc:#x}")
        return nxt


def main():
    elf_path, payload_path = sys.argv[1], sys.argv[2]
    elf = open(elf_path, "rb").read()
    secs = section_headers(elf)
    syms = symbol_table(elf, secs)

    base = syms["_start"] & ~0xFFF
    end = max(secs[n][1] + secs[n][3] for n in (".text", ".payload"))
    mem = bytearray(end - base)
    for n in (".text", ".payload"):
        _, addr, off, size, _ = secs[n]
        mem[addr - base:addr - base + size] = elf[off:off + size]

    payload = open(payload_path, "rb").read()
    start = syms["rom_payload_start"]
    mem[start - base:start - base + len(payload)] = payload
    length = read_word(elf, secs, syms["ROM_PAYLOAD_LEN"])

    dst = 0x0A8040000
    ram = bytearray(64 * 1024)
    cpu = Cpu(mem, base, ram, dst)
    cpu.r[4] = start
    cpu.r[5] = length
    cpu.r[6] = dst
    cpu.r[31] = 0xDEAD0000

    cpu.trace = True
    try:
        cpu.run(syms["lz4_decompress"], stop_at=cpu.r[31])
    except RuntimeError as e:
        print(f"stopped: {e}")
        for pc, w, raw in cpu.trace_log[-12:]:
            print(f"  {pc:#x}: {w:#010x} raw={raw}")
        print(f"  $v0={cpu.r[2]:#x} $s3(remaining)={cpu.r[19]:#x} "
              f"$s0(src)={cpu.r[16]:#x} $s2(dst)={cpu.r[18]:#x}")
        return 1

    print(f"returned $v0 = {cpu.r[2]}")
    print(f"  remaining={cpu.r[19]} src={cpu.r[16]:#x} dst={cpu.r[18]:#x}")
    print(f"  first output: {bytes(cpu.ram[:32]).hex()}")
    if len(sys.argv) > 3:
        want = open(sys.argv[3], "rb").read()
        got = bytes(cpu.ram[:len(want)])
        print(f"  output matches the image: {got == want}")
        if got != want:
            for i, (a, b) in enumerate(zip(got, want)):
                if a != b:
                    print(f"  first difference at {i}: got {a:#04x} want {b:#04x}")
                    break
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
