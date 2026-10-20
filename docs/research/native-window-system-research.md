# Native window system for IRIX 7 (SGI Indy, IP22)

**Decision research.** Which native window system / display architecture should IRIX 7 adopt for
Rust-native software, given GR1/GR2 Newport 2D graphics (REX3 blitter, no 3D), 32–128 MB RAM,
a 100–200 MHz R4400, a framekernel with Theseus-style cells, and a separate X11 compatibility
effort?

**Status:** research report, not an ADR. The recommendation is in §1 and §7; the evidence and the
per-option analysis are in §3–§6; risks are in §11.

---

## 1. Executive summary

**Recommendation: option 1, a minimal custom display server in Rust, built in the Plan 9
`8½`/`rio` and BeOS `app_server` lineage rather than in the Wayland lineage.**

Concretely, IRIX 7 should have one userspace **display cell** that owns the Newport display
pipeline (VC2 timing, XMAP9 palette, REX3 command FIFO, hardware cursor) and multiplexes drawing
for all native clients. Its model is:

* **server-side drawing into server-owned images**, with a compact binary opcode protocol
  (blit, fill, span, glyph run, colour expand, clip), not client-rendered full surfaces;
* **retained window images** in main memory, with direct in-place drawing into the scanout
  image for the topmost visible window;
* **damage-driven repair**, not per-frame recomposition: the common path is a clipped REX3 blit
  straight into the scanout image, and the server acts as a compositor only for overlap, move, raise and
  expose;
* **no alpha, no transforms, no scaling** in the native path; an opaque 8-bit indexed visual is
  the default, with 24-bit direct colour as an option at reduced resolution;
* **one hardware cursor** in the XMAP9 auxiliary plane, never a software cursor;
* **display leases** for full-screen media cells, so an EDF video thread never pays for
  composition;
* **capability-addressed surfaces** (rights: draw, present, read, map) that fit the framekernel's
  object model, with a zero-copy mapped fast path for trusted native cells in the single address
  space.

X11 coexists as a compatibility cell. Phase 1 runs the X server under a **display lease** (a mode
switch: X owns the screen and its own window manager for the duration), which is small, robust and
gives legacy applications the hardware they expect. Phase 2 nests X in a native window (a shadow
framebuffer with damage uploads: the Xnest/Xvfb shape), which is the best cost/benefit point for
coexistence. Phase 3, only if mixed top-level windows are a product requirement, adds rootless
per-X-window embedding (the Xwayland shape).

The reason is arithmetic, not taste. On this machine the scanout buffer is 1.25 MiB at 8 bpp and
3.75 MiB at 24 bpp for 1280×1024; one full-screen recomposition per frame at 60 Hz is therefore
79 MB/s or 236 MB/s of pure write traffic, against a memory controller peak of about 266 MB/s and
measured GIO PIO write bandwidth of 66 MB/s [H §3, §12.1]. A 32-bit ARGB pipeline, which Wayland's
`wl_shm` path makes the default, is 5 MiB per 1280×1024 frame and 315 MB/s at 60 Hz, and it needs
per-pixel alpha blending that Newport does not implement. An always-composite architecture is
therefore not merely expensive on an Indy; at 24 bpp it is outside the hardware's budget before
the CPU does any work.

The second reason is effort and fit. Plan 9's `8½`, a complete window system with a terminal and
cut-and-paste, was 5,100 lines of C, of which the graphics server was 2,295 lines plus 2,214 lines
of drawing operations, on a 68020 [P9]. A Rust reimplementation with capabilities, damage and EDF
awareness should land at 8,000–12,000 lines: roughly 10–12 person-months for a production v1. A
Wayland compositor built on Smithay is a port of a Linux-shaped stack (libudev, libinput, libgbm,
libEGL, libxkbcommon, DRM/KMS, Unix file-descriptor passing) onto a kernel that has none of those
interfaces, and even then it would implement the wrong rendering model for the hardware.

**What is rejected, and why, in one line each:**

| Option | Verdict |
|---|---|
| 2 Wayland (Smithay/wlroots/tinywl/river/sway/COSMIC) | Right ideas (damage, buffer handles, frame callbacks), wrong rendering model and a large Linux-shaped port; reject as the native ABI |
| 3 Direct framebuffer + tiny WM, no compositing | The correct baseline, but without retained images it flickers and forces client repaints; adopt its direct-draw path, add server-side retention |
| 4 Plan 9 `rio` | Closest prior art and the best structural model; adopt it, with capability handles, in-place scanout drawing and damage accounting |
| 5 Arcan | Attractive shmif/frameserver isolation, but GL/EGL + LuaJIT + a large C codebase, and mixed BSD-3/GPL-2.0 licensing; reject |
| 6 BeOS `app_server` | The best worked example of a 2D-only userspace server with per-window backing store and an accelerant layer; borrow the structure, not the code |
| 7 NeXT DPS / Quartz | A PostScript/PDF interpreter in the display path is too much code, too much CPU and cannot be licensed; reject |
| 8 Sun NeWS | Same interpreter problem plus executes client PostScript in the server; reject |
| 9 Android SurfaceFlinger | GPU-era buffer queueing and triple buffering; far too heavy; borrow nothing but the explicit buffer-state idea |
| 10 Chrome Viz | Multi-process web compositor, millions of lines; reject |
| 11 Windows DWM | Requires GPU composition; its one useful lesson is that full-screen apps disable composition |
| 12 Qt Compositor / Mir | Qt/Mir runtimes and GL dependencies; GPL-3.0 is compatible but the port is not viable |
| 13 X11 as the native protocol | Good 2D fit but surrenders the capability model and the native ABI; keep X11 as the compatibility cell only |
| 14 No window system (full-screen cells) | Necessary escape hatch for media and games, not a desktop architecture |

---

## 2. Method, evidence and confidence

The question mixes three kinds of claim: what the hardware can do, what each historical system
actually did, and what the current Rust ecosystem provides. They are evidenced differently.

* **Hardware limits** come from the local hardware reference [H], which is itself sourced from SGI
  chip specifications (REX3, RB2, VC2, XMAP9, MC, GIO) and from Linux/NetBSD driver sources as
  executable evidence. Numbers quoted here are from that document and are marked with its section.
* **Historical architectures** are taken from primary documents where they exist online: Plan 9's
  `8½` paper and `draw(3)`/`rio(1)`/`rio(4)` manual pages [P9, DRAW, RIO1, RIO4], the BeOS/Haiku
  `app_server` source tree [HAIKU], the X11 protocol specification [XPROTO], the Wayland
  architecture document and protocol XML [WAYLAND, WXML], and the Android and Chromium
  architecture documents [BQ, SFHWC, VIZ]. NeXT DPS, NeWS, Quartz and BeOS R5 predate or sit
  outside convenient open publication; those sections rely on the standard bibliographic sources
  [DPS, NEWS, QUARTZ, BEBOOK] and are marked lower confidence where a detail could not be checked
  against a fetched document.
* **Rust ecosystem claims** are checked against the repositories themselves: Smithay's README and
  renderer module, wlroots' README, the `cosmic-comp`, `river` and `sway` repositories, Arcan's
  `COPYING`, and the Qt Wayland Compositor manual [SMITHAY, SMITHAY-SRC, WLROOTS, TINYWL, COSMIC,
  RIVER, SWAY, ARCAN, QTCOMP].

Two conventions matter for reading the rest of the report.

**Estimates are labelled and derive from stated arithmetic.** Where a figure is a calculation
(for example, "236 MB/s for a full-screen 24-bit recomposition at 60 Hz"), the inputs are given so
the reader can disagree with the assumption rather than with a hidden benchmark. No benchmark was
run on Indy hardware for this report; the machine is not available to the researcher.

**"Suitability for 1993 hardware" is judged at the machine's actual configuration**, not at a
modern minimum: 32 MB is the planning floor, 8 MB is not, and there is no GPU, no texture mapper,
no alpha blender and no framebuffer readback fast path. A design that assumes any of those is
unsuitable regardless of its elegance.

---

## 3. Constraints that decide the design

### 3.1 The display pipeline

The Indy's graphics is a 2D pipeline with a very particular shape [H §9].

| Block | What it does | Consequence for the window system |
|---|---|---|
| **REX3** | Command FIFO drawing engine: fills, spans, blits, lines, conditional writes, 5 span masks, fast clear, 4/8/12/24 bpp | The blitter is the compositor. Drawing must be expressed as REX3 ops, not as pixels pushed by the CPU |
| **RB2** | VRAM formatter, read-modify-write for sub-word pixels, 24-bit write mask | 8-bit and 4-bit modes are first-class; packed 32-bit pixels are not native |
| **VC2** | Video timing from SRAM tables; pixel clock ≤ 140 MHz; 32×32 2 bpp or 64×64 1 bpp hardware cursor; Display ID | Mode setting is a table download, not a register write; the cursor is free |
| **XMAP9** | RAMDAC/pixel formatter, 4/8/12/24-bit RGB and indexed modes, 8-bit auxiliary buffer, CLUT behind REX3's DCB bridge | An 8-bit overlay/aux plane exists; the palette is server state |
| **VRAM** | Planar 24-bit storage, 1280×1024 visible plus 64 px offscreen; size per variant not reliably documented | 24-bit is three planes, not packed pixels; offscreen space is only 64 px, so per-window VRAM rectangles are scarce |

Three hard consequences follow.

1. **There is no fast host-to-VRAM path for images.** The hardware reference is explicit: for a
   linear framebuffer, either use `HOSTRW` (slow, 8- or 32-bit) or render via REX3; there is no
   third path [H §9.6]. Measured GIO PIO is 13 MB/s read and 66 MB/s write [H §12.1]. Uploading a
   full 24-bit frame from main memory therefore costs at least 60 ms at 66 MB/s, i.e. under
   17 frames/s before any drawing. **Content displayed at frame rate must be produced in VRAM, not
   uploaded from main memory every frame.** This single fact rules out the Wayland model of "client
   renders into a main-memory `wl_shm` buffer, compositor samples it per frame" for anything
   animated.
2. **Composition is a VRAM-to-VRAM blit at best.** REX3 can copy and fill in VRAM quickly, but the
   scanout image and the window images compete for the same VRAM and the same command FIFO. The
   FIFO is shallow (a few dozen entries) and must be polled [H §9.1], so a compositor cannot queue
   an unbounded frame's worth of ops and then sleep.
3. **There is no alpha.** REX3 has logical ops and a 24-bit compare/mask for conditional writes,
   but no per-pixel alpha blend [H §9.1]. Translucency, rounded corners with anti-aliased edges,
   and ARGB shadows are CPU work. A native visual with an alpha channel would convert every blit
   into a read-modify-write loop on the R4400.

### 3.2 Memory and bandwidth budget

| Mode | Frame bytes | Full-frame copy at 60 Hz | at 76 Hz |
|---|---:|---:|---:|
| 1024×768, 8 bpp | 786,432 (0.75 MiB) | 47 MB/s | 60 MB/s |
| 1280×1024, 8 bpp | 1,310,720 (1.25 MiB) | 79 MB/s | 100 MB/s |
| 1024×768, 24 bpp | 2,359,296 (2.25 MiB) | 142 MB/s | 179 MB/s |
| 1280×1024, 24 bpp | 3,932,160 (3.75 MiB) | 236 MB/s | 299 MB/s |
| 1280×1024, 32 bpp ARGB | 5,242,880 (5 MiB) | 315 MB/s | 399 MB/s |

Reference points: memory controller peak ≈ 266 MB/s for CPU accesses [H §3]; GIO64 peak 320 MB/s,
but PIO write measured at 66 MB/s [H §12.1]. The 32-bit ARGB row exceeds the memory-controller
budget at 60 Hz and sits at the GIO64 peak, four times what the measured PIO path can sustain; the
24-bit 1280×1024 row exceeds the memory budget. Even the 8-bit 1280×1024 row consumes 30–38% of the
memory controller just to copy the frame once, leaving nothing for the CPU, the blitter's own reads,
or media decoding.

This is the quantitative core of the recommendation: **a design that recomposes the full screen
every frame is not viable above 8 bpp, and is wasteful even at 8 bpp.** A design that redraws only
damage, and that draws into the scanout image in place, is viable at every mode.

