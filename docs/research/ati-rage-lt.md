# ATI Rage LT Pro (M4753) graphics driver

**Status:** research and design note, no driver code. This is the research slice for
[#328](https://github.com/irix7/os/issues/328).
**Scope:** the PowerBook G3 Series (M4753, "Wallstreet") ATI 3D Rage LT Pro, its complete
function set, the register blocks, the driver-cell contract, the Mesa/GL path and a staged
plan with testable gates.
**Related:** [`powerbook-g3.md`](powerbook-g3.md),
[`powerbook-qemu-boot.md`](powerbook-qemu-boot.md),
[`native-window-system-research.md`](native-window-system-research.md),
[`rust-x11-research.md`](rust-x11-research.md),
[`../spec/architecture.md`](../internals/architecture.md) D17 and §13,
[`../spec/system.md`](../internals/system.md) §7,
[`../spec/driver-interface.md`](../programming/driver-interface.md).

**Contents:** [1 Summary](#1-summary) · [2 Method](#2-method-and-evidence) ·
[3 Chip and board](#3-the-chip-and-the-board) · [4 Register map](#4-register-block-map) ·
[5 Feature matrix](#5-complete-feature-matrix) ·
[6 Emulators](#6-qemu-emulators-and-real-hardware) ·
[7 Driver cell](#7-driver-cell-and-device-capability-interface) ·
[8 Mesa/GL](#8-mesa-and-the-gl-bridge) ·
[9 Staged plan](#9-staged-plan-and-gates) · [10 Provenance](#10-clean-room-provenance-and-sources) ·
[11 Unknowns](#11-unknowns-and-risks) · [12 Non-goals](#12-non-goals)

---

## 1. Summary

The M4753 has a **3D Rage LT Pro** (ATI mach64 LB/LI/LP family), a PCI 33 MHz,
fixed-function 2D/3D/video controller with **2 MiB or 4 MiB of SGRAM**. The 12.1-inch
model has 2 MiB and an 800×600 STN panel; the 13.3-inch and 14.1-inch models have 4 MiB
and a 1024×768 TFT panel. The 13.3-inch and 14.1-inch models also have S-video output. The
VGA port mirrors the internal panel; Apple does not support an extended desktop.

The chip is not a modern GPU. It has no programmable shaders, no 32-bit Z buffer, no
stencil, no multisample anti-aliasing, no cube or 3D textures, and no AGP texture path on
the PCI variant. It does have a floating-point triangle setup engine (1.0–1.2 million
triangles/s peak), a 4 KiB texture cache, single-pass bilinear/trilinear filtering, alpha
blending, fog, dithering, edge anti-aliasing, a 2D blitter with full ROPs, a hardware
cursor, a video overlay/scaler, an integrated TV encoder and an LCD panel interface.

The publicly archived ATI **RAGE LT PRO Register Reference Guide** (RRG-G03300) documents
the display, 2D, LCD, bus-master and PCI blocks but **deliberately omits the 3D, multimedia
and TV-out register descriptions** (§1.1: "registers related to 3D, Multimedia, and TV Out
have been omitted"). The publicly archived **Graphics Controller Specifications**
(GCS-C03300) document the feature set but not the register semantics. The 3D programming
sequence must therefore be reconstructed from the open Mesa mach64 DRI driver (the only
open Rage Pro 3D state mapping), the 86Box Mach64 and 3D Rage II+ models and the MAME 2D
models for cross-checking, and hardware experiments. That is a real provenance gap and it
is recorded in §10.

QEMU's `ati-vga` is **not this chip**. It emulates a Rage 128 Pro/RV100 with a 2D-only
engine and a different register layout. The issue's premise that `ati-vga` is the same chip
without 3D is therefore too generous: it cannot stand in for the Mach64 even for 2D. It
cannot validate any Mach64 register. 86Box and MAME each model a Mach64/Rage Pro 2D engine,
but neither models the LT Pro's 3D setup engine, LCD panel, TV encoder or PMU wiring. **All
Mach64-specific and all 3D behaviour must be tested on the PowerBook.** The QEMU gate can
only prove PCI enumeration, BAR mapping, the generic framebuffer handover and the cell
plumbing.

The driver is a de-privileged cell (D17) with `Mmio`, `Irq` and `DmaWindow` capabilities.
The GPU is a global `GpuDevice` object in the one-logical-computer model; clients submit
surfaces and command buffers, never raw register writes. The cell is the only writer to the
ATI register aperture.

The staged plan is in §9: Stage 0 (PCI, OF framebuffer, cell skeleton), Stage 1 (display
and 2D), Stage 2 (fixed-function 3D), Stage 3 (overlay, TV, backlight, DDC, power), Stage 4
(Mesa/GL and GLX), Stage 5 (production hardening). Each supported function in §5 has a gate
or a hardware test; silicon-absent functions are marked `N`, and no chip-supported function
is listed as a non-goal.

---

## 2. Method and evidence

The note separates four kinds of claim:

- **Register facts** come from the ATI register references and are cross-checked against the
  Xorg mach64 driver, the Linux `atyfb` driver and the Mesa mach64 DRI driver.
- **Behaviour** comes from the open drivers and emulators. Where the public register
  reference omits a description, the behaviour is marked as reconstructed and must be
  confirmed on hardware.
- **Board wiring** comes from Apple's PowerBook G3 Series technical specifications,
  EveryMac's M4753 entry and the PowerBook G3 research note. Wiring that cannot be
  established from those sources is marked unknown, not assumed.
- **Performance and memory** claims are arithmetic from the stated mode sizes and the
  public 4 MiB/64-bit/100 MHz memory configuration. No benchmark was run for this note; the
  PowerBook hardware is not available to the researcher.

Confidence:

| Area | Confidence | Basis |
|---|---|---|
| PCI IDs, BARs, aperture mapping, endianness | High | RRG-G03300 ch. 2 and 6, Xorg probe table, Linux `atyfb` |
| Display, CRTC, PLL, DAC, cursor, DPMS | High | RRG-G03300 ch. 4, Xorg mach64, Linux `atyfb` |
| 2D fill/blit/line/pattern/ROP/clip | High | RRG-G03300 ch. 5, Xorg mach64, 86Box, MAME |
| FIFO, bus master, interrupts | Medium | RRG-G03300 ch. 5–6; descriptor layout not public |
| 3D register semantics | Medium | Register names and addresses public; descriptions omitted; Mesa evidence; 86Box only for the earlier engine |
| LCD panel, backlight, TV-out, DDC | Low | Public feature list and driver code; M4753 wiring unverified |
| Video capture, ZV, AMC | Low | Chip feature list; no M4753 input connector documented |
| Real-hardware quirks and timing | Unknown | Requires the PowerBook |

---

## 3. The chip and the board

### 3.1 Identity

| Item | Value | Source |
|---|---|---|
| Marketing name | ATI 3D Rage LT Pro | GCS-C03300, RRG-G03300 |
| Chip family | mach64 LT Pro (LB/LI/LP product codes) | RRG-G03300 §4.1.7 |
| PCI vendor | `0x1002` (ATI) | RRG-G03300 ch. 6, pci.ids |
| PCI device, AGP-133, 312-pin | `0x4C42` (LB) | RRG-G03300 Table 6-1 |
| PCI device, PCI-33, 312-pin | `0x4C49` (LI) | RRG-G03300 Table 6-1 |
| PCI device, PCI-33, 256-pin | `0x4C50` (LP), `0x4C51` (LQ) | RRG-G03300 Table 4-4, Xorg probe table |
| PCI class | `0x030000` (display controller, VGA-compatible) | RRG-G03300 ch. 6 |
| Bus | PCI 2.1, 33 MHz; AGP variants exist but not on the M4753 | GCS-C03300 §2.3–2.4 |
| Package | 312-pin BGA (dual-pixel LVDS, 18 bpp dual) or 256-pin BGA (24 bpp single) | GCS-C03300 §2.3 |
| Memory | 2 MiB or 4 MiB SGRAM on the M4753; chip supports 2–8 MiB SDRAM/SGRAM | Apple SP128, GCS-C03300 |
| Memory bus | 64-bit, 100 MHz, 800 MB/s peak | GCS-C03300 §2.2.2 |
| DAC | Triple 8-bit, 24-bit palette; 200–230 MHz on AGP variants; PCI variant lower | GCS-C03300 §2.3 |
| LVDS | On-chip, up to 10 channels, 65 MHz pixel clock, 455 Mb/s per channel | RRG-G03300 §2.1.1 |
| TV-out | Integrated ImpacTV2 encoder, second triple DAC, composite/S-video/RGB SCART | GCS-C03300 §2.1.2 |
| Video input | ZV port, AMC; no built-in camera or tuner on the M4753 | GCS-C03300 §2.1.1, §2.2.14 |

The M4753 is the PCI-33 variant. The 312-pin and 256-pin packages differ in the digital
panel interface and the TV encoder pin-out; the PCI ID distinguishes them. The exact ID on a
given PowerBook must be read from PCI configuration and `CONFIG_CHIP_ID` (0x38) at probe.
The Xorg probe table maps `0x4C42`, `0x4C44`, `0x4C49`, `0x4C50` and `0x4C51` to
`ATI_CHIP_264LTPRO`; the RRG's PCI table gives `0x4C49` and `0x4C50`/`0x4C51` for the PCI
parts. The likely M4753 value is `0x4C49` on the 312-pin 4 MiB board, but `0x4C50` or
`0x4C51` must not be rejected by the probe.

`CONFIG_CHIP_ID` returns the two ASCII product letters in bits 15:0 (`4C49` = "LI",
`4C50` = "LP"); the upper half reports the class, major revision, foundry and minor
revision, with the exact field positions in the register diagram. `CONFIG_STAT0` (0x39)
reports the memory type, VGA strap, clock-enable strap and `PANEL_ID[4:0]`; `CONFIG_STAT1` (0x25)
reports the PCI subsystem vendor/device IDs; `CONFIG_STAT2` (0x26) reports the bus-type
strap (normal or reverse) and the ROM write strap. These are the probe registers.

### 3.2 What the chip supports

The feature list below is from GCS-C03300 §2.2.3, §2.2.10–2.2.13 and RRG-G03300 §2.1; the
full function-by-function matrix is §5.

| Class | Functions |
|---|---|
| Display | CRTC1 and CRTC2, independent timing and scanout, interlace, double scan, overscan, panning, page flip, DPMS, genlock, 8/16/24/32 bpp, packed 24 bpp, hardware cursor 64×64×2, hardware icon 128×128, 256×24 palette, gamma, DDC/EDID via hardware I2C |
| LCD | LVDS and digital panel interface, panel power sequencing, backlight modulation, ratiometric expansion, horizontal/vertical stretching and filtering, STN and TFT panels up to 1280×1024, 8/9/12/15/18/24-bit panels, simulcast CRT and panel |
| 2D | Bitblt, screen-to-screen and host blit, line draw, polygon/rectangle fill, trapezoid fill, 8×8 mono and 4×2/8×1 colour patterns, full 16 ROPs plus (D+S)/2, scissoring, colour compare, write mask, monochrome expansion, 1 bpp to any colour expansion, 8/16/24/32 bpp, fast fill, double buffering, masked/transparent blit, virtual sprites, context chaining |
| 3D | Floating-point triangle setup, points/lines/triangles/quads/lists/strips, flat/Gouraud/specular shading, perspective-correct mipmapped texturing, 11 mip levels, 4 KiB texture cache, single-pass bilinear/trilinear, 2 texture units, alpha blending, alpha test, 16-bit Z, fog, dithering, edge anti-aliasing, sub-pixel/sub-texel, chroma key, video textures, texture lighting, LOD bias, texture compositing, Z-compare blit |
| Video | Overlay window, front-end scaler, horizontal/vertical scaling and filtering, YUV422 packed, YUV410/420 planar, YUV444 texture, colour-space conversion, graphics/video keying, brightness/saturation, de-interlacing, multi-stream video, mirror/flip |
| TV | Integrated NTSC/PAL encoder, composite/S-video/RGB SCART, independent H/V scaling, flicker filter, Line 21 closed caption, Macrovision 7.01 (bonding option) |
| Capture | ZV port (PCMCIA CCIR601), AMC 16-bit video port, one-shot and continuous capture, two capture buffers, VBI capture |
| DMA/FIFO | 64/128/192-entry command FIFO, GUI bus-master descriptor table (16–128 KiB), system bus-master table, draw-engine bus master, block transfers up to 4096 bytes per descriptor, scatter-gather |
| Interrupts | VBLANK, VLINE, CRTC2 VBLANK/VLINE, overlay EOF, capture buffer 0/1, one-shot capture, bus-master EOL, GP IO, FIFO error (optional/debug) |
| Power | ACPI D0/D1/D2/D3, PCI power management, DPMS, PLL sleep, DAC power-down, panel bias/backlight/digital power sequencing, dynamic clock switching, self-refresh, 32 kHz refresh |
| Legacy | Full VGA register set, VGA aperture, VGA palette snooping, VGA text and graphics modes |

### 3.3 The M4753 board

| Function | Board wiring | Evidence / note |
|---|---|---|
| Internal panel, 12.1-inch | 800×600 STN, 2 MiB SGRAM | Apple SP128; EveryMac |
| Internal panel, 13.3-inch | 1024×768 TFT, 4 MiB SGRAM | Apple SP128; EveryMac |
| Internal panel, 14.1-inch | 1024×768 TFT, 4 MiB SGRAM | Apple SP128; EveryMac |
| Panel link | LVDS or digital panel interface; exact type unknown | Read `CONFIG_PANEL` and inspect the cable; the chip supports both |
| Backlight | PMU or ATI `BLON`/`BL_PWM_LEVEL`; the PMU is the likely brightness path | Verify both paths on hardware |
| External VGA | HDI-15, mirroring only, up to 1024×768 | Apple SP128; EveryMac |
| DDC/EDID on VGA | Likely wired; unverified | Chip has hardware I2C; read an external monitor's EDID |
| S-video | 13.3-inch and 14.1-inch only | Apple SP128; EveryMac |
| Composite video | No separate connector; S-video Y/C only, composite via adapter if at all | Board inspection / Apple adapter documentation |
| TV standard | NTSC/PAL selected in the TV encoder | Chip supports both; verify with a TV |
| Video capture | No built-in camera, tuner or video input | Chip capture registers exist; board has no input connector |
| ZV port | May be routed to the PC Card slots; unverified | Chip supports ZV; no evidence of a built-in connector |
| AMC | No AMC connector | Chip supports it; board does not expose it |
| Hardware cursor | Wired to the primary display | Chip feature; test on hardware |
| Genlock | No genlock connector | Chip registers exist; board does not expose them |
| PCI interrupt | INTA# routed through Heathrow/Gatwick | OF `interrupt-map`; kernel-ppc has no PCI IRQ routing yet |
| PCI bus master DMA | Wired | Chip bus master; no IOMMU documented on the M4753 |
| PMU | Present | `via-pmu`; the kernel-ppc PMU command protocol is a separate slice |

The board is a **mirroring** design, not an extended-desktop design. The chip can drive two
independent CRTCs and the LT Pro adds 2D/3D operations on the secondary display
(`USR1_DST_OFF_PITCH`/`USR2_DST_OFF_PITCH`, `CRTC2_*`, `DSP2_*`), but Apple's product
specification says mirroring only. Independent secondary scanout is a chip capability that
the board may support electrically; it is not a supported product configuration and must be
tested before it is offered. It is recorded as mirror-only, not as chip-absent.

The 12.1-inch model has no S-video and 2 MiB. A driver that assumes 4 MiB or an S-video
connector is wrong on that model. Probe `MEM_CNTL.MEM_SIZE` and the presence of the TV
encoder path, and let the panel/board description decide the default mode.

### 3.4 The limits that decide the design

1. **4 MiB of VRAM.** A 1024×768×32 front buffer is 3 MiB; double buffering is 6 MiB and
   does not fit. A 1024×768×24 front buffer is 2.25 MiB; double buffering is 4.5 MiB and
   does not fit. A 1024×768×16 front buffer is 1.5 MiB; double buffering plus a 16-bit Z
   buffer is 4.5 MiB and does not fit. The native desktop at 24 bpp uses 2.25 MiB and
   leaves 1.75 MiB for offscreen surfaces and the cursor. 3D at native resolution is
   therefore a 16 bpp, single-Z, reduced-texture configuration, or a smaller viewport.
2. **No AGP.** The PCI variant has no AGP texturing and no motion compensation. Textures
   live in local VRAM. The 4 KiB texture cache reduces bandwidth, not capacity.
3. **Fixed function.** No shaders, no GLSL, no FBOs, no stencil, no 32-bit Z, no
   multisample. The 3D pipeline is the OpenGL 1.x fixed-function set, and only the subset
   the Rage Pro implements.
4. **No IOMMU.** The ATI bus master can address system memory. The framework grants a
   `DmaWindow`; the driver must pin and validate every descriptor. The DMA window is a
   policy guarantee, not hardware confinement.
5. **Big-endian host.** The chip's registers are 32-bit little-endian; the framebuffer
   aperture has 16/32 bpp byte-swap modes. Every register access and every pixel path must
   be checked on the PowerPC, not inferred from x86.
6. **The public 3D documentation is incomplete.** The register names and addresses are
   public; the descriptions are not. The driver must be built from reconstructed behaviour
   and hardware tests, not from a complete specification.

---

## 4. Register-block map

### 4.1 PCI configuration and BARs

| Register | Offset | Use |
|---|---|---|
| Vendor/device ID | 0x00 | `0x1002:0x4C49`/`4C50`/`4C51` |
| Command/status | 0x04 | I/O, memory, bus-master enable; status |
| Revision/class | 0x08–0x0B | ASIC ID, class `0x030000` |
| BAR0 | 0x10 | Linear memory aperture (framebuffer + register blocks) |
| BAR1 | 0x14 | Block-decoded I/O aperture |
| BAR2 | 0x18 | 4 KiB auxiliary register aperture |
| Reserved | 0x28 | Not used |
| Subsystem IDs | 0x2C–0x2F | `CONFIG_STAT1` mirror |
| ROM BAR | 0x30 | BIOS ROM |
| Capabilities | 0x34 | AGP capability (AGP parts), PM capability at 0x5C |
| Interrupt line/pin | 0x3C/0x3D | INTA#, line assigned by firmware |
| PM capability | 0x5C–0x63 | D1/D2, PME |

The BARs are assigned by Open Firmware. The driver reads them; it does not reassign them.
BAR0 is the aperture described in §4.2. BAR1 is the block I/O window for the indexed LCD,
I2C and some display registers. BAR2 is the always-enabled 4 KiB register aperture.

### 4.2 Apertures and endianness

The chip has two mapping modes. The M4753 uses **linear aperture mode**.

| Region | Address | Contents |
|---|---|---|
| Primary linear aperture | BAR0, 2×8 MiB | Frame buffer at the base; register block 0 in the top 1 KiB of the first 8 MiB; register block 1 1 KiB below block 0 |
| Auxiliary register aperture | BAR2, 4 KiB | Block 0 at base+1 KiB, block 1 at base+0 |
| Block I/O | BAR1, 64 KiB I/O | Display/config registers 0x00–0x3F, LCD index/data, I2C |
| VGA aperture | 0xA0000–0xBFFFF | VGA modes; not used in extended mode |
| VGA I/O | 0x3B4/0x3D4, 0x3C0–0x3CF, 0x46E8 | VGA registers |

Register addressing within the register aperture:

- Block 0 register `r` (0x00–0xFF): byte offset `0x400 + (r << 2)`.
- Block 1 register `0x100 + r` (0x00–0xFF): byte offset `(r << 2)`.

This is the mapping used by the Mesa mach64 DRI (`DWMREG0 = 0x0400`, `DWMREG1 = 0x0000`)
and by Xorg. It is a fact of the aperture layout, not a Mesa invention.

**Endianness.** All registers are 32-bit little-endian. `MEM_CNTL` (0x2C) has
`LOWER_APER_ENDIAN` and `UPPER_APER_ENDIAN` fields: 00 = little-endian (no swap),
01 = big-endian 16 bpp swap, 10 = big-endian 32 bpp swap. The big-endian aperture is
8 MiB above the little-endian aperture. Xorg maps the big-endian aperture on big-endian
hosts. `HOST_CNTL.HOST_BIG_ENDIAN_EN` (0x90) performs the same translation for host-data
blits, and `DP_PIX_WIDTH.DP_BYTE_PIX_ORDER` controls pixel order within a byte for 1/4/8 bpp.

On the PowerPC, a 32-bit store to a little-endian PCI register may be byte-swapped by the
host bridge or may need an explicit swap in the driver. This is a Stage 0 gate: write a
known pattern to `SCRATCH_REG0` (0x20), read it back with and without a swap, and log which
one matches. Do not assume the x86 convention. The frame buffer aperture endianness is
independent of the register endianness.

### 4.3 Block 0: display, configuration and the GUI engine

DWORD offsets, block 0. Register names are the ATI mnemonics.

| Range | Registers | Use |
|---|---|---|
| 0x00–0x07 | `CRTC_H_TOTAL_DISP`, `CRTC_H_SYNC_STRT_WID`, `CRTC_V_TOTAL_DISP`, `CRTC_V_SYNC_STRT_WID`, `CRTC_VLINE_CRNT_VLINE`, `CRTC_OFF_PITCH`, `CRTC_INT_CNTL`, `CRTC_GEN_CNTL` | Primary CRTC timing, scanout offset, interrupts, pixel width, interlace, double scan, DPMS sync disable, CRTC enable |
| 0x10–0x12 | `OVR_CLR`, `OVR_WID_LEFT_RIGHT`, `OVR_WID_TOP_BOTTOM` | Overscan |
| 0x13–0x17 | `VGA_DSP_CONFIG`, `VGA_DSP_ON_OFF`, `DSP2_CONFIG`, `DSP2_ON_OFF`, `CRTC2_OFF_PITCH` | Display FIFO, secondary display |
| 0x18–0x1C | `CUR_CLR0`, `CUR_CLR1`, `CUR_OFFSET`, `CUR_HORZ_VERT_POSN`, `CUR_HORZ_VERT_OFF` | Hardware cursor/icon |
| 0x1D | `LCD_INDEX`/`TV_OUT_INDEX` | Index for the LCD or TV register space |
| 0x1E | `GP_IO` | General-purpose I/O (I2C fallback) |
| 0x1F | `HW_DEBUG` | FIFO width, Z flush, bus-master fix, VBLANK double-buffer status |
| 0x20–0x23 | `SCRATCH_REG0`–`3` | Probe/self-test |
| 0x24 | `CLOCK_CNTL` | Pixel clock select and PLL indirect access |
| 0x25–0x27 | `CONFIG_STAT1`, `CONFIG_STAT2`, `LCD_DATA`/TV data | Board straps, indexed data |
| 0x28 | `BUS_CNTL` | Bus master, retry, bursts, reset |
| 0x2B–0x2E | `EXT_MEM_CNTL`, `MEM_CNTL`, `MEM_VGA_WP_SEL`, `MEM_VGA_RP_SEL` | Memory controller; read-only for the driver except the VGA boundary |
| 0x30–0x31 | `DAC_REGS`, `DAC_CNTL` | Palette and DAC control |
| 0x34 | `GEN_TEST_CNTL` | Cursor enable, GUI reset, memory reset, test modes |
| 0x35 | `CUSTOM_MACRO_CNTL` | Custom macros; leave as firmware set |
| 0x37–0x39 | `CONFIG_CNTL`, `CONFIG_CHIP_ID`, `CONFIG_STAT0` | Aperture, chip ID, memory/panel straps |
| 0x3A | `CRC_SIG` | Diagnostic signature |
| 0x3B–0x3F | MPP and `TVO_CNTL` | Multimedia port, TV control |

The 2D engine occupies 0x40–0xFF:

| Range | Registers | Use |
|---|---|---|
| 0x40–0x4D | `DST_OFF_PITCH`, `DST_X`, `DST_Y`, `DST_Y_X`, `DST_WIDTH`, `DST_HEIGHT`, `DST_HEIGHT_WIDTH`, `DST_X_WIDTH`, `DST_BRES_LNTH`, `DST_BRES_ERR`, `DST_BRES_INC`, `DST_BRES_DEC`, `DST_CNTL`, `DST_Y_X` | Destination trajectory, rectangle/line/trapezoid start, Bresenham |
| 0x4E–0x51 | `TRAIL_BRES_ERR`, `TRAIL_BRES_INC`, `TRAIL_BRES_DEC`, `LEAD_BRES_LNTH` | Trapezoid trailing edge |
| 0x52–0x54 | `Z_OFF_PITCH`, `Z_CNTL`, `ALPHA_TST_CNTL` | Z buffer, alpha test |
| 0x56–0x5F | `SECONDARY_*` | Secondary texture coordinates and starts |
| 0x60–0x6D | `SRC_OFF_PITCH`, `SRC_X`, `SRC_Y`, `SRC_Y_X`, `SRC_WIDTH1`, `SRC_HEIGHT1`, `SRC_HEIGHT1_WIDTH1`, `SRC_X_START`, `SRC_Y_START`, `SRC_Y_X_START`, `SRC_WIDTH2`, `SRC_HEIGHT2`, `SRC_HEIGHT2_WIDTH2`, `SRC_CNTL` | Source trajectory, pattern, brush |
| 0x70–0x7A | `TEX_0_OFF`–`TEX_10_OFF` | Texture mip offsets (aliased with scaler registers) |
| 0x7B–0x7F | `S_Y_INC`/`SCALE_PITCH`, `SCALE_X_INC`, `SCALE_Y_INC`, `SCALE_VACC`, `SCALE_3D_CNTL` | Scaler and 3D pipeline control |
| 0x80–0x90 | `HOST_DATA[0..15]`, `HOST_CNTL` | Host-data blit |
| 0x91–0x93 | `BM_HOSTDATA`, `BM_ADDR`/`BM_DATA`, `BM_GUI_TABLE_CMD` | Draw-engine bus master |
| 0xA0–0xA2 | `PAT_REG0`, `PAT_REG1`, `PAT_CNTL` | Pattern |
| 0xA8–0xAD | `SC_LEFT`, `SC_RIGHT`, `SC_LEFT_RIGHT`, `SC_TOP`, `SC_BOTTOM`, `SC_TOP_BOTTOM` | Scissors |
| 0xAE–0xBF | `USR1_DST_OFF_PITCH`, `USR2_DST_OFF_PITCH`, `DP_BKGD_CLR`, `DP_FRGD_CLR`/`DP_FOG_CLR`, `DP_WRITE_MSK`, `DP_PIX_WIDTH`, `DP_MIX`, `DP_SRC`, `DP_FRGD_CLR_MIX`, `DP_FRGD_BKGD_CLR`, `DST_X_Y`, `DST_WIDTH_HEIGHT`, `USR_DST_PITCH`, `DP_SET_GUI_ENGINE2`, `DP_SET_GUI_ENGINE` | Data path, ROPs, pixel widths, context loads |
| 0xC0–0xC4 | `CLR_CMP_CLR`, `CLR_CMP_MSK`, `CLR_CMP_CNTL`, `FIFO_STAT` | Colour compare, command FIFO status |
| 0xCC | `GUI_TRAJ_CNTL` | Composite trajectory control (source pattern, byte align, big-endian host) |
| 0xCE | `GUI_STAT` | GUI/3D active, scissor status, free FIFO DWORDs |
| 0xD0–0xDF | Texture palette, `STW_EXP`, `LOG_MAX_INC`, `S_X_INC`, `S_Y_INC`, `S_START`, `W_X_INC`, `W_Y_INC`, `W_START`, `T_X_INC`, `T_Y_INC`, `T_START`, `TEX_SIZE_PITCH`, `TEX_CNTL`, `TEX_PALETTE` | Texture mapping |
| 0xE0–0xFE | Scaler accumulators, `COMPOSITE_SHADOW_ID`, specular/RGB/Z/alpha/fog interpolators | Scaler and 3D colour/Z/alpha/fog |

### 4.4 Block 1: multimedia, 3D setup and bus master

DWORD offsets, block 1 (register index `0x100 + r`).

| Range | Registers | Use |
|---|---|---|
| 0x1_00–0x1_06 | `OVERLAY_Y_X_START`, `OVERLAY_Y_X_END`, `OVERLAY_VIDEO_KEY_CLR`, `OVERLAY_VIDEO_KEY_MSK`, `OVERLAY_GRAPHICS_KEY_CLR`, `OVERLAY_GRAPHICS_KEY_MSK`, `OVERLAY_KEY_CNTL` | Overlay window and keying |
| 0x1_08–0x1_09 | `OVERLAY_SCALE_INC`, `OVERLAY_SCALE_CNTL` | Overlay scaling enable and factors |
| 0x1_10–0x1_15 | `CAPTURE_START_END`, `CAPTURE_X_WIDTH`, `VIDEO_FORMAT`, `VBI_START_END`, `CAPTURE_CONFIG`, `TRIG_CNTL` | Video capture timing and format |
| 0x1_16–0x1_17 | `OVERLAY_EXCLUSIVE_HORZ`, `OVERLAY_EXCLUSIVE_VERT` | Overlay bounds |
| 0x1_18–0x1_1A | `VBL_WIDTH`, `CAPTURE_DEBUG`, `VIDEO_SYNC_TEST` | Vertical blank width, capture diagnostics |
| 0x1_1C–0x1_1F | `SNAPSHOT_VH_COUNTS`, `SNAPSHOT_F_COUNT`, `N_VIF_COUNT`, `SNAPSHOT_VIF_COUNT` | Genlock snapshots |
| 0x1_20–0x1_22 | `CAPTURE_BUF0_OFFSET`, `CAPTURE_BUF1_OFFSET`, `ONESHOT_BUF_OFFSET` | Capture buffers |
| 0x1_2C–0x1_2F | `SNAPSHOT2_F_COUNT`, `N_VIF2_COUNT`, `SNAPSHOT2_VIF_COUNT` | Genlock snapshots, second CRTC |
| 0x1_30–0x1_33 | `MPP_CONFIG`, `MPP_STROBE_SEQ`, `MPP_ADDR`, `MPP_DATA` | Multimedia peripheral port |
| 0x1_40 | `TVO_CNTL` | TV-out control/data |
| 0x1_52–0x1_53 | `AGP_BASE`, `AGP_CNTL` | AGP (not present on the PCI variant) |
| 0x1_54–0x1_59 | `SCALER_COLOUR_CNTL`, `SCALER_H_COEFF0`–`4` | Overlay colour and horizontal coefficients |
| 0x1_5C–0x1_5E | `GUI_CMDFIFO_DEBUG`, `GUI_CMDFIFO_DATA`, `GUI_CNTL` | FIFO snoop, size mode |
| 0x1_60–0x1_63 | `BM_FRAME_BUF_OFFSET`, `BM_SYSTEM_MEM_ADDR`, `BM_COMMAND`, `BM_STATUS` | System bus master |
| 0x1_6E–0x1_6F | `BM_GUI_TABLE`, `BM_SYSTEM_TABLE` | GUI/system descriptor tables |
| 0x1_75–0x1_78 | `SCALER_BUF0_OFFSET_U/V`, `SCALER_BUF1_OFFSET_U/V` | Planar YUV buffers |
| 0x1_90–0x1_A6 | `VERTEX_1_S/T/W/SPEC_ARGB/Z/ARGB/X_Y`, `VERTEX_2_*`, `VERTEX_3_*` | Triangle setup vertices |
| 0x1_B0–0x1_B2 | `VERTEX_3_SECONDARY_S/T/W` | Third vertex, second texture |
| 0x1_C0–0x1_C1 | `ONE_OVER_AREA_UC`, `SETUP_CNTL` | Reciprocal area trigger, setup control |
| 0x1_CA–0x1_CF | `VERTEX_1_SECONDARY_S/T/W`, `VERTEX_2_SECONDARY_S/T/W` | Second texture coordinates |

### 4.5 Indexed spaces

| Space | Index | Data | Registers |
|---|---|---|---|
| LCD panel | `LCD_INDEX` 0x1D (block I/O 0x29) | `LCD_DATA` 0x27 (block I/O 0x2A) | `CONFIG_PANEL` (0), `LCD_GEN_CNTL` (1), `POWER_MANAGEMENT` (8), `ZVGPIO` (9), `HORZ_STRETCHING` (A), `VERT_STRETCHING` (B) |
| TV-out | `TV_OUT_INDEX` 0x1D | `TVO_CNTL` 0x3F | `TV_MASTER_CNTL` (0x10), `TV_RGB_CNTL` (0x12), `TV_SYNC_CNTL` (0x14), timing (0x20–0x2C), scaler (0x70–0x75), modulator (0x80), DAC (0xA0), VBI (0xB8–0xBE) |
| PLL | `CLOCK_CNTL` 0x24 | Indirect through `CLOCK_CNTL` | `MPLL_CNTL`, `VPLL_CNTL`, `PLL_REF_DIV`, `PLL_GEN_CNTL`, `PLL_TEST_*`, LVDS PLL |
| VGA | I/O ports | Standard VGA | CRTC, sequencer, graphics, attribute, DAC |

The PLL protocol is: write the register address to `CLOCK_CNTL` with `PLL_WR_EN = 0`,
read `CLOCK_CNTL` byte 2 for data; or write the address with `PLL_WR_EN = 1` and the data
byte. The chip has four programmable pixel clocks; `CLOCK_CNTL.CLOCK_SEL` selects one in
extended mode. `PLL_GEN_CNTL.PLL_SLEEP` powers down the MPLL/VPLL. The LVDS PLL has its own
registers (`LVDSPLL_CNTL0/1`). The driver should copy the PLL values the firmware already
programmed for the panel mode before attempting a new mode; it should not synthesise a PLL
from scratch until the panel and the DAC are understood.

### 4.6 Command FIFO and batching

- The FIFO is 64, 128 or 192 DWORDs, selected by `GUI_CNTL.CMDFIFO_SIZE_MODE`
  (00 = 192, 01 = 128 default, 10 = 64, 11 = reserved). Change the size only when the FIFO
  is empty.
- 2D and 3D registers are **written through the FIFO and read directly** (RRG-G03300
  §2.3.1). A write to a GUI register offset enters the FIFO; a read bypasses it.
- `FIFO_STAT` (0xC4) reports the occupancy of the last 16 entries and `FIFO_ERR` (bit 31).
  `GUI_STAT` (0xCE) bits 15:0 report the number of free DWORDs; bit 0 is `GUI_ACTIVE`
  (GUI busy, 3D busy, FIFO not empty or context load in progress).
- Writing when insufficient entries are free sets `FIFO_ERR` and **locks the draw engine**.
  The recovery is to pulse `GEN_TEST_CNTL.GEN_GUI_RESETB` (0x34, high-to-low) and
  reinitialise the GUI state. The FIFO error may be wired to an interrupt for debug, but
  the public register set does not expose a dedicated enable; treat it as a polling
  condition.
- The driver batches no more than the free count minus a safety margin (for example 32
  DWORDs) and checks `GUI_STAT` between batches. It never spins with interrupts disabled.
- `GUI_ACTIVE` must be clear before a mode switch, a palette reload or a CPU read of the
  frame buffer. The Mach64 engine caches frame-buffer data; the first CPU read after a
  draw can return stale zeroes. Xorg and Mesa both work around this by waiting for idle and
  pre-reading a word. The driver must do the same before readback tests.
- The bus-master path (`BM_GUI_TABLE`, 0x1_6E) is a circular descriptor table in system
  memory: 16, 32, 64 or 128 KiB, entries of a system address, a frame-buffer offset and a
  command. `BM_SYSTEM_TABLE` (0x1_6F) is the system-memory transfer table. The public RRG
  gives the register interface but not the descriptor bit layout; that layout is in the
  confidential programmer's guide and in the open drivers. v1 should use direct FIFO
  writes and defer the bus-master ring until the descriptor layout has been confirmed on
  hardware.
- The command-stream encoding used by the Mesa mach64 DRI is a 32-bit header
  `(count << 16) | register_index` followed by `count` data DWORDs. This is the encoding
  the driver can use for a future command buffer; it is reconstructed from Mesa, not from a
  public ATI specification, so it is a Stage 2 hardware check.

---

## 5. Complete feature matrix

### 5.1 How to read the matrix

`Chip` is what the silicon supports, independent of the board:

- `Y` = full hardware support.
- `P` = partial, limited or only through a specific path (explained in the row).
- `N` = not implemented by the silicon.

`Board` is what the M4753 wires or exposes:

- `W` = wired and usable.
- `M` = wired but mirror-only or otherwise restricted by the product.
- `A` = not wired/exposed on this board.
- `?` = unknown; must be established by inspection or hardware test.

`Gate` is the cheapest test that can prove the row:

- `S` = software/unit test with a register model or a host-side reference.
- `Q` = QEMU generic gate (PCI, BAR, framebuffer plumbing; not Mach64).
- `E` = emulator cross-check (86Box or MAME, manual, not CI).
- `H` = real PowerBook hardware.
- `B` = board inspection or schematic check; the function exists in silicon but the M4753
  does not expose it, so there is nothing to drive on this board.

`Q` appears only in the Stage 0 gates in §9. No Mach64 row below uses `Q`, because QEMU's
`ati-vga` is a Rage 128, not a Mach64. `E` is available as a manual cross-check for 2D rows
but is not a CI gate, so it is not listed in the Gate column. The Gate column therefore
contains `S` and `H`, and every `H` gate must run on the PowerBook.

A row with `Chip=Y` and `Board=A` is a chip capability the board does not expose. It stays
in the matrix and is not moved to the non-goals. A row with `Chip=N` is a genuine silicon
limitation; those are the only items that may appear in §5.9. A `Gate —` row is
silicon-absent (`Chip N`); a board-absent function (`Board A`) uses `B`, a board
inspection, and is not dropped.

### 5.2 Display

| Function | Chip | Registers | Driver path | Board | Gate |
|---|---|---|---|---|---|
| CRTC1 timing: H/V total, display, sync start/width, polarity | Y | 0x00–0x03, `CRTC_GEN_CNTL` 0x07 | `GpuDevice::set_mode`; PLL + CRTC program | W | H |
| Interlace and double scan | Y | `CRTC_GEN_CNTL` bits 0–1 | Mode flags | W | H |
| Overscan borders | Y | 0x10–0x12 | Mode flags | W | H |
| Panning and scrolling | Y | `CRTC_OFF_PITCH` 0x05 | `Surface::pan` | W | H |
| Page flip and double buffer | Y | `CRTC_OFF_PITCH`, `CRTC_INT_CNTL`, `HW_DEBUG.SEL_VBLANK_DBL_BUF` | `Surface::flip` on VBLANK | W | H |
| Pixel formats 8/16/24/32 bpp | Y | `CRTC_GEN_CNTL.CRTC_PIX_WIDTH` | Mode set | W | H |
| Packed 24 bpp | P | `CRTC_PIX_WIDTH=5`, GUI 24-bit rotation (`DST_CNTL.DST_24_ROT_EN`) | 2D/3D 24-bit path | W | H |
| PLL and pixel clock | Y | `CLOCK_CNTL` 0x24 + PLL registers | Mode set; copy firmware values first | W | H |
| Four pixel-clock settings | Y | `CLOCK_CNTL.CLOCK_SEL` | Mode set | W | H |
| Palette, 256×24 | Y | `DAC_REGS` 0x30, `DAC_CNTL` 0x31 | `GpuDevice::set_palette` | W | H |
| Gamma correction | Y | `DAC_CNTL.DAC_TYPE`, 24-bit palette | `set_gamma` | W | H |
| 8-bit DAC / 6-bit DAC | Y | `DAC_CNTL.DAC_8BIT_EN` | Mode set | W | H |
| Palette snooping / secondary palette | Y | `DAC_CNTL.PALETTE_ACCESS_CNTL`, `PALETTE2_SNOOP_EN` | Multihead/secondary | M | H |
| Hardware cursor 64×64×2 | Y | `CUR_*` 0x18–0x1C; `GEN_TEST_CNTL.GEN_CUR_ENABLE` | `Cursor::set_image`, `Cursor::move` | W | H |
| Hardware icon 128×128 | Y | `CUR_*`, icon enable | Same path; VGA-era use | W | H |
| Cursor colours: two 24-bit colours, transparent, complement | Y | `CUR_CLR0/1`, 2-bit pixel codes | `Cursor::set_image` | W | H |
| DPMS: HSYNC/VSYNC disable, DAC power-down | Y | `CRTC_GEN_CNTL.CRTC_HSYNC_DIS/VSYNC_DIS`, `DAC_CNTL.DAC_PDWN` | `GpuDevice::dpms` | W | H |
| DPMS: panel power, bias, digital power | Y | LCD `POWER_MANAGEMENT` index 8 | `GpuDevice::dpms`; coordinate PMU | W | H |
| LCD panel interface: LVDS or digital | Y | LCD indexed registers, `CONFIG_PANEL` | `Panel::init` | W | H |
| LCD panel type and depth | Y | `CONFIG_PANEL`, `CONFIG_STAT0.PANEL_ID` | Probe and mode set | W | H |
| LCD horizontal/vertical stretching, filtering, centring | Y | LCD `HORZ_STRETCHING`/`VERT_STRETCHING` | `Panel::set_scaling` | W | H |
| Ratiometric expansion | Y | LCD stretch registers | `Panel::set_scaling` | W | H |
| Backlight modulation | Y | LCD `POWER_MANAGEMENT.BLON`, `ZVGPIO.BL_PWM_LEVEL` | `Backlight::set`; PMU path | W | H |
| Panel power sequencing | Y | LCD `POWER_MANAGEMENT`, `PWRSEQ_DELAY` | `Panel::power` | W | H |
| Secondary CRTC timing and scanout | Y | `CRTC2_*` 0x00–0x03, 0x17; `CRTC_GEN_CNTL.CRTC2_ENABLE` | `GpuDevice::set_mode(secondary)` | M | H |
| Secondary 2D/3D destination | Y | `USR1_DST_OFF_PITCH`, `USR2_DST_OFF_PITCH`, `DP_SET_GUI_ENGINE2` | Secondary surface path | M | H |
| Secondary display FIFO | Y | `DSP2_CONFIG`, `DSP2_ON_OFF` | Secondary mode set | M | H |
| DAC source select (CRTC1/CRTC2) | Y | `DAC_CNTL.DAC1_CLK_SEL` | `set_mode` | M | H |
| External VGA mirroring | Y | CRTC2 + DAC select; board mirror | `set_mode(mirror)` | M | H |
| Independent external scanout | Y | CRTC2 + `LCD_SRC_SEL` | Not offered; chip test only | ? | H |
| DDC/EDID via hardware I2C | Y | Block I/O `I2C_CNTL_0` 0x0F, `I2C_CNTL_1` 0x2F | `Ddc::read_edid` | ? | H |
| DDC via GP_IO bit-bang | Y | `GP_IO` 0x1E | Fallback | ? | H |
| TV-out: NTSC/PAL, composite/S-video/RGB | Y | `TV_OUT_INDEX` 0x1D, `TVO_CNTL` 0x3F, indexed TV registers | `Tv::set_mode`, `Tv::enable` | W | H |
| TV-out: independent H/V scaling, flicker filter | Y | TV scaler/timing registers | `Tv::set_scaling` | W | H |
| TV-out: closed caption, VBI | Y | TV VBI registers | Deferred | W | H |
| TV-out: Macrovision | P | TV modulator control; bonding option | Deferred/not enabled | ? | H |
| Genlock: CRT-to-video snapshots | Y | `SNAPSHOT_*` 0x1_1C–0x1_2F | Not exposed | A | B |
| VGA text and graphics modes | Y | VGA registers, VGA aperture | Firmware/legacy only | W | H |
| VGA aperture and palette snooping | Y | `CONFIG_CNTL`, `CONFIG_STAT0`, `DAC_CNTL` | Preserve; do not disturb | W | H |
| PCI power management D1/D2/PME | Y | PCI PM capability 0x5C | `Power::set_state` | W | H |

The TV-out rows apply to the 13.3-inch and 14.1-inch models; the 12.1-inch model has no
S-video connector. The panel rows apply to all three models, with different panel types and
memory sizes.

### 5.3 2D

| Function | Chip | Registers | Driver path | Board | Gate |
|---|---|---|---|---|---|
| Solid fill | Y | `DP_FRGD_CLR` 0xB1, `DST_*` 0x40–0x47, `DP_PIX_WIDTH` 0xB4, `DP_MIX` 0xB5 | `Surface::fill` | W | S,H |
| Screen-to-screen blit | Y | `SRC_*` 0x60–0x6D, `DST_*`, `DP_SRC` 0xB6 | `Surface::blit` | W | S,H |
| Host blit | Y | `HOST_DATA` 0x80–0x8F, `HOST_CNTL` 0x90 | `Surface::upload` (slow path) | W | S,H |
| Direct CPU aperture write | Y | Frame-buffer aperture, `MEM_CNTL` endian bits | `Surface::map` fast path | W | H |
| Line draw (Bresenham) | Y | `DST_BRES_LNTH` 0x48, `DST_CNTL` 0x4C | `Surface::line` | W | S,H |
| Polygon and rectangle fill | Y | `DST_CNTL.DST_POLYGON_EN`, `DST_WIDTH/HEIGHT` | `Surface::fill_polygon` | W | S,H |
| Trapezoid fill | Y | `DST_CNTL`, `TRAIL_BRES_*` | `Surface::fill_trapezoid` | W | S,H |
| Pattern: mono 8×8 | Y | `PAT_REG0/1` 0xA0/0xA1, `PAT_CNTL` 0xA2 | `Surface::fill_pattern` | W | S,H |
| Pattern: colour 4×2 and 8×1 | Y | `PAT_REG0/1`, `PAT_CNTL` | `Surface::fill_pattern` | W | S,H |
| 8×8×8 brush | Y | `SRC_CNTL.SRC_8x8x8_BRUSH`, linear source | `Surface::fill_brush` | W | H |
| ROPs: 16 ternary functions plus (D+S)/2 | Y | `DP_MIX` 0xB5 | `Surface::rop` | W | S,H |
| Scissoring | Y | `SC_*` 0xA8–0xAD | `Surface::set_clip` | W | S,H |
| Colour compare / transparent blit | Y | `CLR_CMP_*` 0xC0–0xC2 | `Surface::blit_keyed` | W | S,H |
| Write mask | Y | `DP_WRITE_MSK` 0xB2 | `Surface::mask` | W | S,H |
| Monochrome expansion | Y | `DP_SRC.DP_MONO_SRC`, `DP_PIX_WIDTH` 1 bpp | `Surface::blit_mono` | W | S,H |
| Colour expansion 1 bpp → 8/16/32 | Y | `DP_PIX_WIDTH`, `DP_SRC` | `Surface::blit_mono` | W | S,H |
| Arbitrary format conversion (8↔16↔32) | N | — | Software or 3D scaler | — | — |
| 8/16/24/32 bpp destination | Y | `DP_PIX_WIDTH.DP_DST_PIX_WIDTH` | `Surface::set_format` | W | S,H |
| Packed 24 bpp | P | `DST_CNTL.DST_24_ROT_EN`, 8 bpp datapath | `Surface::set_format` | W | H |
| Big-endian pixel order | Y | `DP_PIX_WIDTH.DP_BYTE_PIX_ORDER`, `HOST_CNTL.HOST_BIG_ENDIAN_EN`, `MEM_CNTL` aperture endian | `Surface::set_endian` | W | H |
| Fast fill / auto fast fill | Y | `GUI_TRAJ_CNTL.FAST_FILL_EN`, `HW_DEBUG` | `Surface::fill` | W | H |
| Double buffering | Y | `CRTC_OFF_PITCH`, two surfaces | `Surface::flip` | W | H |
| Masked blit / virtual sprites | Y | `CLR_CMP_*`, `DP_WRITE_MSK`, `SRC_CNTL` | `Surface::blit_keyed` | W | H |
| Context chaining | Y | FIFO stream, `DP_SET_GUI_ENGINE`/`2` | `CommandQueue` batch | W | H |
| 2D stretch/scale blit | N | — | Overlay/LCD/TV scaler, not GUI blit | — | — |
| Command FIFO | Y | `FIFO_STAT` 0xC4, `GUI_STAT` 0xCE, `GUI_CNTL` 0x1_5E | `CommandQueue::submit` | W | H |
| FIFO error detect and recover | Y | `FIFO_STAT.FIFO_ERR`, `GEN_TEST_CNTL.GEN_GUI_RESETB` | `Engine::recover` | W | H |
| GUI bus-master table | Y | `BM_GUI_TABLE` 0x1_6E, `BM_GUI_TABLE_CMD` 0x93 | Deferred ring path | W | H |
| System bus-master table | Y | `BM_SYSTEM_TABLE` 0x1_6F, `BM_COMMAND` 0x1_62 | Deferred blit path | W | H |
| Draw-engine bus master | Y | `BM_HOSTDATA` 0x91, `BM_ADDR`/`BM_DATA` 0x92 | Deferred | W | H |
| Block transfers up to 4096 bytes per descriptor | Y | `BM_COMMAND`, descriptors | Deferred | W | H |

### 5.4 3D fixed function

The public RRG lists the Setup Engine and texture register names and addresses but omits
their descriptions. The 2D chapter still describes shared registers such as `DST_CNTL`,
`Z_CNTL` and `ALPHA_TST_CNTL`. The "Behaviour source" column records what the row is
reconstructed from.

| Function | Chip | Registers | Behaviour source | Driver path | Board | Gate |
|---|---|---|---|---|---|---|
| Triangle setup, floating point | Y | `VERTEX_1/2/3_*` 0x1_90–0x1_A6, `ONE_OVER_AREA_UC` 0x1_C0, `SETUP_CNTL` 0x1_C1 | Mesa mach64 DRI, GCS 2.2.3 | `Triangles::submit` | W | H |
| 1.0–1.2 million triangles/s | Y | Setup engine | GCS 2.2.3 | Benchmark | W | H |
| Flat shading | Y | `SETUP_CNTL.FLAT_SHADE_*` | Mesa | `State::flat` | W | H |
| Gouraud shading | Y | `VERTEX_n_ARGB`, RGB interpolators 0xF0–0xF8 | Mesa | `State::shade` | W | H |
| Specular shading | Y | `VERTEX_n_SPEC_ARGB`, `ALPHA_TST_CNTL.SPECULAR_LIGHT_EN`, specular interpolators 0xE7–0xEF | Mesa | `State::specular` | W | H |
| Points | P | 3D primitive claim in GCS; no distinct point setup register in public RRG | GCS 2.2.12; Mesa renders points as quads | `Triangles::point` (quad) | W | H |
| Lines | P | 3D primitive claim; trapezoid/line path in RRG-G02700 | GCS 2.2.12; Mesa uses 2D line or quads | `Triangles::line` | W | H |
| Triangles, quads, lists, strips | Y | `SETUP_CNTL.SET_UP_CONTINUE`, vertex registers | GCS 2.2.12; Mesa | `Triangles::submit` | W | H |
| Z-compare blit | P | `Z_CNTL.Z_SRC_2D`, 2D blit path | GCS 2.2.12; register path unclear | `Surface::blit_z` | W | H |
| Texture mapping, 2D | Y | `TEX_0..10_OFF` 0x70–0x7A, `TEX_SIZE_PITCH` 0xDC, `TEX_CNTL` 0xDD, S/T/W interpolators | Mesa, GCS | `Textures::bind` | W | H |
| Mipmaps, 11 levels | Y | `TEX_0..10_OFF`, `TEX_CNTL.MIP_MAP_DISABLE` | Mesa | `Textures::mipmap` | W | H |
| Perspective correction | Y | W interpolators, `ONE_OVER_AREA`/`_UC` | Mesa, GCS | `Textures::perspective` | W | H |
| Nearest filtering | Y | `TEX_CNTL.TEX_BLEND_FCN_NEAREST` | Mesa | `Textures::filter` | W | H |
| Bilinear filtering | Y | `SCALE_3D_CNTL.BILINEAR_TEX_EN`, `TEX_BLEND_FCN_LINEAR` | Mesa | `Textures::filter` | W | H |
| Trilinear filtering, single pass | Y | `TEX_BLEND_FCN_TRILINEAR` | Mesa, GCS | `Textures::filter` | W | H |
| Texture formats: RGBA32, RGBA16, RGB16, RGB8, ARGB4444, YUV444 | Y | `DP_PIX_WIDTH.DP_SCALE_PIX_WIDTH`, `TEX_CNTL` | Mesa, GCS | `Textures::format` | W | H |
| Compressed: YUV422, CI4, CI8 | Y | `TEX_CNTL`, palette | Mesa, GCS | `Textures::format` | W | H |
| VQ compression up to 8:1 | P | `TEX_CNTL.TEX_VQ_COMPRESSED` | GCS; no open driver support found | Deferred | W | H |
| 4 KiB texture cache | Y | `TEX_CNTL.TEX_CACHE_*`, `SCALE_3D_CNTL.TEX_CACHE_*` | Mesa, GCS | `Textures::cache` | W | H |
| Texture cache flush | Y | `TEX_CNTL.TEX_CACHE_FLUSH` | Mesa | `Textures::flush` | W | H |
| Texture palettes | Y | `TEX_PALETTE_INDEX` 0xD0, `TEX_PALETTE` 0xDF | Mesa | `Textures::palette` | W | H |
| Texture compositing / 2 texture units | Y | `TEX_CNTL.TEXTURE_COMPOSITE`, `SECONDARY_*` 0x56–0x5F, `VERTEX_n_SECONDARY_*` | Mesa `multitex`, GCS | `Textures::composite` | W | H |
| Texture lighting (replace/modulate/alpha decal) | Y | `SCALE_3D_CNTL.TEX_LIGHT_FCN_*` | Mesa | `State::tex_env` | W | H |
| Chroma key / texture key | Y | `CLR_CMP_*`, `TEX_CNTL.TEX_MAP_AEN` | Mesa, GCS | `Textures::key` | W | H |
| Video textures (YUV) | Y | `DP_PIX_WIDTH` YUV widths, `TEX_CNTL` | GCS | `Textures::yuv` | W | H |
| LOD bias | Y | `TEX_CNTL.LOD_BIAS` | Mesa | `Textures::lod_bias` | W | H |
| Texture morphing / reflections / shadows / spotlights | P | Combinations of the above; no dedicated registers | GCS effects list | `State` combinations | W | H |
| Alpha blending: 8 source and 8 destination factors | Y | `SCALE_3D_CNTL.ALPHA_BLEND_SRC/DST` | Mesa | `State::blend` | W | H |
| Alpha blend equation: add only | P | No subtract/rev-subtract; Mesa falls back | Mesa fallback list | `State::blend` | W | H |
| Alpha test: 8 compare functions, reference alpha | Y | `ALPHA_TST_CNTL` 0x54 | Mesa | `State::alpha_test` | W | H |
| 16-bit Z buffer | Y | `Z_OFF_PITCH` 0x52, `Z_CNTL` 0x53 | Mesa, GCS | `Depth::set` | W | H |
| Z compare: 8 functions, Z mask | Y | `Z_CNTL.Z_TEST_*`, `Z_MASK_EN` | Mesa | `Depth::set` | W | H |
| Fog: linear, per-vertex | Y | `SCALE_3D_CNTL` fog enable, `FOG_*` 0xFC–0xFE, `DP_FOG_CLR` 0xB1 | Mesa | `State::fog` | W | H |
| Dithering, 16 bpp | Y | `SCALE_3D_CNTL.DITHER_*`, `RED_DITHER_MAX`, `ROUND_EN` | Mesa, GCS | `State::dither` | W | H |
| Edge anti-aliasing | Y | `SCALE_3D_CNTL.EDGE_ANTI_ALIAS` | Mesa, GCS | `State::aa` | W | H |
| Multisample anti-aliasing | N | — | GCS says edge AA only | — | — | — |
| Scissors | Y | `SC_*` 0xA8–0xAD | Mesa | `State::clip` | W | H |
| Clear / fast fill | Y | 2D `DP_FRGD_CLR`, `DST_*`; `Z_CNTL.Z_SRC_2D` | Mesa, GCS | `Surface::clear` | W | H |
| Sub-pixel and sub-texel accuracy | Y | Vertex X/Y 14.2, Z 1.15, texture increments | Mesa, GCS | `Triangles` | W | H |
| 32-bit colour rendering (RGBA32) | Y | `DP_PIX_WIDTH`, `SCALE_3D_CNTL` | GCS | `Surface` format | W | H |
| 32-bit Z buffer | N | — | GCS says 16-bit Z | — | — | — |
| Stencil buffer | N | — | No stencil registers | — | — | — |
| Cube maps / 3D textures | N | — | 2D mipmaps only | — | — | — |
| Programmable shaders / GLSL | N | — | Fixed function | — | — | — |
| AGP texture from system memory | N | `TEX_CNTL.TEX_SRC_AGP` exists but AGP absent on PCI | GCS 2.4.1 | — | — | — |
| Motion compensation | N | AGP variants only | GCS 2.4.1 | — | — | — |
| Full-screen/window double buffering | Y | `CRTC_OFF_PITCH`, two surfaces | GCS | `Surface::flip` | W | H |

### 5.5 Video

| Function | Chip | Registers | Driver path | Board | Gate |
|---|---|---|---|---|---|
| Overlay window position and size | Y | `OVERLAY_Y_X_START/END` 0x1_00/0x1_01, `OVERLAY_EXCLUSIVE_*` 0x1_16/0x1_17 | `Overlay::configure` | W | H |
| Overlay scaling, H/V | Y | `OVERLAY_SCALE_INC` 0x1_08, `OVERLAY_SCALE_CNTL` 0x1_09 | `Overlay::scale` | W | H |
| YUV422 packed (VYUY, YVYU) | Y | `VIDEO_FORMAT` 0x1_12, `DP_PIX_WIDTH` | `Overlay::format` | W | H |
| YUV410/YUV420 planar | Y | `SCALER_BUF0/1_OFFSET_U/V` 0x1_75–0x1_78, scaler | `Overlay::format` | W | H |
| Colour-space conversion and temperature | Y | `SCALER_COLOUR_CNTL` 0x1_54, `DP_PIX_WIDTH.DP_CONVERSION_TEMP` | `Overlay::colour` | W | H |
| Graphics/video keying | Y | `OVERLAY_VIDEO_KEY_CLR/MSK`, `OVERLAY_GRAPHICS_KEY_CLR/MSK`, `OVERLAY_KEY_CNTL` | `Overlay::key` | W | H |
| Brightness and saturation | Y | `SCALER_COLOUR_CNTL` | `Overlay::colour` | W | H |
| Horizontal scaler coefficients | Y | `SCALER_H_COEFF0`–`4` 0x1_55–0x1_59 | `Overlay::coefficients` | W | H |
| De-interlacing / flicker filter | Y | Scaler and TV filter registers | `Overlay::deinterlace` | W | H |
| Multi-stream video | Y | Two buffers/streams, front/back scaler | Deferred | W | H |
| Video mirror/flip | Y | Scaler registers | `Overlay::flip` | W | H |
| TV-out (see display) | Y | TV indexed block | `Tv` | W | H |
| S-video output | Y | TV DAC | `Tv` | W | H |
| Composite output | Y | TV DAC | `Tv` | A | H |
| Video capture: one-shot | Y | `CAPTURE_*` 0x1_10–0x1_1A, 0x1_20–0x1_22, `ONESHOT_BUF_OFFSET` | `Capture` (future) | A | B |
| Video capture: continuous, two buffers | Y | `CAPTURE_BUF0/1_OFFSET`, `CAPTURE_CONFIG` | `Capture` (future) | A | B |
| Video capture: VBI | Y | `VBI_START_END`, `VBI_*` TV registers | `Capture` (future) | A | B |
| ZV port (CCIR601, PCMCIA) | Y | Capture registers | `Capture` (future) | ? | H |
| AMC 16-bit video port | Y | `MPP_*` 0x1_30–0x1_33 | Not exposed | A | B |

S-video is present on the 13.3-inch and 14.1-inch models only; the 12.1-inch model has no
S-video connector. Composite video has no separate connector on any M4753 model; it is
available only through an external S-video-to-composite adapter, if at all.

### 5.6 DMA, FIFO and interrupts

| Function | Chip | Registers | Driver path | Board | Gate |
|---|---|---|---|---|---|
| Command FIFO, 64/128/192 entries | Y | `FIFO_STAT` 0xC4, `GUI_STAT` 0xCE, `GUI_CNTL` 0x1_5E | `CommandQueue` | W | H |
| FIFO occupancy and free count | Y | `FIFO_STAT`, `GUI_STAT[15:0]` | `CommandQueue` | W | H |
| FIFO error detect | Y | `FIFO_STAT.FIFO_ERR` | `Engine::recover` | W | H |
| FIFO error recovery | Y | `GEN_TEST_CNTL.GEN_GUI_RESETB` | `Engine::recover` | W | H |
| GUI bus-master descriptor ring | Y | `BM_GUI_TABLE` 0x1_6E, `BM_GUI_TABLE_CMD` 0x93 | Deferred | W | H |
| System bus-master table | Y | `BM_SYSTEM_TABLE` 0x1_6F, `BM_COMMAND` 0x1_62 | Deferred | W | H |
| Draw-engine bus master | Y | `BM_HOSTDATA` 0x91, `BM_ADDR`/`BM_DATA` 0x92 | Deferred | W | H |
| Block transfer / scatter-gather | Y | `BM_COMMAND`, descriptors up to 4096 bytes | Deferred | W | H |
| Vertex/command DMA | P | Bus-master stream carrying register packets | Deferred; Mesa evidence | W | H |
| Bus-master reset | Y | `BUS_CNTL.BUS_MSTR_RESET` | `Engine::reset` | W | H |
| Bus-master enable/disable | Y | `BUS_CNTL.BUS_MASTER_DIS` | `Engine::start/stop` | W | H |
| VBLANK interrupt | Y | `CRTC_INT_CNTL` bit 0/1/2 | `Irq` event | W | H |
| VLINE interrupt | Y | `CRTC_INT_CNTL` bit 3/4/5 | `Irq` event | W | H |
| CRTC2 VBLANK/VLINE interrupt | Y | `CRTC_INT_CNTL` `CRTC2_VBLANK_INT_*`, `CRTC2_VLINE_INT_*` | Secondary path | M | H |
| Overlay end-of-frame interrupt | Y | `CRTC_INT_CNTL` `OVERLAY_EOF_INT_*` | `Irq` event | W | H |
| Capture buffer 0/1 interrupt | Y | `CRTC_INT_CNTL` `CUPBUF0_INT_*`, `CUPBUF1_INT_*` | Future | A | B |
| One-shot capture interrupt | Y | `CRTC_INT_CNTL` `ONESHOT_CAP_INT_*` | Future | A | B |
| Bus-master EOL interrupt | Y | `CRTC_INT_CNTL` `BUSMASTER_EOL_INT_*` | Deferred | W | H |
| GP IO interrupt | Y | `CRTC_INT_CNTL` `GP_INT_*` | Not used | ? | H |
| FIFO error interrupt | P | Optional/debug; enable bit not identified in the public text | Poll only | W | H |
| Interrupt acknowledge | Y | `CRTC_INT_CNTL` write-1-to-clear bits | `Irq::ack` | W | H |

### 5.7 Power

| Function | Chip | Registers | Driver path | Board | Gate |
|---|---|---|---|---|---|
| DPMS | Y | `CRTC_GEN_CNTL`, `DAC_CNTL` | `Power::dpms` | W | H |
| PLL sleep | Y | `PLL_GEN_CNTL.PLL_SLEEP` | `Power::pll_sleep` | W | H |
| DAC power-down | Y | `DAC_CNTL.DAC_PDWN` | `Power::dac_off` | W | H |
| Panel bias/backlight/digital power | Y | LCD `POWER_MANAGEMENT` | `Panel::power` | W | H |
| Panel power sequence timing | Y | LCD `PWRSEQ_DELAY` | `Panel::power` | W | H |
| Dynamic clock switching | Y | `CONFIG_STAT0.CFG_CLOCK_EN`, PLL | `Power::clock` | W | H |
| Self-refresh and 32 kHz refresh | Y | LCD `POWER_MANAGEMENT` | `Power::suspend` | W | H |
| ACPI/PCI PM D1/D2 | Y | PCI PM capability 0x5C, `POWER_MANAGEMENT` | `Power::set_state` | W | H |
| PMU backlight and battery | — | PMU, not ATI | `Pmu::backlight` (separate cell) | W | H |
| PMU sleep/wake | — | PMU | Coordinate with the power cell | W | H |

### 5.8 Legacy

| Function | Chip | Registers | Driver path | Board | Gate |
|---|---|---|---|---|---|
| VGA text modes | Y | VGA CRTC/sequencer/graphics/attribute | Preserve; not a v1 path | W | H |
| VGA graphics modes | Y | VGA registers, VGA aperture | Preserve | W | H |
| VGA palette and DAC | Y | VGA DAC 0x3C6–0x3C9 | Preserve | W | H |
| VGA I/O decode | Y | 0x3B4/0x3D4, 0x3C0–0x3CF, 0x46E8, 0x102 | Do not claim | W | H |
| Firmware coexistence | — | OF display node, VGA straps | Save/restore mode on exit | W | H |

### 5.9 What is genuinely absent

Only silicon limitations belong here:

- programmable vertex and fragment shaders, GLSL, SPIR-V, Vulkan;
- 32-bit Z, stencil, multisample AA, cube maps, 3D textures;
- AGP texturing and motion compensation on the PCI variant;
- arbitrary pixel-format conversion in the 2D engine (1 bpp expansion only);
- 2D stretch blits in the GUI engine (the scaler is a separate path);
- a full IOMMU for the bus master.

Board-absent functions (video capture input, AMC, genlock, composite connector,
independent extended desktop) are **not** silicon limitations. They stay in the matrix as
`Board=A`, `Board=?` or `Board=M`, and the driver must not report them as unsupported by the chip.

### 5.10 Matrix summary

| Class | Rows | Chip `Y` | Chip `P` | Chip `N` | Chip n/a | Board `W` | Board `M` | Board `A` | Board `?` | Board n/a |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Display | 40 | 38 | 2 | 0 | 0 | 29 | 6 | 1 | 4 | 0 |
| 2D | 31 | 28 | 1 | 2 | 0 | 29 | 0 | 0 | 0 | 2 |
| 3D | 47 | 34 | 6 | 7 | 0 | 40 | 0 | 0 | 0 | 7 |
| Video | 19 | 19 | 0 | 0 | 0 | 13 | 0 | 5 | 1 | 0 |
| DMA/FIFO/IRQ | 21 | 19 | 2 | 0 | 0 | 17 | 1 | 2 | 1 | 0 |
| Power | 10 | 8 | 0 | 0 | 2 | 10 | 0 | 0 | 0 | 0 |
| Legacy | 5 | 4 | 0 | 0 | 1 | 5 | 0 | 0 | 0 | 0 |
| **Total** | **173** | **150** | **11** | **9** | **3** | **143** | **7** | **8** | **6** | **9** |

The counts are row counts from §5.2–5.8. `Chip n/a` is a board-level row (PMU or firmware
coexistence); `Board n/a` is a silicon limitation where board wiring does not apply. The
important number is the `Board A` and `Board ?` columns: every one of those rows is a
hardware-inspection task, not a reason to drop the function.

---

## 6. QEMU, emulators and real hardware

### 6.1 QEMU `ati-vga` is a different chip

QEMU's `ati-vga` device (QEMU 11.1.1, `hw/display/ati.c`, GPL-2.0-or-later) is a **Rage 128
Pro/RV100** emulation. Its header says: "We only aim for Rage 128 Pro (and some RV100) and
2D only at first ... No 3D at all yet". It models VGA, a linear framebuffer, a Rage 128 2D
engine and a VBLANK interrupt. It does not model the Mach64 register layout, the Mach64
2D engine, the LT Pro FIFO, the LT Pro 3D setup engine, the LCD panel, the TV encoder, the
overlay/scaler, the bus-master descriptor ring or the PMU.

| Feature | QEMU `ati-vga` | Real Rage LT Pro |
|---|---|---|
| Chip identity | Rage 128 Pro (`0x5046`) or RV100 (`0x5159`) | Rage LT Pro (`0x4C49`/`4C50`/`4C51`) |
| Register layout | Rage 128 | Mach64 |
| 2D engine | Rage 128 subset | Mach64 full 2D |
| 3D engine | None | Rage Pro setup + raster |
| Command FIFO | Rage 128 style | 64/128/192-entry Mach64 FIFO |
| Bus-master ring | None | GUI/system tables |
| Overlay/scaler | None | Full |
| LCD panel | None | Full |
| TV-out | None | Full |
| DDC/I2C | Some | Hardware I2C + GP_IO |
| Interrupts | VBLANK only | VBLANK, VLINE, overlay, capture, BM EOL, GP |
| PMU/backlight | None | Board-level |

**Consequence:** the Mach64 driver must not bind to QEMU's `ati-vga`. A QEMU gate can prove
that the PCI enumerator finds a display device, assigns BARs and maps a linear framebuffer,
and that the cell framework delivers MMIO and an interrupt. It cannot prove a single
Mach64 register. Do not use QEMU 2D results as evidence for the Mach64 2D engine.

### 6.2 86Box and MAME

86Box (`src/video/vid_ati_mach64*.c`, GPL-2.0) models Mach64 GX/CT/VT/VT2/VT3 and the
3D Rage II+ (GTB), including a 2D engine, the command FIFO, the hardware cursor, the
scaler and, for the GTB, a trapezoid 3D engine. It cites the public RRG and is a useful
**independent reconstruction** of Mach64 2D behaviour. It does not model the LT Pro: its
VT3 device is PCI ID `0x5655`, its 3D is the Rage II+ engine, and it has no LT Pro LCD,
TV or PMU.

MAME (`src/devices/video/atirage.cpp`, BSD-3-Clause) models mach64 VT, Rage II, Rage IIc,
Rage II+ and Rage Pro (`0x4750`) with a 2D engine and a 32-entry FIFO report. It does not
model 3D, and it does not model the LT Pro.

| Oracle | 2D | 3D | FIFO | LCD/TV/PMU | Use |
|---|---|---|---|---|---|
| QEMU `ati-vga` | Rage 128 only | No | No | No | PCI/BAR/framebuffer plumbing only |
| 86Box Mach64 | Yes, Mach64 subset | Rage II+ only | Yes | No | Manual 2D cross-check |
| MAME `atirage` | Yes, Rage Pro 2D | No | Yes (32 reported) | No | Manual 2D cross-check |
| Real PowerBook | Yes | Yes | Yes | Yes | The only authoritative oracle |

Neither 86Box nor MAME is a CI gate for this driver. They are reference implementations to
compare register sequences and readback values against when a hardware run is inconvenient.
Their licences (GPL-2.0 and BSD-3-Clause) mean their code is cited, never copied.

### 6.3 What must be hardware-tested

The following have no emulator coverage and must be tested on the M4753:

1. **Register endianness** on the PowerPC host bridge (scratch-register pattern).
2. **Aperture endianness**: `MEM_CNTL` big-endian 16/32 bpp modes and the big-endian
   aperture; direct CPU pixel writes and reads.
3. **Mode setting**: PLL values, CRTC timing, panel sync, the shadow-register H-sync bug
   documented by Xorg (panel may not sync if shadow and non-shadow H-sync differ).
4. **Panel type and depth**: `CONFIG_PANEL`, `PANEL_ID`, 18-bit vs 24-bit, LVDS vs digital.
5. **Backlight**: PMU vs ATI `BLON`/`BL_PWM_LEVEL`; brightness range; panel power sequence.
6. **Hardware cursor**: 64×64 and 128×128, colours, transparency, complement, position
   clamping at negative coordinates.
7. **DPMS**: HSYNC/VSYNC disable and panel power, with the monitor and the panel.
8. **Page flip**: tear-free flip on VBLANK at 60 Hz.
9. **2D engine**: fill, blit, line, pattern, ROP, clip, colour compare, write mask, mono
   expansion, 8/16/24/32 bpp, packed 24, FIFO backpressure, FIFO error recovery.
10. **3D engine**: every state in §5.4, the fixed-point vertex formats, the
    `ONE_OVER_AREA_UC` trigger, strip/fan continuation, texture formats, mip levels,
    filtering, multitexture, blending, Z, fog, dither, AA, scissors, clear.
11. **DMA**: descriptor layout, block transfers, cache coherency, bus-master EOL interrupt.
12. **Interrupts**: VBLANK, VLINE, overlay EOF, sharing with other PCI devices, ack.
13. **Overlay**: YUV422 and planar, scaling, keying, brightness/saturation, tear-free.
14. **TV-out**: NTSC and PAL, S-video, colour bars, overscan, flicker filter.
15. **DDC/EDID**: external monitor, hot-plug behaviour (or absence).
16. **CRTC2**: mirroring and independent scanout, if the board supports it.
17. **Power**: suspend/resume with the PMU, panel power, PLL sleep, DAC power-down.
18. **VGA coexistence**: leave VGA usable for firmware/legacy, restore on exit.

---

## 7. Driver cell and device-capability interface

### 7.1 The object model

The GPU is a global device object in the one-logical-computer model
([`system.md`](../internals/system.md) §7). The driver cell is its home in the single-node case;
on a mesh, the object's home is the node with the GPU, and remote clients submit command
buffers and fences over the fabric. A remote client never maps the GPU's BARs.

```text
GpuDevice (global object, home = node with the GPU)
  |-- info: chip, PCI ID, VRAM, modes, features
  |-- Surface: VMO-backed pixels, format, stride, residency
  |-- CommandQueue: ordered submission, fences
  |-- Display: scanout, flip, DPMS, mode
  |-- Cursor: image, position, visibility
  |-- Overlay: window, buffers, scaler, keying
  |-- Tv: standard, enable
  |-- Ddc: EDID read
  |-- Power: DPMS, panel, backlight
```

The cell is the only writer to the ATI register aperture. A client can hold a `Surface`
capability with `draw`, `present`, `read` or `map` rights; it cannot hold an MMIO capability
for the GPU unless the framework explicitly grants a raw-device capability for debugging
(the default is no).

### 7.2 Capabilities

The cell receives the standard driver capabilities from
[`driver-interface.md`](../programming/driver-interface.md):

| Capability | Use in this driver |
|---|---|
| `Device` | Identifies the ATI instance; carries the MMIO range, IRQ and DMA window rights |
| `Mmio` | BAR0 linear aperture (frame buffer + register blocks), BAR1 block I/O, BAR2 auxiliary register aperture |
| `Irq` | PCI INTA# via the platform interrupt controller |
| `DmaWindow` | Physically contiguous, cache-managed buffers for the command ring, textures and readback |
| `Port` | IRQ event delivery and completion notifications |

The framework programs the PCI interrupt routing and the DMA window; the cell never touches
the interrupt controller, the PCI config mechanism after probe, or another device's BARs.
On the M4753 there is no IOMMU; the DMA window is a policy grant, not a hardware boundary.
The cell pins and validates every address it gives the ATI bus master.

### 7.3 `ctl`, `data` and `status`

The device directory is `/dev/gfx/0`.

`ctl` verbs:

| Verb | Meaning |
|---|---|
| `start`, `stop`, `reset` | Lifecycle |
| `info`, `version`, `selftest` | Identity and diagnostics |
| `mode <w>x<h>x<bpp>` | Primary mode |
| `mode secondary <w>x<h>x<bpp>` | Secondary mode, if enabled |
| `mirror on/off` | External mirroring |
| `palette <index> <rrggbb>` | Palette entry |
| `gamma <r> <g> <b>` | Gamma |
| `cursor on/off`, `cursor <x> <y>`, `cursor image <handle>` | Hardware cursor |
| `blank on/off`, `dpms on/standby/suspend/off` | DPMS |
| `flip <surface>` | Page flip on VBLANK |
| `overlay <surface> <src> <dst>`, `overlay off` | Overlay |
| `tv ntsc/pal/off`, `tv testbars` | TV-out |
| `backlight <0-100>` | Panel backlight |
| `ddc read` | Read EDID |
| `fifo stats`, `engine reset` | FIFO diagnostics and recovery |
| `panel info` | `CONFIG_PANEL`, `PANEL_ID`, LVDS/digital |

`data` is the command-stream and frame path for trusted clients: a binary opcode stream
(fill, blit, span, glyph run, triangle batch) or a mapped surface. Untrusted legacy clients
go through the display server. `status` carries:

```text
state=ready
driver=ati-rage-lt
chip=264ltpro
pci=1002:4c49
vram=4MiB
mode=1024x768x24
refresh=60
panel=1024x768x18
backlight=pmu
cursor=on
fifo_free=96
fifo_err=0
engine=idle
irqs=12345
vblank=60
overlay=off
tv=off
ddc=present
last_error=
```

The exact key set is device-specific; the common keys are stable, additions are additive.
Errors use the standard `EINVAL`, `EBUSY`, `EIO`, `ENODEV`, `ENOSPC`, `ETIMEDOUT`,
`EOPNOTSUPP` codes.

### 7.4 Surfaces and command queues

A `Surface` is a VMO with geometry, stride, format (`I8`, `RGB16`, `RGB24`, `RGB32`),
residency (`VRAM`, `MEMORY`, `AUTO`) and rights (`DRAW`, `PRESENT`, `READ`, `MAP`). The
driver allocates VRAM from a first-fit allocator over the 2/4 MiB aperture, reserves the
scanout image, the cursor block, the Z buffer and the texture heap, and reports the free
map in `status`. `AUTO` means VRAM for the scanout/topmost surface and memory otherwise.

A `CommandQueue` is an ordered ring of 16–32-byte ops: `FILL`, `COPY`, `SPAN`, `GLYPHS`,
`TRIANGLES`, `SET_STATE`, `SET_CLIP`, `CLEAR`, `FLIP`, `FENCE`. The cell validates bounds,
formats and clipping, translates the ops into Mach64 register writes, batches up to the
FIFO watermark, and signals the fence after `GUI_ACTIVE` clears. A client can submit a
command buffer as a VMO; the cell copies or maps it, never executes client pointers.

For the native display server, the fast path is a mapped surface and a shared ring; for the
X11 compatibility server, the same surface API is used in the nested/rootless phases; for
a legacy IRIX client, the X server is the mediator and the client never sees the GPU.

### 7.5 DMA, cache and endianness

- The command ring and texture uploads use `DmaWindow` buffers. Before the ATI reads them,
  the cell flushes the D-cache (`dcbf`/`sync` on PowerPC) and, if the platform is not
  coherent, invalidates after the ATI writes.
- Cache coherency on the M4753's PCI bridge is not documented in the sources available.
  Treat it as non-coherent until a DMA pattern test proves otherwise.
- Every register access uses the endianness established by the Stage 0 scratch test. The
  frame buffer uses the aperture endian mode from `MEM_CNTL` or the big-endian aperture;
  the two are independent.
- The cell never uses a client's virtual address in a descriptor. It pins the backing
  pages, translates to physical addresses through the framework, and validates that the
  range is inside the granted `DmaWindow`.

### 7.6 Failure and recovery

- On a FIFO error, the cell stops submitting, pulses `GEN_GUI_RESETB`, reinitialises the
  GUI state (pixel widths, pitches, scissors, ROP, pattern, colour compare, 3D state) and
  reports `fifo_err` in `status`. The client's fence completes with `EIO`.
- On a hang (`GUI_ACTIVE` set for longer than a frame), the cell resets the GUI engine and,
  if that fails, resets the whole device via `BUS_CNTL.BUS_MSTR_RESET` and
  `GEN_TEST_CNTL.GEN_SOFT_RESET`, then restores the mode from a saved register set.
- On a cell fault, v1 panics; the restartable-cell model is a later framework feature. The
  driver keeps a saved register set so a restarted cell can restore the scanout without a
  full mode set.
- On a power event, the cell saves the mode, panel and palette state, powers the panel
  down in the documented sequence, and restores on resume. The PMU cell owns the
  backlight/battery side; the GPU cell owns the panel rail and the PLL.

### 7.7 The one-logical-computer path

`GpuDevice` is resolved through the global namespace. A local client submits a command
buffer and waits on a fence. A remote client on another node submits the same command
buffer over the fabric; the GPU cell executes it locally and returns a CAS object or a
shared-memory ring. The remote client never maps the GPU's MMIO. Offscreen 3D can run on
the GPU node while the application runs elsewhere; the result is copied back as a normal
object. This is the model in [`system.md`](../internals/system.md) §7, applied to the Rage LT
Pro. For the M4753, the single-node path is the only implemented path in this slice.

---

## 8. Mesa and the GL bridge

### 8.1 What GL level is honest

The Rage LT Pro is a fixed-function OpenGL 1.x part. The highest honest target is
**OpenGL 1.2/1.3 with a fixed-function subset**, not GL 2.0 and not GLSL. The Mesa mach64
DRI driver (Mesa 6.5.3–7.0.3, MIT) is the reference implementation; it exposes the Rage Pro
pipeline through DRI1 and falls back to software for the features the hardware lacks.

| GL feature | Rage LT Pro | Driver path |
|---|---|---|
| GL 1.1 fixed-function transform and lighting | CPU (Mesa TNL) | Mesa pipeline |
| Gouraud and flat shading | Hardware | `VERTEX_n_ARGB`, `SETUP_CNTL` |
| Specular | Hardware | `VERTEX_n_SPEC_ARGB` |
| Perspective-correct texturing | Hardware | W interpolators, `ONE_OVER_AREA` |
| Mipmapping, bilinear, trilinear | Hardware | `TEX_n_OFF`, `TEX_CNTL` |
| Multitexture (2 units) | Hardware | `TEX_CNTL.TEXTURE_COMPOSITE`, secondary registers |
| Alpha blending (ADD) | Hardware | `SCALE_3D_CNTL` |
| Alpha test | Hardware | `ALPHA_TST_CNTL` |
| 16-bit depth test | Hardware | `Z_CNTL` |
| Fog (linear) | Hardware | `FOG_*`, `DP_FOG_CLR` |
| Dithering | Hardware | `SCALE_3D_CNTL` |
| Edge anti-aliasing | Hardware | `SCALE_3D_CNTL.EDGE_ANTI_ALIAS` |
| Stencil | Software/fallback | Not present |
| Separate blend equations | Software/fallback | ADD only |
| Logic ops | Software/fallback | Mesa falls back |
| GLSL, FBOs, hardware vertex buffers | Software or not offered | Not present |
| Cube maps, 3D textures | Not offered | Not present |
| Multisample | Not offered | Edge AA only |

### 8.2 The Mesa port

The fastest credible route is to port the **Mesa classic mach64 DRI** (MIT, last present in
Mesa 7.0.x) as a client of the GPU service:

1. Keep Mesa's TNL, state validation and fixed-function pipeline on the CPU.
2. Replace the DRI1/DRM interface (`drmCommandWrite(DRM_MACH64_VERTEX/BLIT/CLEAR/FLUSH)`)
   with `CommandQueue::submit` and `Surface` handles.
3. Keep the Mach64 state mapping (`SCALE_3D_CNTL`, `TEX_CNTL`, `Z_CNTL`,
   `ALPHA_TST_CNTL`, `VERTEX_*`) but rewrite it in Rust inside the GPU cell, not in Mesa.
4. Run the Mesa front end in the application or in a GL cell; the GPU cell validates and
   executes the command stream.

This is a 6–10 person-month port if the Mesa classic tree is kept as a separate component
and only the Mach64 backend is replaced. It is not a Gallium driver: there is no Mach64
Gallium backend, and writing one is a larger, less certain project.

### 8.3 The native GL service

For native Rust applications, the GPU service exposes a safe fixed-function API:
`set_state`, `draw_triangles`, `bind_texture`, `set_render_target`, `present`. It is not a
new GL implementation; it is the same state machine the Mesa backend uses, with a Rust
type-safe wrapper. Applications that want a GL-shaped API use the Mesa port; applications
that want a small 3D API use this service directly.

### 8.4 GLX for IRIX applications

IRIX applications link `libGL.so` and speak GLX. The compatibility container needs a GLX
server in the X11 server, translating GLX to the native GL service. The existing X11
research estimates 30–60 person-months for a GL 1.x software rasteriser plus the GLX wire
protocol; with the Mach64 backend, the rasteriser is partly hardware, but the GLX protocol,
visuals/fbconfigs, `MakeCurrent`, `SwapBuffers` and indirect/direct contexts remain. GLX is
a separate project, not part of this driver slice. The driver makes it possible; it does
not deliver it.

### 8.5 Memory budgets

The 4 MiB limit decides the usable 3D modes. Front, back and Z are all in VRAM; textures
share the remainder. The 16 bpp rows use 16-bit colour and 16-bit Z; the 32 bpp rows use
32-bit colour and 16-bit Z. The 24 bpp row has no Z buffer because double-buffered 24 bpp
already exceeds 4 MiB.

| Mode | Front | Back | Z | Total | Fits 4 MiB? | Texture space left |
|---|---:|---:|---:|---:|---|---:|
| 640×480×16 | 0.59 MiB | 0.59 MiB | 0.59 MiB | 1.76 MiB | Yes | 2.24 MiB |
| 640×480×32 | 1.17 MiB | 1.17 MiB | 0.59 MiB | 2.93 MiB | Yes | 1.07 MiB |
| 800×600×16 | 0.92 MiB | 0.92 MiB | 0.92 MiB | 2.75 MiB | Yes | 1.25 MiB |
| 800×600×32 | 1.83 MiB | 1.83 MiB | 0.92 MiB | 4.58 MiB | No | — |
| 1024×768×16 | 1.50 MiB | 1.50 MiB | 1.50 MiB | 4.50 MiB | No | — |
| 1024×768×16, no Z | 1.50 MiB | 1.50 MiB | — | 3.00 MiB | Yes | 1.00 MiB |
| 1024×768×24 | 2.25 MiB | 2.25 MiB | — | 4.50 MiB | No | — |
| 1024×768×32 | 3.00 MiB | 3.00 MiB | — | 6.00 MiB | No | — |

Consequences:

- 3D at 1024×768 is a 16 bpp, double-buffered, **no-Z** configuration, or a smaller
  viewport with Z. A depth-tested 1024×768 scene does not fit with a 4 MiB board.
- 800×600×16 with Z is the largest comfortable 3D mode; 640×480×16 leaves room for
  textures.
- The 2D desktop at 1024×768×24 uses 2.25 MiB; the remaining 1.75 MiB must hold offscreen
  surfaces, the cursor and any retained images. The native window system's damage-driven,
  memory-backed design is the right fit.
- Video overlay does not consume a full frame buffer; it reads YUV buffers from memory and
  scales into the display, which is why DVD playback at 24 bpp is feasible on a 4 MiB part.

---

## 9. Staged plan and gates

Each stage has a QEMU gate for the generic plumbing and a hardware gate for the chip
function. A stage is not complete until the hardware gate passes on an M4753. The QEMU
gate never substitutes for the hardware gate on a Mach64-specific function.

### Stage 0: prerequisites, PCI and the OF framebuffer

Scope: PCI enumeration on the PowerBook, BAR mapping, register endianness, chip identity,
OF framebuffer handover, GPU cell skeleton.

| Gate | Test | Pass criterion |
|---|---|---|
| G0.1 | PCI scan of the M4753 | Logs `1002:4c49` (or `4c50`/`4c51`), BAR0/BAR1/BAR2 sizes and addresses, INTA line |
| G0.2 | `CONFIG_CHIP_ID`, `CONFIG_STAT0/1/2`, `MEM_CNTL.MEM_SIZE` | Chip type "LI"/"LP", 2 or 4 MiB, panel ID, VGA/bus straps logged |
| G0.3 | Scratch-register endianness | `SCRATCH_REG0` write/read matches with exactly one of swapped/unswapped access |
| G0.4 | Aperture map | Block 0 at `0x400 + (r<<2)`, block 1 at `r<<2`; frame buffer readable; no faults |
| G0.5 | OF framebuffer handover | Draw a test pattern through the OF `display` node; read back and CRC; no ATI register writes |
| G0.6 | Cell skeleton | `Mmio`/`Irq`/`DmaWindow` grants; `ctl`/`data`/`status` files; `info` reports the chip |
| G0.7 | QEMU generic gate | `ati-vga` enumerated, BARs mapped, framebuffer drawn; the Mach64 driver refuses to bind |
| G0.8 | Fault injection | Invalid BAR, missing OF node, wrong chip ID: clean `ENODEV`, no panic |

Prerequisite: the kernel-ppc currently has no PCI config-space access, no PCI IRQ routing
and no real-hardware boot path. Those are dependencies of this stage, not part of the
driver. The PowerBook boot path (OF `boot` from a file, network or CD) must exist before
any hardware gate can run.

### Stage 1: display and 2D

Scope: mode setting, panel, PLL, palette, cursor, DPMS, page flip, DDC, 2D fill/blit/line/
pattern/ROP/clip/mono/endian, FIFO backpressure and recovery.

| Gate | Test | Pass criterion |
|---|---|---|
| G1.1 | 1024×768×16 panel mode | Stable image for 60 s; CRTC/PLL readback matches; no FIFO error |
| G1.2 | 1024×768×24 panel mode | Stable image; 24-bit palette and gamma gradient; panel sync |
| G1.3 | 800×600 STN mode (12.1-inch) | Stable image; correct panel type and stretching |
| G1.4 | Palette and gamma | 256-entry ramp; gamma curve; hardware cursor colours |
| G1.5 | Hardware cursor | 64×64 and 128×128; move, hide, negative coordinates; no corruption |
| G1.6 | DPMS | On/standby/suspend/off; panel and external monitor; power measurement |
| G1.7 | Page flip | 60 flips/s for 60 s; no tearing on a moving bar; VBLANK count matches refresh |
| G1.8 | DDC/EDID | Read a known monitor's EDID; log manufacturer, mode list; hot-plug behaviour |
| G1.9 | 2D conformance | Fill, blit, line, pattern, ROP, clip, compare, mask, mono expansion; readback CRC equals software reference for 8/16/24/32 bpp |
| G1.10 | FIFO stress | 100,000 mixed ops; `FIFO_ERR` stays clear; throughput recorded; recovery from an injected overflow |
| G1.11 | VBLANK IRQ | Count over 10 s equals refresh × 10; shared IRQ with ATA/Ethernet does not lose events |
| G1.12 | 86Box/MAME cross-check | The same 2D register sequences produce the same pixels in the emulator, where modelled |
| G1.13 | Suspend/resume | Mode, palette, cursor and panel state restored; no corruption |

### Stage 2: fixed-function 3D

Scope: setup engine, primitives, shading, textures, blending, depth, fog, dither, AA,
scissors, clear, performance.

| Gate | Test | Pass criterion |
|---|---|---|
| G2.1 | Flat triangle | 640×480×16; readback matches expected colour and geometry within 1 LSB |
| G2.2 | Gouraud triangle | Colour interpolation matches the reference gradient |
| G2.3 | Textured triangle | RGBA16/RGB16 texture; nearest and bilinear; readback matches |
| G2.4 | Mipmap and trilinear | 11 levels; LOD selection; no seam; readback matches |
| G2.5 | Perspective correction | Checked floor/quad; no affine warping |
| G2.6 | Multitexture | Two textures in one pass; composite function matches |
| G2.7 | Alpha blending and alpha test | All 8×8 factors and 8 test functions; reference alpha |
| G2.8 | Z buffer | 16-bit Z; 8 compare functions; Z mask; overlapping triangles |
| G2.9 | Fog | Linear fog; colour and density; near/far |
| G2.10 | Dithering | 16 bpp gradient; no banding beyond the reference |
| G2.11 | Edge AA | Edge pixels match the reference; no full-scene AA claim |
| G2.12 | Scissors and clear | Clip region; fast fill; Z clear via `Z_SRC_2D` |
| G2.13 | Points and lines | Quad/line paths; no gaps; hardware primitive path if it works |
| G2.14 | Performance | 1M triangles/s peak claim; measured triangles/s at 640×480×16 |
| G2.15 | Mesa smoke test | Mesa mach64 port renders `glxgears`/`quads`/a conformance subset |
| G2.16 | FIFO under 3D | 3D state changes and vertex batches do not overflow or lock |

### Stage 3: video, TV, backlight, power

Scope: overlay, scaler, YUV, TV-out, backlight, panel power, capture (if the board has it).

| Gate | Test | Pass criterion |
|---|---|---|
| G3.1 | Overlay YUV422 | 720×480 → 1024×768; scaling and colour bars; readback/visual |
| G3.2 | Overlay planar YUV | YUV420 buffers; correct U/V planes; no colour swap |
| G3.3 | Overlay keying | Video key and graphics key; exclusive bounds |
| G3.4 | Brightness/saturation | Range and linearity; reset to defaults |
| G3.5 | TV-out NTSC | S-video to a TV/grabber; colour bars; overscan; flicker filter |
| G3.6 | TV-out PAL | Same at 50 Hz |
| G3.7 | Backlight | PMU and/or ATI PWM; 0–100%; no flicker; panel rails |
| G3.8 | Panel power sequence | Bias, digital, backlight order and delays; no panel damage |
| G3.9 | Capture (if wired) | One-shot and continuous; two buffers; VBI; ZV source |
| G3.10 | Power | Suspend/resume; PLL sleep; DAC power-down; measured current |

### Stage 4: Mesa/GL and GLX

Scope: Mesa classic mach64 port, native GL service, GLX bridge.

| Gate | Test | Pass criterion |
|---|---|---|
| G4.1 | Mesa backend | GL 1.2/1.3 conformance subset; no fallback on accelerated paths |
| G4.2 | State coverage | Every §5.4 state maps to a register; no silent software fallback |
| G4.3 | Texture memory | Texture heap respects the 4 MiB budget; eviction works |
| G4.4 | Native GL service | Rust API renders the same scenes as the Mesa backend |
| G4.5 | GLX protocol | IRIX GLX client connects, creates a context, swaps buffers |
| G4.6 | Performance | Frame rate at 640×480×16 and 800×600×16; no regressions |

GLX is a separate project; G4.5/G4.6 are later gates, not v1.

### Stage 5: production

Scope: restartable cell, error injection, MCP control, power management, long-run stress.

| Gate | Test | Pass criterion |
|---|---|---|
| G5.1 | 24-hour mixed workload | No FIFO error, no leak, no corruption |
| G5.2 | Error injection | FIFO overflow, bus-master abort, lost IRQ: recovery without reboot |
| G5.3 | Cell restart | Framework revokes and restarts the cell; scanout restored |
| G5.4 | Mode matrix | Every supported mode and panel combination |
| G5.5 | MCP | `status` and `ctl` are scriptable; no raw register access by default |
| G5.6 | Power cycle | 100 cold boots; no state dependence |

### Effort

| Workstream | Estimate | Basis |
|---|---:|---|
| Stage 0 (PCI, OF, cell skeleton) | 3–5 PM | New PCI config/IRQ path on PowerPC |
| Stage 1 (display and 2D) | 4–6 PM | Xorg/atyfb evidence, register-level work |
| Stage 2 (3D) | 6–10 PM | Reconstructed semantics, hardware iteration |
| Stage 3 (video/TV/power) | 3–5 PM | Overlay and panel work |
| Mesa classic port | 6–10 PM | MIT driver, backend replacement |
| GLX bridge | 30–60 PM | Existing X11 research estimate, separate project |
| Production hardening | 3–5 PM | Stress, recovery, power |

These are estimates, not commitments. The 3D and GLX numbers carry the most uncertainty
because the 3D register descriptions are not public.

---

## 10. Clean-room provenance and sources

### 10.1 Policy

1. **No source code from the cited drivers, emulators or vendor documents enters this
   tree.** Open drivers and emulators are read-only oracles. Their code, comments, tables
   and naming are not copied. Where a register sequence is derived from one, the note
   records the source and the fact that it is reconstructed.
2. **Facts are separated from expression.** Register offsets, bit positions, field widths
   and sequences are facts. A table or a paragraph of prose from a document is expression.
   The tables in this note are reconstructed from multiple sources and cite them; they are
   not transcriptions of an ATI table.
3. **The ATI documents are copyrighted.** RRG-G03300 is marked "Restricted Version" and
   GCS-C03300 "Proprietary and Confidential". They are publicly archived but not freely
   licensed. Cite them; do not vendor the PDFs into the repository; do not reproduce their
   tables wholesale.
4. **The programmer's guide is confidential.** PRG-215R3-00-10 is marked "CONFIDENTIAL
   MATERIAL" and is not a public specification. It must not be used as a primary source.
   If a contributor has NDA access, that is a separate legal review. The fallback is the
   public RRG/GCS, the open drivers and hardware experiments.
5. **The 3D gap is explicit.** The public RRG omits the 3D, multimedia and TV-out register
   descriptions. The driver must not pretend the gap does not exist. The reconstructed 3D
   path is a design hypothesis until Stage 2 passes on hardware.
6. **Review.** Every driver function carries a provenance entry: source tag, what was
   taken (fact or behaviour), and the test that confirms it. A second reviewer checks the
   implementation against the GPL/MIT sources for textual similarity before merge.

### 10.2 Sources

| Tag | Source | URL | Licence | Use in this note |
|---|---|---|---|---|
| RRG03300 | ATI, *3D RAGE LT PRO Register Reference Guide*, RRG-G03300 Rev 2.01X, Apr 1998 | <https://archive.org/details/bitsavers_atiRAGEPRORAGELTPRORegisterReferenceGuide199804_2651262> | ATI copyright, "Restricted Version" | Register names, offsets, display/2D/LCD/bus-master descriptions; 3D/multimedia/TV descriptions omitted |
| GCS03300 | ATI, *3D RAGE LT PRO Graphics Controller Specifications*, GCS-C03300 Rev 1.01, 1997 | <https://archive.org/details/bitsavers_atiRAGEPROOTechnicalReferenceManualRev1.011997_10506301> | ATI copyright, "Proprietary and Confidential" | Feature set, memory, package, panel, TV, video, 3D feature list |
| RRG02700 | ATI, *mach64 Register Reference Guide: ATI-264VT and 3D RAGE*, RRG-G02700 Rev 0.10, 1996 | <https://archive.org/details/bitsavers_atiRRGG027erenceGuideATI264VTand3DRAGE1996_2605107> | ATI copyright, "Proprietary and Confidential" | 3D Rage trapezoid/line/texture interpolation, 2D register semantics |
| PRG215 | ATI, *RAGE PRO and Derivatives Programmer's Guide*, PRG-215R3-00-10 Rev 1.0, Mar 2000 | <https://archive.org/details/3drageprow-95-sdk-4.1zip> | ATI copyright, "CONFIDENTIAL MATERIAL" | **Not used as a primary source.** Recorded because it is the only complete 3D description found and it is not public. |
| XORG | Xorg, `xf86-video-mach64` 6.7.0 | <https://gitlab.freedesktop.org/xorg/driver/xf86-video-mach64> | MIT-style (Marc Aurele La France; Precision Insight MIT) | Chip table, 2D accel, cursor, XV, I2C, mode/panel handling, DRI |
| LINUXATY | Linux, `drivers/video/fbdev/aty/` | <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/video/fbdev/aty> | GPL-2.0-only | Mode tables, PLL, panel, backlight, hardware quirks |
| MESA | Mesa 6.5.3/7.0.3, `src/mesa/drivers/dri/mach64/` | <https://gitlab.freedesktop.org/mesa/mesa/-/tree/mesa-6.5.3-1/src/mesa/drivers/dri/mach64> | MIT (Gareth Hughes, Leif Delgass, José Fonseca) | 3D register mapping, vertex packets, state, texture formats, fallback list |
| 86BOX | 86Box, `src/video/vid_ati_mach64*.c` | <https://github.com/86Box/86Box/tree/master/src/video> | GPL-2.0 | Mach64 2D/FIFO/cursor/scaler behaviour, independent reconstruction |
| MAME | MAME, `src/devices/video/atirage.cpp` | <https://github.com/mamedev/mame/blob/master/src/devices/video/atirage.cpp> | BSD-3-Clause | Mach64/Rage Pro 2D behaviour, FIFO report |
| QEMU | QEMU, `hw/display/ati.c`, `ati_2d.c` | <https://gitlab.com/qemu-project/qemu/-/blob/master/hw/display/ati.c> | GPL-2.0-or-later | Establishing that `ati-vga` is Rage 128, 2D only |
| PCIIDS | PCI ID Repository, `pci.ids` | <https://pci-ids.ucw.cz/> | GPL-2.0 (pciutils) | Device ID names |
| APPLE | Apple, *PowerBook G3 Series — Technical Specifications*, SP128 | <https://support.apple.com/kb/SP128> | Apple copyright | 2/4 MiB VRAM, 1024×768, S-video, HDI-15, M4753 |
| EVERYMAC | EveryMac, *PowerBook G3 233 (Wallstreet) Specs* | <https://everymac.com/systems/apple/powerbook_g3/specs/powerbook_g3_233.html> | Site copyright | Panel sizes, VRAM, mirroring only, S-video, ports |
| WIKI | Wikipedia, *PowerBook G3* | <https://en.wikipedia.org/wiki/PowerBook_G3> | CC BY-SA 4.0 | Model/panel/VRAM table, cross-check only |
| PMU | Linux, `drivers/macintosh/via-pmu.c` and PMU backlight code | <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/macintosh> | GPL-2.0-only | PMU existence, backlight path evidence |
| PBG3 | Local, [`powerbook-g3.md`](powerbook-g3.md) | in-tree | project | M4753 Mac I/O, PMU, machine layer |
| PBQEMU | Local, [`powerbook-qemu-boot.md`](powerbook-qemu-boot.md) | in-tree | project | QEMU g3beige limits, no framebuffer console yet |

Access date for all fetched sources: 2026-11-02.

### 10.3 Gaps and NDA-only material

| Gap | Evidence | Fallback |
|---|---|---|
| 3D register descriptions | RRG-G03300 §1.1 says 3D, multimedia and TV registers are omitted | GCS feature list; Mesa mach64 DRI state mapping; 86Box Rage II+ engine; hardware tests |
| Bus-master descriptor layout | RRG-G03300 ch. 6 gives registers but not descriptor bits | PRG-215 is confidential; use direct FIFO writes in v1; reconstruct the descriptor on hardware |
| TV-out register descriptions | RRG-G03300 omits multimedia/TV descriptions | GCS feature list; Xorg TV/I2C code; hardware scope/TV tests |
| Panel timing tables | RRG gives panel registers but not the M4753 panel's values | Read the firmware-programmed registers, `CONFIG_PANEL`, `PANEL_ID`; test the panel |
| Backlight wiring | No public schematic | PMU driver evidence; test both PMU and ATI PWM paths |
| DDC wiring | No public schematic | Read EDID; fall back to a mode list |
| PCI IRQ routing | OF `interrupt-map`, not documented in the sources | Read the OF tree; test with VBLANK |
| DMA coherency | No public bridge documentation | DMA pattern test with and without cache flush |

The PRG-215 is recorded but **not used**. Its existence confirms that a complete 3D
description exists under NDA; it does not make the description public. A contributor who
has it under NDA must not paste it into the repository or into a design note.

### 10.4 What must not enter the tree

- Any source file, header, table, comment block or register-sequence macro from Linux,
  86Box, QEMU, Xorg, Mesa or MAME.
- Any scan, OCR or table image from RRG-G03300, GCS-C03300 or RRG-G02700.
- Any excerpt from PRG-215.
- Any code generated by transcribing a GPL/MIT function line by line, even if renamed.

The register tables in this note are reconstructed summaries with citations. They are
design input, not a substitute for the vendor documents.

---

## 11. Unknowns and risks

| Unknown | Impact | Test | Fallback |
|---|---|---|---|
| Exact PCI ID and package on the M4753 | Probe may reject the board | Read PCI config and `CONFIG_CHIP_ID` | Accept `4C49`, `4C50`, `4C51` |
| Register endianness on the PowerPC bridge | Every register access | Scratch pattern, swapped and unswapped | Use the matching accessor |
| Aperture endianness and big-endian aperture | Direct CPU pixel writes | `MEM_CNTL` read; write/read pattern | Use host blits or software swap |
| Panel type, depth and link | Mode set may blank the panel | `CONFIG_PANEL`, `PANEL_ID`, cable inspection | Copy firmware mode; do not mode-set until confirmed |
| Backlight control path | Brightness may not work | PMU and ATI PWM tests | PMU path |
| DDC/EDID wiring | No monitor identification | Read EDID | Static mode list |
| TV-out wiring and standard | S-video may not work | Scope/TV test | Disable TV path |
| CRTC2 independent scanout | Mirroring only may be a hard limit | Test on hardware | Mirror only |
| IRQ routing and sharing | No VBLANK events | OF `interrupt-map`; count IRQs | Poll `CRTC_INT_CNTL` |
| DMA coherency | Data corruption | DMA pattern with/without flush | Explicit flush/invalidate |
| FIFO depth and descriptor format | Bus master unreliable | Direct FIFO first; descriptor experiments later | v1 direct FIFO only |
| 3D setup formats and trigger | 3D does not render | Mesa sequence + hardware readback | Iterate on hardware; no emulator |
| 3D performance with 4 MiB | Low frame rate | Benchmark at 640×480/800×600 | Smaller modes/textures |
| Multitexture on the PCI LT Pro | GL feature gap | Hardware test | Single texture |
| 32 bpp rendering within 4 MiB | Native-resolution 32 bpp does not fit; low-resolution does | Memory budget + hardware | 16 bpp 3D |
| Panel power sequence | Panel damage or no display | Scope the rails; follow Xorg/atyfb sequences | Keep firmware sequence |
| PMU suspend/resume | Resume corruption | Suspend/resume loop | Disable suspend in v1 |
| Legal status of the ATI documents | Distribution risk | Legal review | Cite, do not redistribute |

---

## 12. Non-goals

Only silicon limitations and product decisions belong here. Board-absent functions from §5
are not repeated: they are chip capabilities that the M4753 does not expose.

- Programmable shaders, GLSL, SPIR-V, Vulkan, GL 2.0+.
- 32-bit Z, stencil, multisample AA, cube maps, 3D textures.
- AGP texturing and motion compensation on the M4753 (PCI variant).
- Arbitrary 2D pixel-format conversion and 2D stretch blits in the GUI engine.
- A full IOMMU or hardware confinement of the ATI bus master.
- Multiple GPUs, hotplug, or a full compositor in this slice.
- A complete GLX implementation in this slice; GLX is a separate project.
- Ray tracing and video encoding.

---

*Prepared 2026-11-02. Research input to the driver design for #328, not an ADR. The
recommendation is: build the cell from the public register facts and the reconstructed 2D
path first, treat 3D as a hardware-validated reconstruction, and do not claim QEMU coverage
for the Mach64.*
