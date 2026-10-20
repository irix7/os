# X11 in Rust for IRIX 7 (IP22 Indy)

**Scope.** State of X11 in Rust, and what it would take to give a clean-room GPL-3.0
Rust OS on SGI Indy hardware (IP22, MIPS III R4400, big-endian, 32–128 MB, GR1
Newport / GR2 Express 2D graphics) a working X11 server for legacy IRIX 6.5
binaries.

**Date.** 20 October 2026. All repository figures (stars, LOC, versions) are as
measured on that date.

**Method.** Primary sources only: upstream repositories cloned and measured
(`xserver`, `yserver`, `x12`, `rxserver`, `x11rb`, `xpra`, `xf86-video-newport`),
GitHub/GitLab APIs, crates.io API, the X11 protocol and extension specifications
at `x.org`, and SGI's own IRIX 6.5.3f manual pages (the `Xsgi(1)` server page).
LOC counts are physical lines of `.c`/`.h` or `.rs`, including comments and
blank lines; they are comparative, not a productivity metric. Repository stars
and dates come from the forge APIs.

**Contents.** §1 summary · §2 what an X11 server is · §3 Rust servers (Q1) ·
§4 Rust ecosystem (Q2) · §5 other-language servers (Q3) · §6 what to implement
(Q4) · §7 paths and recommendation (Q5) · §8 prior art (Q6) · §9 conclusions,
risks, sources.

---

## 1. Summary and verdict

1. **No complete X11 server exists in Rust in the protocol sense** (no XIE, PEX,
   DPS, DDX model, or legacy visual model, and no big-endian host support). The
   closest — and the only serious, working one — is
   [`joske/yserver`](https://github.com/joske/yserver) (MIT, ~612 stars,
   created April 2026, very active). It runs real desktop environments (CDE,
   XFCE, MATE, Cinnamon, i3) on Linux DRM/KMS hardware and passes large parts of
   the X.Org X Test Suite. It is **not
   usable as-is** on an Indy: it targets DRM/KMS + Vulkan, explicitly drops
   non-TrueColor visuals, indirect GLX, the DDX driver ABI and legacy font
   behaviour, and its only non-KMS backend is a nested X11 backend that has been
   abandoned. It is, however, an excellent MIT-licensed architectural reference
   and a ready-made conformance-testing methodology.
2. **No Rust X11 server targets big-endian hosts as a first-class platform.**
   yserver recently bolted on byte-swapped *clients* (a large, instructive
   effort recorded in `docs/xts-baseline.md`), which proves the wire layer can be
   made endian-clean, but the host is assumed little-endian throughout its
   renderer and Vulkan paths.
3. **There is no complete Rust X11 *client library* either.** `x11rb` is a
   complete, mature, pure-Rust implementation of the X11 *wire protocol* on the
   client side (MIT/Apache-2.0). That is the single most valuable reusable
   component in the ecosystem, and even it is client-side only: it has no
   request-dispatch/server side.
4. **The rest of X11 is C, and it is all MIT-licensed** (Xorg, Xwayland, Xvfb,
   Xnest, Xephyr, kdrive/TinyX, Cygwin/X). Xpra is GPL-2.0+ and nx-libs (X2Go)
   is GPL-2.0, but both are *proxies*, not servers. WeirdX is a GPL Java server
   that proves a from-scratch server is tractable but is too incomplete and too
   slow to reuse.
5. **A minimal IRIX-capable X11 server is a bounded, well-specified job**:
   roughly 20–30k lines of Rust for core protocol + software framebuffer + the
   extensions IRIX applications actually use, or 35–45k for broad parity. That is
   smaller than yserver (~215k Rust LOC before its test modules) because IRIX-era
   applications need none of Composite/Damage/Present/DRI3/RandR 1.4/XI2.
6. **Recommendation: path (a), a clean-room Rust X11 server**, built
   backend-agnostically so the native compositor is just one `Screen` backend and
   an in-memory framebuffer is another. Do **not** port Xorg or kdrive, and do
   not attempt Xwayland. Budget ~11–17 person-months for a core server that runs
   `xterm`/`twm`, 16–25 person-months to reach 4Dwm/`dtterm`, and 19–30
   person-months for broad IRIX parity, with GLX as a separately-scoped bridge to
   the native GL service. See §7.
7. **Critical scoping warning.** The server is only half the X11 problem. IRIX
   binaries dynamically link `libX11.so`, `libXext.so`, `libXi.so`, `libXt.so`
   and `libXm.so`; a clean-room OS must supply those *client-side* ABIs too.
   Whatever server path is chosen, an Xlib ABI implementation (~20–30k LOC and
   its own compatibility test burden) is on the critical path. This report covers
   the server; §7.8 flags the client-library work explicitly.

---

## 2. What "X11 server" means here, and the shape of the problem

X11 is a wire protocol, not an API. A server is a process that:

- accepts connections on a UNIX socket (`/tmp/.X11-unix/X0`) or TCP port 6000+n;
- performs the connection handshake: authorisation, byte-order declaration,
  and a `Setup` reply advertising screens, depths, visuals, pixmap formats and
  the vendor string;
- owns a resource namespace of XIDs (windows, pixmaps, GCs, fonts, colormaps,
  cursors, pictures, glyphsets, …) per client, with per-client ID ranges;
- dispatches 119 core requests (X11R6) plus extensions;
- maintains the window tree, event masks, grabs, focus, selections, properties
  and the ICCCM conventions that toolkits depend on;
- rasterises drawing requests into drawables, and composites the root window to
  the display.

Three properties matter for this project:

1. **The protocol is endian-neutral on the wire.** The connection setup carries a
   single byte, `'B'` (MSBFirst) or `'l'` (LSBFirst), and the server must speak
   the client's order. Xorg does this with a per-client `swapped` flag and an
   entire parallel `SwappedProcVector` (`xserver/dix/dispatch.c:2040`,
   `:3671–3788`; `xserver/os/io.c:110`). An IRIX-only deployment can initially
   implement MSBFirst only — every IRIX client is big-endian — which removes a
   large class of bugs and roughly halves the wire-layer test matrix.
2. **A server is a display server, not a window manager.** 4Dwm/`dtwm` is a
   client. The server must provide correct `SubstructureRedirect`, `ConfigureRequest`,
   `MapRequest`, `PropertyNotify`, `SelectionClear`, `SendEvent` and grab
   semantics, or 4Dwm will not manage windows.
3. **"Minimal" is defined by the applications, not by the spec.** The IRIX
   desktop stack (4Dwm, `dtterm`, `dtfile`, `toolchest`, Motif applications) uses
   a narrow slice: core protocol, SHAPE, MIT-SHM, XTEST occasionally, XInput for
   exotic input devices, and — for anything graphical — GLX. XIE/PEX/DPS exist on
   IRIX but are used by a minority of applications. §6 quantifies this.

---

## 3. Q1 — Is there a complete X11 server implemented in Rust?

**No complete one that suits this project. Yes, one real one.** The field is
tiny and very recent:

| Project | Stars | Licence | LOC (Rust, non-test) | State | Verdict |
|---|---:|---|---:|---|---|
| [`joske/yserver`](https://github.com/joske/yserver) | 612 | MIT | ~215k (before test modules; ~320k incl. tests) | Runs CDE, XFCE, MATE, Cinnamon, i3 on Linux DRM/KMS; FreeBSD support; XTS-driven | Closest by far; not reusable as-is |
| [`systemrustd/x12`](https://github.com/systemrustd/x12) | 0 | MIT (inherited) | ~95k (before test modules) | Hard fork of yserver "that aims to be cross platform"; README is one line; no releases | Watch, don't depend |
| [`PerikiyoXD/rxserver`](https://github.com/PerikiyoXD/rxserver) | 29 | Apache-2.0 | ~15k | TCP-only toy; renders via `softbuffer`; "many core requests not implemented" | Learning exercise |
| [`x11rs/x11server`](https://github.com/x11rs/x11server) | 0 | Unlicense | ~0 (2 KB repo) | Empty 2020 stub | Dead |
| [`x11-lab/x11-lab`](https://github.com/x11-lab/x11-lab) | 0 | MIT | small | Tutorial on X11 forwarding to a Rust "server" | Teaching material, not a server |

### 3.1 yserver — the real thing

Source: <https://github.com/joske/yserver>, MIT, first commit 2026-04-27.
Architecture is documented in
[`docs/high-level-design.md`](https://github.com/joske/yserver/blob/master/docs/high-level-design.md):

- Four crates: `yserver-protocol` (wire encode/decode, 24k LOC, own schema, **no
  XCB XML**), `yserver-core` (protocol state: clients, resources, windows,
  properties, atoms, selections, input, RandR, XFIXES, DAMAGE, COMPOSITE, XKB,
  XInput2), `yserver-render` (Vulkan/KMS), `yserver` (backends, libinput, VT).
- Single-threaded core loop with `mio`; backend is `&mut dyn Backend`; two
  backends: standalone DRM/KMS (production) and `ynest` (nested X11, **development
  stopped**).
- Rendering is Vulkan directly (no EGL/GBM/Mesa GL), atomic KMS modesetting,
  GBM scanout buffers, libinput input.

**What it proves.** A from-scratch Rust X11 server can run a full desktop. The
README lists CDE (`dtsession`, `dtwm`, `dtterm`, `dtfile`, `dtpad`, Motif/Xaw
apps), XFCE, MATE, Cinnamon, i3, Steam, Chrome and `x11vnc`. Its
`docs/extensions.txt` advertises: BIG-REQUESTS, Composite, DAMAGE, DRI3, GLX,
Generic Event, MIT-SCREEN-SAVER, MIT-SHM, Present, RANDR, RECORD, RENDER, SHAPE,
SYNC, X-Resource, XC-MISC, XFIXES, XFree86-VidMode, XINERAMA, XInputExtension,
XKEYBOARD, XTEST. That is a modern desktop contract, and it is a superset of what
IRIX applications need — except in the places where it is deliberately a subset.

**Why it cannot be dropped into IRIX 7.** From
[`docs/high-level-design.md`](https://github.com/joske/yserver/blob/master/docs/high-level-design.md)
(Non-Goals), verbatim:

> - Supporting hardware that lacks DRM/KMS + Mesa support (any OS).
> - Supporting non-TrueColor legacy visuals as a first-class feature.
> - Supporting indirect or remote GLX.
> - Supporting endian-swapped clients unless a concrete modern use case appears.
> - Implementing obsolete font functionality beyond what real clients still need.

And from the README: it drops "non-TrueColor visuals, indirect GLX, the DDX
driver ABI, endian-swapped clients".

Consequences for an Indy:

1. **No DRM/KMS, no Vulkan.** GR1/GR2 are plain framebuffers with a hardware
   cursor and, for GR1, a colour-map. There is no Mesa, no GBM, no Vulkan, no
   `libdrm`. The entire `yserver` + `yserver-render` half of the tree (~120k Rust
   LOC before tests) is inapplicable.
2. **Non-TrueColor is a stated non-goal**, but IRIX on an 8-bit XL Indy defaults
   to 8-bit PseudoColor, and a large fraction of IRIX applications allocate
   colours (`XAllocColor`, `XAllocColorCells`) and depend on colormap semantics.
   Supporting only TrueColor would break the native look and many apps.
3. **Vulkan is not the only renderer problem.** `yserver` assumes a GPU
   rasteriser for every primitive (batched instanced glyph draws, Vulkan command
   buffers). An Indy needs a competent CPU rasteriser for 8/24-bit framebuffers —
   which is closer to Xorg's `fb`/`mi` than to yserver's engine.
4. **Big-endian host.** `yserver` has recently added byte-swapped *client*
   support (see `crates/yserver-protocol/src/x11/wire_swap.rs` and
   `request_swap.rs`), and its XTS notes document the effort: Xproto PASS went
   1 → 26 → 195 → 229 → 337 as setup/reply/event/request swapping landed
   ([`docs/xts-baseline.md`](https://github.com/joske/yserver/blob/master/docs/xts-baseline.md)).
   That is the *client* direction on a little-endian host. It does not make the
   host big-endian: the renderer comments show LE assumptions in pixel formats
   (`PICT_a8r8g8b8` described as "a little-endian CARD32"), SPIR-V loading and
   DRM BGRA paths.

**Licence.** MIT — compatible with GPL-3.0 (see ADR-0012's gate), so code *could*
legally be reused. But this project's clean-room rule (ADR-0009/ADR-0014) means
yserver may only be used as a behavioural reference and test oracle, not copied.
That is the right call anyway: its backend half is wrong for the hardware.

**What is genuinely worth taking from it (as knowledge, not code):**

- The four-crate split (`protocol` / `core` / `render` / `backend`) is the right
  seam: wire codec, protocol state, rasteriser, hardware. Copy the *architecture*,
  clean-room.
- Its extension-version audit and XTS harness
  (`docs/xts-baseline.md`, `docs/extension-version-audit-2026-09-26.md`) give a
  ready-made definition of "done" and a regression methodology.
- Its bug archaeology is a list of traps: `BadLength` per opcode, `BadIDChoice`
  ranges, `MappingNotify` ordering, `GraphicsExpose`/`NoExpose`, `GetProperty`
  `bytes_after` semantics, `RotateProperties`, colormap read-only rules, focus
  `NotifyInferior`/`NotifyAncestor` details. These are exactly the details that
  make a server "work with Motif" rather than "pass a smoke test".

### 3.2 x12

<https://github.com/systemrustd/x12> — a hard fork of yserver whose README is
literally one line: "A modern X11 server written from scratch in Rust." Its tree
retains yserver's `kms/v2` renderer, `xshmfence`, XKB and DRM code, and it has no
documentation, releases or test results. It is not a viable dependency; it is
only evidence that others see the same gap (cross-platform X11 server in Rust).

### 3.3 rxserver

<https://github.com/PerikiyoXD/rxserver> (Apache-2.0, 2025-06). ~15k LOC. From its
README: "It listens for X11 client connections over TCP and renders windows to a
virtual display (a native OS window, via `softbuffer`). Still under active
development. Many core X11 requests are implemented, others are not yet." No
UNIX-socket transport, no SHM, no serious extension set. It is a useful reading
reference for a compact request decoder, not a server.

### 3.4 Conclusion for Q1

The closest thing to a complete Rust X11 server is yserver, and it is a modern
Linux desktop server, not a portable legacy server. **Nothing in Rust today can
host IRIX 6.5 applications on GR1/GR2 hardware without substantial new work.**
The honest framing is: yserver proves feasibility and supplies a test
methodology; the IRIX 7 server is new code.

---

## 4. Q2 — The Rust X11 ecosystem, catalogued

### 4.1 Wire protocol and bindings

| Crate / repo | Version, downloads | Licence | Provides | Maturity | Suitability for IRIX 7 |
|---|---|---|---|---|---|
| [`x11rb`](https://github.com/psychon/x11rb) / [`x11rb-protocol`](https://crates.io/crates/x11rb-protocol) | 0.14.0; 75.9M / 73.8M all-time | MIT OR Apache-2.0 | Complete X11 **client** protocol in pure Rust, generated from `xcb-proto` XML; all extensions; UNIX/TCP transports; FD passing; optional libxcb backend behind `allow-unsafe-code` | Very mature (since 2019, 431★, active) | **Best-in-class client library.** No server dispatcher: it can parse requests (`TryParse`, `Request` trait in `x11rb-protocol/src/protocol/xproto.rs`) but has no resource model, no event routing, no screens. Useful for the *client* side of the OS (native apps, test tools) and as a protocol schema reference |
| [`breadx`](https://github.com/bread-graphics/breadx) | 3.1.0; 64k | BSL-1.0 | Pure-Rust X11 client protocol, async-friendly, "100% safe and mutex-free" | Stale (last push 2023-03) | Alternative client; BSL-1.0 is GPL-3.0-compatible but not a reason to prefer it over x11rb |
| [`x11`](https://crates.io/crates/x11) / `x11-dl` ([AltF02/x11-rs](https://github.com/AltF02/x11-rs)) | 2.21.0; 38.5M / 72.3M | MIT | FFI bindings to `libX11`/`libXext`/…, link-time or `dlopen` | Maintained-ish (last push 2024-08), historically unsound in places (see x11rb's `doc/comparison.md`) | Requires a real libX11 — i.e. the thing we must write. Only useful for host-side tooling |
| [`xcb`](https://github.com/rust-x-bindings/rust-xcb) | 1.7.1; 9.3M | MIT | Safe-ish wrappers over `libxcb` (C) | Active (2026), 168★ | Requires libxcb; irrelevant to a clean-room OS |
| `xcb-dl`, `xproto`, `xrb` | 0.2.0 / 2.0.1 / 0.0.0 | MIT-ish / MIT / MIT | FFI or early-phase protocol crates | Dead or trivial | Ignore |
| [`tiny-xlib`](https://github.com/rust-windowing/tiny-xlib) | 0.2.5; 7.8M | Apache-2.0 | Minimal *bindings* to Xlib (used by `softbuffer`) | Active | Not an implementation; host tooling only |

**The single most important fact in this table:** `x11rb` is a complete,
pure-Rust implementation of the *client* half of X11, and it is the crate the
whole Rust GUI stack depends on (`winit`, `smithay`, `softbuffer`, `x11-clipboard`,
`wezterm`, `alacritty`, …). There is no equivalent for the server half. yserver
wrote its own protocol crate rather than reuse `x11rb-protocol`, which tells you
something about the ergonomics of using generated client types for dispatch.

`xcb-proto`'s XML (MIT, <https://gitlab.freedesktop.org/xorg/proto/xcbproto>) is
the canonical machine-readable description of the protocol and all extensions,
and `x11rb` vendors a copy. For a clean-room project it is a *specification*
input (a schema of the wire format), not an implementation, and is safe to use
as such.

A note on names: there is no crate called `xcb-rs`. The maintained safe binding
is `xcb` (repo `rust-x-bindings/rust-xcb`); `xcb-dl` is the `dlopen` variant;
`x11-rs` is the repo name for the `x11`/`x11-dl` crates (now `AltF02/x11-rs`).

### 4.2 Toolkits and compositor infrastructure

| Project | Version / stars | Licence | What it is | X11 relevance | Suitability |
|---|---|---|---|---|---|
| [`winit`](https://github.com/rust-windowing/winit) | 0.30.13; 6.2k★ | Apache-2.0 | Cross-platform window creation; X11 backend built on `x11rb` + XKB + XCursor + XRandR | **X11 client only** — creates windows on an existing server | Would be a *consumer* of our server; not a component of it |
| [`smithay`](https://github.com/Smithay/smithay) | 0.7.0; 3.3k★ | MIT | Wayland compositor library; has `src/backend/x11` (compositor inside an X11 window) and an `xwayland` feature that **spawns the C Xwayland binary** | X11 as a *host* for a Wayland compositor, and as a client of Xwayland | Not an X server. Its X11 backend is the mirror image of what we need |
| [`smithay-client-toolkit`](https://github.com/Smithay/client-toolkit) | 0.21.1; 443★ | MIT | Wayland client toolkit | None | Irrelevant |
| [`wayland-rs`](https://github.com/Smithay/wayland-rs) | 0.31.x; 1.4k★ | MIT | Pure-Rust Wayland client **and server** protocol | None directly | Relevant only if the native window system is Wayland-shaped |
| [`calloop`](https://github.com/Smithay/calloop) | 0.14.5; 296★ | MIT | Callback event loop | None | Useful infrastructure for a single-threaded server loop (yserver uses `mio` instead) |
| [`xkbcommon`](https://crates.io/crates/xkbcommon) | 0.9.0; 9.5M | MIT | Bindings to `libxkbcommon` | XKB keymap compilation/consumption | Needs libxkbcommon (C). A server needs its own XKB *server* implementation; this crate is client-side keymap handling |
| [`xcursor`](https://crates.io/crates/xcursor) | 0.3.11; 44M | MIT | Xcursor theme loading | Cursor file parsing | Reusable if we load Xcursor themes; IRIX uses core cursor fonts instead |
| [`x11-clipboard`](https://github.com/quininer/x11-clipboard) | 0.9.3; 12.6M | MIT | X11 clipboard (CLIPBOARD/PRIMARY) via X properties | Client side, in "maintenance mode" | Not reusable server-side; the selection machinery must live in the server |
| [`wlroots`](https://gitlab.freedesktop.org/wlroots/wlroots) + [`wlroots-rs`](https://github.com/swaywm/wlroots-rs) | — / 163★ | MIT | C compositor library; Rust bindings **archived 2020** | Wayland only | Irrelevant, and the bindings are dead |

### 4.3 What the ecosystem does *not* contain

Exhaustive GitHub searches (`x11 server language:Rust`, `xserver language:Rust`,
`x11 protocol server language:Rust`, `x11 display server language:Rust`) return
36, 4, 0 and 6 repositories respectively, and after removing MCP servers,
screenshot tools and VNC bridges only yserver, x12, rxserver and two stubs
remain. There is:

- **no** Rust Xlib implementation (`libX11`-compatible client library);
- **no** Rust Xext/Xi/Xt/Motif implementation;
- **no** Rust X server framework or reusable server-side protocol crate;
- **no** Rust Xwayland equivalent (Smithay merely spawns the C one);
- **no** Rust DDX/driver model, and no interest in one (yserver explicitly
  rejects the DDX ABI).

This is the landscape: one strong client stack (`x11rb`), one strong but
Linux-desktop-shaped server (yserver), and nothing in between.

---

## 5. Q3 — X11 servers in other languages

All LOC figures below are measured from a shallow clone of the upstream tree on
2026-10-20; "core" means `dix` + `os` + `mi` + `fb` + `include`, i.e. the parts
every Xorg-family server shares.

### 5.1 The Xorg family

| Server | Where | LOC | Licence | Dependencies | What it is |
|---|---|---:|---|---|---|
| **Xorg** | [`xorg/xserver`](https://gitlab.freedesktop.org/xorg/xserver) | ~509k C/H total; ~297k excluding `hw/`; `dix` 44k, `os` 16k, `mi` 20.8k, `fb` 8.3k, `Xext` 26.8k, `Xi` 18.9k, `xkb` 25.8k, `glx` 30.9k, `render` 8.7k, `randr` 11.7k; `hw/xfree86` 124k | MIT (`COPYING`, SPDX MIT) | libc/pthreads, pixman, libXfont2 + FreeType + fontconfig, libdrm, libpciaccess, libxkbfile/xkbcomp, zlib, libmd/OpenSSL, libepoxy/glamor for GL | The reference implementation; DDX driver model, loadable modules, all extensions |
| **Xwayland** | `xserver/hw/xwayland` | 20.9k | MIT | wayland-server/client, EGL, GL, glamor, DRI3, libdrm, xkbcommon | X server that presents into Wayland surfaces; each X window is a `wl_surface`; rendering goes through glamor (GL) |
| **Xvfb** | `xserver/hw/vfb/InitOutput.c` (1,156 lines) | 1.3k DDX + shared core | MIT | pixman, libXfont2, FreeType/fontconfig | Software-only server: `fb` framebuffer in RAM, no input, no hardware. The canonical "server with no display" |
| **Xnest** | `xserver/hw/xnest` | 4.6k | MIT | **libX11 + libXext** | A server implemented as an X *client*: translates protocol requests into Xlib calls on a host display. The original nested server |
| **Xephyr** | `xserver/hw/kdrive/ephyr` | 6.7k | MIT | libxcb + xcb-shm, host X server | Modern nested server: renders into a host X window via XCB/SHM |
| **kdrive / TinyX** | `xserver/hw/kdrive/src` | 12.3k (kdrive) | MIT | pixman, libXfont2, evdev/tslib for input | Small DDX intended for embedded/framebuffer systems; `fbdev`, `ephyr` backends. Historically the "TinyX" porting target |
| **Cygwin/X** | `xserver/hw/xwin` | 32.6k | MIT | Cygwin POSIX layer, Windows GDI/DirectDraw | X server for Windows; a DDX targeting a foreign windowing API |
| **XQuartz** | `xserver/hw/xquartz` | 15.8k | MIT | macOS, Xplugin | X server for macOS; proves the core is portable across radically different window systems |
| **Xdummy** | [`xorg/driver/xf86-video-dummy`](https://gitlab.freedesktop.org/xorg/driver/xf86-video-dummy) + shell script | ~1–2k | MIT | Xorg | A dummy video driver used headless with RandR custom modes; not a separate server |

**What reuse would take.** The Xorg family is MIT, so licence is not the
obstacle. The obstacles are:

1. **POSIX and libc.** `os/` assumes `poll`, `socketpair`, `SIGIO`/`sigaction`,
   `mmap`, `shm`, `getpwuid`, `setuid`, threads. A Rust OS must supply these or
   a porting shim. `os/` is only 16k LOC, so a shim is feasible.
2. **pixman.** All rasterisation in `fb`/`mi`/`exa` is pixman. pixman is C,
   portable, endian-clean (it has explicit big-endian paths and is built on
   s390x/ppc64), ~50k LOC. It would have to be compiled into the OS or
   reimplemented. There is no Rust pixman.
3. **Fonts.** `libXfont2` + FreeType + fontconfig is a large C dependency chain.
   For IRIX-era core fonts (PCF/BDF) a small in-tree reader is enough; for
   Type1/Speedo (IRIX shipped both) it is not.
4. **DDX model.** Xorg's `hw/xfree86` (124k LOC) is a driver model for PCI
   graphics, DRM, KMS, input hotplug and module loading. None of it applies to
   GR1/GR2. Porting "Xorg" really means porting `dix`+`os`+`mi`+`fb` and writing
   a new DDX — at which point you have written most of option (a) anyway.
5. **Endianness.** Xorg builds and runs on big-endian Linux (s390x, ppc64,
   sparc64, m68k) and on big-endian macOS/PowerPC historically, so the core is
   endian-clean. This is genuine prior art (§8).

**Effort to reuse:** 20–40 person-months for a working Xorg-on-IRIX-7, most of
it in the libc/pixman/font plumbing and a Newport/Express DDX, plus a permanent
maintenance burden of ~300k C LOC that is alien to the rest of the OS. It is
also the wrong shape: Xorg's architecture assumes a Unix process model, dynamic
loading and hardware abstraction layers that a 32 MB Indy does not want.

### 5.2 Proxies and remote-display systems (not servers)

| System | Language / LOC | Licence | What it actually is | Reuse |
|---|---|---|---|---|
| **Xpra** | Python, 311k LOC ([Xpra-org/xpra](https://github.com/Xpra-org/xpra)) | GPL-2.0+ | An X11 *client* and proxy: connects to an existing X server (Xvfb/Xdummy/Xorg), captures windows, forwards them over the network | None for a server. Its "seamless windows" and damage-tracking ideas are interesting but it needs a real X server underneath |
| **X2Go** | C/C++ via [nx-libs](https://github.com/ArcticaProject/nx-libs) | GPL-2.0 (v2, per `debian/copyright`; **not** "or later") | Remote desktop built on NX compression; `nxagent` is a fork of Xnest (75k LOC) inside a 308k-LOC Xserver tree | `nxagent` is proof that an Xnest-derived server can be a product. But GPL-2.0-only is **incompatible with GPL-3.0**, so it cannot be combined with this OS's code |

NX's `nxagent` is worth studying for one reason: it is a full X server (Xnest
lineage) that renders into an off-screen framebuffer and ships damage rectangles.
That is structurally the same shape as "X server rendering into the native
compositor". It is not usable code here, for licence and C reasons.

### 5.3 Independent servers

| Server | Language / size | Licence | What it proves | Reuse |
|---|---|---|---|---|
| **WeirdX** | Pure Java, 1.0.32, 402 KB source tarball ([jcraft.com/weirdx](https://www.jcraft.com/weirdx/)) | GNU GPL | A from-scratch X11R6.3 server (partial core) with rootless mode, XDMCP, 1/8/16 bpp, no SHM; runs `twm`, `xterm` | Architectural proof and a source of ideas for a small server; unusable on MIPS III and lacks the extensions |
| **Xming** | C/C++, Xorg-derived, Windows-only binaries ([SourceForge](https://sourceforge.net/projects/xming/)) | Mixed permissive metadata (BSD/MIT/PD/zlib); distribution is binary-first, last update 2016 | Xorg's DDX model can target Win32 | None: Windows-specific, no maintained source tree |
| **Cygwin/X** | see above | MIT | Ditto | None |

### 5.4 What this means for reuse

Every reusable server is C and carries a Unix/POSIX + pixman + font stack. The
smallest credible "port a real server" target is **kdrive** (12.3k LOC DDX) on
top of `dix`/`os`/`mi`/`fb` (~90k LOC) — roughly 100k LOC of C, plus pixman, plus
fonts, plus a libc shim. That is not small, but it is bounded and it is the only
path with a realistic chance of running 4Dwm in the near term if the OS already
has a POSIX layer.

Xwayland and Xorg's `hw/xfree86` are not candidates at all: they assume Wayland
or PCI/DRM respectively, neither of which exists on an Indy.

---

## 6. Q4 — What a minimal X11 server must implement to run real IRIX apps

The authoritative specification is the *X Window System Protocol, Version 11,
Release 6.8* (<https://www.x.org/releases/X11R7.7/doc/xproto/x11protocol.html>).
Everything below is a scoping of that document against IRIX 6.5 behaviour, with
the IRIX evidence cited where it exists.

### 6.1 Core protocol (the non-negotiable floor)

**6.1.1 Connection setup and authorisation**

- 12-byte connection prefix: byte order (`'B'` MSBFirst for every IRIX client),
  protocol major/minor (11/0), auth protocol name and data lengths.
- Authorisation: `MIT-MAGIC-COOKIE-1` via `~/.Xauthority` / `XAUTHORITY`;
  `XDM-AUTHORIZATION-1` and host-based access are optional. `xhost`/`xauth`
  compatibility matters because IRIX `xdm` starts sessions with cookies.
- `Setup` reply: release number, resource ID base and mask, vendor string,
  maximum request length, number of screens, and per-screen: root window,
  default colormap, white/black pixels, current input masks, width/height in
  pixels and mm, min/max installed colormaps, root depth, and the full
  depth → visual-type list, plus the pixmap format list (depth, bits per pixel,
  scanline pad, bitmap bit order, bitmap scanline unit, image byte order).
- **Trap:** the resource ID base/mask must be consistent with `XC-MISC` and with
  client-side XID allocation. IRIX `libX11` allocates IDs from the mask; a wrong
  mask produces `BadIDChoice` storms.
- **Trap:** the vendor string and release number are probed by some apps and by
  `xdpyinfo`; yserver's XTS notes record fixing these to Xorg values
  (`"The X.Org Foundation"`, release `12_401_011`) to satisfy `Xlib3` tests.

**6.1.2 Resource model**

Per-client XID ranges, `SetCloseDownMode` (`DestroyAll`, `RetainPermanent`,
`RetainTemporary`), resource lookup with `BadIDChoice` on foreign/out-of-range
IDs, and correct cleanup on disconnect (including freeing selections owned by
the client and sending `SelectionClear`). Windows, pixmaps, GCs, fonts,
colormaps, cursors, and (for extensions) pictures, glyphsets, regions,
damage objects, fences.

**6.1.3 Window tree and lifecycle**

`CreateWindow`, `ChangeWindowAttributes`, `GetWindowAttributes`, `DestroyWindow`,
`DestroySubwindows`, `ReparentWindow`, `MapWindow`, `MapSubwindows`,
`UnmapWindow`, `UnmapSubwindows`, `ConfigureWindow`, `CirculateWindow`,
`QueryTree`, `GetGeometry`.

Semantics that window managers depend on:

- substructure `NotifyMask`, `SubstructureNotify`, `SubstructureRedirect`;
- `ConfigureRequest`/`MapRequest`/`CirculateRequest` redirection, with
  `override-redirect` bypass;
- stacking order, border widths, bit gravity / win gravity, backing store,
  save-under, `override-redirect`, `redirect` (Composite-era, optional);
- `MapNotify`/`UnmapNotify`/`DestroyNotify`/`ReparentNotify`/`ConfigureNotify`/
  `GravityNotify`/`CirculateNotify` with correct `event`/`window` fields and
  `above`/`sibling` ordering;
- `VisibilityNotify` (`Unobscured`, `PartiallyObscured`, `FullyObscured`);
- `Expose` generation on map, unmap, move, resize, clear, shape change, and
  `ClearArea`; `Expose` region coalescing (4Dwm and Motif redraw correctly only
  if exposes are honest — over-exposing is slow, under-exposing leaves artefacts);
- `PropertyNotify` with `NewValue`/`Deleted` and correct timestamps.

**6.1.4 Drawing**

`CreatePixmap`, `FreePixmap`, `CreateGC`, `ChangeGC`, `CopyGC`, `SetDashes`,
`SetClipRectangles`, `FreeGC`, `ClearArea`, `CopyArea`, `CopyPlane`, `PolyPoint`,
`PolyLine`, `PolySegment`, `PolyRectangle`, `PolyArc`, `FillPoly`,
`PolyFillRectangle`, `PolyFillArc`, `PutImage`, `GetImage`, `PolyText8/16`,
`ImageText8/16`.

The drawing model is the part people underestimate:

- GC state: function (16 raster ops), plane mask, foreground/background,
  line width, line style (solid/double-dash), cap/join style, fill style
  (solid/tiled/stippled/opaque-stippled), fill rule, tile/stipple pixmaps,
  tile/stipple origin, font, subwindow mode (`ClipByChildren`/`IncludeInferiors`),
  graphics exposures, clip mask, dash list, arc mode.
- Coordinates are signed 16-bit; rectangles have signed x/y and unsigned w/h;
  `PolyArc` angles are in 64ths of a degree; `PolyText` uses 8-bit or 16-bit
  character deltas with a 255/65535 font-shift convention.
- `CopyArea`/`CopyPlane` must generate `GraphicsExpose`/`NoExpose` when
  `graphics-exposures` is set — yserver's notes call this out as a Motif/Xaw
  compatibility requirement.
- `PutImage` formats: `XYBitmap`, `XYPixmap`, `ZPixmap`; `GetImage` must return
  the server's image byte order, and `ZPixmap` for 8/24/32-bit needs exact
  padding (scanline pad).
- `IncludeInferiors` subwindow mode: drawing into a parent must see through
  child windows; this is used by screenshots and by some toolkits.
- Clipping: pixmap clip masks, rectangles, and shape clip (see SHAPE).

**6.1.5 Colour**

`CreateColormap`, `FreeColormap`, `CopyColormapAndFree`, `InstallColormap`,
`UninstallColormap`, `ListInstalledColormaps`, `AllocColor`, `AllocNamedColor`,
`AllocColorCells`, `AllocColorPlanes`, `FreeColors`, `StoreColors`,
`StoreNamedColor`, `QueryColors`, `LookupColor`.

This is where the Indy differs most from yserver's target:

- 8-bit GR1 (`XL`) defaults to **PseudoColor**, with a hardware colour map;
  24-bit GR1/GR2 defaults to **TrueColor**. IRIX `Xsgi` also exposes 2- and
  4-bit PseudoColor overlay visuals on some hardware
  ([`Xsgi(1)`](http://www.typewritten.org/Manual/SGI/IRIX/6.5.3f/man1/Xsgi.html)).
- `AllocColorCells`/`AllocColorPlanes` must work (read/write cells), with
  `BadAlloc` on TrueColor (yserver's XTS notes list exactly this fix).
- `StoreColors` on a read-only colormap must raise `BadAccess`.
- Colormap installation events and `ColormapNotify` matter for 4Dwm, which
  manages a private/default colour map and expects `InstallColormap` to work.
- IRIX's `-pseudomap gl|envm|pseudo|4sight` options show that GL and X share
  the default PseudoColor map; a software GL bridge will have to cooperate with
  the X colormap allocator rather than assume TrueColor.

**6.1.6 Text and fonts**

`OpenFont`, `CloseFont`, `QueryFont`, `QueryTextExtents`, `ListFonts`,
`ListFontsWithInfo`, `SetFontPath`, `GetFontPath`, plus the text requests above.

IRIX ships fonts under `/usr/lib/X11/fonts/{100dpi,75dpi,misc,Type1,Speedo,CID}`
and `Xsgi` accepts `-fp` and `-fc cursorFont` (default `cursor`)
(`Xsgi(1)`). Consequences:

- A PCF/BDF reader and XLFD matching are the minimum. `fixed`, `6x13`,
  `cursor`, `nil2` and the 75/100 dpi families cover most Motif apps.
- IRIX applications commonly use `XCreateFontSet`/`Xmb` text; that still goes
  through core `OpenFont`/`QueryFont`/`PolyText16`, but it needs correct
  per-character metrics and `XFontSetExtents`, plus locale encodings.
- Type1/Speedo/CID are scalable outline fonts. A clean-room server can either
  rasterise them (FreeType-class work) or omit them initially; apps that ask for
  `-adobe-helvetica-...` will fall back to bitmap fonts if the XLFD match is
  missing. Expect visual degradation, not failure.
- `Xsgi` supports bitmap font scaling (`-nobitscale` turns it off); we should
  not — apps that request arbitrary pixel sizes will get `BadName` and fall back.

**6.1.7 Input, focus and grabs**

`QueryKeymap`, `ChangeKeyboardMapping`, `GetKeyboardMapping`,
`ChangeKeyboardControl`, `GetKeyboardControl`, `Bell`, `ChangePointerControl`,
`GetPointerControl`, `SetScreenSaver`, `GetScreenSaver`, `ForceScreenSaver`,
`SetPointerMapping`, `GetPointerMapping`, `SetModifierMapping`,
`GetModifierMapping`, `GetMotionEvents`, `QueryPointer`, `TranslateCoordinates`,
`WarpPointer`, `SetInputFocus`, `GetInputFocus`, `GrabPointer`, `UngrabPointer`,
`GrabButton`, `UngrabButton`, `ChangeActivePointerGrab`, `GrabKeyboard`,
`UngrabKeyboard`, `GrabKey`, `UngrabKey`, `AllowEvents`, `GrabServer`,
`UngrabServer`.

Semantics that make or break Motif/4Dwm:

- passive grabs with modifier masks and `owner_events`; `AnyModifier` and
  `AnyButton`;
- `AllowEvents` modes `AsyncPointer`, `SyncPointer`, `ReplayPointer`,
  `AsyncKeyboard`, `SyncKeyboard`, `ReplayKeyboard`, `AsyncBoth`, `SyncBoth` —
  menu pop-ups and click-to-focus rely on replay;
- focus events with `detail` = `NotifyAncestor`/`NotifyVirtual`/`NotifyInferior`/
  `NotifyNonlinear`/`NotifyNonlinearVirtual`/`NotifyPointer`/`NotifyPointerRoot`/
  `NotifyDetailNone`, and `mode` = `NotifyNormal`/`NotifyGrab`/`NotifyUngrab`/
  `NotifyWhileGrabbed`;
- `KeyPress`/`KeyRelease`/`ButtonPress`/`ButtonRelease`/`MotionNotify` with
  correct `state`, `time`, `root_x/root_y`, `event_x/event_y`, and `same_screen`;
- auto-repeat detection (`XkbSetDetectableAutoRepeat`) — Motif apps get
  double-clicks wrong without it;
- pointer acceleration/threshold and the core pointer's valuators.

`Xsgi` supports the X Input Extension for "tablet, spaceball, dials, etc."
(`Xsgi(1)`), so Xi1 is not optional for users with those devices, but the core
pointer/keyboard path is sufficient for the desktop.

**6.1.8 Selections, properties, atoms and ICCCM**

`InternAtom`, `GetAtomName`, `ChangeProperty`, `DeleteProperty`,
`GetProperty`, `ListProperties`, `RotateProperties`, `SetSelectionOwner`,
`GetSelectionOwner`, `ConvertSelection`, `SendEvent`.

- Predefined atoms (1–68) and correct `only_if_exists` semantics.
- `GetProperty` must return `bytes_after` in bytes for format 8 and in *items*
  for formats 16/32 (a classic bug; yserver's notes record fixing the
  type-mismatch case).
- Selection ownership, `SelectionRequest`/`SelectionNotify`/`SelectionClear`,
  ICCCM targets (`TARGETS`, `MULTIPLE`, `TIMESTAMP`, `DELETE`, `INSERT_SELECTION`),
  and `CutBuffer` (`SetSelectionOwner` on `CUT_BUFFER0`..`7`) — WeirdX explicitly
  implements CUT_BUFFER0 for copy/paste, and IRIX apps use it too.
- WM protocols: `WM_DELETE_WINDOW`, `WM_TAKE_FOCUS`, `WM_SAVE_YOURSELF`,
  `WM_STATE`, `_NET_WM_*` (the latter only for modern ports, not IRIX).
- ICCCM 2.0 (<https://www.x.org/releases/X11R7.7/doc/xorg-docs/icccm/icccm.html>)
  is the contract 4Dwm implements; the server must not violate it.

**6.1.9 Screens, backing store, save-unders**

- At least one screen; multi-screen (`:0.0`, `:0.1`) is optional but `Xsgi`
  supports `-hw`, `-stacked`, `-first`, so some IRIX setups use it.
- Backing store (`Always`/`WhenMapped`/`NotUseful`) and save-unders can be
  implemented honestly or reported as `NotUseful`/`False`; apps must cope, but
  `Xsgi` supports `-bs`, `-wm`, `-su` and some IRIX apps assume `WhenMapped`.
  A simple "copy the obscured region to an offscreen pixmap" implementation is
  cheap and removes a class of redraw bugs.
- Screen saver: core `SetScreenSaver`/`ForceScreenSaver` plus MIT-SCREEN-SAVER.

**6.1.10 Errors and sequencing**

All 17 core errors with correct `bad_value`, `minor_opcode`, `major_opcode`
and sequence numbers; `BadLength` vs `BadRequest` distinction; errors must not
desynchronise the stream (the reply/error sequence number is the client's only
ordering anchor). Replies must be sent in request order, and events generated by
a request must precede its reply (e.g. `MappingNotify` before the
`GetKeyboardMapping` reply).

### 6.2 Wire-level details specific to this target

| Concern | Requirement | Notes |
|---|---|---|
| Byte order | MSBFirst only, initially | All IRIX clients are big-endian. A `swapped` path can be added later for remote x86 clients; Xorg's model is `client->swapped` + `SwappedProcVector` (`xserver/dix/dispatch.c:2040`, `:3671`) |
| Request length | 4-byte units in the header; BIG-REQUESTS for >256 KB | IRIX `libX11` will use BIG-REQUESTS if the server advertises it; advertise and implement |
| Replies | 32-byte header + payload, `length` in 4-byte units beyond the header | `GetImage`/`ListFontsWithInfo` are the awkward ones |
| Images | Advertise `image-byte-order = MSBFirst`, `bitmap-bit-order = MSBFirst`, scanline pad 32 (or 8 for 8-bit) | Must match what the rasteriser actually produces; the client trusts the setup reply |
| ZPixmap 24-bit | 32 bits per pixel, 24 significant, MSB-first in a 32-bit word | IRIX 24-bit visuals are ZPixmap 32bpp; do not emit packed 24-bit |
| ZPixmap 8-bit | 8 bits per pixel, pad 32 | PseudoColor |
| Events | 32-byte fixed events; generic events (XI2/GE) are also 32 bytes, with an 8-byte prefix and 16 bytes of event data | Only needed if XI2/GE is implemented |
| Timestamps | Milliseconds, monotonic, wrap-safe; `CurrentTime` = 0 | Grab/`MotionNotify` ordering depends on it |

### 6.3 Extensions: what IRIX actually uses

The following table is the core of the scoping decision. "IRIX evidence" cites
SGI documentation where available; otherwise it is inference from the X11
release and the application's library dependencies, marked as such.

| Extension | Spec | IRIX evidence | What the server must do | Est. Rust LOC | Priority |
|---|---|---|---|---:|---|
| **BIG-REQUESTS** | [bigreq](https://www.x.org/releases/X11R7.7/doc/bigreqsproto/bigreq.html) | X11R6 era; `libX11` negotiates automatically | Advertise; accept 32-bit lengths | ~200 | **P0** |
| **XC-MISC** | [xc-misc](https://www.x.org/releases/X11R7.7/doc/xcmiscproto/xc-misc.html) | `libX11` uses it to recycle XIDs | `GetVersion`, `GetXIDRange`, `GetXIDList` | ~200 | **P0** |
| **SHAPE** | [shape](https://www.x.org/releases/X11R7.7/doc/xextproto/shape.html) | `Xsgi(1)` names SHAPE explicitly | Bounding/clip/input regions, `ShapeNotify`, `GetRectangles` with `YXBanded` ordering | ~800 | **P0** |
| **MIT-SHM** | [shm](https://www.x.org/releases/X11R7.7/doc/xextproto/shm.html) | IRIX uses shared-memory transport for local clients (`Xsgi(1)`), and MIT-SHM is standard in X11R6 | `ShmQueryExtension`, `Attach`, `Detach`, `PutImage`, `GetImage`, `CreatePixmap`, `Completion` events | ~900 | **P0** |
| **XTEST** | [xtest](https://www.x.org/releases/X11R7.7/doc/xextproto/xtest.html) | X11R6.1+; test/automation tools | `GetVersion`, `FakeInput`, `GrabControl`, `CompareCursor` | ~400 | P0 (cheap, huge test value) |
| **MIT-SCREEN-SAVER** | [saver](https://www.x.org/releases/X11R7.7/doc/scrnsaverproto/saver.html) | X11R6; `xscreensaver`-style apps | `QueryExtension`, `QueryInfo`, `SelectInput`, `SetAttributes`, `UnsetAttributes`, `Suspend` | ~300 | P0 |
| **SYNC** | [sync](https://www.x.org/releases/X11R7.7/doc/xextproto/sync.html) | X11R6.1; used by DBE/GLX-era apps | counters, alarms, fences, `Await` | ~1,500 | P1 |
| **DOUBLE-BUFFER** | [dbe](https://www.x.org/releases/X11R7.7/doc/xextproto/dbe.html) | X11R6.1; SGI animation apps | `GetVisualInfo`, `AllocateBackBuffer`, `SwapBuffers`, `BeginIdiom`/`EndIdiom` | ~1,200 | P1 |
| **XKEYBOARD** | [xkb](https://www.x.org/releases/X11R7.7/doc/kbproto/xkbproto.html) | `Xsgi` uses `.xkm` keymap files (`Xsgi(1)`) — that is the XKB compiled format | `UseExtension`, `GetMap`/`SetMap`, `GetState`, `LatchLockState`, `GetControls`, `GetNames`, `GetIndicatorState`, `GetCompatMap`, `GetGeometry`, per-client keymap | ~4,000–6,000 (subset) | **P0/P1** |
| **XInput (Xi1)** | [XIproto](https://www.x.org/releases/X11R7.7/doc/inputproto/XIproto.txt) | `Xsgi(1)`: "tablet, spaceball, dials, etc." | `ListInputDevices`, `OpenDevice`, `GetDeviceButtonMapping`, `QueryDeviceState`, `SelectExtensionEvent`, `GrabDevice`, valuator events | ~4,000–6,000 | P1 |
| **RENDER** | [render](https://www.x.org/releases/X11R7.7/doc/renderproto/renderproto.txt) | Post-2000; **not** in IRIX 6.5's original server (inference) | Pictures, glyphsets, `Composite`, trapezoids, triangles, formats | ~3,000–5,000 | P2 |
| **XINERAMA** | Xinerama protocol (xineramaproto; not in X11R7.7 doc set) | SGI used multiple *screens*, not Xinerama (inference) | `QueryVersion`, `QueryScreens`; can be a 1-screen stub | ~300 | P2 |
| **XFree86-VidMode** | `XF86VidMode` (XFree86) | **Not** an IRIX extension; only ported SDL/GL apps probe it | Read-only `GetModeLine`/`GetGamma` stub is enough | ~400 | P2 (stub) |
| **XFree86-DGA** | XFree86 DGA | **Not** IRIX | Do not advertise | 0 | Never |
| **GLX** | [GLX 1.4 spec](https://registry.khronos.org/OpenGL/specs/gl/glx1.4.pdf) | IRIX is GLX's birthplace; OpenGL apps link `libGL.so` | GLX wire protocol + a GL 1.x implementation; visuals/fbconfigs, `MakeCurrent`, `SwapBuffers`, `GetProcAddress` is client-side | 5,000+ for the protocol, 30k+ for software GL | P3 (separate project) |
| **XIE** | XIE 5.0 (X11R6.3-era; dropped from X11R7) | IRIX shipped XIE in the X11R6 era (SGI led the specification); `Xsgi(1)` does not enumerate it | Full image-processing protocol and codecs | 50k+ | **Never** (see §6.4) |
| **PEX** | PEX 5.x | IRIX 5.x/6.x shipped PEX; obsolete by 6.5 | 3D PHIGS protocol | huge | Never |
| **DPS / Adobe PostScript** | Display PostScript | `Xsgi(1)` names "Adobe PostScript" | PostScript interpreter in the server | huge | Never |
| **RECORD** | [record](https://www.x.org/releases/X11R7.7/doc/recordproto/record.html) | X11R6; used by some test tools | `CreateContext`, `RegisterClients`, `EnableContext`, data events | ~1,500 | P2 |
| **XFIXES** | [fixes](https://www.x.org/releases/X11R7.7/doc/fixesproto/fixesproto.txt) | Post-2000; not IRIX | Regions, cursor/selection notifications, `SaveSet` | ~1,500 | P2 |
| **Composite / DAMAGE** | [composite](https://www.x.org/releases/X11R7.7/doc/compositeproto/compositeproto.txt), [damage](https://www.x.org/releases/X11R7.7/doc/damageproto/damageproto.txt) | Not IRIX | Offscreen redirect, damage tracking | ~3,000 | P2 (only if the native compositor wants it) |
| **RandR** | [randr](https://www.x.org/releases/X11R7.7/doc/randrproto/randrproto.txt) | Not in IRIX 6.5 (Xsgi uses `-hw`/`-boards` instead) | `QueryVersion`, `GetScreenResources`, modes/outputs; a static 1-output implementation is enough | ~1,000 | P2 |
| **Present / DRI3** | Present, DRI3 | Not IRIX | vsync/pixmap presentation, DMA-BUF | ~2,000 | Never on GR1/GR2 |
| **XVideo** | [xv](https://www.x.org/releases/X11R7.7/doc/videoproto/xv-protocol-v2.txt) | IRIX used SGI VL/dmedia instead | Video ports, adaptors, `PutImage` | ~3,000 | P3 (stub) |
| **SECURITY** | [security](https://www.x.org/releases/X11R7.7/doc/xextproto/security.html) | Not an IRIX extension in practice | Authorisation levels | ~1,000 | Never |
| **X-Resource** | [res](https://www.x.org/releases/X11R7.7/doc/resourceproto/resproto.txt) | Not IRIX | Client resource accounting | ~500 | P3 (stub) |
| **XDMCP** | [XDMCP](https://www.x.org/releases/X11R7.7/doc/libXdmcp/xdmcp.html) | IRIX `xdm` supports it; Xsgi man page says "Xdmcp doesn't support DES" | UDP display-manager protocol (not an X extension) | ~1,500 | P2 |

**Reading the table.** P0 is ~3,000 lines of extension code on top of the core;
P1 adds ~12,000; the full IRIX-plausible set (P0+P1+P2) is ~25,000 lines. That
is the whole extension budget, and it is small because IRIX predates the modern
compositor stack.

### 6.4 What IRIX applications actually need

**Indigo Magic desktop / 4Dwm.** 4Dwm is SGI's Motif-based window manager (mwm
lineage, heavily SGI-extended); it is an ordinary X client. It needs: core
window/event/grab
semantics, SHAPE (for window frames and icons), colormaps, `WM_*` properties,
`SaveSet`, `ReparentWindow`, `ConfigureRequest` redirection, and correct focus
handling. It does **not** need RENDER, Composite, Damage, Present, DRI3, RandR
or Xinerama. `dtterm`, `dtfile`, `dtpad`, `toolchest` and the icon catalogue are
Motif/Xaw/XPM clients with the same needs plus core fonts and colour allocation.

**Motif (`libXm`).** Motif's menu system uses passive grabs and `AllowEvents`
replay; its text widgets use `XCreateFontSet`; its colour handling uses
`XAllocColor`/`XAllocColorCells` and `XQueryColors`. It probes `XShapeQueryExtension`,
`XInputExtension` and `XkbUseExtension` and degrades gracefully if absent — but
the degradation is visible (no shaped menus, wrong key repeat, wrong modifiers).

**OpenGL / GLX.** IRIX's defining application class. Every IRIX GL app links
`libGL.so` and speaks GLX to the server. On an Indy without the Indy 3D option,
GR1/GR2 are 2D only, so GL apps would need a software GL. This is a separate
30–60 person-month project (a GL 1.x software rasteriser plus the GLX wire
protocol), and it should be treated as a later phase, not part of "X11".

**dmedia / VL.** SGI's Digital Media and Video Library are OS media APIs, not X
extensions. Applications that display video in a window need a window ID and
exposure events from the X server; the pixels themselves travel through the
graphics pipe, not the X protocol. The exact VL-to-X binding is media-stack
work, not server work. Do not conflate the two.

**XIE.** IRIX shipped the X Image Extension in the X11R6 era (SGI led the
specification), and SGI's imaging tools (`imgworks`, `ipaste`, parts of
`mediarecorder`) can use it; `Xsgi(1)` itself only names DPS, XInput and SHAPE.
XIE 5.0 is a large
image-processing protocol with JPEG/TIFF codecs and its own resource model. It
was dropped from X11R7 and has essentially no modern implementation. The
realistic position is: **do not implement XIE; do not advertise it.** Applications
that hard-require XIE will fail; applications that probe and fall back will work.
This should be an explicit, documented compatibility exclusion.

**PEX and DPS.** Same reasoning. PEX is the PHIGS 3D extension; DPS is Adobe
Display PostScript. Both are obsolete, huge and rarely required by 6.5-era
applications that also run on other X servers. Do not implement.

### 6.5 Non-protocol server obligations

- **Fonts on disk.** `-fp` default path and `mkfontdir`/`fonts.dir` handling;
  `rgb.txt` for `XAllocNamedColor`; the `cursor` font for `CreateGlyphCursor`.
- **Keymaps.** IRIX `Xsgi` loads `.xkm` files from `/usr/lib/X11/input`; a
  clean-room server can ship a compiled-in US keymap and optionally parse `.xkm`
  later. `XkbGetMap` clients (Motif, some IRIX tools) must at least see a
  consistent map.
- **Authority and transport.** `/tmp/.X11-unix/X0`, `~/.Xauthority`, and
  `-auth`; the IRIX shared-memory transport can be ignored if `libX11` falls
  back to UNIX sockets as documented in `Xsgi(1)` — but this must be validated
  early with a real IRIX `libX11.so`, because it is the one IRIX-specific
  transport assumption that could bite.
- **Performance envelope.** R4400 at 100–200 MHz, 32–128 MB, no GPU. The server
  must be a scanline rasteriser with a shadow framebuffer, hardware cursor if
  available, and no per-pixel allocation. RENDER and Composite must not be on
  the hot path. A 1280×1024 8-bit framebuffer is 1.3 MB; 24-bit is 3.9 MB;
  backing store for a full screen is another 1.3–3.9 MB — affordable only if
  opt-in.
- **Memory discipline.** No `Vec` per drawing op, no per-request allocation in
  the steady state, bounded event queues, and a hard cap on pixmap memory with
  `BadAlloc` when exceeded (IRIX apps allocate large pixmaps for images).

### 6.6 Minimum test matrix

| Level | Tests |
|---|---|
| Wire | X.Org X Test Suite (`xts5`) scenarios `Xproto`, `Xlib3`, `Xlib9`, `Xlib10`, `ShapeExt`, `XTest`; run against a host Xvfb as the oracle |
| Tools | `xdpyinfo`, `xlsfonts`, `xfontsel`, `xauth`, `xhost`, `xmodmap`, `xset`, `xsetroot`, `xrdb` |
| Apps | `xterm`, `xeyes`, `xcalc`, `xman`, `twm`, `mwm` (if available), `4Dwm`, `dtterm`, `dtfile`, `toolchest` |
| Stress | `x11perf` (fill/copy/text), `x11perf -scroll500`, `xterm` with `yes`, multiple clients, `x11vnc` as a screen grabber |
| Visual | screenshot diff against Xvfb/Xorg with the same font path and visual |

---

## 7. Q5 — The realistic paths compared

### 7.1 (a) Clean-room minimal X11 server in Rust

Write a new server, spec-driven, with a backend abstraction: an in-memory
framebuffer for development/tests, a native-compositor backend for production,
and later a Newport/Express scanout backend.

**Pros**

- Fits ADR-0010 (Rust monolith) and ADR-0009/0014 (clean-room) exactly.
- Small: ~20–30k LOC for the IRIX-relevant subset, versus ~300k for Xorg.
- No POSIX/pixman/fontconfig/libdrm dependency chain; only libc-equivalent
  syscalls the OS already provides.
- Endianness is a design choice, not a port: MSBFirst from day one, no swap
  layer until remote clients are wanted.
- The backend seam gives the native window system a first-class integration
  (X windows as compositor surfaces) rather than a nested-X hack.
- Testable early with a host-side framebuffer backend and Xvfb as an oracle.

**Cons**

- All the protocol edge cases are yours: grabs, replay, focus details, expose
  regions, colour allocation, font metrics, `BadLength` vs `BadRequest`.
  yserver's XTS history (Xproto PASS 1 → 337 in a concentrated push) is the
  realistic curve.
- Fonts: PCF/BDF is easy; Type1/Speedo/CID is not.
- XKB and Xi1 are deceptively large.
- GLX is out of scope and must be a separate project.
- No upstream to inherit security fixes or protocol updates from.

**Effort:** 11–17 person-months for a core server (core protocol + P0
extensions + 8-bit PseudoColor + 24-bit TrueColor + fonts + a framebuffer
backend, running `xterm`/`twm`); 16–25 person-months to reach 4Dwm + `dtterm` +
Motif applications (adds XKB, Xi1, RENDER/DBE/SYNC and the Newport/Express
backends); 19–30 person-months for broad IRIX parity including hardening. GLX
adds 30–60 person-months separately.

### 7.2 (b) Port Xorg / kdrive

Bring `dix`+`os`+`mi`+`fb`+`kdrive` (~100k C LOC) plus pixman and a font stack
into the OS, and write a Newport/Express DDX.

**Pros**

- Highest fidelity: 20+ years of bug fixes, complete extension set, XTS-clean.
- MIT licence, big-endian support proven on s390x/ppc64/sparc64.
- kdrive is the smallest credible target and was designed for embedded
  framebuffer devices; `fbdev` maps well to GR1/GR2.
- Immediate access to Xvfb/Xephyr-style testing and to the whole X11 test
  ecosystem.

**Cons**

- Contradicts the project's Rust-native and clean-room constraints: it imports
  ~100k LOC of C plus pixman, FreeType and fontconfig into a GPL-3.0 Rust OS.
- Needs a POSIX layer (sockets, `poll`, `mmap`, SysV SHM, signals, threads) and
  a C toolchain in the image; the Indy has 32–128 MB and little RAM to spare.
- Xorg's architecture assumes dynamic module loading, PCI, DRM and hotplug;
  most of `hw/xfree86` (124k LOC) is dead weight.
- The DDX driver model is exactly what yserver rejected as legacy baggage; it
  buys compatibility at the cost of permanent maintenance of alien code.
- Performance on R4400 with pixman's generic paths is likely worse than a
  purpose-written scanline rasteriser.

**Effort:** 12–23 person-months to get kdrive running on Indy-class hardware
(2–4 libc/POSIX, 3–6 pixman/fonts, 2–3 DDX, 2–4 compositor integration, 3–6
debugging), plus ongoing C maintenance. Full Xorg is 20–40 person-months and is
not sensible.

**Verdict:** technically the fastest route to a working 4Dwm, strategically
wrong for this project. It could be justified only as a temporary oracle or as
a bring-up vehicle, never as the shipped server.

### 7.3 (c) Xlib shim over the native compositor

Provide an `libX11.so`-compatible library whose entry points talk to the native
window system directly, with no X wire protocol.

**Pros**

- No server process, no protocol serialisation, no XID namespace.
- Potentially lower latency for simple drawing.
- Reuses the native compositor's input, focus and window management.

**Cons**

- You still implement the entire Xlib API, including the parts that are
  protocol semantics in disguise: event queues, grabs, `XGrabServer`, property
  change notifications, selection transfer, `XSync`, error handling, resource
  lifetimes, `XGetImage`, colour allocation, fonts.
- Any application that uses an extension (`XShape*`, `XShm*`, `XTest*`, `Xkb*`,
  `Xi*`) needs a bespoke shim per extension; any application that speaks the
  protocol directly (a surprising number of toolkits and emulators do) breaks.
- Remote display, XDMCP and X11 forwarding become impossible.
- GLX cannot be shimmed meaningfully: GLX is a protocol, and IRIX `libGL`
  expects a GLX server.
- It does not reduce the client-side work (you still need Xlib, Xext, Xt,
  Motif); it only removes the server, at the cost of fidelity.

**Effort:** 10–16 person-months for a shim covering the Motif/Xaw subset, with
a long tail of application-specific breakage. Higher risk than (a) for similar
effort.

**Verdict:** the worst of both worlds for a compatibility-first OS. Reject.

### 7.4 (d) Xwayland-style bridge

Run a Wayland compositor natively and Xwayland (C, 20.9k LOC) as the X server,
with the native compositor as the Wayland server.

**Pros**

- Reuses the most complete X server implementation available.
- Xwayland's protocol coverage is excellent (it is Xorg's core plus a Wayland
  backend).
- If the native window system is already Wayland-shaped, the integration is
  well-understood.

**Cons**

- Requires a Wayland compositor, EGL, GL and glamor; Xwayland renders every X
  drawable through GL. On GR1/GR2 there is no GL, so this means Mesa
  llvmpipe/softpipe — tens of MB of code and RAM on a 32 MB machine.
- Needs `wl_surface` per window, DRI3/Present-style buffer sharing, and a
  functioning `libdrm`/GBM path that the Indy does not have.
- Wayland's client-side libraries and protocols are a second moving part; a
  Rust Wayland compositor (Smithay) is feasible but adds its own cost.
- Same clean-room/Rust objections as (b), plus a much heavier runtime.

**Effort:** 25–40 person-months (Wayland compositor + Xwayland port + Mesa
software GL + integration), with a real risk of being unusable at 32 MB.

**Verdict:** not viable for this hardware.

### 7.5 (e) Defer X11

Ship the OS with a native window system and no X11; revisit later.

**Pros**

- Zero cost now; keeps the kernel/compat work unblocked.
- Avoids committing to a server architecture before the native window system,
  input model and graphics driver are settled.
- Legacy apps that are console-only still run.

**Cons**

- The headline promise ("runs existing IRIX 6.5 binaries unchanged") is not met
  for GUI applications, which is most of what IRIX is remembered for.
- Delay increases the risk that the client-side ABI work (libX11/Xt/Motif) is
  discovered too late.

**Effort:** 0 now; the same 12–30 person-months later, plus integration risk.

**Verdict:** a schedule decision, not an architecture. Reasonable only if the
native window system and GR1/GR2 driver must land first (they must).

### 7.6 Comparison table

| Criterion | (a) Clean-room Rust | (b) Port kdrive/Xorg | (c) Xlib shim | (d) Xwayland | (e) Defer |
|---|---|---|---|---|---|
| Fits Rust-native mandate | **Yes** | No (C) | Yes | No (C + Wayland) | Yes |
| Fits clean-room mandate | **Yes** | No | Yes | No | Yes |
| Effort to first 4Dwm | 16–25 PM | 12–23 PM | 10–16 PM | 25–40 PM | n/a |
| Effort to broad parity | 19–30 PM | 20–40 PM | 20–30 PM (est.) | 35–50 PM | n/a |
| Protocol fidelity | Medium at first, high with XTS | **High** | Low | **High** | n/a |
| Extension coverage | Exactly what we build | Everything | Per-extension shims | Everything | n/a |
| GLX | Separate project | Available but needs GL | Impossible | Via glamor | n/a |
| Runtime footprint | **Low** (fits 32 MB) | High (C + pixman) | Low | **Very high** | n/a |
| Big-endian | By design | Proven | By design | Proven (s390x) | n/a |
| Maintenance | Ours, small | Alien, large | Ours, medium | Alien, large | n/a |
| Risk | Protocol edge cases | Build/integration | Semantic gaps | Hardware infeasible | Product gap |

### 7.7 Recommendation

**Choose (a): a clean-room Rust X11 server with a backend seam.**

Rationale, in order of weight:

1. **(b) and (d) violate the project's own constitution** (ADR-0010 Rust
   monolith; ADR-0009/0014 clean-room). Even if they were technically easier,
   they would not be shippable in this repository. (b) also imports a POSIX and
   graphics stack that the Indy cannot afford.
2. **(c) does not actually save the expensive work.** The hard parts of X11 for
   compatibility are semantics (grabs, focus, exposures, colour, selections),
   and a shim must reimplement them with less information and no protocol
   boundary to test against.
3. **(a) is small enough to be credible.** The IRIX-relevant subset is ~20–30k
   LOC. yserver demonstrates that a from-scratch Rust server can run CDE — the
   very desktop IRIX ships — and its XTS history shows where the time goes.
4. **The hardware forces the architecture.** No DRM, no Vulkan, no GL, 32 MB.
   A scanline rasteriser over a shadow framebuffer with a hardware cursor is the
   only design that fits, and that is a backend, not a porting project.

**Prescribed shape:**

- Four crates, mirroring the seam yserver converged on and Xorg's
  `dix`/DDX separation:
  `x11-proto` (wire types, MSBFirst), `x11-server` (resources, windows, events,
  grabs, colour, fonts, extensions), `x11-render` (scanline rasteriser,
  framebuffer, cursor), `x11-backend-*` (fbdev/Newport/Express/native).
- Backends: `null`/memory (tests), `host-window` (development on a Linux host,
  like Xephyr), `newport` (GR1), `express` (GR2), `compositor` (native window
  system).
- Implement **MSBFirst only**; add a `swapped` path behind a feature flag later.
- Implement **8-bit PseudoColor and 24-bit TrueColor** from the start; do not
  repeat yserver's TrueColor-only decision.
- Definition of done for v1: 4Dwm + `dtterm` + `dtfile` + `toolchest` + a Motif
  test app, with `xts5` Xproto/Xlib3/Xlib9/ShapeExt green or explicitly waived.
- GLX, XIE, PEX and DPS are explicit non-goals for v1, documented as such.

**Effort, phased:**

| Phase | Scope | Person-months | Exit criterion |
|---|---|---:|---|
| 0 | Spec harness, Xvfb oracle, wire codec, setup/auth, MSBFirst | 2–3 | `xdpyinfo` connects and reports correct screens/visuals |
| 1 | Core resources, windows, events, GCs, drawing, `PutImage`/`GetImage` | 4–6 | `xterm`, `xeyes`, `xcalc` run; XTS Xproto ≥ 80% |
| 2 | Fonts (PCF/BDF), colour (PseudoColor + TrueColor), selections, grabs, focus | 3–5 | `twm` manages windows; `xfontsel`, `xclip` work |
| 3 | P0 extensions: BIG-REQUESTS, XC-MISC, SHAPE, MIT-SHM, XTEST, screen saver | 2–3 | XTS ShapeExt green; `x11perf` stable |
| 4 | XKB subset, Xi1, SYNC/DBE, RENDER; Newport/Express backends; performance | 5–8 | 4Dwm + `dtterm` + Motif apps; 8-bit colormaps correct |
| 5 | Hardening: XTS waivers, memory limits, multi-screen, XDMCP | 3–5 | 24 h soak with the IRIX desktop; no leaks |
| **Total** | | **19–30** | |
| GLX (separate) | GLX wire protocol + software GL 1.x | 30–60 | OpenGL apps |

The ranges are deliberately wide: protocol work is dominated by edge cases, and
the first encounter with a real IRIX `libX11.so` could shift them.

### 7.8 The client-side gap (do not lose sight of it)

A server alone does not run IRIX binaries. They link, at minimum:

- `libX11.so` — Xlib, ~20–30k LOC to reimplement the ABI and semantics;
- `libXext.so` — SHAPE, MIT-SHM, DBE, SYNC, XTEST, XInput wrappers;
- `libXi.so` — XInput;
- `libXt.so` — X Toolkit Intrinsics (~10–15k LOC);
- `libXm.so` — Motif 2.x (~100k+ LOC, the dominant cost);
- `libXpm`, `libICE`, `libSM`, `libXmu`, `libXaw` as used.

There are two strategic options, and the project must choose explicitly:

1. **Clean-room reimplement all of them.** Then X11 server effort is a minority
   of the GUI compatibility effort; Motif alone dwarfs it.
2. **Allow the original IRIX shared libraries as binary compatibility
   artefacts** (not published, not part of the clean-room source). Then the
   client side is free and the server is the only X11 work — but this collides
   with ADR-0009/0014 and needs a decision.

This report assumes option 1 for planning purposes, because that is what the
ADRs say today. Either way, **the server should be built first**: it is the
component that cannot be borrowed, and it defines the ABI surface the client
libraries must speak to.

---

## 8. Q6 — Prior art

### 8.1 X11 on big-endian hardware

X11 is one of the few GUI protocols designed from the start to be
byte-order-neutral. The evidence is broad:

| System | What it proves | Source |
|---|---|---|
| **IRIX `Xsgi`** | A full X11R6 server on big-endian MIPS III, with SHAPE, XInput and DPS (named in the man page), multiple screens, overlay planes, a shared-memory local transport, and XIE in the X11R6 era | [`Xsgi(1)`](http://www.typewritten.org/Manual/SGI/IRIX/6.5.3f/man1/Xsgi.html) |
| **Xorg on s390x / ppc64 / sparc64** | The modern Xorg core, `fb`, `mi`, pixman and the extension set are endian-clean; Debian builds `xorg-server`/`Xvfb` for these big-endian architectures | <https://buildd.debian.org/status/package.php?p=xorg-server> |
| **XQuartz on PowerPC** | Apple shipped X11 (XFree86, then Xorg) on big-endian G4/G5 Macs for years | <https://www.xquartz.org/> |
| **AIX, Solaris/SPARC, HP-UX/PA-RISC** | Commercial X11 servers on big-endian Unix were the norm through the 1990s | Historical; X11R6 vendor servers |
| **NetBSD/sgimips** | NetBSD runs on Indy with a `newport(4)` framebuffer console — a maintained, open driver for the same hardware | <https://www.netbsd.org/ports/sgimips/>, <https://man.netbsd.org/sgimips/newport.4> |
| **Linux/MIPS on Indy** | XFree86/Xorg shipped a working `newport` driver for GR1 (8- and 24-bit, hardware cursor, 1280×1024) | [`xf86-video-newport`](https://gitlab.freedesktop.org/xorg/driver/xf86-video-newport) |

**The single most relevant prior-art artefact is `xf86-video-newport`**
(~4,000 LOC, MIT, Guido Guenther, 2000–2003; still in Xorg). It is a real Xorg
DDX for the exact graphics chip in the target machine, derived from the Linux
kernel `newport` console driver. Its README states:

> This is an unaccelerated driver for the SGI newport cards (a.k.a. XL) as found
> in the SGI Indy and Indigo2. Both the 8bit and 24bit versions are tested and
> working. […] There's only a 1280x1024 mode.

That gives us: register-level knowledge, mode timing, colour-map handling,
hardware-cursor programming and a proven 8-bit PseudoColor path — all of which
a new Rust backend needs. It does **not** give us the server, and its licence
(MIT) is compatible, but the clean-room policy means it is a reference, not a
source to transcribe.

**Endianness pitfalls to expect** (from Xorg/yserver experience):

- Image byte order and bitmap bit order in the `Setup` reply must match the
  rasteriser; a mismatch produces "correct protocol, garbled pixels".
- RENDER picture formats are defined as ARGB/ABGR CARD32s in *native* order;
  on big-endian the byte layout of `a8r8g8b8` differs from little-endian, and
  every composite path must be tested.
- `GetImage`/`PutImage` ZPixmap padding and `scanline-pad` are easy to get wrong.
- XKB keymaps and `xkbcomp` output are endian-tagged; a BE server must not
  accept LE-compiled `.xkm` files blindly.
- Font bitmap formats (PCF) have a defined on-disk byte order independent of
  the host; BDF is text. Neither is a portability problem, but test the glyph
  unpacking on big-endian.

### 8.2 X11 servers on unusual or windowless platforms

| Project | Lesson for IRIX 7 |
|---|---|
| **Xnest** | A server can be a client of another display; the protocol boundary is a clean seam. This is the design template for a "native compositor backend", but it presupposes a host X server |
| **Xephyr** | Same idea with XCB + SHM; shows how to blit a shadow framebuffer into a host window at interactive rates |
| **Xvfb** | A server needs no hardware at all: `fb` + `mi` + `dix` is sufficient. The best development and CI backend |
| **WeirdX** | A from-scratch, pure-Java X11R6.3 server with rootless mode, XDMCP and no SHM. Proof that "minimal but real" is achievable; also a warning about how much of X11 is optional in practice |
| **nxagent (nx-libs)** | Xnest-derived server plus damage-based compression; 75k LOC. Proof that a nested server can be a product. GPL-2.0-only blocks reuse here |
| **Cygwin/X, Xming, XQuartz** | A DDX can target a completely foreign window system; the core is portable if the DDX seam is right |
| **Xpra** | Screen forwarding is a proxy problem, not a server problem; do not conflate the two |

### 8.3 Rust X11 server experiments

| Project | Date | Outcome |
|---|---|---|
| [`yserver`](https://github.com/joske/yserver) | Apr 2026 – present | The only serious attempt. CDE/XFCE/MATE/Cinnamon working; XTS-driven; MIT. Explicitly modern-Linux-shaped |
| [`x12`](https://github.com/systemrustd/x12) | 2026 | Hard fork of yserver for cross-platform; undocumented, unproven |
| [`rxserver`](https://github.com/PerikiyoXD/rxserver) | Jun 2025 – Jul 2026 | TCP-only, `softbuffer` output, many core requests missing; ~15k LOC |
| [`x11rs/x11server`](https://github.com/x11rs/x11server) | Jul 2020 | Empty stub (2 KB) |
| [`X-Lucifer/rust-xserver`](https://github.com/X-Lucifer/rust-xserver) | — | Empty/experimental |
| [`x11-lab/x11-lab`](https://github.com/x11-lab/x11-lab) | 2026 | Tutorial, not a server |

No Rust server has been run on big-endian hardware, and none implements a
legacy visual model. yserver's big-endian *client* work is the only
endian-related server code in the ecosystem, and it is a useful map of what
breaks (see `docs/xts-baseline.md` and `docs/known-issues.md` in that repo).

### 8.4 Rust on big-endian MIPS

The project has already settled this: `targets/mips3-sgi-irix7-n32.json`
declares `"target-endian": "big"`, `arch: mips64`, `cpu: mips3`, n32 ABI, and
the toolchain spike (issue #41) proved stock rustc + `-Z build-std` works with a
custom target. Practical consequences for the server:

- `cfg(target_endian = "big")` paths must be exercised in CI on the target or a
  big-endian emulator, not just on x86_64 with a cross-compile.
- Avoid `bytemuck`/`zerocopy` struct casts over wire buffers; decode explicitly
  with `from_be_bytes`, as yserver does.
- `core::sync::atomic` on R4400: 32- and 64-bit atomics exist (MIPS II/III
  `ll`/`sc`, `lld`/`scd`), and the target spec declares `max-atomic-width: 64`;
  verify the emitted sequences and avoid `AtomicU128` (absent).
- Soft-float (`+soft-float` in the target spec): no FPU assumptions in the
  rasteriser; fixed-point is safer for geometry anyway.

### 8.5 What to take from the prior art

1. **From Xorg/Xvfb:** the protocol semantics and a test oracle. Run Xvfb on the
   host, capture its replies/events, diff against ours. Never copy code.
2. **From `xf86-video-newport`:** the hardware behaviour of GR1 (registers,
   mode, cursor, colour map). Re-derive the driver clean-room from the
   documentation and observed behaviour; the MIT licence makes reference safe,
   the ADRs make transcription unsafe.
3. **From yserver:** the crate seam, the XTS methodology and the bug list. It is
   the best available checklist of "things that make Motif apps work".
4. **From WeirdX/Xnest:** proof that a small server is viable, and that
   "rootless" (windows as native surfaces) is a legitimate model.
5. **From nxagent:** damage-rectangle presentation into an off-screen
   framebuffer — structurally what a native-compositor backend does.

---

## 9. Conclusions, risks and open questions

### 9.1 The six answers in brief

1. **Is there a complete Rust X11 server?** One real one, `yserver` (MIT), which
   runs modern Linux desktops but is not portable to GR1/GR2 and deliberately
   excludes the legacy visual model IRIX needs. No complete, portable, legacy-
   capable Rust server exists.
2. **Rust ecosystem:** dominated by one excellent client library (`x11rb`,
   MIT/Apache-2.0) and one strong server (`yserver`). Everything else is FFI
   bindings to C (`x11`, `xcb`), a client toolkit (`winit`, Smithay, wayland-rs),
   or dead. There is no Rust Xlib, no Rust extension library, no server
   framework.
3. **Other-language servers:** Xorg and its DDX family are MIT and technically
   reusable but bring ~100–300k LOC of C, pixman, fonts and a POSIX dependency
   into a Rust OS. Xwayland needs Wayland + EGL + GL and is infeasible on this
   hardware. Xpra/nxagent are proxies/nested servers, not reuse targets
   (nxagent is GPL-2.0-only, incompatible with GPL-3.0). WeirdX proves the
   from-scratch approach but is Java and incomplete.
4. **What a minimal server needs:** the full core protocol (setup, resources,
   windows, events, GCs, drawing, images, fonts, colour, selections, grabs,
   focus, errors), MSBFirst wire order, and a small extension set: BIG-REQUESTS,
   XC-MISC, SHAPE, MIT-SHM, XTEST, MIT-SCREEN-SAVER, then XKB, Xi1, SYNC, DBE,
   RENDER. GLX is a separate project; XIE/PEX/DPS should be explicit non-goals.
5. **Paths:** (a) clean-room Rust server — **recommended**, 16–25 PM to 4Dwm,
   19–30 PM to parity; (b) port kdrive/Xorg — 12–23 PM but violates the
   Rust/clean-room mandate and imports a C stack; (c) Xlib shim — 10–16 PM but
   loses protocol fidelity and still needs all the semantics; (d) Xwayland —
   25–40 PM and infeasible at 32 MB; (e) defer — a schedule choice, not an
   architecture.
6. **Prior art:** X11 is endian-neutral by design and has run on big-endian
   MIPS (IRIX `Xsgi` itself), s390x/ppc64/sparc64 (Xorg), PowerPC (XQuartz),
   SPARC/PA-RISC (commercial Unix). For Indy specifically there is
   `xf86-video-newport` (Xorg DDX, ~4k LOC, MIT) and NetBSD's `newport(4)`.
   Rust server experiments are yserver, x12, rxserver and two stubs; none on
   big-endian.

### 9.2 Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| IRIX `libX11` insists on the SGI shared-memory transport and does not fall back cleanly | Medium | High | Spike early with a real IRIX binary; implement the transport if needed (it is a simple shared ring, not MIT-SHM) |
| Type1/Speedo/CID fonts unavailable → wrong metrics/visuals | High | Medium | Ship bitmap fonts first; add a FreeType-class rasteriser only if acceptance apps require it; document degradation |
| XKB `.xkm` compatibility (IRIX keymaps) | Medium | Medium | Start with a built-in US map; parse `.xkm` later; test `xmodmap`/`xkbcomp` behaviour |
| 8-bit PseudoColor colormap semantics wrong (GL sharing, `AllocColorCells`) | High | High | Implement PseudoColor first, test with 4Dwm and a colour-map torture app; use `-pseudomap` behaviour as a spec |
| Motif/Xt ABI dwarfs the server | Certain | High | Decide early whether IRIX shared libs are allowed; if not, plan Motif as a separate multi-year workstream |
| R4400 performance (expose storms, `x11perf`) | Medium | Medium | Scanline rasteriser, damage-tracked presents, avoid RENDER on the hot path, profile on emulator |
| Memory exhaustion from pixmaps/backing store | Medium | Medium | Hard accounting, `BadAlloc`, backing store off by default |
| Clean-room contamination from reading yserver/Xorg code | Medium | High | Behavioural reference only; harness-enforced (ADR-0014); keep an audit log of consulted sources |
| Third-party crate assumes little-endian | Medium | Medium | Avoid `bytemuck` casts on wire data; CI on big-endian emulator |
| No 3D → GLX apps fail | Certain | High | Declare GLX a non-goal for v1; plan a software GL project separately |

### 9.3 Open questions (decisions needed)

1. **Binary compatibility libraries.** May IRIX binaries link against the
   original `libX11.so`/`libXm.so` from the IRIX discs (not published, not part
   of the clean-room source), or must every client library be reimplemented?
   This single decision changes the GUI programme by an order of magnitude.
2. **Acceptance suite.** Which applications define "runs IRIX binaries"? A
   credible v1 list: 4Dwm, `dtterm`, `dtfile`, `toolchest`, `xterm`, `xclock`,
   one Motif app (e.g. `xman`/`dtpad`), one GL app deferred.
3. **Default visual.** Is 8-bit PseudoColor required for v1, or is 24-bit XL
   (`-class TrueColor -depth 24`) acceptable as the default with PseudoColor
   later? IRIX apps differ in their tolerance.
4. **Native window system interface.** Does the compositor expose a surface /
   damage / input API that an X backend can target without a nested protocol?
5. **GLX strategy.** None, a software Mesa port, or a bridge to a native GL
   service? This is the largest single decision after the server itself.
6. **XDMCP/xdm.** Is graphical login in scope for v1, or is `startx`-style
   launch enough?
7. **Multi-screen.** Is `:0.1` (second board/head) in scope, or is one screen
   enough for the Indy configuration?

### 9.4 Licence matrix

| Component | Licence | GPL-3.0 compatible? | Note |
|---|---|---|---|
| `yserver` | MIT | Yes | Clean-room policy still forbids copying; behavioural reference only |
| `x11rb` / `x11rb-protocol` | MIT OR Apache-2.0 | Yes | Safe dependency for client-side tooling |
| `breadx` | BSL-1.0 | Yes | Permissive; stale |
| `winit`, `smithay`, `wayland-rs`, `calloop` | MIT / Apache-2.0 | Yes | Client/compositor side |
| Xorg, Xwayland, Xvfb, Xnest, Xephyr, kdrive, Cygwin/X | MIT | Yes | But see the Rust/clean-room constraints |
| `xf86-video-newport` | MIT | Yes | Reference only under ADR-0009/0014 |
| Xpra | GPL-2.0+ | Yes (GPLv2-or-later → GPLv3) | Proxy, not a server |
| nx-libs / nxagent | GPL-2.0 (v2, per `debian/copyright`) | **No** if v2-only | Cannot be combined with GPL-3.0 |
| WeirdX | GNU GPL (version not verified in source headers) | Only if v2-or-later | Java; not reusable regardless |
| Xming | Metadata: BSD/MIT/PD/zlib | Unclear | Binary distribution, no maintained source |
| IRIX `Xsgi`/`libX11` binaries | SGI proprietary | No (distribution) | Clean-room forbids; see open question 1 |

### 9.5 Sources

**Servers and implementations**

- yserver — <https://github.com/joske/yserver>; `docs/high-level-design.md`,
  `docs/extensions.txt`, `docs/xts-baseline.md`, `docs/known-issues.md`,
  `docs/status.md`, `crates/yserver-protocol/src/x11/wire_swap.rs`,
  `crates/yserver-protocol/src/x11/request_swap.rs`, `Cargo.toml`.
- x12 — <https://github.com/systemrustd/x12>.
- rxserver — <https://github.com/PerikiyoXD/rxserver>.
- x11rs/x11server — <https://github.com/x11rs/x11server>.
- x11-lab — <https://github.com/x11-lab/x11-lab>.
- Xorg server — <https://gitlab.freedesktop.org/xorg/xserver>; measured paths:
  `dix/dispatch.c`, `os/io.c`, `hw/vfb/InitOutput.c`, `hw/xnest/Display.c`,
  `hw/kdrive/ephyr/hostx.c`, `hw/xwayland/xwayland.c`, `Xext/*.c`, `dbe/dbe.c`,
  `render/*.c`, `xfixes/*.c`, `xkb/*.c`, `glx/*.c`, `Xi/*.c`.
- kdrive — `xserver/hw/kdrive/src/{kdrive.c,kinput.c,kshadow.c,kcmap.c}`.
- Xwayland — `xserver/hw/xwayland/`.
- Xvfb — `xserver/hw/vfb/InitOutput.c`.
- Xnest — `xserver/hw/xnest/`.
- Xephyr — `xserver/hw/kdrive/ephyr/hostx.c`.
- Cygwin/X — `xserver/hw/xwin/`.
- XQuartz — `xserver/hw/xquartz/`, <https://www.xquartz.org/>.
- `xf86-video-newport` — <https://gitlab.freedesktop.org/xorg/driver/xf86-video-newport>;
  `README`, `src/newport_driver.c`, `src/newport_accel.c`, `src/newport_regs.c`.
- nx-libs — <https://github.com/ArcticaProject/nx-libs>; `nx-X11/programs/Xserver/hw/nxagent/`.
- Xpra — <https://github.com/Xpra-org/xpra>.
- WeirdX — <https://www.jcraft.com/weirdx/>.
- Xming — <https://sourceforge.net/projects/xming/>.

**Rust crates and libraries**

- x11rb — <https://github.com/psychon/x11rb>, <https://crates.io/crates/x11rb>;
  `doc/comparison.md`, `x11rb-protocol/src/protocol/xproto.rs`,
  `x11rb-protocol/src/connect.rs` (byte-order selection).
- x11-rs — <https://github.com/AltF02/x11-rs>.
- rust-xcb — <https://github.com/rust-x-bindings/rust-xcb>.
- x11-clipboard — <https://github.com/quininer/x11-clipboard>.
- winit — <https://github.com/rust-windowing/winit>; `Cargo.toml` (`x11rb`,
  `x11-dl`).
- smithay — <https://github.com/Smithay/smithay>; `src/backend/x11/`,
  `Cargo.toml` (`backend_x11`, `xwayland` features).
- wayland-rs — <https://github.com/Smithay/wayland-rs>.
- wlroots / wlroots-rs — <https://gitlab.freedesktop.org/wlroots/wlroots>,
  <https://github.com/swaywm/wlroots-rs> (archived 2020).
- breadx — <https://github.com/bread-graphics/breadx>.
- xkbcommon — <https://github.com/rust-x-bindings/xkbcommon-rs>.
- calloop — <https://github.com/Smithay/calloop>.

**Specifications**

- Core protocol — <https://www.x.org/releases/X11R7.7/doc/xproto/x11protocol.html>.
- BIG-REQUESTS — <https://www.x.org/releases/X11R7.7/doc/bigreqsproto/bigreq.html>.
- SHAPE — <https://www.x.org/releases/X11R7.7/doc/xextproto/shape.html>.
- MIT-SHM — <https://www.x.org/releases/X11R7.7/doc/xextproto/shm.html>.
- XTEST — <https://www.x.org/releases/X11R7.7/doc/xextproto/xtest.html>.
- SYNC — <https://www.x.org/releases/X11R7.7/doc/xextproto/sync.html>.
- DBE — <https://www.x.org/releases/X11R7.7/doc/xextproto/dbe.html>.
- XKB — <https://www.x.org/releases/X11R7.7/doc/kbproto/xkbproto.html>.
- Xi1 — <https://www.x.org/releases/X11R7.7/doc/inputproto/XIproto.txt>.
- XFIXES — <https://www.x.org/releases/X11R7.7/doc/fixesproto/fixesproto.txt>.
- RENDER — <https://www.x.org/releases/X11R7.7/doc/renderproto/renderproto.txt>.
- RandR — <https://www.x.org/releases/X11R7.7/doc/randrproto/randrproto.txt>.
- Composite — <https://www.x.org/releases/X11R7.7/doc/compositeproto/compositeproto.txt>.
- DAMAGE — <https://www.x.org/releases/X11R7.7/doc/damageproto/damageproto.txt>.
- RECORD — <https://www.x.org/releases/X11R7.7/doc/recordproto/record.html>.
- Screen saver — <https://www.x.org/releases/X11R7.7/doc/scrnsaverproto/saver.html>.
- XC-MISC — <https://www.x.org/releases/X11R7.7/doc/xcmiscproto/xc-misc.html>.
- X-Resource — <https://www.x.org/releases/X11R7.7/doc/resourceproto/resproto.txt>.
- XVideo — <https://www.x.org/releases/X11R7.7/doc/videoproto/xv-protocol-v2.txt>.
- XDMCP — <https://www.x.org/releases/X11R7.7/doc/libXdmcp/xdmcp.html>.
- ICCCM — <https://www.x.org/releases/X11R7.7/doc/xorg-docs/icccm/icccm.html>.
- Xlib — <https://www.x.org/releases/X11R7.7/doc/libX11/libX11/libX11.html>.
- X Toolkit Intrinsics — <https://www.x.org/releases/X11R7.7/doc/libXt/intrinsics.html>.
- GLX 1.4 — <https://registry.khronos.org/OpenGL/specs/gl/glx1.4.pdf>.
- X server porting layer — <https://www.x.org/releases/X11R7.7/doc/xorg-server/Xserver-spec.html>.
- Fonts in X11R7.7 — <https://www.x.org/releases/X11R7.7/doc/xorg-docs/fonts/fonts.html>.

**IRIX and hardware**

- `Xsgi(1)` — <http://www.typewritten.org/Manual/SGI/IRIX/6.5.3f/man1/Xsgi.html>.
- IRIX 6.2 technical report (X11R6) — <http://www.sgistuff.net/software/irixintro/documents/irix6.2TR.html>.
- NetBSD/sgimips — <https://www.netbsd.org/ports/sgimips/>; `newport(4)` —
  <https://man.netbsd.org/sgimips/newport.4>.
- IRIX 7 project constraints — `/mnt/europa/os/docs/map.md`,
  `docs/spec/architecture.md`, `targets/mips3-sgi-irix7-n32.json`.

### 9.6 Measurement notes

- Xorg LOC: shallow clone of `xorg/xserver` at 2026-10-20, `find` over `.c`/`.h`,
  summed with `wc -l`. Figures include comments and blank lines and exclude
  generated `protocol` code (not present in-tree).
- Rust LOC: same method over `.rs`. Two figures are given where it matters:
  the total, and the count up to the first `#[cfg(test)]` in each file (an
  approximation of shipped code). "Non-test" directory filtering excludes
  `tests/` trees but the `#[cfg(test)]` cut is what produces the ~215k yserver
  figure.
- Stars/downloads: GitHub API and crates.io API on 2026-10-20.
- Licence identifications are from the repositories' `COPYING`/`LICENSE` files
  and crates.io metadata; the nx-libs GPL-2 finding is from
  `nx-libs/debian/copyright`.

---

*Prepared for the IRIX 7 project. This document is research input, not an ADR;
the decisions it recommends still need to be taken through the project's ADR
process.*