RAM is the other half. The framebuffer lives in VRAM, not main memory, so a 32 MB machine has
32 MB for code and data. Ten 800×600 ARGB window buffers would be 18.3 MiB, i.e. 57% of RAM, before
the applications allocate anything. The same ten windows at 8 bpp are 4.6 MiB, which is affordable; but offscreen VRAM is far
too small to hold them (REX3 addresses only 64 pixels beyond the visible area [H §9.1]), so main
memory is where retained images must live. They must be **depth-matched to the screen**, not
normalised to a modern 32-bit format.

### 3.3 CPU, scheduling and the framekernel

The R4400 runs at 100, 150 or 200 MHz depending on the Indy model [H §2]. It has no cache
coherency with GIO DMA [H §12.4], so every buffer that a device reads or writes needs explicit
cache maintenance. That matters for a compositor that hands client buffers to a blitter: either the
buffer is in VRAM (no cache issue), or it is in main memory and must be cleaned before the blit and
invalidated after.

IRIX 7's kernel is a framekernel with Theseus-style cells [THESEUS]: native Rust code shares one address
space and uses capability handles; legacy IRIX binaries get MMU address spaces. For the window
system this means:

* passing a surface between a client and the server can be a **capability**, not a file descriptor
  and not a socket message with an SCM_RIGHTS attachment; the server can `map` a client's surface
  only if the client grants a mapping right;
* a trusted native cell can be given a **direct mapping** of its own surface, so drawing into it
  needs no copy and no IPC per primitive; the server still owns the blitter and the scanout, and
  the cell signals damage through a shared ring;
* a legacy cell cannot be trusted with a mapping, so its drawing is mediated by the server (the
  Plan 9 model), which is also what an X server expects.

EDF scheduling is a soft real-time policy for media threads. The display server is not a media
thread, but it is on the critical path of every media frame that is presented. The design must make
the server's per-frame work **bounded, preemptible and reservable**: a sporadic-server budget per
frame period, a deadline at the next vertical blank, and a rule that the server never holds the
REX3 FIFO or a lock across a blocking operation. Full-screen media cells should be able to bypass
the server entirely for scanout, otherwise EDF deadlines are hostage to compositor queueing.

### 3.4 What the Indy does not have

There is no 3D engine, no Z-buffer, no texture memory, no alpha blender, no YUV overlay scaler on
the base graphics (Vino is capture, not display), no cache-coherent DMA, no genlock, and no
documented vblank interrupt in the sources available [H §9.7, §18]. The last point is a real risk
for frame pacing: a compositor that wants "flip at vblank" may have to poll VC2 status or accept
tearing. This is flagged again in §11.

---

## 4. Evaluation criteria

Each option is assessed on eleven axes. The weights reflect what decides a 1993-vintage,
2D-only, memory-constrained machine with a native capability ABI and a separate X11 obligation.

| # | Axis | Weight | What a good answer looks like here |
|---|---|---|---|
| 1 | **Rendering model vs hardware** | 25% | Drawing expressed as blitter ops into VRAM; no per-frame full-frame copy; no alpha requirement; 8-bit indexed is first-class |
| 2 | **Memory cost** | 15% | Per-window storage at screen depth, not 32-bit; retained images bounded; no triple buffering |
| 3 | **CPU / bandwidth cost** | 15% | Work proportional to damage, not screen area; no readback; no per-primitive syscall |
| 4 | **Latency and frame pacing** | 10% | Input-to-photon ≤ 2 frames at 60 Hz; bounded compositor work; vblank-aligned |
| 5 | **EDF compatibility** | 10% | Server work is a preemptible, reservable sporadic task; media can bypass composition |
| 6 | **Rust ecosystem maturity** | 8% | Reusable, maintained, permissively licensed crates; no Linux-only assumptions |
| 7 | **Licence compatibility with GPL-3.0** | 4% | GPL-3.0-compatible if code is borrowed; clean-room otherwise |
| 8 | **Implementation effort** | 8% | ≤ 12 person-months to a credible v1 |
| 9 | **X11 coexistence** | 3% | Shared framebuffer or per-window embedding without a second full compositor |
| 10 | **Capability/object-model fit** | 1% | Surfaces are handles with rights; no ambient authority; no global names |
| 11 | **Longevity / failure modes** | 1% | Small enough to audit; no interpreter; graceful degradation to full-screen |

Axes 1–3 carry 55% of the weight because they are where a 1993 machine differs from a 2020 one.
Axes 6–8 carry 20% because a theoretically perfect design that cannot be built is not a candidate.
The remaining 25% covers latency, scheduling, X11, security and maintenance.

**Scoring.** Each axis is scored 0–5, where 5 is "designed for this constraint" and 0 is "actively
hostile to it". The weighted total is reported in §6 for orientation only; the recommendation in
§7 is argued from the axes that dominate, not from the arithmetic mean. In particular, a high
ecosystem score cannot rescue a design whose rendering model costs 3× the available bandwidth.

**What "native" means in this report.** A native window system is the one IRIX 7's own Rust
programs target, with the capability ABI, and the one that owns the display in the default boot.
X11 is a compatibility cell inside it (or beside it), not the definition of the desktop.

---

## 5. Option analysis

The fourteen options are grouped by family: custom/minimal (§5.1, §5.3, §5.4, §5.14), Wayland and
its derivatives (§5.2, §5.12), historical display servers (§5.5–§5.8), mobile/desktop compositors
(§5.9–§5.11), and X11 itself (§5.13). Each entry follows the same headings so they can be compared
without rereading prose.

### 5.1 Minimal custom compositor in Rust

**Architecture.** A userspace server owns the display pipeline and a set of surfaces. Clients
create surfaces through capability handles, draw into them (directly if mapped, or by sending
drawing ops), attach damage rectangles, and commit. The server composites damaged regions into the
scanout image, routes input by hit-testing its scene graph, and manages focus, stacking and
window policy (or delegates policy to a separate WM cell). There is no protocol standard to
conform to, so every semantic — buffer lifetime, resize, focus, popups, clipboard, drag and drop —
is a design decision that must be made and documented.

**Memory and CPU.** The good version is the cheapest of all options: surfaces at screen depth
(1 byte/pixel at 8 bpp), retained only for visible or recently visible windows, damage-proportional
work, and a fast path that blits a window's VRAM image to the screen without an intermediate
buffer. The bad version — the one that copies Wayland's shape — allocates ARGB buffers and
recomposites the full screen, and costs 4× the memory and 3–4× the bandwidth of the good version
(§3.2). The difference is entirely in the surface format and the composition policy, not in the
word "custom".

**Latency.** One IPC round trip per commit if the client is in another address space; zero if the
client is a trusted cell with a mapped surface and a shared damage ring. A damage-only server can
finish a typical 640×480 window update in well under a frame at 8 bpp: 300 KB at the measured
66 MB/s PIO rate is 4.5 ms worst case, and a VRAM-to-VRAM REX3 blit is faster and does not touch
the CPU cache. The server can coalesce commits until the next vblank and meet a 16.7 ms deadline
with a small budget.

**Suitability for 1993 hardware.** Excellent, if and only if it is designed around the blitter and
8-bit indexed colour. The danger is scope: a custom protocol attracts features (alpha, scaling,
fractional HiDPI, client-side decorations) that this hardware cannot afford. The design must
declare them out of scope in the protocol's first version.

**Rust ecosystem.** No dependencies are required beyond `core`/`alloc` and the kernel's capability
calls. This is a strength on a tier-3 big-endian MIPS target where every dependency is a porting
project. The protocol codec can be a small `#[repr(C)]` enum plus a byte slice; a formal spec and a
fuzz target are worth more than a crate.

**Licence.** GPL-3.0 for IRIX 7's own code; no compatibility question.

**Effort.** 10–12 person-months for a v1 with surfaces, damage, input, focus, stacking, clipboard,
font cache, cursor and an X11 bridge. The Plan 9 anchor is 5,100 lines of C for a complete system
including a terminal [P9]; a capability-based Rust system with damage tracking and EDF hooks should
be 8,000–12,000 lines, plus tests.

**X11 integration.** Excellent: the X server is just another client with a surface. Both the
"X in a box" and rootless models are possible (§8). No protocol translation layer is needed,
because the native server is the display owner.

**EDF and the graphics server.** The server is a sporadic task with a per-period budget; its work
is triggered by commits and bounded by damage area; it never spins on the REX3 FIFO. Media cells
can take a display lease and bypass it. This is the best fit of any option.

**Blitter.** Direct. REX3 ops map onto the protocol almost one-to-one: fill, span, blit, glyph
blit, colour expand, clip rectangle, write mask. The server can batch ops into the FIFO and split
batches at the FIFO watermark.

**Verdict.** The right family. Its weakness is that "custom" is not a design; §7 supplies the
missing design.

---

### 5.2 Wayland compositor (Smithay, wlroots, tinywl, river, sway, COSMIC)

**Architecture.** Wayland is a protocol in which clients render into buffers they own and the
compositor composites the resulting surfaces. The architecture document is explicit: "in the
wayland case, the rendering happens in the client, and the client just sends a request to the
compositor to indicate the region that was updated. The compositor collects damage requests from
its clients and then recomposites the screen" [WAYLAND]. `wl_shm` provides shared-memory buffers
for clients that do not have a GPU path [WXML §wl_shm]; the reference hardware enabling requires
EGL/GLES2 and a buffer-sharing mechanism [WAYLAND]. The compositor owns modesetting and input.

Smithay is a Rust library of compositor building blocks, not a compositor: "Smithay aims to provide
building blocks to create Wayland compositors in Rust" [SMITHAY]. It supports the core protocol and
many extensions, ships a sample compositor (`anvil`), and has a `pixman` software renderer behind
the `renderer_pixman` feature [SMITHAY-SRC], which is the one part of the stack that is relevant to
a machine with no GPU. Its system dependencies are the problem: `libwayland`, `libxkbcommon`,
`libudev`, `libinput`, `libgbm`, `libseat`, `xwayland`, `libEGL`, `libpixman`,
`libdisplay-info` [SMITHAY]. wlroots describes itself as "about 60,000 lines of code you were going
to write anyway", with backends for KMS/DRM, libinput, Wayland, X11 and headless, and an Xwayland
abstraction [WLROOTS]. `tinywl` is 1,101 lines of C and is explicitly an educational example, not
a production compositor [TINYWL].

**Memory and CPU.** Wayland's canonical buffer is 32-bit ARGB (`WL_SHM_FORMAT_ARGB8888`), because
the compositor is expected to blend. Even an opaque XRGB8888 buffer is 4 bytes/pixel. For a
1280×1024 screen that is 5 MiB per buffer; a client typically keeps two or three, which is
10–15 MiB, or 31–47% of a 32 MB machine's RAM, for one full-screen client. Per frame, the compositor
must read each damaged client buffer and write the scanout buffer: at 32-bit 1280×1024 and 60 Hz
that is 315 MB/s of reads plus 315 MB/s of writes (the 24-bit case is 236 MB/s each), before alpha
(§3.2). With a `pixman` renderer the CPU does the blending; Newport cannot.

**Latency.** Wayland's frame model, in which clients commit whole surfaces and pace themselves with
frame callbacks, is a good model and it is not the problem. The problem is that a software compositor adds a full-frame copy
between client and scanout, and that `wl_shm` requires the client to render into main memory and
the server to read it back through the cache. On Indy the copy is the latency.

**Suitability for 1993 hardware.** Poor. Wayland was designed for a world of GPU-accelerated
clients, DMA-BUF/GBM buffer sharing, and a compositor that samples textures. None of those exist
here. A software Wayland stack can be made to run on a framebuffer (there are embedded compositors
that do), but it runs the hardware in its least efficient mode: main-memory ARGB buffers, a
per-frame read and write, and CPU alpha.

**Rust ecosystem maturity.** High on Linux, low on IRIX 7. `wayland-server` and `wayland-backend`
are mature but assume Unix sockets, `SCM_RIGHTS` file-descriptor passing, and a POSIX environment.
Smithay adds `calloop` (epoll), `drm`, `libinput`, `udev`, `xkbcommon`, and EGL. Porting those to a
framekernel is not a matter of writing a backend: the server's event loop, buffer import and
input paths all assume Linux interfaces. A custom Smithay backend would still need a Wayland
protocol implementation, a `wl_shm` implementation over capability memory, and an input path.

