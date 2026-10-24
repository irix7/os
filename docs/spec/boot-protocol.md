# IRIX 7 — Boot Protocol (ROM ↔ Kernel)

**Status:** draft (the current assembly ROM does not emit `BootInfo`)
**Version:** 1.0
**Owner:** boot
**Related:** [`architecture.md`](architecture.md) §10, [`../plan/milestones.md`](../plan/milestones.md) M1–M2.

## 1. Scope

This is the binary contract between the **ROM** (MIPS III assembly, executed directly from
the PROM window at `0xBFC00000`) and the **IRIX 7 kernel** (n32/MIPS III). It defines the
handoff registers, the `BootInfo` structure, the memory map, the CPU/cache state, validation
and failure behaviour.

The ROM is a stub, not a full firmware: it brings the machine up, copies the kernel image
into RAM and jumps to it. Everything the machine does afterwards — including the telnet
console and the network stack — is the kernel's.

It does not define the kernel's own boot sequence, the boot bundle format or the root image;
those are in [`image-format.md`](image-format.md) (planned) and `architecture.md` §10.
Loadable modules are specified in [`modules.md`](modules.md).

## 2. Handoff

The ROM:

1. initialises the CPU and invalidates the TLB;
2. maps a probe window so the DRAM banks can be sized, then programs the memory
   controller to map them at `LOMEM_BASE` (`0x0800_0000`);
3. tests every byte of DRAM, still with no stack and no RAM-resident state of its own;
4. initialises the serial console (the Z85C30 in the IOC);
5. copies the kernel image out of PROM to `0x8804_0000`;
6. writes the boot flag and jumps to the kernel entry with the registers below.

| Register | Value |
|---|---|
| `$a0` | Physical address of `BootInfo`; 0 when the ROM has none (the kernel then falls back to the register handoff) |
| `$a1` | Total RAM bytes the ROM found |
| `$a2` | Physical base address of RAM (`0x0800_0000`) |
| `$sp` | Valid stack in RAM, 16-byte aligned, at least 64 KiB |
| `$ra` | 0 |
| `$v0` | 0 |

CPU state at entry:

| State | Value |
|---|---|
| Endianness | Big-endian |
| Privilege | Kernel mode (`Status.KSU = 0`) |
| Interrupts | Disabled (`Status.IE = 0`, `Status.IM = 0`) |
| Exception level | `Status.EXL = 0`, `Status.BEV = 0` |
| MMU/TLB | Off; no kuseg mappings (`Status.KUc`/`KUp` clear) |
| Caches | I-cache and D-cache enabled; kseg0 cacheable |
| FPU | Disabled (`Status.CU1 = 0`) |
| Watchdog | Disarmed |
| PC | Kernel `_start` |

The current ROM passes `$a0 = 0` (no `BootInfo`), `$a1` = RAM bytes, `$a2` = RAM base and
`$sp` = the kernel stack top. The kernel falls back to that geometry, reserves
`0x0800_0000..0x08B0_0000` and starts the frame allocator at `0x08B0_0000`.

The kernel may re-enable or reconfigure anything after it has validated the handoff. It must
not call ARCS after it takes over the TLB and caches; anything it needs from ARCS must be
copied out of `BootInfo` first.

## 3. BootInfo layout

`BootInfo` is a **binary contract**, not a Rust ABI. It is `#[repr(C, align(8))]`, 256 bytes,
with fixed offsets. The o32 ROM and the n32 kernel must produce and consume the same bytes.
Compile-time assertions in both codebases check the size and offsets.