**Licence.** Smithay MIT [SMITHAY-LICENCE], wlroots MIT [WLROOTS-LICENCE], sway MIT [SWAY-LICENCE],
COSMIC `cosmic-comp` GPL-3.0 [COSMIC-LICENCE], river GPL-3.0-only (SPDX headers and
`LICENSES/GPL-3.0-only.txt`). All are GPL-3.0-compatible. Licence is not the obstacle; the port
is.

**Effort.** Very high: 24–36 person-months to port Smithay's backend, event loop, buffer import and
input to IRIX 7, plus a compositor and shell, and the result is still the wrong rendering model.
Using wlroots would mean porting 60,000 lines of C and its Linux dependencies, which is worse.

**X11 integration.** XWayland is the native pattern and the strongest argument for Wayland: a
rootless X server whose windows are Wayland surfaces. But Xwayland is an Xorg DDX that depends on
glamor/EGL, `xkbcommon`, `libinput`-style input, and a Wayland client library. On IRIX 7 it would
have to be replaced by a custom X server anyway, at which point the Wayland layer is pure overhead.

**EDF and the graphics server.** A compositor can be made deadline-driven, and wlroots/Smithay
damage tracking is good. But the per-frame work is proportional to the screen, not the damage, once
a full-screen client animates; an EDF budget would have to be large, and media deadlines would
compete with composition instead of bypassing it.

**Blitter.** Poor. The compositor's job is to sample client buffers, which on this hardware means
CPU reads and VRAM writes. REX3 can blit VRAM-to-VRAM and fill, but it cannot read a main-memory
ARGB buffer, blend it, and write VRAM. A Wayland renderer here is a software rasteriser using the
CPU, with the blitter used only for the final copy.

**Verdict.** Reject as the native protocol. Keep the ideas: damage rectangles, buffer handles,
frame callbacks, explicit commit. They can be adopted without the protocol, the ARGB assumption or
the Linux dependency stack.

---

### 5.3 Direct framebuffer + tiny window manager, no compositing

**Architecture.** The server owns the framebuffer and a clipping list. Clients draw directly into
the framebuffer through server-mediated operations; the server clips each operation to the
client's visible region and to the stacking order. There is no per-window backing store by default;
when a window is uncovered, the server either sends an expose event and the client repaints, or
restores a save-under rectangle. This is the X11 model without the X protocol, and the classic
1990s workstation model.

**Memory and CPU.** The cheapest possible: no per-window buffers, no composition, no readback.
Drawing is proportional to the primitive's area, and a clipped fill or blit is a single REX3
operation. The cost is paid in repaint storms: dragging a window over another forces the obscured
client to redraw, and a client that is slow to repaint leaves holes.

**Latency.** The lowest of any option: a drawing op goes to the server, is clipped, and is executed
by REX3 immediately; there is no frame assembly. Input-to-photon is bounded by the client's repaint,
not by a compositor.

**Suitability for 1993 hardware.** Excellent, and historically proven: this is how X11, rio and
Windows 3.x worked. The weaknesses are user-visible (flicker, exposure handling, no translucent
overlays) rather than architectural.

**Rust ecosystem.** Trivial; no dependencies.

**Licence.** Own code, GPL-3.0.

**Effort.** 3–5 person-months for the server and WM; the hard part is client-side exposure
correctness, which is a per-application burden.

**X11 integration.** The closest match of any option: X11's own model is this model, so an X server
can share the framebuffer and the clipping list with native clients if both are mediated by the
same server. In practice two window managers cannot both own the screen; the native WM must manage
X top-level windows (rootless) or the X server must run in a box.

**EDF and the graphics server.** Excellent: no periodic compositor, only per-op work. The server can
be a simple priority task, and media can take the screen.

**Blitter.** Ideal: REX3's span masks and clipping are designed for exactly this.

**Verdict.** The correct baseline, but incomplete. Without retained images, modern interactive
behaviour (overlaps, menus, tooltips, drag) is painful. Adopt its direct-draw path and add
retained images for obscured windows (§5.4/§7).

---

### 5.4 Plan 9 `rio`-style window system

**Architecture.** Plan 9's window system is a file server. Clients open `/dev/draw/new`, receive a
connection number and the display image's geometry, then allocate images and draw by writing
binary messages to `/dev/draw/n/data`; the server stores the image bytes and executes the
operations [DRAW]. The `8½` paper describes the design: "the clients do not store the actual data
for any of their bitmaps locally. Instead, the protocol provides a message to allocate a bitmap, to
be stored in the server, and returns to the client an integer identifier ... no bitmap graphics
operations are executed in the client at all; they are all performed on its behalf by the server"
[P9]. There are 23 messages, about half for fonts; the bitblt message is a small fixed header
(opcode, destination id, destination point, source id, source rectangle, function code) [P9].

`rio` is the window manager built on this: each window is an image in the graphics server, and rio
multiplexes the draw protocol so each client believes it has a private screen [RIO1, RIO4]. The
server does the clipping and the layers; rio mostly rearranges messages and handles the menu and
window furniture. The paper's implementation note is the key: "to manage the overlapping windows it
uses the layers model ... Thus it has little work to do and is a fairly simple program" [P9].

**Memory and CPU.** Retained images cost width × height × depth per window, in the server. At 8 bpp
a 640×480 window is 300 KB; ten of them are 3 MB, which is affordable on a 32 MB machine. At 24 bpp
the same ten are 9 MB, which is not. The protocol is compact (tens of bytes per op) and the server's
work is a switch statement plus clipping, which is exactly what the R4400 is good at. The 8½
server was 2,295 lines plus 2,214 lines of graphics operations on a 68020 [P9].

**Latency.** One write per batch of ops; the server executes them in order. Because images are
server-resident, there is no upload per frame: a scrolling text window is a sequence of blits
within VRAM, and a video window can be an image that the server blits. This is the lowest-latency
retained model available and it maps directly onto REX3.

**Suitability for 1993 hardware.** Excellent. It was designed for raster displays of this era, with
palette colour, no alpha, and a slow CPU. Its one modern gap is that it assumes a file-server
interface (9P) and a per-window image in server memory; on Indy those images should be
memory-resident at screen depth, with in-place drawing into the scanout image for the topmost
window (offscreen VRAM is too small for per-window images). The protocol should be a capability call
or a shared ring rather than a file write, to avoid syscall overhead per op.

**Rust ecosystem.** No crate needed; the design is small enough to reimplement. A Rust version can
use a `#[repr(C)]` message enum and a ring buffer, with a fuzz target for the decoder.

**Licence.** Plan 9 from Bell Labs is distributed under the Lucent Public License; 9front has its
own permissive licence. IRIX 7 should **clean-room reimplement the design**, not copy code, both
because of licence compatibility and because the capability model differs. Ideas and interfaces are
not copyrightable; the code is.

**Effort.** 4–7 person-months, because the protocol and server are small and the hard parts (fonts,
clipping, layers) are well documented.

**X11 integration.** Rio has no X11 story; X would run as a client window (a nested X server) or
take over the screen. That is acceptable but weaker than a custom server that anticipates X11
(§8).

**EDF and the graphics server.** Good: retained images mean no per-frame composition; a video
client can draw into its image and the server blits on damage. But rio's model does not distinguish
media from UI, so a large blit can monopolise the server; the IRIX 7 version needs a budget and a
display lease.

**Blitter.** Excellent. The `bitblt` message is literally an REX3 blit; the font messages are glyph
blits; the layer library is a clipping list.

**Verdict.** The best structural prior art. Adopt it, with three changes: capability handles instead
of 9P files, in-place drawing into the scanout image for the topmost window, and an EDF-aware
scheduler with a display-lease path.

---

### 5.5 Arcan

**Architecture.** Arcan is a display server with a scripting layer (Lua) and a frameserver model.
Clients are separate processes that connect over `shmif`, a shared-memory interface with an event
queue and a command queue; the server composes their buffers and can apply shaders. It has
bridges for X11 (`xarcan`), Wayland (`arcan-wayland`), SDL and others, and a headless/"lwa"
mode [ARCAN]. The design is interesting for IRIX 7 because `shmif` is an explicit, versioned,
shared-memory protocol with a clear separation between the server and a frameserver, which is
close to the capability model.

**Memory and CPU.** Arcan's normal path is GPU-accelerated (EGL/GLES), with a "lwa" software path
that is not the primary target. The scripting layer runs LuaJIT, which is fast for Lua but is
another runtime in the trusted display path. A 1993 port would have to replace the GL renderer with
a software one and the Lua scripts with compiled policy, at which point little of Arcan remains.

**Latency.** One shared-memory hop per frame plus the server's composition; the event queue is
designed for interactive use, not for a 16.7 ms deadline with a 33 MHz blitter.

**Suitability for 1993 hardware.** Poor. GL/EGL, LuaJIT, and a large C codebase are three
dependencies too many. The shmif design is worth studying; the implementation is not portable.

**Rust ecosystem.** None; Arcan is C.

**Licence.** Mostly 3-Clause BSD, but `arcan_lua.c` is GPL-2.0 and some frameservers are GPL-2.0+
or LGPL [ARCAN-COPYING]. GPL-2.0-only code is incompatible with GPL-3.0, so a GPL-3.0 IRIX 7
cannot link Arcan as a whole without careful separation. That is a legal annoyance on top of the
technical mismatch.

**Effort.** Very high: porting the platform layer, the GL renderer and the scripting runtime, then
rewriting the scripts.

**X11 integration.** `xarcan` is a working example of an X server inside a window, which is useful
prior art for §8. It is not a reason to adopt Arcan.

**EDF and the graphics server.** The frameserver model adds a process hop per frame and the Lua
layer adds unbounded execution; both are hostile to soft real-time.

**Blitter.** No. Arcan's renderer is GL; a software renderer would be a CPU rasteriser.

**Verdict.** Reject, but read `shmif`'s protocol design as prior art for capability-passed buffers.

---

### 5.6 BeOS `app_server`-style

**Architecture.** BeOS's `app_server` is a userspace server that owns the display and draws on
behalf of applications. Applications have `BWindow`/`BView` objects; drawing calls are sent to the
server, which maintains per-window state, clipping and a backing bitmap, and blits to the screen.
`BDirectWindow` gives a trusted application a direct view of the framebuffer with a clipping
region, for video and games. The server talks to hardware through an "accelerant" add-on, a
userland driver module that exposes 2D acceleration hooks. Haiku's open-source reimplementation
shows the structure: `AppServer`, `Desktop`, `ScreenManager`, `Screen`, `ServerWindow`,
`ServerBitmap`, `DrawingEngine`, `HWInterface`, `AccelerantHWInterface`, `UpdateQueue`,
`DirectWindowInfo`, `ClientMemoryAllocator`, and a `View`/`Window`/`Workspace` layer
[HAIKU].

**Memory and CPU.** Per-window backing bitmaps at screen depth; BeOS ran on 8–64 MB machines in
1995, so the model is proven at IRIX 7's memory scale. Drawing is server-side and can use the
accelerant; the cost is a message per drawing call (or per batch) and a blit from the backing
bitmap to the screen. That blit is VRAM-to-VRAM if the backing bitmap is in VRAM, which is the
right configuration for Indy.

**Latency.** Asynchronous messages decouple the client from the server, which is good for
throughput and bad for latency if the client does not sync. BeOS's `BView::Sync()` and the server's
update queue are the mechanisms that bound it. A Rust version should make commit explicit and
batch ops, rather than one message per primitive.

**Suitability for 1993 hardware.** Very good. This is the closest complete example of a 2D-only,
userspace, per-window-backing-store display server with a hardware abstraction for acceleration.
The lessons: keep the server single-threaded per screen, batch updates, and give video a direct
path.

**Rust ecosystem.** None; Haiku's server is C++ and large (70+ translation units in
`src/servers/app` [HAIKU]). A clean-room Rust design can take the structure without the code.

**Licence.** Haiku is MIT; BeOS itself was proprietary. Clean-room reimplementation of the
architecture is fine.

**Effort.** 8–12 person-months if reimplemented from scratch with the full view hierarchy,
decorators and workspace model; 6–9 months for the subset IRIX 7 needs.

**X11 integration.** None built in; X would be a client with a surface, or a mode switch.

**EDF and the graphics server.** Per-window threads in the BeOS design are a scheduling liability:
many server threads at various priorities make EDF analysis hard and invite priority inversion.
A Rust version should use one server thread plus a bounded worker pool, not a thread per window.

**Blitter.** Good: the accelerant model is exactly a blitter abstraction, and Haiku's
`AccelerantHWInterface` is a worked example of routing 2D ops to hardware.

**Verdict.** The best source of structural ideas after Plan 9. Borrow the per-window backing store,
the update queue and the accelerant split; do not borrow the thread-per-window model.

---

### 5.7 NeXT Display PostScript / Quartz

**Architecture.** NeXT's `WindowServer` ran a Display PostScript interpreter; applications sent
PostScript programs over a Mach port, and the server executed them against a graphics state,
rasterising text and shapes into the framebuffer. Quartz replaced DPS with a PDF-based model:
drawing is described as PDF operators into a context, and the WindowServer composites window
surfaces; later versions moved composition to the GPU with Quartz Extreme. The Quartz 2D
programming guide documents the PDF-based drawing model that survives in Core Graphics [QUARTZ].

**Memory and CPU.** An interpreter is a large amount of code and state: font machinery, a graphics
state stack, path flattening and rasterisation. NeXTSTEP needed 8 MB and a 25 MHz 68030/040 to run
acceptably at 1120×832, which sounds comparable to an Indy, but the comparison is misleading: the
Indy has a blitter and a much faster CPU, while DPS's rasteriser does not use the blitter for
anything except images. Every glyph and line is interpreted and rasterised in software.

**Latency.** PostScript operators are buffered and can be arbitrarily expensive (a `forall` loop, a
huge path). The server has no bounded per-request cost, which is the opposite of what an EDF system
needs. Text-heavy UI is fast enough in practice because glyphs are cached, but the tail latency is
bad.

**Suitability for 1993 hardware.** Marginal. A PostScript interpreter on an R4400 could be made to
work, but it is a large, subtle, memory-hungry component, and it turns the blitter into a
second-class citizen. The model is also display-list-oriented, which is the wrong abstraction for
a device that executes spans and blits.

**Rust ecosystem.** None. There is no production DPS interpreter in Rust, and writing one is a
multi-year project. PDF/PostScript crates exist for parsing, not for interactive rasterisation.

**Licence.** DPS is Adobe proprietary; PostScript is Adobe IP; Quartz is Apple proprietary. Neither
can be reimplemented under GPL-3.0 from the original code, and a clean-room PostScript interpreter
is a patent and trade-secret minefield. This alone disqualifies the option.

**Effort.** 36+ person-months for a credible interpreter and font system, plus the window server.

**X11 integration.** None. DPS and X11 are alternative display worlds; running X on top would mean
writing an X server anyway.

**EDF and the graphics server.** Bad: unbounded interpreter execution in the display path.

**Blitter.** Poor: the interpreter rasterises into a framebuffer; REX3 is used only for image
uploads and copies.

**Verdict.** Reject. Quartz's PDF model is elegant for printing and resolution independence, but it
is the wrong cost structure for a 1993 blitter.

---

### 5.8 Sun NeWS

**Architecture.** NeWS was a networked window system whose server executed PostScript. Clients sent
PostScript programs to the server, which ran them in lightweight processes with their own graphics
state; the server could be extended by downloading code, and an X11 server (`Xnews`) was
implemented in PostScript on top of it. The canonical description is *The NeWS Book* [NEWS].

**Memory and CPU.** A PostScript interpreter plus a per-client execution context. On Sun 3
hardware, NeWS had a reputation for being elegant and slow; the interpreter cost dominated
interactive drawing. The extensibility model (arbitrary client code in the server) makes the
server's worst-case execution time unbounded.

**Latency.** Network-oriented and interpreter-bound; the lightweight-process model gives good
concurrency but not bounded latency.

**Suitability for 1993 hardware.** Poor to marginal. The Indy is faster than a Sun 3, but the
fundamental cost (interpret every drawing operation) is the same, and the blitter is underused.

**Rust ecosystem.** None.

**Licence.** Proprietary Sun/Adobe; source was released under a Sun licence but not a
GPL-3.0-compatible one. Clean-room reimplementation of PostScript is the same problem as DPS.

**Effort.** Enormous; NeWS is a research system with a language runtime.

**X11 integration.** `Xnews` is historically fascinating (an X server written in PostScript) and
practically a cautionary tale: it was slower and less compatible than a native X server.

**EDF and the graphics server.** Bad: arbitrary client PostScript executes in the server.

**Blitter.** Poor.

**Verdict.** Reject. The one idea worth keeping is that the window system can be programmable, but
IRIX 7 should get programmability from Rust cells, not from a server-side interpreter.

---

### 5.9 Android SurfaceFlinger

**Architecture.** SurfaceFlinger is Android's system compositor. Applications render into buffers
allocated by `gralloc` and queued through `BufferQueue`; SurfaceFlinger consumes queued buffers and
composes them, either through the Hardware Composer HAL (overlay planes) or the GPU (RenderEngine).
Vsync drives the pipeline, and triple buffering is the norm. The Android graphics architecture
documents the buffer queue and the SurfaceFlinger/HWC split [BQ, SFHWC].

**Memory and CPU.** Three buffers per active surface is the design assumption, plus gralloc metadata
and a binder round trip per queue operation. On a 32 MB machine, three 1280×1024×4 buffers are
15 MB for one full-screen app. The HWC path assumes display hardware with overlay planes; Indy has
one framebuffer and no overlay scaler.

**Latency.** Designed for 60 Hz with a vsync pipeline and a deep buffer queue; the latency is
typically 2–3 frames, which is acceptable on a phone and wasteful on a machine with a 33 MHz
blitter and no GPU. "Project Butter" exists precisely because the naive pipeline was too janky.

**Suitability for 1993 hardware.** Very poor. It is a GPU-era architecture with a HAL for display
composition, a binder IPC layer, and a large C++ runtime.

**Rust ecosystem.** The platform is C++ (Rust is supported for some components but not the graphics
stack). No reusable compositor crate.

**Licence.** Apache-2.0, which is compatible with GPL-3.0, so licensing is not the issue.

**Effort.** Enormous; porting binder, gralloc, HWC and RenderEngine is a project larger than IRIX 7
itself.

**X11 integration.** None; Android does not run X natively.

**EDF and the graphics server.** Vsync-driven and buffer-queue-based; a real-time media thread would
compete with SurfaceFlinger's composition and binder traffic.

**Blitter.** Poor: the compositor expects GPU overlays or a GPU renderer.

**Verdict.** Reject. The one idea worth borrowing is explicit buffer state (FREE/QUEUED/ACQUIRED)
with fences, which a native protocol can adopt in a much simpler form.

---

### 5.10 Chrome Viz

**Architecture.** Viz is Chromium's visual compositing service: a display compositor that aggregates
surfaces (quads) from renderers and draws them, with a GPU process and a software fallback. Its
README describes it as "the client library and service implementations for compositing and gpu
presentation", with Mojo interfaces, hosts, clients and services [VIZ]. The compositor (`cc`)
maintains a layer tree, tiles, damage and a display list; the display compositor draws the final
frame.

**Memory and CPU.** Multiple processes, multiple full-frame buffers, tile memory, and a Skia
rasteriser. The design is built for web pages with thousands of layers and for GPU rasterisation;
the software path is a fallback, not the target.

**Latency.** Multi-process commit and aggregation, typically one to two frames; the architecture
optimises throughput and correctness across processes, not a 16.7 ms deadline on a uniprocessor.

**Suitability for 1993 hardware.** Absurd. Chromium's graphics stack is millions of lines and
assumes gigabytes of RAM.

**Rust ecosystem.** None (C++); the ideas (surface aggregation, quads, damage) are documented and
can be borrowed conceptually.

**Licence.** BSD-3-Clause, GPL-3.0-compatible.

**Effort.** Not applicable.

**X11 integration.** None relevant.

**EDF and the graphics server.** Hostile: multiple processes, Mojo IPC, unbounded rasterisation.

**Blitter.** Poor.

**Verdict.** Reject. Read the damage/quad model for vocabulary, nothing more.

---

### 5.11 Windows DWM

**Architecture.** Since Windows Vista, the Desktop Window Manager redirects each top-level window's
drawing into an offscreen surface and composites those surfaces with Direct3D. Composition is always
on; applications that need direct access to the primary surface (full-screen games) cause DWM to
disable composition until they release it. Microsoft's documentation states that DWM uses the GPU
for desktop composition and that full-screen applications must disable it [DWM].

**Memory and CPU.** A redirected surface per window plus GPU resources; DWM's memory footprint and
GPU requirement are routine on 2006 hardware and impossible on an Indy. A software Direct3D
rasteriser (WARP) is not available on MIPS and would be far too slow anyway.

**Latency.** GPU composition with a compositor-owned frame queue; typically one to two frames, with
`DwmFlush` for synchronisation. The model is sound; the implementation is not portable.

**Suitability for 1993 hardware.** Very poor. It assumes a GPU, a compositor that can sample
textures, and enough VRAM for per-window surfaces.

**Rust ecosystem.** None.

**Licence.** Proprietary.

**Effort.** Not applicable.

**X11 integration.** None.

**EDF and the graphics server.** DWM runs at a fixed compositor cadence; real-time apps disable it.
That is a useful pattern (composition off for full-screen), not an architecture to port.

**Blitter.** Poor.

**Verdict.** Reject. Keep the "full-screen disables composition" escape hatch.

---

### 5.12 Qt Compositor / Mir

**Architecture.** Qt Wayland Compositor is a Qt module that provides QML and C++ APIs for building a
Wayland compositor; client content is displayed through Qt Quick's scene graph, which is
GPU-accelerated (OpenGL/Vulkan/Metal) and can fall back to software in limited configurations
[QTCOMP]. Mir is a C++ library for building Wayland-based shells, with `MirAL` for window
management policy, used by Ubuntu Touch and embedded products [MIR]. Both are full compositor
frameworks, not protocols.

**Memory and CPU.** A Qt runtime is tens of megabytes; a QML scene graph wants a GPU and a GL
driver. Mir's server plus its GL renderer is similarly heavy. Neither fits in 32 MB alongside
applications, and neither has a Newport backend.

**Latency.** Both are designed for 60 Hz GPU composition; latency is fine on target hardware, not
on a CPU rasteriser.

**Suitability for 1993 hardware.** Poor.

**Rust ecosystem.** None; both are C++ with generated bindings at best.

**Licence.** Qt Wayland Compositor is GPL-3.0 or commercial [QTCOMP]; Mir is GPL-3.0. Both are
GPL-3.0-compatible, so licensing is not the blocker; the runtime is.

**Effort.** Very high: port Qt or Mir's platform layer, GL, input, and the scene graph.

**X11 integration.** Both can host XWayland in principle, with the same porting problem as §5.2.

**EDF and the graphics server.** GPU-compositor scheduling; not EDF-aware.

**Blitter.** Poor.

**Verdict.** Reject.

---

### 5.13 X11 itself as the native protocol

**Architecture.** X11 is a client/server protocol with server-side resources (windows, pixmaps,
fonts, graphics contexts), server-side drawing, clipping, and an event model. The X server owns the
display and executes requests; clients send requests over a transport and receive events and replies
[XPROTO]. The X.Org server's DIX/DDX split separates protocol handling from device-dependent
drawing, and its acceleration architectures (XAA, EXA) route 2D ops to hardware.

**Memory and CPU.** X11's model is close to the hardware: server-side pixmaps and windows at screen
depth, drawing ops executed by the server, no per-frame composition. A well-implemented X server on
Indy would use REX3 for spans, fills and copies, and would be fast. The cost is the protocol's
generalisation: resource tables, atoms, properties, selections, extensions, and a large request
space.

**Latency.** Good: requests are batched, drawing is immediate, and there is no compositor. Round
trips (font queries, geometry) are the latency trap, as they always were.

**Suitability for 1993 hardware.** Historically excellent — X11 was designed for this class of
machine, and IRIX itself shipped X11 with 4Dwm. The problem is not performance; it is architecture.

**Rust ecosystem.** `x11rb` is a mature Rust X11 client library (MIT/Apache-2.0). There is no
mature pure-Rust X server. Using X11 natively means native apps are X clients, losing the
capability model, the object ABI and the framekernel's scheduling integration.

**Licence.** X.Org is MIT; `x11rb` is MIT/Apache-2.0. Compatible.

**Effort.** Reusing an existing X server (Xorg) would require porting its DDX, input, and OS layer
to IRIX 7: 12–24 person-months, and it would still not be the native API. Writing a new X server is
more.