| Offset | Size | Field | Notes |
|---|---|---|---|
| `0x00` | 4 | `magic` | `0x49374249` (`"I7BI"` big-endian) |
| `0x04` | 2 | `version_major` | 1 |
| `0x06` | 2 | `version_minor` | 0 |
| `0x08` | 4 | `size` | 256 |
| `0x0C` | 4 | `checksum` | CRC32C over the 256 bytes with this field zeroed |
| `0x10` | 8 | `mem_map_phys` | Physical address of the memory-map array |
| `0x18` | 4 | `mem_map_count` | Number of entries (≤ 64) |
| `0x1C` | 4 | `mem_map_entry_size` | 24 |
| `0x20` | 8 | `kernel_phys` | Kernel load address |
| `0x28` | 8 | `kernel_size` | Kernel size in bytes |
| `0x30` | 32 | `kernel_sha256` | SHA-256 of the kernel |
| `0x50` | 8 | `bundle_phys` | Boot bundle load address |
| `0x58` | 8 | `bundle_size` | Boot bundle size |
| `0x60` | 32 | `bundle_sha256` | SHA-256 of the bundle |
| `0x80` | 8 | `cmdline_phys` | Physical address of a NUL-terminated string |
| `0x88` | 4 | `cmdline_len` | Length including the NUL |
| `0x8C` | 4 | `console_kind` | 0 none, 1 UART, 2 framebuffer, 3 both |
| `0x90` | 8 | `console_phys` | UART register base or framebuffer base |
| `0x98` | 4 | `console_width` | Framebuffer width |
| `0x9C` | 4 | `console_height` | Framebuffer height |
| `0xA0` | 4 | `console_stride` | Bytes per line |
| `0xA4` | 4 | `console_bpp` | 8 or 24 |
| `0xA8` | 4 | `console_baud` | UART baud |
| `0xAC` | 6 | `mac` | Ethernet MAC |
| `0xB2` | 2 | `pad0` | Zero |
| `0xB4` | 4 | `ip` | IPv4 address, network order |
| `0xB8` | 4 | `netmask` | IPv4 netmask |
| `0xBC` | 4 | `gateway` | IPv4 gateway |
| `0xC0` | 4 | `dns` | IPv4 DNS |
| `0xC4` | 4 | `dhcp` | 1 if DHCP was used |
| `0xC8` | 4 | `rtc_valid` | 1 if the DS1386 clock is valid |
| `0xCC` | 4 | `flags` | See §4 |
| `0xD0` | 8 | `r2_url_phys` | Physical address of the NUL-terminated R2 base URL |
| `0xD8` | 4 | `r2_url_len` | Length including the NUL |
| `0xDC` | 4 | `cpu_hz` | Measured or PROM-reported CPU clock |
| `0xE0` | 8 | `ram_size` | Usable RAM in bytes |
| `0xE8` | 4 | `board_id` | 0 Indy, 1 Indigo2 IP22, 2 Indigo2 IP28, 3 unknown |
| `0xEC` | 4 | `prom_version` | ARCS/PROM version, 0 if unknown |
| `0xF0` | 4 | `console_poll` | Kseg1 virtual address of the resident console/network poll routine; 0 if absent |
| `0xF4` | 4 | `net_state` | Kseg1 virtual address of the live network/console state passed to `console_poll`; 0 if absent |
| `0xF8` | 4 | `seeq_state` | Kseg1 virtual address of the live Seeq device; 0 if absent |
| `0xFC` | 4 | `reserved` | Zero |

`BootInfo` is followed by the memory map and the strings; the ROM places them in a reserved
region and records their physical addresses.

### 3.1 Resident console service

The telnet console is the kernel's own: the ROM is a stub that brings the machine up and
hands over, so there is no ROM-resident console to poll. `console_poll` is 0 and the kernel
does not call it.

The ROM and the kernel are still one program at two link addresses, and the rule that the
kernel must not re-create state the ROM initialised still holds — it simply has nothing to
inherit yet. When the ROM grows a network stack, `net_state` and `seeq_state` will point at
that live state and the kernel will drive it from its idle path.

### 3.2 Flags

| Bit | Meaning |
|---|---|
| 0 | D-cache enabled |
| 1 | I-cache enabled |
| 2 | MMU/TLB enabled (must be 0 at entry) |
| 3 | Interrupts enabled (must be 0 at entry) |
| 4 | Watchdog armed (must be 0 at entry) |
| 5 | RTC valid |
| 6 | Network configured |
| 7 | Framebuffer console available |
| 8 | Boot bundle present |
| 9 | Kernel digest verified |
| 10 | Bundle digest verified |
| 11 | Boot was a fallback (PROM/local image) |
| 12–31 | Reserved, zero |

## 4. Memory map

Each entry is 24 bytes:

| Offset | Size | Field | Notes |
|---|---|---|---|
| `0x00` | 8 | `base` | Physical base |
| `0x08` | 8 | `size` | Bytes |
| `0x10` | 4 | `kind` | See below |
| `0x14` | 4 | `flags` | 0 for now |

Kinds:

| Value | Meaning |
|---|---|
| 0 | Usable RAM |
| 1 | Reserved (do not use) |
| 2 | PROM/ARCS |
| 3 | MMIO |
| 4 | Framebuffer |
| 5 | `BootInfo` and strings |
| 6 | Kernel image |
| 7 | Boot bundle |
| 8 | Configuration |
| 9 | Persistent NVRAM |

Rules:

- The map may be unsorted; the kernel sorts it.
- It must cover all usable RAM and every MMIO region the kernel needs.
- The kernel must not assume that RAM is contiguous across `0x1800_0000`.
- The kernel copies the map and strings into its own memory before reusing the region.

## 5. Validation

The kernel validates, in order:

1. `magic == "I7BI"`.
2. `version_major == 1`; minor is ignored if unknown.
3. `size >= 256` and `mem_map_entry_size == 24`.
4. CRC32C matches with the checksum field zeroed.
5. `mem_map_count <= 64`; every entry is within the 4 GiB physical map.
6. `kernel_phys`/`kernel_size` and `bundle_phys`/`bundle_size` are inside usable RAM and do
   not overlap `BootInfo` or the stack.
7. `cmdline` and `r2_url` are NUL-terminated within their declared lengths.
8. The kernel and bundle digests match (the ROM already checked; the kernel checks again).

On failure, the kernel prints a diagnostic to the serial console and halts. It does not
attempt to recover by guessing.

## 6. Versioning

- Major version changes are incompatible; the kernel refuses an unknown major.
- Minor versions add fields at the end or repurpose reserved bits; the kernel accepts a
  smaller `size` and zero-fills the rest.
- Unknown flag bits are ignored.
- The memory-map entry size is explicit; a future entry can grow.

## 7. Failure and fallback

The ROM's own failure behaviour is:

| Failure | Behaviour |
|---|---|
| No DRAM found | Print `ROM: RAM TEST FAILED` on the serial console and halt |
| RAM test mismatch | Print the failing address and halt |
| Kernel image will not decompress | Print a diagnostic and halt |

The ROM does not fetch anything: the kernel image is embedded in the PROM image, so
there is no link to lose and no digest to check. The failure modes that used to belong
here — no DHCP, DNS failure, HTTP errors — move to the kernel once it owns the network.

## 8. Test plan

- [x] iris: ROM → kernel → serial marker, with the RAM test reporting the whole machine.
- [x] iris: ROM → kernel → graphical boot menu on the REX3 display.
- [x] The PS/2 scancode decoder is unit-tested on the host (`kernel/tests/kbd.rs`).
- [ ] `BootInfo` size/offset assertions in o32 and n32.
- [ ] CRC32C known-answer tests.
- [ ] A stub kernel validates a synthetic `BootInfo` and prints the memory map.
- [ ] iris: ROM → real kernel → M2 demo.
- [ ] Real Indy: same, with the PROM as the fallback.
- [ ] Corrupt magic, version, checksum, map count and overlapping regions are rejected.
- [ ] A 256 MiB machine and a 32 MiB machine produce valid maps.

## 9. Open questions

- Exact kernel load address (the current plan is a kseg0 address such as `0x8804_0000`,
  mapping physical `0x0804_0000`; the ROM and linker script must agree).
- Whether the boot bundle is loaded by the ROM or fetched by the kernel after handoff.
- Whether the R2 base URL belongs in `BootInfo` or in a separate config blob.
- MCP token provisioning (not in `BootInfo`; the token must not be world-readable).
- Watchdog ownership during the handoff (the ROM must leave it disarmed).

## 10. Kernel boot readiness

The ROM handoff in §2 is followed by the kernel's own boot sequence.  The kernel
emits exactly one readiness line per boot, after the RTC, network and
boot-time module autoload have run:

```text
BOOT-READY v=1 boot=<unix> kernel=up network=up mcp=listening
```

A boot that cannot satisfy the MCP prerequisite emits a distinct line instead:

```text
BOOT-NOT-READY v=1 boot=<unix> kernel=up network=<up|down> mcp=<disabled|failed> reason=<token>
```

`boot` is the RTC Unix second at which the marker was emitted, or `0` when the
RTC has no valid time.  It is monotonic across normal reboots but can repeat if
the machine is reset within the same second.  `reason` is one of `no-mcp-url`,
`autoload-failed` or `network-down`.

The marker is written through the console service, so it appears on serial, in
the MCP serial ring and on a connected telnet session.  The boot menu is shown
after the marker; a diagnostic menu does not affect readiness.