**X11 integration.** It is X11, so integration is trivial and the native/compat distinction
disappears — which is the problem. IRIX 7 would be an X11 OS with a Rust kernel, not a
capability-based OS with an X11 compatibility cell.

**EDF and the graphics server.** The X server's scheduling is not EDF-aware; requests are processed
in arrival order, and a large `PutImage` or a font operation can block the server for a long time.
It can be wrapped in a budget, but not controlled at the op level without modifying it.

**Blitter.** Good in principle (XAA/EXA), poor in practice on Newport without a dedicated DDX.

**Verdict.** Reject as the native protocol; retain X11 as the compatibility cell. The X protocol's
server-side drawing model is evidence for §7, not a candidate for the native ABI.

---

### 5.14 No window system at all (full-screen cells)

**Architecture.** Each cell gets the display exclusively; switching is a mode change or a
surface swap. There is no multiplexing, no window manager, no clipping, and no server drawing on
behalf of clients. The cell owns a scanout surface and draws into it directly (through the kernel's
framebuffer mapping or a capability).

**Memory and CPU.** Minimal: one screen buffer, no per-window state, no compositor. The cell can
drive REX3 directly if it is trusted, or through a thin kernel interface.

**Latency.** The best possible: the application's drawing goes to the display with no server hop.
For a media player or a game, this is ideal.

**Suitability for 1993 hardware.** Perfect for the workloads it fits (full-screen video, games,
kiosks, presentations) and unusable as a general desktop: no dialogs, no clipboard between apps, no
way to run two programs side by side.

**Rust ecosystem.** Trivial.

**Licence.** Own code.

**Effort.** Days to weeks for the mode/surface switch; the applications do the rest.

**X11 integration.** Mode switch: the X server takes the display, native cells are suspended. This
is exactly how a "X session" works on IRIX and how full-screen games work on every OS. It is simple
and fast, and it means X and native apps cannot be visible together.

**EDF and the graphics server.** The best possible: the media thread owns the display and its
deadline is the frame period. No server budget, no priority inversion.

**Blitter.** Direct ownership; the cell can use REX3 or write the framebuffer.

**Verdict.** Not a window system, but a necessary escape hatch. Every design in §7 should include a
"take the display" capability for full-screen cells, with the same semantics as DWM disabling
composition.

---

## 6. Comparison

Two tables are used because fifteen columns do not read well. Table A covers the hardware-facing
axes (the ones that decide whether a design can exist on an Indy); Table B covers the
engineering-facing axes. Both use the same row order.

**Reference case for the cost columns:** one full-screen 1280×1024 window, and one full-frame update
at 60 Hz. Frame sizes: 1.25 MiB at 8 bpp, 3.75 MiB at 24 bpp, 5 MiB at 32 bpp (§3.2). A typical
800×600 window is 36% of these figures.

### Table A — Hardware fit

| # | Option | Rendering model | Memory for one full-screen window | Work per full-frame update | Latency (typical) | 1993 fit | Uses REX3? |
|---|---|---|---:|---|---|---|---|
| 1 | **Minimal custom** (recommended shape) | Server-side drawing, retained images, damage | 1.25 MiB @ 8 bpp; 3.75 @ 24 | Damage-proportional; full frame only on expose | 1 frame | **Excellent** | Yes, direct |
| 2 | **Wayland** (Smithay/wlroots/etc.) | Client renders to `wl_shm`/DMABUF, server recomposites | 5 MiB per ARGB buffer × 2–3 + scanout | Full-frame read + write + alpha | 1–2 frames | **Poor** | Only final copy; CPU blend |
| 3 | **Direct FB + tiny WM** | Server-mediated drawing, clipping, no retention | 0 (no backing store) | Damage-proportional, plus client repaints | < 1 frame | **Excellent** | Yes, direct |
| 4 | **Plan 9 `rio`** | Server-resident images, draw messages, layers | 1.25 MiB @ 8 bpp (server) | Damage-proportional; blit from image | 1 frame | **Excellent** | Yes, blit/font ops |
| 5 | **Arcan** | shmif buffers, GL composition, Lua policy | 5 MiB+ per buffer, GL textures | Full-frame GL/CPU composition | 1–2 frames | **Poor** | No |
| 6 | **BeOS `app_server`** | Server-side drawing, per-window backing, accelerant | 1.25 MiB @ 8 bpp per window | Message + backing→screen blit | 1–2 frames | **Very good** | Yes, via accelerant |
| 7 | **NeXT DPS / Quartz** | PostScript/PDF interpreter in server | Interpreter state + buffers, several MiB | Interpreter + software raster | 1–3 frames, unbounded tail | **Marginal** | Only for images |
| 8 | **Sun NeWS** | PostScript VM, downloadable programs | Interpreter + per-client VM | Interpreter | Unbounded | **Poor** | No |
| 9 | **Android SurfaceFlinger** | BufferQueue + HWC/GPU composition, triple buffering | 15 MiB for 3× ARGB full-screen | GPU composition; CPU fallback heavy | 2–3 frames | **Very poor** | No |
| 10 | **Chrome Viz** | Surface aggregation, quads, GPU/software raster | Multiple full-frame buffers + tiles | Multi-process aggregation + raster | 1–2 frames | **Absurd** | No |
| 11 | **Windows DWM** | Per-window redirection surfaces, D3D composition | Per-window GPU surfaces | GPU composition | 1–2 frames | **Very poor** | No |
| 12 | **Qt Compositor / Mir** | Wayland compositor frameworks, GL scene graph | Qt/Mir runtime + GL buffers | GPU composition | 1–2 frames | **Poor** | No |
| 13 | **X11 as native** | Server-side drawing, resources, clipping | 1.25 MiB @ 8 bpp (pixmaps/windows) | Damage-proportional | < 1 frame | **Excellent** (historically) | Yes, with a DDX |
| 14 | **No window system** | Exclusive full-screen surface | 1.25 MiB @ 8 bpp | None (app draws direct) | 0 | **Excellent for media** | Direct ownership |

For designs that draw in place, the memory figure is the *retained backing store* for a window that is
not on screen, not the scanout image, which exists in every design. The scanout image itself is
1.25 MiB at 1280×1024 8 bpp.

### Table B — Engineering fit

| # | Option | Rust ecosystem | Licence | Effort (person-months) | X11 coexistence | EDF compatibility | Capability fit | Weighted score /5 |
|---|---|---|---|---|---|---|---|---|
| 1 | **Minimal custom** | None needed; own code | GPL-3.0 (own) | 10–12 | Excellent (surface client; rootless or boxed) | Excellent (sporadic server + lease) | Excellent | **4.8** |
| 2 | **Wayland** | Mature on Linux; Linux-shaped port | MIT / GPL-3.0, compatible | 24–36 | XWayland pattern, but Xwayland needs a Linux-ish stack | Fair (screen-proportional) | Fair | 1.6 |
| 3 | **Direct FB + tiny WM** | None needed | GPL-3.0 (own) | 3–5 | Good (shared clipping; one WM) | Excellent | Fair (no retained handles) | 4.6 |
| 4 | **Plan 9 `rio`** | None needed; small reimplementation | LPL; clean-room only | 4–7 | Weak (X as a client or mode switch) | Good (needs budget/lease added) | Good | 4.7 |
| 5 | **Arcan** | None (C) | BSD-3 + GPL-2.0 parts; GPL-3.0 conflict risk | 18–30 | Good (`xarcan` prior art) | Poor | Fair | 1.2 |
| 6 | **BeOS `app_server`** | None (C++), structure reusable | MIT (Haiku) / proprietary BeOS | 8–12 | Weak (X as client) | Poor if thread-per-window; fair if redesigned | Good | 3.3 |
| 7 | **NeXT DPS / Quartz** | None | Proprietary (Adobe/Apple) | 36+ | None | Poor (unbounded interpreter) | Poor | 1.1 |
| 8 | **Sun NeWS** | None | Proprietary (Sun/Adobe) | 36+ | Historical `Xnews`, slow | Poor | Poor | 0.9 |
| 9 | **Android SurfaceFlinger** | None (C++) | Apache-2.0, compatible | 36+ | None | Poor | Fair | 1.4 |
| 10 | **Chrome Viz** | None (C++) | BSD-3, compatible | Not viable | None | Poor | Poor | 0.4 |
| 11 | **Windows DWM** | None | Proprietary | Not viable | None | Fair (disable for full-screen) | Poor | 0.4 |
| 12 | **Qt Compositor / Mir** | None (C++) | GPL-3.0, compatible | 24–36 | XWayland pattern | Fair | Fair | 0.7 |
| 13 | **X11 as native** | `x11rb` client mature; no Rust server | MIT, compatible | 12–24 (port Xorg) | N/A (is X11) | Poor (request-order server) | Poor | 3.9 |
| 14 | **No window system** | Trivial | GPL-3.0 (own) | < 1 | Mode switch only | Excellent | N/A | n/a (not a window system) |

**Reading the table.** The scores cluster into three groups. Options 1, 3 and 4 score within 0.2 of
each other, and that is the honest result: they are the same family, and the differences between
them are about retained images, not about architecture. The recommendation in §7 is a synthesis of
all three — the direct-draw path of option 3, the server-resident images of option 4, and the
capability/damage design of option 1. Option 14 scores highest on the axes it is scored on but is
not a window system and is excluded from the ranking; it survives as the full-screen escape hatch.
Option 13 (X11) is the only rejected option with a competitive hardware score, which is exactly why
it is a serious temptation and why §7 rejects it on architecture rather than performance.

The Wayland family (2, 12) and the mobile/web/desktop compositors (9, 10, 11) all fail on the same
three axes: they assume client-side ARGB rendering, per-frame recomposition, and a GPU. NeWS and DPS
fail on a fourth: an interpreter in the display path. Arcan and BeOS are interesting but are large
C/C++ codebases whose useful ideas can be reimplemented faster than they can be ported.

---

## 7. Recommendation: the IRIX 7 display server

### 7.1 The thesis in one paragraph

IRIX 7 should have a **display server, not a compositor**. The server owns the Newport pipeline,
stores window contents as images it can draw into, and executes drawing operations on behalf of
clients through REX3. It repairs the screen by redrawing damage and by restoring obscured regions
from retained images where those exist, and it asks clients to repaint where they do not. It never
assembles a complete frame from client buffers as a matter of course. This is the Plan 9 `8½`
model with VRAM-aware residency, capability handles and an EDF budget; it is the BeOS `app_server`
model without the thread-per-window design; it is the X11 model without the X protocol.

The word "compositor" should be avoided in the design documents, because it invites the assumption
that every frame is assembled from surfaces. On this hardware that assumption is the difference
between a system that runs at 76 Hz and one that runs at 12.

### 7.2 Structure

```
                    ┌──────────────────────────────────────────────┐
   native cell      │  display cell (userspace, GPL-3.0 Rust)      │
   (Rust, SAS)      │                                              │
  ┌──────────┐      │  ┌─────────┐   ┌──────────┐   ┌───────────┐  │
  │ app logic│      │  │ protocol│   │ window   │   │ REX3      │  │
  │ + toolkit│◄────►│  │ decoder │──►│ tree +   │──►│ command   │  │
  │          │ ring │  │ + auth  │   │ clipping │   │ batching  │  │
  └────┬─────┘      │  └─────────┘   └──────────┘   └─────┬─────┘  │
       │ mapped     │        │                            │        │
       │ surface    │        ▼                            ▼        │
  ┌────▼─────┐      │  ┌──────────┐              ┌──────────────┐  │
  │ surface  │      │  │ backing  │              │ VRAM:        │  │
  │ (VRAM or │      │  │ store    │              │ scanout +    │  │
  │  memory) │      │  │ (memory) │              │ live windows │  │
  └──────────┘      │  └──────────┘              └──────────────┘  │
                    └──────────────────────────────────────────────┘
        ▲ input: IOC2 kbd/mouse ──────────────┘
        │
   X11 compat cell: X server → one native surface (nested) or a display lease
```

The display cell is a normal userspace cell with capability rights to the graphics MMIO, the
framebuffer VRAM, the interrupt object and the input device. It is not in the kernel. A kernel-level
"console" driver exists only to hand over the display at boot and to take it back on a panic; it
does not implement drawing.

### 7.3 Surfaces, formats and residency

A **surface** is a capability-addressed object with:

| Property | Values | Rationale |
|---|---|---|
| Geometry | width, height, stride | Stride must be explicit: REX3 spans are not required to be power-of-two |
| Format | `I8` (indexed, 8 bpp), `RGB24` (three 8-bit planes / packed for upload), `I4` (4 bpp, optional) | Matches XMAP9 and RB2; no ARGB in v1 |
| Residency | `VRAM`, `MEMORY`, `AUTO` | VRAM is scarce and must be allocated deliberately |
| Flags | `OPAQUE`, `RETAINED`, `SCANOUT_ELIGIBLE`, `CURSOR` | Policy hints, not rights |
| Rights (handle) | `DRAW`, `PRESENT`, `READ`, `MAP` | Enforced by the framekernel, not by the server |

**No 32-bit format in the native protocol.** An ARGB surface would be 4 bytes/pixel, need CPU
blending, and cannot be scanned out. If a client has 32-bit source data (a decoded image, a legacy
blit), it converts on upload to `I8` or `RGB24`. The protocol's `put_image` op is the only bulk
pixel path, and it is explicitly the slow path.

**Residency policy.** The server sizes VRAM from the ARCS board description (the hardware reference
flags VRAM size as undocumented and warns against assuming it [H §9.7, §18]) and reserves:

1. the **scanout image** (visible mode size), which is also the drawing surface for windows that are
   on screen;
2. the hardware cursor in the XMAP9 auxiliary buffer (32×32 at 2 bpp or 64×64 at 1 bpp [H §9.3]);
3. a small **offscreen pool** for menus, popups and one or two small surfaces. This pool is small by
   construction: REX3 addresses only 64 pixels beyond the visible area [H §9.1], so it is at most a
   few hundred KB even on a 2 MB board, and it is larger only if bring-up proves that extra VRAM is
   addressable;
4. the **font cache** (1 bpp glyph bitmaps, a few hundred KB), taken from the offscreen pool or from
   unused scanout rows when the mode is shorter than the panel.

Everything else lives in main memory as a `RETAINED` backing store at screen depth. `AUTO`
residency means "draw in place if topmost and fully visible, otherwise memory".

The key policy is **in-place residency**. The topmost, fully visible window is not a separate VRAM
surface: it is a rectangle of the scanout image, and the server draws into it in place. There is no
blit to present it and no per-frame work, so a keystroke in a terminal is a glyph blit into VRAM and
nothing else. When that window is about to be obscured, the server either (a) copies the covered
rectangle to its backing store, or (b) discards it and sends `expose` on reveal. (a) costs a VRAM
read (HOSTRW read is 13 MB/s [H §12.1]) and is therefore done lazily, incrementally, and only for
`RETAINED` surfaces; (b) is the default for ordinary UI. This is precisely X11's
backing-store/save-under trade-off, and it is the right one for 1993 memory and bandwidth.

A separate `VRAM` surface is therefore reserved for small objects (menus, popups, cursor) that fit
the offscreen pool. A full-screen window is displayed by drawing in place, not by allocating a
second full-screen buffer.

### 7.4 Protocol

The protocol is a small binary message set, not a generic RPC. Headers are fixed-size and the
encoding is native-endian with an explicit flag in the handshake, because IRIX 7 is big-endian and
byte-swapping every coordinate is waste.

**Control plane (capability calls, one per object lifetime event):**

| Call | Effect |
|---|---|
| `create_surface(w, h, format, residency, flags) -> Surface` | Allocates; charges memory to the client's cell |
| `destroy_surface(Surface)` | Frees; revokes outstanding mappings |
| `map_surface(Surface, rights) -> Mapping` | Only with `MAP` right; returns a capability to the pages |
| `create_ring(Surface) -> Ring` | Shared-memory command/event ring for this surface |
| `lease_display(Surface, mode) -> Lease` | Exclusive full-screen; server stops drawing |
| `revoke(Lease)` | Server repaints and resumes |

**Data plane (ring ops, batched):**

| Op | Fields | Notes |
|---|---|---|
| `FILL` | dst, rect, pixel | REX3 fill / fast clear |
| `COPY` | dst, src, src_rect, dst_pt, rop | REX3 blit; `src` may be the scanout image |
| `SPAN` | dst, y, x0, x1, pixel | Horizontal span, the primitive under most fills |
| `GLYPHS` | dst, font, origin, run[], colour | Server-side glyph cache; one op per string |
| `PUT_IMAGE` | dst, rect, format, offset | Bulk upload from a mapped buffer; slow path |
| `SET_CLIP` | dst, region | Region is a list of rectangles in the ring |
| `COMMIT` | surface, damage[] | Publishes; server coalesces to vblank |
| `FLUSH` / `SYNC` | — | Backpressure and round-trip |

A ring op is 16–32 bytes. A 4 KiB ring holds 128–256 ops, which is enough for a text-heavy frame
without a syscall per primitive. In the single address space, writing the ring is a memory write and
the doorbell is a port operation, not a syscall and not a context switch; a legacy cell in its own
MMU address space pays the kernel's IPC path instead, which is the price of compatibility. The
client writes ops and rings a doorbell port; the server drains
up to a byte budget per scheduling quantum. When the ring is full the client blocks on a port or
gets `EAGAIN`; **the server never grows an unbounded queue**, because an unbounded queue is an
unbounded latency for every other client and an EDF analysis that cannot be done.

**Events** (server → client, same ring, separate region): `EXPOSE(rects)`, `CONFIGURE(w,h)`,
`FOCUS(in/out)`, `KEY`, `BUTTON`, `MOTION`, `CLOSE`, `LEASE_REVOKED`. Events carry a sequence
number so a client can coalesce motion and detect lost exposes.

**Why not 9P/file writes?** The Plan 9 model writes messages to a file, which is elegant and
debuggable. On IRIX 7 a file write per batch means a syscall per batch and a copy from the client's
buffer into the kernel's; the ring avoids both, and the capability model gives the server a
lifetime-safe handle that 9P fids do not. The op set and semantics are still Plan 9's; only the
transport changes. A 9P front-end can be added later for remote clients and for debugging, exactly
as `8½` could run inside one of its own windows [P9].

### 7.5 Drawing and the blitter

The server is the **only** writer to the REX3 FIFO. Clients never touch MMIO. This is a security
property (a cell cannot wedge the display) and a scheduling property (the server can batch and
budget).

Batching rules:

* Ops are accumulated into a command buffer in main memory, then written to the FIFO in chunks no
  larger than the FIFO watermark (the FIFO is a few dozen entries deep [H §9.1]).
* The server checks the FIFO-full status between chunks and yields; it never spins with interrupts
  disabled. A FIFO-full condition is also a GIO interrupt source, which the kernel can expose as an
  event rather than a busy-wait [H §9.1].
* Coordinates are biased by +4096 in X and Y, and writes outside the VRAM boundary are silently
  culled by REX3 [H §9.1]. The server must clamp anyway, because reads are not clipped and a
  malformed `COPY` source rectangle could read outside the surface.
* 24-bit is three 8-bit planes, not packed RGB [H §9.1, §9.7]. The protocol's `RGB24` surfaces are
  stored planar or converted at upload; the server must never assume a packed linear framebuffer.
* The span masks (`SMASK0..4`) and the 24-bit compare/mask (`CIDMATCH`/`CIDMASK`) are the clipping
  and conditional-write primitives. The server programs them per op; it does not emulate clipping
  in software.
* The hardware cursor is programmed directly and never composited. A software cursor would turn
  every mouse move into a damage rectangle and a blit, which is exactly the kind of per-frame work
  this design exists to avoid.

Text is the dominant UI workload, so the font cache is not an optimisation; it is the design. Glyphs
are 1 bpp images in VRAM, uploaded once, blitted with REX3. A proportional font at 8×16 is 16 bytes
per glyph; a 256-glyph font is 4 KiB. Cache the ASCII set plus the client's declared charset; evict
by LRU. The server does not rasterise outlines; a font cell produces bitmaps and uploads them
through the same `PUT_IMAGE` path as any other image.

### 7.6 Composition policy

The server composites only when it must:

| Event | Action |
|---|---|
| Client draws into a fully visible topmost window | Direct REX3 ops into the scanout image, in place; no composition, no copy |
| Client draws into an obscured or partially visible window | Ops go to its backing store (memory); the visible part is blitted to VRAM; the rest is deferred |
| Window raised, moved or resized | Repaint the newly exposed region from backing stores where available; otherwise send `EXPOSE` to the affected clients |
| Window destroyed or minimised | Repaint from the window below; no snapshot |
| Overlay/menu/tooltip | A small topmost surface; still opaque, still blitted, no alpha |
| Full-screen media | Display lease; server draws nothing; on revocation it repaints the desktop |

There is no global back buffer and no frame-wide composition. The scanout image is the only buffer:
drawing goes into it, and the video pipeline scans it out continuously. Tearing is controlled by
coalescing `COMMIT` to the vertical blank where the hardware allows it; where it does not, the
server accepts tearing at window-drag time rather than adding a second full-screen buffer it cannot
afford.

### 7.7 Scheduling and EDF

The display cell is scheduled as a **sporadic server**: a budget `B` in every period `P`, with a
deadline at the next vertical blank minus a safety margin. A starting point is `B = 2 ms`,
`P = 16.7 ms` (about 12% of one CPU at 60 Hz), to be tuned from measured REX3 throughput during
bring-up. The budget is not a cap on drawing throughput; it is a cap on how long the server can
delay a media thread.

Rules that make the budget meaningful:

1. **Bounded batches.** The server drains at most `Q` ring bytes or `T` microseconds per activation,
   whichever comes first, then requeues. A client cannot force an unbounded activation by filling a
   ring.
2. **No blocking under a lock.** The server takes the window-tree lock, computes a batch, releases
   the lock, then submits to REX3. FIFO waits happen outside the lock.
3. **Input has its own short path.** Key and button events are enqueued by a small high-priority
   handler that never draws, so typing does not wait for a blit batch. The drawing path may be
   throttled; the input path may not.
4. **Media bypass.** A cell holding a display lease (or a full-screen `SCANOUT_ELIGIBLE` surface)
   does not go through the server at all. Its EDF deadline is the frame period; the server's
   only involvement is to not draw over it.
5. **Revocation is a scheduling event.** When a lease is revoked (user switches away, a dialog
   appears), the server gets a one-shot budget extension to repaint, or it displays a "restoring"
   frame and repaints over the next few frames.

The server itself should run at a priority above interactive clients and below hard media, in the
same EDF class as soft media but with a smaller relative deadline. It must never be the highest
priority thread in the system: a bug in the display server should degrade the display, not starve
audio.

### 7.8 Input, focus and window policy

Input is read from IOC2 (PS/2 keyboard and mouse via the 82C42) by the display cell, which owns the
device capability. The server hit-tests the window tree, applies focus policy, and sends events.
Pointer motion is coalesced; button and key events are not.

Window management policy (stacking, focus-follows-mouse vs click-to-focus, decorations, virtual
desktops) is **not** in the display server. A separate WM cell consumes a `window-management`
capability that permits `raise`, `move`, `resize`, `focus`, `close` but not drawing. This keeps the
trusted display path small and lets the WM be replaced (tiling, stacking, kiosk) without touching
the server. It also mirrors the river lesson that compositor and WM policy should be separable
[RIVER], without adopting Wayland.

Decorations are drawn by the WM into its own surfaces, or by the server as a thin border if the WM
requests it. The server does not ship a theme.

### 7.9 Capability and failure model

* A surface handle is a capability. Closing the last handle frees the surface; the server cannot
  leak VRAM on client death.
* `MAP` is a separate right. Only cells that the user or the system has granted it can write pixels
  directly; a mapped surface is still subject to `COMMIT` and damage accounting.
* The server validates every rectangle against the surface geometry and, for VRAM objects, the VRAM
  bounds. REX3
  silently culls out-of-range writes but does not clip reads [H §9.1], so validation is mandatory
  for `COPY` sources.
* If the display cell crashes, the kernel's console driver can take the display, draw a panic
  message, and restart the cell. Native clients keep their surfaces (they are framekernel objects)
  and receive `EXPOSE` after restart; X clients lose their connection, as they would if an X server
  died.
* If a client stops responding to `EXPOSE`, the server paints its rectangle with a "not responding"
  stipple rather than freezing the desktop.

### 7.10 Explicit non-goals for v1

These are out of scope not because they are undesirable but because each one costs a multiple of the
whole budget:

* alpha blending, rounded corners, shadows, blur;
* scaling, rotation, fractional scaling;
* client-side decorations as a protocol requirement;
* per-frame full-screen composition;
* colour management beyond an 8-bit CLUT and a 24-bit direct mode;
* multi-monitor (the Indy has one graphics head; ZX/Elan is a different machine);
* remote display over the network (a 9P bridge can come later);
* 3D (there is no 3D hardware; software rasterisation belongs in a client, not the display server).

---

## 8. X11 coexistence

X11 is a separate decision, but the native architecture constrains how X11 can be integrated. Three
models are possible; they differ in who owns the blitter and how many copies a pixel makes.

### 8.1 Model A — display lease (mode switch)

The X server takes exclusive ownership of the display: the native server revokes all surfaces,
stops drawing, and hands over the REX3/XMAP9/VC2 capabilities for the duration. X runs its own
window manager (4Dwm or equivalent) and its own cursor. Switching back restores the native desktop.

* **Cost:** one capability transfer and a full repaint on each switch. No per-pixel cost.
* **Latency:** native, because X drives the hardware directly.
* **Work:** small on the native side; the X side is whatever the X11 decision produces.
* **UX:** no mixing. This is the classic "X session" and the full-screen game model.
* **When:** the correct v1 default if legacy X applications are occasional and full-screen-ish, and
  the mandatory escape hatch for anything that needs direct hardware access.

This is the only model in which X can use REX3 at full speed without the native server relaying its
requests. It is also the only model that is safe if the X server is an unmodified legacy IRIX
binary that expects to program the hardware itself.

### 8.2 Model B — nested X in a native window (shadow framebuffer)

The X server renders into its own memory framebuffer (a "shadow" screen) and the native server
uploads the damaged rectangles into a native surface, which the WM treats as one window. This is the
Xnest/Xvfb/Xephyr shape, except the host is the native display server rather than another X server.

* **Cost:** one extra copy per damaged pixel. At 1024×768×8 bpp a full-screen upload is 786 KB,
  which is 12 ms at the measured 66 MB/s PIO write rate [H §12.1]; typical damage is a fraction of
  that. At 24 bpp the same upload is 2.36 MB and 36 ms, so model B is only comfortable at 8 bpp.
* **Latency:** one frame plus the upload; acceptable for text and dialogs, poor for video or
  animation inside X.
* **Work:** moderate. The X server needs a memory framebuffer and damage tracking (Xorg's shadow
  framebuffer plus `Damage` can provide both); the native side needs `PUT_IMAGE` and a surface.
* **UX:** good enough for legacy applications, and it coexists with native windows.
* **When:** the recommended v1 coexistence model, with the caveat that it is an 8-bit path.

The important property is that model B does not require the X server to understand REX3. It only
requires a linear framebuffer and a damage report, which is exactly what a portable X server already
has. That keeps the X11 port independent of the native graphics design, which is valuable given that
the X11 effort is separately scoped.

If the X server is an **unmodified legacy IRIX binary**, its framebuffer must be allocated by the
compatibility layer as a shareable object when the process is created, so that the display cell can
map it and read the damaged rectangles. Otherwise the X server must be modified to render into a
surface it receives from the native server. The first option is much less work and is another
argument for starting with model A, where the X server owns the display and no sharing is needed at
all.

### 8.3 Model C — rootless embedding (Xwayland shape)

Each top-level X window becomes a native surface, managed by the native WM; the X server keeps its
own root window but does not display it. This is Xwayland's rootless mode, and it is the best UX:
X and native windows share the desktop, the clipboard and the focus model.

* **Cost:** one surface per X window, at screen depth; no extra copy if the X server draws through
  native ops, one copy if it draws into a shadow and uploads per window.
* **Latency:** as good as the native path if drawing is translated; one copy otherwise.
* **Work:** high. The X server must implement the Composite extension's redirection semantics, the
  native WM must manage foreign windows, and the two WMs must agree on stacking, focus and
  decorations. Xwayland's rootless mode is the reference for the protocol mechanics, but its code
  depends on a Wayland client library and cannot be reused directly.
* **When:** a later phase, only if mixed top-level X/native windows are a product requirement.

### 8.4 Recommendation for X11

Ship **model A** first, because it is small, robust and gives legacy applications the hardware they
expect. Add **model B** for coexistence once the native server is stable; it is the best
cost/benefit point and it keeps the X server's graphics backend independent. Treat **model C** as a
stretch goal, not a v1 commitment.

Two constraints apply to all three models:

1. **One blitter owner at a time.** REX3 has a single command FIFO and no context switch. Either the
   native server owns it, or the X server does under a lease, but never both concurrently. A
   "shared framebuffer" in which both program the FIFO is a correctness bug waiting for a race.
2. **The X server's visual model must match the hardware.** IRIX applications expect PseudoColor
   (8-bit indexed) and/or TrueColor visuals, and they allocate colormaps. The native server's `I8`
   surfaces and CLUT are a natural host for PseudoColor; a 24-bit direct mode can host TrueColor.
   An ARGB-only native visual would break colour allocation for legacy apps, which is another reason
   not to adopt one.

---

## 9. Implementation plan and effort

The estimate below is for the native display server and its integration, not for X11 itself. It
assumes a team already familiar with the kernel's capability model and with the Newport hardware
reference.

| Phase | Deliverable | Effort | Notes |
|---|---|---:|---|
| 0 | Hardware bring-up: VC2 mode table, XMAP9 palette, REX3 FIFO, cursor, vblank probe | 1.5–2 pm | Highest uncertainty; see §11 |
| 1 | Surface objects, VRAM allocator, `FILL`/`COPY`/`SPAN`/`COMMIT`, scanout | 2 pm | The core; 8 bpp only |
| 2 | Input (IOC2 PS/2), focus, stacking, expose, WM policy cell | 1.5 pm | First usable desktop |
| 3 | Font cache and `GLYPHS`, clipboard, resize, decorations | 1.5 pm | Text dominates the workload |
| 4 | Ring protocol, batching, backpressure, capability rights, revocation | 1 pm | Security and latency |
| 5 | EDF sporadic server, budget accounting, display leases | 0.5 pm | Needs kernel scheduling hooks |
| 6 | X11 model A, then model B | 1–2 pm (native side) | X server effort separate |
| 7 | Hardening: fuzzing the decoder, fault injection, VRAM exhaustion, client death | 1 pm | Non-negotiable |
| **Total** | | **10–12 pm** | 8,000–12,000 lines of Rust, including tests |

The Plan 9 anchor (5,100 lines of C for the whole system including a terminal [P9]) and the tinywl
anchor (1,101 lines of C for a minimal wlroots compositor, excluding wlroots [TINYWL]) bracket the
estimate from below; the extra comes from capabilities, damage, EDF accounting and the X11 bridge.

A useful de-risking strategy is to build phase 1–2 as a **framebuffer-only prototype** on the Indy
before the capability model is final: one full-screen surface, `FILL`/`COPY`, a PS/2 mouse and a
font cache. If that prototype cannot sustain a comfortable text-editing session at 1024×768 8 bpp,
the design assumptions are wrong and should be revisited before the protocol is frozen.

### 9.1 What to reuse, what to write

| Component | Decision |
|---|---|
| Protocol codec | Write; `#[repr(C)]` ops + ring, fuzz target |
| Surface allocator | Write; VRAM bitmap allocator for the scanout/offscreen pool, plus a main-memory backing-store allocator |
| Font rasteriser | Reuse a small, permissively licensed bitmap font (e.g. a public-domain 8×16) for v1; outline rasterisation later in a font cell |
| Clipping | Write; rectangle regions, the classic X11/Plan 9 algorithm |
| Input | Write a small IOC2 PS/2 driver; reuse the kernel's event object |
| X11 | Separate project; the native side needs `PUT_IMAGE`, a surface and a display lease |
| Wayland | Do not adopt; if ever needed, write a translator from Wayland to the native protocol, as Arcan does in reverse |

---

## 10. Why this is not Wayland with extra steps

It is worth stating the differences explicitly, because the recommendation borrows Wayland's good
ideas and rejects its architecture.

| Question | Wayland | IRIX 7 native |
|---|---|---|
| Who renders? | Client, into its own buffer | Server, into a server-owned image, on the client's behalf |
| Buffer format | ARGB8888/XRGB8888 by default | I8/RGB24, screen depth |
| Where is the buffer? | Main memory (`wl_shm`) or GPU memory (DMABUF) | In place in the scanout image for the topmost window; main memory for retained backing |
| Per-frame work | Composite every damaged surface into scanout | Blit damage; usually no composition at all |
| Alpha | Expected | Absent |
| Buffer lifetime | Client-owned, released by the compositor | Server-owned image, capability handle, freed on last handle |
| IPC | Unix socket + FD passing | Capability call + shared ring |
| Assumed OS | Linux/POSIX (udev, DRM, EGL, libinput) | Framekernel cells, no POSIX assumptions |
| Effect of a slow client | Compositor still composites its last buffer | Its window is stale; the rest of the desktop is unaffected |

The last row is the underrated advantage of server-side drawing: a client that stops drawing cannot
force the compositor to keep copying a stale full-screen buffer. In a damage-driven server, the
stale window simply stops changing, and the cost of its existence is zero until it is exposed.

---

## 11. Residual risks

Risks are ranked by expected cost, with the mitigation that should be in the design from day one.

### 11.1 Hardware uncertainty (high)

* **VRAM size is not reliably documented**; the 8-bit XL is commonly reported as 1 MB and the
  24-bit as 2 MB, but 1280×1024×24 needs 3.75 MB [H §9.7, §18]. *Mitigation:* size from ARCS,
  refuse to enter a mode that does not fit, default to 1024×768 at 8 bpp, and size the offscreen
  pool from the measured VRAM rather than from a constant.
* **No documented vblank interrupt.** Frame pacing may have to poll VC2 status, and tearing may be
  unavoidable during drags. *Mitigation:* measure VC2 status bits during bring-up; design `COMMIT`
  so that a missed vblank degrades to "draw immediately" rather than to a stall; accept tearing at
  drag time as X11 did.
* **REX3 FIFO depth and fill rate are not specified.** A batch that is too large may block the
  server past its budget. *Mitigation:* conservative chunking, a measured watermark, and a
  FIFO-full interrupt path rather than busy-waiting.
* **24-bit planar storage.** A naive `RGB24` surface will produce wrong colours or three-plane
  reads. *Mitigation:* make 8 bpp the only v1 format; add 24-bit only after the planar path is
  verified against the XMAP9 packing.

### 11.2 Scheduling (medium-high)

* **Priority inversion through the blitter.** A low-priority client can fill the ring, and the
  server's drain of that ring can delay a media thread. *Mitigation:* per-client ring quotas, a
  server budget, and a rule that the server processes at most one client's batch per activation
  before checking the deadline.
* **Lease revocation storms.** A full-screen media app that loses its lease forces a full repaint
  while the media thread still has a deadline. *Mitigation:* revoke at a frame boundary, give the
  server a one-shot extension, and let the media cell keep its last frame on screen until the
  repaint is complete.
* **The server is a single point of latency for input.** If it is throttled, typing lags.
  *Mitigation:* separate input handling from drawing in the server (input runs on a small, high-
  priority thread that only enqueues events), and keep the drawing budget small.

### 11.3 Software risk (medium)

* **Protocol scope creep.** Every feature request (alpha, scaling, HiDPI, client-side decorations)
  is cheap on modern hardware and expensive here. *Mitigation:* the non-goals list in §7.10 is part
  of the protocol's compatibility contract; adding a feature requires deleting another.
* **Exposure correctness.** Server-side drawing with no backing store puts the burden on clients to
  repaint correctly. *Mitigation:* `RETAINED` surfaces for toolkits, a test suite that randomly
  obscures and reveals windows, and a "slow client" stipple rather than a freeze.
* **Rust on MIPS III big-endian.** The target is tier 3; there is no prebuilt `std`, and LLVM's
  MIPS backend has had big-endian codegen bugs. *Mitigation:* keep the server `no_std + alloc`,
  build `core`/`alloc` from source with a pinned toolchain, run the test suite on an emulator
  (QEMU's MIPS big-endian target) and on hardware, and avoid 64-bit atomics and unaligned accesses.
* **X11 integration is a separate project with its own schedule.** *Mitigation:* do not make the
  native server depend on X11; model A/B are additive.

### 11.4 Licence and provenance (low, but must be managed)

* **Plan 9 code carries the Lucent Public License, not GPL-3.0.** Whether or not the LPL is
  GPL-compatible in a given jurisdiction, the conservative choice is not to mix it into a GPL-3.0
  codebase. *Mitigation:* clean-room implementation from the published protocol description, with a
  written provenance record; the `8½` paper and the `draw(3)` manual are specifications, not source.
* **Arcan's GPL-2.0 files are incompatible with GPL-3.0.** *Mitigation:* do not link Arcan; if
  `shmif` ideas are used, reimplement them.
* **Fonts and icons have their own licences.** *Mitigation:* use public-domain or OFL fonts with a
  recorded provenance, and keep the licence texts in the source tree.

### 11.5 Product risk (low)

* **No compositor means no translucent effects.** If the product story depends on them, this design
  will not deliver them; the honest answer is that the hardware cannot, and a software alpha path
  would consume the machine.
* **Legacy X applications may assume a full screen and a specific WM.** Model A handles this; model
  B may expose differences in screen size and visual selection.

---

## 12. Appendix: scoring matrix

The weighted score in §6 is the dot product of the eleven axis scores with the weights in §4. The
raw scores are reproduced here so a reader can re-weight them. Scores are the author's judgement,
argued per option in §5; they are not measurements.

Weights: hardware fit 25, memory 15, CPU/bandwidth 15, latency 10, EDF 10, Rust 8, licence 4,
effort 8, X11 3, capability fit 1, longevity 1.

| # | Option | HW | Mem | CPU | Lat | EDF | Rust | Lic | Effort | X11 | Cap | Long | Weighted |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | Minimal custom | 5 | 5 | 5 | 4 | 5 | 5 | 5 | 4 | 5 | 5 | 4 | **4.8** |
| 2 | Wayland | 1 | 1 | 1 | 3 | 2 | 2 | 5 | 1 | 3 | 2 | 2 | 1.6 |
| 3 | Direct FB + tiny WM | 5 | 5 | 4 | 4 | 5 | 5 | 5 | 5 | 3 | 3 | 2 | 4.6 |
| 4 | Plan 9 `rio` | 5 | 5 | 5 | 5 | 4 | 4 | 4 | 5 | 2 | 3 | 4 | 4.7 |
| 5 | Arcan | 1 | 1 | 1 | 2 | 1 | 0 | 2 | 1 | 4 | 2 | 1 | 1.2 |
| 6 | BeOS `app_server` | 4 | 4 | 4 | 3 | 2 | 1 | 4 | 3 | 2 | 3 | 3 | 3.3 |
| 7 | NeXT DPS / Quartz | 2 | 1 | 1 | 2 | 1 | 0 | 0 | 0 | 0 | 1 | 1 | 1.1 |
| 8 | Sun NeWS | 2 | 1 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 | 0.9 |
| 9 | SurfaceFlinger | 1 | 1 | 1 | 3 | 2 | 1 | 5 | 0 | 0 | 2 | 2 | 1.4 |
| 10 | Chrome Viz | 0 | 0 | 0 | 2 | 0 | 0 | 5 | 0 | 0 | 1 | 1 | 0.4 |
| 11 | Windows DWM | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 0 | 0 | 1 | 1 | 0.4 |
| 12 | Qt Compositor / Mir | 1 | 0 | 0 | 2 | 0 | 0 | 5 | 0 | 0 | 2 | 1 | 0.7 |
| 13 | X11 as native | 5 | 5 | 4 | 4 | 1 | 3 | 5 | 2 | 5 | 1 | 3 | 3.9 |
| 14 | No window system | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 2 | 2 | 2 | (4.9, unscored) |

The cluster at 4.6–4.8 (options 1, 3, 4) is real: those designs share the same rendering model and
differ in retention policy. The recommendation in §7 combines them. Option 14's arithmetic score is
shown only to make the point that efficiency is not the only axis; it is not a candidate for a
desktop window system.

---

## 13. Sources

Sources are listed in the order they are first cited. "[H]" is the local hardware reference; it is
not a published document but is included because the hardware numbers in this report depend on it.

| Tag | Source |
|---|---|
| [H] | *SGI Indy (IP22 / "Guinness") hardware reference*, local research note, `/tmp/opencode/indy-hardware.md`, §2, §3, §9, §12, §18. Compiled from SGI chip specifications (REX3, RB2, VC2, XMAP9, MC, GIO) and Linux/NetBSD driver sources. |
| [THESEUS] | Theseus OS, a framekernel OS with cell-based isolation, https://github.com/theseus-os/Theseus (repository; the framekernel/cell model referenced in §3.3). |
| [WAYLAND] | Wayland, "Wayland Architecture", https://wayland.freedesktop.org/architecture.html (fetched 2026-10-20). |
| [WXML] | Wayland, `protocol/wayland.xml`, https://gitlab.freedesktop.org/wayland/wayland/-/blob/main/protocol/wayland.xml (fetched 2026-10-20), `wl_shm`, `wl_surface.frame`, `wl_surface.commit`. |
| [SMITHAY] | Smithay, "A smithy for rusty Wayland compositors", README, https://github.com/Smithay/smithay (fetched 2026-10-20). |
| [SMITHAY-SRC] | Smithay, `src/backend/renderer/mod.rs`, `renderer_pixman` feature, https://github.com/Smithay/smithay/blob/master/src/backend/renderer/mod.rs (fetched 2026-10-20). |
| [SMITHAY-LICENCE] | Smithay, `LICENSE.txt`, MIT, https://github.com/Smithay/smithay/blob/master/LICENSE.txt. |
| [WLROOTS] | wlroots, README ("about 60,000 lines of code you were going to write anyway"; backends; Xwayland abstraction), https://gitlab.freedesktop.org/wlroots/wlroots (fetched 2026-10-20). |
| [WLROOTS-LICENCE] | wlroots, `LICENSE`, MIT. |
| [TINYWL] | wlroots, `tinywl/tinywl.c`, 1,101 lines, https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/master/tinywl/tinywl.c (fetched 2026-10-20). |
| [COSMIC] | `cosmic-comp`, compositor for the COSMIC desktop, https://github.com/pop-os/cosmic-comp. |
| [COSMIC-LICENCE] | `cosmic-comp`, `LICENSE`, GPL-3.0. |
| [RIVER] | river, "a non-monolithic Wayland compositor", https://github.com/riverwm/river (fetched 2026-10-20); river-window-management-v1 protocol; SPDX `GPL-3.0-only` headers and `LICENSES/GPL-3.0-only.txt` on the Codeberg repository. |
| [SWAY] | sway, README and `LICENSE`, MIT, https://github.com/swaywm/sway. |
| [SWAY-LICENCE] | sway, `LICENSE`, MIT. |
| [ARCAN] | Arcan, README, https://github.com/letoram/arcan (fetched 2026-10-20); `shmif`, `xarcan`, `arcan-wayland`. |
| [ARCAN-COPYING] | Arcan, `COPYING`, https://github.com/letoram/arcan/blob/master/COPYING (fetched 2026-10-20): 3-Clause BSD, GPL-2.0+ and LGPL components; `arcan_lua.c` GPL-2.0. |
| [MIR] | Canonical, Mir, README, https://github.com/canonical/mir (fetched 2026-10-20). |
| [QTCOMP] | Qt, "Qt Wayland Compositor" manual, https://doc.qt.io/qt-6/qtwaylandcompositor-index.html (fetched 2026-10-20); "Commercial or GPLv3". |
| [P9] | Rob Pike, "8½, the Plan 9 Window System", *Computing Systems* 4(2), 1991; HTML at http://doc.cat-v.org/plan_9/4th_edition/papers/812/ (fetched 2026-10-20). Figures quoted: 5,100 lines total; server 2,295 lines + 2,214 lines of graphics operations; 23 messages; clients' bitmaps stored in the server; under 90 KB of text on a 68020. |
| [DRAW] | Plan 9, `draw(3)`, https://man.9front.org/3/draw (fetched 2026-10-20): `/dev/draw/new`, per-connection `ctl`/`data`/`colormap`/`refresh`, binary messages, server-side images. |
| [RIO1] | Plan 9, `rio(1)`, https://man.9front.org/1/rio (fetched 2026-10-20). |
| [RIO4] | Plan 9, `rio(4)`, https://man.9front.org/4/rio (fetched 2026-10-20): window system files, per-window context. |
| [HAIKU] | Haiku, `src/servers/app` (AppServer, Desktop, ScreenManager, Screen, ServerWindow, ServerBitmap, DrawingEngine, HWInterface, AccelerantHWInterface, UpdateQueue, DirectWindowInfo, ClientMemoryAllocator, View/Window/Workspace), `Jamfile`, https://github.com/haiku/haiku/tree/master/src/servers/app (fetched 2026-10-20). |
| [BEBOOK] | Be, *The Be Book*, "The Application Server", https://www.haiku-os.org/legacy-docs/bebook/ (fetched 2026-10-20); BeOS R5 `BWindow`/`BView`/`BDirectWindow` semantics. |
| [DPS] | Adobe Systems, *Display PostScript System* documentation (1990); NeXT, *NeXTSTEP Operating System Software* (1993), WindowServer and DPS chapters. Bibliographic; not fetched in full. |
| [NEWS] | James Gosling, David Rosenthal, Michelle Arden, *The NeWS Book: An Introduction to the Network/Extensible Window System*, Springer-Verlag, 1989. Bibliographic. |
| [QUARTZ] | Apple, *Quartz 2D Programming Guide*, https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/drawingwithquartz2d/Introduction/Introduction.html (fetched 2026-10-20); PDF-based drawing model. Quartz Extreme's GPU composition is described in Apple's Mac OS X 10.2 developer documentation (bibliographic). |
| [BQ] | Android, "BufferQueue and gralloc", https://source.android.com/docs/core/graphics/arch-bq-gralloc (fetched 2026-10-20). |
| [SFHWC] | Android, "SurfaceFlinger and Hardware Composer", https://source.android.com/docs/core/graphics/arch-sf-hwc (fetched 2026-10-20). |
| [VIZ] | Chromium, `components/viz/README.md`, https://chromium.googlesource.com/chromium/src/+/HEAD/components/viz/README.md (fetched 2026-10-20). |
| [DWM] | Microsoft, "Enable and control DWM composition", https://learn.microsoft.com/en-us/windows/win32/dwm/composition-ovw (fetched 2026-10-20). |
| [RIVER-BLOG] | Isaac Freund, "Separating the Wayland Compositor and Window Manager", https://isaacfreund.com/blog/river-window-management/ (linked from the river README; the design split referenced in §7.8). |
| [XPROTO] | X.Org, *X Window System Protocol, X Version 11*, https://www.x.org/releases/X11R7.7/doc/xproto/x11protocol.html (fetched 2026-10-20). |
| [XORG] | X.Org, X server DIX/DDX architecture and XAA/EXA acceleration documentation, https://www.x.org/wiki/ (bibliographic entry point; not fetched in full). |

### Source confidence

* **Fetched and quoted:** [WAYLAND], [WXML], [SMITHAY], [SMITHAY-SRC], [WLROOTS], [TINYWL],
  [COSMIC], [RIVER], [SWAY], [ARCAN], [ARCAN-COPYING], [MIR], [QTCOMP], [P9], [DRAW], [RIO1],
  [RIO4], [QUARTZ], [BQ], [SFHWC], [VIZ], [DWM], [XPROTO], [H]. [HAIKU] was checked as a source
  tree listing (file names and structure), not read line by line.
* **Bibliographic, not fetched in full:** [DPS], [NEWS], [XORG], [BEBOOK], [THESEUS],
  [RIVER-BLOG]. Their architecture descriptions are standard and widely documented; specific
  performance claims about them in this report are qualitative and marked as such.
* **Estimates, not measurements:** all bandwidth figures derived from frame sizes and the bus
  numbers in [H]; all effort figures are person-month estimates; no Indy benchmark was run.

---

*Prepared 2026-10-20. This report is research input to an ADR, not the ADR itself. The decision it
recommends is: adopt a server-side, retained-image, damage-driven display server in Rust, with
capability handles, an 8-bit indexed default, display leases for media, and X11 as a nested
compatibility cell.*
