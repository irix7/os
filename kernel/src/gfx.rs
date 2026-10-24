// SPDX-License-Identifier: GPL-3.0-only
//! REX3 (Newport) graphics: display bring-up and drawing primitives.
//!
//! The REX3 register block lives at physical 0x1F0F_0000, reachable through
//! the kseg1 alias at 0xBF0F_0000.  Writing any register at `base + 0x800`
//! starts the drawing pipeline ("GO"); writes below that only stage values.
//!
//! Coordinates are biased by 4096 in X and Y, so screen pixel (0, 0) is
//! encoded as (4096 << 16) | 4096.
//!
//! The display is programmed in 24-bit RGB: REX3's `COLORRED`/`COLORGRN`/
//! `COLORBLUE` registers hold 12.11 fixed point (the integer part in bits
//! [22:11]), and a block or span fill writes the colour straight into the
//! framebuffer, which the video path reads as BGR.

use core::ptr::{read_volatile, write_volatile};

use crate::font::{FONT_BASELINE, FONT_DATA, FONT_INFO};
use crate::{console_println, print_hex32, print_str, println};

/// REX3 register block (kseg1 alias of physical 0x1F0F_0000).
const REX3: u32 = 0xBF0F_0000;
/// Offset that turns a register write into a pipeline start.
const GO: u32 = 0x0800;
/// Coordinate bias.
const COORD_BIAS: i32 = 4096;

/// Displayable width in pixels.
pub const SCREEN_WIDTH: i32 = 1280;
/// Displayable height in pixels.
pub const SCREEN_HEIGHT: i32 = 1024;

/// CPU-side shadow of the visible framebuffer: BGR, top-down, 3 bytes/pixel.
///
/// It lives in the reserved low window (kseg0 `0x8870_0000`, physical
/// `0x0870_0000`), between the 2 MiB module area and the frame allocator's
/// first page at `0x08B0_0000`.  The REX3 primitive wrappers keep it in step;
/// the MCP screen service reads it.
const SHADOW_BASE: usize = 0x8870_0000;
const SHADOW_STRIDE: usize = SCREEN_WIDTH as usize * 3;
const SHADOW_SIZE: usize = SHADOW_STRIDE * SCREEN_HEIGHT as usize;

fn shadow_put(x: i32, y: i32, r: u32, g: u32, b: u32) {
    if x < 0 || y < 0 || x >= SCREEN_WIDTH || y >= SCREEN_HEIGHT {
        return;
    }
    let off = SHADOW_BASE + y as usize * SHADOW_STRIDE + x as usize * 3;
    // SAFETY: the shadow is reserved for this purpose; the kernel is
    // single-threaded.  BMP order is B, G, R.
    unsafe {
        *(off as *mut u8) = b as u8;
        *((off + 1) as *mut u8) = g as u8;
        *((off + 2) as *mut u8) = r as u8;
    }
}

/// Copy `out.len()` bytes of the shadow starting at `offset`.
pub fn shadow_read(offset: usize, out: &mut [u8]) -> usize {
    if offset >= SHADOW_SIZE {
        return 0;
    }
    let n = out.len().min(SHADOW_SIZE - offset);
    // SAFETY: both regions are valid for n bytes.
    unsafe {
        core::ptr::copy_nonoverlapping((SHADOW_BASE + offset) as *const u8, out.as_mut_ptr(), n);
    }
    n
}

// Register offsets.
const DRAWMODE1: u32 = 0x0000;
const DRAWMODE0: u32 = 0x0004;
const LSMODE: u32 = 0x0008;
const LSPATTERN: u32 = 0x000C;
const LSPATSAVE: u32 = 0x0010;
const ZPATTERN: u32 = 0x0014;
const COLORBACK: u32 = 0x0018;
const COLORVRAM: u32 = 0x001C;
const SMASK0X: u32 = 0x0028;
const SMASK0Y: u32 = 0x002C;
const XSAVE: u32 = 0x0110;
const XYMOVE: u32 = 0x0114;
const BRESD: u32 = 0x0118;
const BRESS1: u32 = 0x011C;
const BRESOCTINC1: u32 = 0x0120;
const BRESRNDINC2: u32 = 0x0124;
const BRESE1: u32 = 0x0128;
const BRESS2: u32 = 0x012C;
const AWEIGHT0: u32 = 0x0130;
const AWEIGHT1: u32 = 0x0134;
const XYSTARTI: u32 = 0x0150;
const XYENDI: u32 = 0x0154;
const COLORRED: u32 = 0x0200;
const COLORALPHA: u32 = 0x0204;
const COLORGRN: u32 = 0x0208;
const COLORBLUE: u32 = 0x020C;
const WRMASK: u32 = 0x0220;
const COLORI: u32 = 0x0224;
const COLORX: u32 = 0x0228;
const SMASK1X: u32 = 0x1300;
const SMASK1Y: u32 = 0x1304;
const SMASK2X: u32 = 0x1308;
const SMASK2Y: u32 = 0x130C;
const SMASK3X: u32 = 0x1310;
const SMASK3Y: u32 = 0x1314;
const SMASK4X: u32 = 0x1318;
const SMASK4Y: u32 = 0x131C;
const TOPSCAN: u32 = 0x1320;
const XYWIN: u32 = 0x1324;
const CLIPMODE: u32 = 0x1328;
const STALL1: u32 = 0x132C;
const CONFIG: u32 = 0x1330;
const STATUS: u32 = 0x1338;

// DCB bridge: VC2, CMAP, XMAP9 and the BT445 RAMDAC hang off these registers.
const DCBMODE: u32 = 0x0238;
const DCBDATA0: u32 = 0x0240;

/// DCB target addresses.
const DCB_ADDR_VC2: u32 = 0;
const DCB_ADDR_XMAP: u32 = 4; // writes both XMAP9s on an XL24 board
const DCB_ADDR_RAMDAC: u32 = 7;
/// DCBMODE data widths.
const DCB_WIDTH_8: u32 = 1;
const DCB_WIDTH_16: u32 = 2;
const DCB_WIDTH_32: u32 = 0;

/// VC2 registers used by the mode-set routine.
const VC2_VIDEO_ENTRY_PTR: u32 = 0x00;
const VC2_CURSOR_ENTRY_PTR: u32 = 0x01;
const VC2_CURSOR_X: u32 = 0x02;
const VC2_CURSOR_Y: u32 = 0x03;
const VC2_RAM_ADDR: u32 = 0x07;
const VC2_VT_FRAME_PTR: u32 = 0x08;
const VC2_SCANLINE_LEN: u32 = 0x06;
const VC2_DISPLAY_CONTROL: u32 = 0x10;
const VC2_CONFIG: u32 = 0x1f;
const VC2_CTRL_BLACKOUT: u32 = 1 << 1;
const VC2_CTRL_VIDEO_TIMING_EN: u32 = 1 << 2;
const VC2_CTRL_CURSOR_FUNC_EN: u32 = 1 << 4;
const VC2_CTRL_CURSOR_EN: u32 = 1 << 7;
const VT_HPOS_VC_N: u32 = 1 << 1;
const VT_CBLANK_XMAP_N: u32 = 1 << 6;

/// XMAP9 register selects.
const XMAP_MODE_TABLE: u32 = 5;
/// RGB24, 24 bits per pixel, buffer 0, no overlay.
const XMAP_MODE_RGB24: u32 = 0x0000_0F00;

/// STATUS bit 3: the drawing engine is busy.
const STATUS_GFXBUSY: u32 = 1 << 3;
/// STATUS bits 9:7: command FIFO level.
const STATUS_GFIFOLEVEL_SHIFT: u32 = 7;
const STATUS_GFIFOLEVEL_MASK: u32 = 0x3F;

/// Wait-path counters used by the diagnostics self-test.
static mut WAIT_CALLS: u32 = 0;
static mut WAIT_LOOPS: u32 = 0;

/// DRAWMODE1: RGB planes, 24-bit draw depth, compare disabled, logicop SRC.
///
/// The bit layout matters: planes is bits [2:0], drawdepth [4:3], compare
/// [14:12], rgbmode is bit 15, and logicop is bits [31:28].  Without the
/// rgbmode bit the engine stays in colour-index mode and only the red
/// channel of the fill colour reaches the framebuffer.
const DM1_RGB24: u32 = 0x3000_F319;
/// DRAWMODE0: DRAW opcode, block address mode, stop on X and Y.
const DM0_BLOCK: u32 = 0x0306;
/// DRAWMODE0: DRAW opcode, span address mode, stop on X.
const DM0_SPAN: u32 = 0x0102;

/// Stage a register value (no pipeline start).
#[inline]
fn reg(off: u32, val: u32) {
    unsafe { write_volatile((REX3 + off) as *mut u32, val) }
}

/// Stage a register value and start the pipeline.
#[inline]
fn reg_go(off: u32, val: u32) {
    unsafe { write_volatile((REX3 + GO + off) as *mut u32, val) }
}

/// Wait for the command FIFO to drain.
///
/// The FIFO holds only a few dozen entries and a full queue drops the write,
/// so a driver has to throttle.  Real SGI code does the same thing with
/// `REX3WAIT(REX)`, which polls STATUS for the busy bit.
fn wait_idle() {
    unsafe {
        WAIT_CALLS = WAIT_CALLS.wrapping_add(1);
    }
    loop {
        let status = unsafe { read_volatile((REX3 + STATUS) as *const u32) };
        let busy = status & STATUS_GFXBUSY != 0;
        let level = (status >> STATUS_GFIFOLEVEL_SHIFT) & STATUS_GFIFOLEVEL_MASK;
        if !busy && level == 0 {
            return;
        }
        unsafe {
            WAIT_LOOPS = WAIT_LOOPS.wrapping_add(1);
        }
        core::hint::spin_loop();
    }
}

fn read_reg(offset: u32) -> u32 {
    unsafe { read_volatile((REX3 + offset) as *const u32) }
}

/// Encode a screen coordinate for XYSTARTI/XYENDI.
#[inline]
fn xy(x: i32, y: i32) -> u32 {
    let xi = (x + COORD_BIAS) as u16 as u32;
    let yi = (y + COORD_BIAS) as u16 as u32;
    (xi << 16) | yi
}

fn dcb_mode(addr: u32, crs: u32, width: u32) -> u32 {
    (addr << 7) | (crs << 4) | width
}

/// Write one 16-bit VC2 register through the REX3 DCB bridge.
///
/// VC2 mode setting is a table download, not a single register write.  The
/// kernel provides the 1280x1024 table itself so the display comes up even
/// when the emulator/host has not preselected a Newport mode.
fn vc2_reg(index: u32, value: u32) {
    reg(DCBMODE, dcb_mode(DCB_ADDR_VC2, 0, DCB_WIDTH_16));
    reg(DCBDATA0, index & 0x1f);
    reg(DCBMODE, dcb_mode(DCB_ADDR_VC2, 1, DCB_WIDTH_16));
    reg(DCBDATA0, value & 0xffff);
}

fn vc2_set_ram_addr(address: u32) {
    vc2_reg(VC2_RAM_ADDR, address);
}

fn vc2_ram_word(value: u16) {
    reg(DCBMODE, dcb_mode(DCB_ADDR_VC2, 3, DCB_WIDTH_16));
    reg(DCBDATA0, value as u32);
}

/// Emit a run of VC2 video-timing pixels.
///
/// Each line-sequence entry covers at most 254 pixels: duration is stored in
/// half-pixel units and one entry carries the three signal-state bytes.  The
/// active-video run asserts `VT_CBLANK_XMAP_N`; the final run carries EOL.
fn vc2_push_pixels(mut pixels: u32, state_a: u32, state_c: u32, eol: bool) {
    while pixels > 0 {
        let chunk = pixels.min(254);
        let duration = (chunk / 2).max(1) as u32;
        let last = eol && pixels <= chunk;
        let word0 = ((last as u32) << 15) | (duration << 8) | state_a;
        let word1 = state_c;
        vc2_ram_word(word0 as u16);
        vc2_ram_word(word1 as u16);
        pixels -= chunk;
    }
}

/// Download the 1280x1024 @ ~72 Hz VC2 timing table and enable the display.
fn program_vc2_1280x1024() {
    const FRAME_PTR: u32 = 0x100;
    const LINE_SEQ_PTR: u32 = 0x200;
    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 1024;

    vc2_set_ram_addr(FRAME_PTR);
    vc2_ram_word(LINE_SEQ_PTR as u16);
    vc2_ram_word(HEIGHT as u16);

    vc2_set_ram_addr(LINE_SEQ_PTR);
    // Horizontal blank, HPOS pulse, gap to active video, then active pixels.
    vc2_push_pixels(408, 0, 0, false);
    vc2_push_pixels(2, VT_HPOS_VC_N, 0, false);
    vc2_push_pixels(36, 0, 0, false);
    vc2_push_pixels(WIDTH, 0, VT_CBLANK_XMAP_N, true);
    // The VC2 line table chains each line sequence to the next.  A self-loop
    // lets the frame entry repeat this one active line for every raster line.
    vc2_ram_word(LINE_SEQ_PTR as u16);

    vc2_reg(VC2_VIDEO_ENTRY_PTR, FRAME_PTR);
    vc2_reg(VC2_VT_FRAME_PTR, FRAME_PTR);
    vc2_reg(VC2_SCANLINE_LEN, WIDTH << 5);
    vc2_reg(VC2_CURSOR_ENTRY_PTR, 0);
    vc2_reg(VC2_CURSOR_X, 0);
    vc2_reg(VC2_CURSOR_Y, 0);
    vc2_reg(VC2_CONFIG, 0);
    vc2_reg(
        VC2_DISPLAY_CONTROL,
        VC2_CTRL_BLACKOUT | VC2_CTRL_VIDEO_TIMING_EN | VC2_CTRL_CURSOR_FUNC_EN | VC2_CTRL_CURSOR_EN,
    );
    wait_idle();
}

/// Clear the auxiliary (overlay/popup/CID) planes.
///
/// The REX3 RGB planes are initialised by the menu fill, but the auxiliary
/// framebuffer powers up with undefined contents.  Its popup bits are checked
/// by the display path before the main pixel, so leaving them random produces
/// coloured snow over an otherwise correct RGB image.
fn clear_aux_planes() {
    for planes in [4u32, 5, 6] {
        wait_idle();
        reg(DRAWMODE1, (DM1_RGB24 & !0x7) | planes);
        reg(WRMASK, 0x00FF_FFFF);
        reg(COLORRED, 0);
        reg(COLORGRN, 0);
        reg(COLORBLUE, 0);
        reg(COLORALPHA, 0);
        reg(COLORVRAM, 0);
        reg(XYENDI, xy(SCREEN_WIDTH - 1, SCREEN_HEIGHT - 1));
        reg(XYSTARTI, xy(0, 0));
        reg_go(DRAWMODE0, DM0_BLOCK);
    }
    wait_idle();
    reg(DRAWMODE1, DM1_RGB24);
}

/// Program both XMAP9s for 24-bit RGB on every display ID.
///
/// The display path starts with XMAP mode 0, which is colour-indexed and reads
/// the (uninitialised) CMAP.  REX3 writes 24-bit RGB, so without this the
/// framebuffer is drawn but the monitor sees black.
fn program_xmap_rgb24() {
    reg(
        DCBMODE,
        dcb_mode(DCB_ADDR_XMAP, XMAP_MODE_TABLE, DCB_WIDTH_32),
    );
    for id in 0..32u32 {
        reg(DCBDATA0, (id << 24) | XMAP_MODE_RGB24);
    }
    wait_idle();
}

/// Load an identity gamma table into the Bt445 RAMDAC.
///
/// The compositor applies the RAMDAC LUT to every pixel.  Its power-on state
/// is all zeroes, so an otherwise correct RGB image comes out black until the
/// table is loaded.
fn program_identity_gamma() {
    // CRS 0 is the palette address register; start at index 0.
    reg(DCBMODE, dcb_mode(DCB_ADDR_RAMDAC, 0, DCB_WIDTH_8));
    reg(DCBDATA0, 0);
    // CRS 1 is the 256 x RGB palette.  The chip auto-increments its address
    // after the blue component, so one pass of 768 bytes fills it.
    reg(DCBMODE, dcb_mode(DCB_ADDR_RAMDAC, 1, DCB_WIDTH_8));
    for value in 0..=255u32 {
        let byte = value << 24;
        reg(DCBDATA0, byte); // red
        reg(DCBDATA0, byte); // green
        reg(DCBDATA0, byte); // blue
    }
    wait_idle();
}

/// Bring the drawing engine up: reset the pipeline state and leave it
/// configured for 24-bit RGB block and span fills.
pub fn init() {
    // SAFETY: the shadow is reserved for the renderer.
    unsafe {
        core::ptr::write_bytes(SHADOW_BASE as *mut u8, 0, SHADOW_SIZE);
    }
    reg(LSMODE, 0);
    reg(LSPATTERN, 0);
    reg(LSPATSAVE, 0);
    reg(ZPATTERN, 0);
    reg(COLORBACK, 0xDEAD_BEEF);
    reg(COLORVRAM, 0x00FF_FFFF);
    reg(SMASK0X, 0);
    reg(SMASK0Y, 0);
    reg(XSAVE, 0);
    reg(XYMOVE, 0);
    reg(BRESD, 0);
    reg(BRESS1, 0);
    reg(BRESOCTINC1, 0);
    reg(BRESRNDINC2, 0);
    reg(BRESE1, 0);
    reg(BRESS2, 0);
    reg(AWEIGHT0, 0);
    reg(AWEIGHT1, 0);
    reg(COLORRED, 0);
    reg(COLORALPHA, 0);
    reg(COLORGRN, 0);
    reg(COLORBLUE, 0);
    reg(WRMASK, 0x00FF_FFFF);
    reg(COLORI, 0);
    reg(COLORX, 0);
    reg(SMASK1X, 0);
    reg(SMASK1Y, 0);
    reg(SMASK2X, 0);
    reg(SMASK2Y, 0);
    reg(SMASK3X, 0);
    reg(SMASK3Y, 0);
    reg(SMASK4X, 0);
    reg(SMASK4Y, 0);
    reg(XYWIN, 0);
    reg(TOPSCAN, 0x3FF);
    // Permit every two-bit CID so nothing is clipped away.
    reg(CLIPMODE, 0xF << 9);
    reg(STALL1, 0);
    reg(CONFIG, 0);
    clear_aux_planes();
    program_vc2_1280x1024();
    program_xmap_rgb24();
    program_identity_gamma();
}

/// Fill the rectangle `(x0, y0)..=(x1, y1)` with an RGB colour.
pub fn fill_rect(x0: i32, y0: i32, x1: i32, y1: i32, r: u32, g: u32, b: u32) {
    for y in y0.max(0)..=y1.min(SCREEN_HEIGHT - 1) {
        for x in x0.max(0)..=x1.min(SCREEN_WIDTH - 1) {
            shadow_put(x, y, r, g, b);
        }
    }
    wait_idle();
    reg(DRAWMODE1, DM1_RGB24);
    reg(WRMASK, 0x00FF_FFFF);
    reg(COLORRED, r << 11);
    reg(COLORGRN, g << 11);
    reg(COLORBLUE, b << 11);
    reg(XYENDI, xy(x1, y1));
    reg(XYSTARTI, xy(x0, y0));
    reg_go(DRAWMODE0, DM0_BLOCK);
}

/// Fill the horizontal run `x0..=x1` on row `y` with an RGB colour.
pub fn fill_span(x0: i32, x1: i32, y: i32, r: u32, g: u32, b: u32) {
    if x1 < x0 {
        return;
    }
    for x in x0.max(0)..=x1.min(SCREEN_WIDTH - 1) {
        shadow_put(x, y, r, g, b);
    }
    wait_idle();
    reg(DRAWMODE1, DM1_RGB24);
    reg(WRMASK, 0x00FF_FFFF);
    reg(COLORRED, r << 11);
    reg(COLORGRN, g << 11);
    reg(COLORBLUE, b << 11);
    reg(XYENDI, xy(x1, y));
    reg(XYSTARTI, xy(x0, y));
    reg_go(DRAWMODE0, DM0_SPAN);
}

/// Verify the REX3 mode and FIFO-drain contract from the driver side.
///
/// This is deliberately not an emulator conformance test.  It checks that the
/// fill path actually programs RGB24 mode (including `DRAWMODE1` bit 15), that
/// the register readback agrees, and that the shallow-FIFO wait was called and
/// ended with `GFXBUSY=0` and FIFO level 0.  A driver regression that clears
/// rgbmode or bypasses `REX3WAIT` fails here even if the emulator is more
/// permissive than the real REX3.
pub fn self_test() -> bool {
    let calls_before = unsafe { WAIT_CALLS };
    // One small span at the bottom-right avoids touching the menu body.
    fill_span(
        SCREEN_WIDTH - 2,
        SCREEN_WIDTH - 1,
        SCREEN_HEIGHT - 1,
        1,
        2,
        3,
    );
    wait_idle();

    let mode = read_reg(DRAWMODE1);
    let status = read_reg(STATUS);
    let calls = unsafe { WAIT_CALLS }.wrapping_sub(calls_before);
    let mode_ok = mode & (1 << 15) != 0 && mode & 0x7 == 1 && (mode >> 3) & 0x3 == 3;
    let fifo_ok = calls >= 2
        && status & STATUS_GFXBUSY == 0
        && (status >> STATUS_GFIFOLEVEL_SHIFT) & STATUS_GFIFOLEVEL_MASK == 0;
    let pixel_offset =
        ((SCREEN_HEIGHT - 1) as usize * SCREEN_WIDTH as usize + (SCREEN_WIDTH - 1) as usize) * 3;
    let mut pixel = [0u8; 3];
    shadow_read(pixel_offset, &mut pixel);
    let shadow_ok = pixel == [3, 2, 1]; // shadow is BGR; fill was r=1,g=2,b=3

    console_println("gfx: self-test");
    print_str("  drawmode1=0x");
    print_hex32(mode);
    print_str(" rgbmode=");
    print_str(if mode & (1 << 15) != 0 {
        "set"
    } else {
        "CLEAR"
    });
    print_str(" status=0x");
    print_hex32(status);
    print_str(" waits=");
    print_dec(calls);
    print_str(" loops=");
    print_dec(unsafe { WAIT_LOOPS });
    println("");
    let ok = mode_ok && fifo_ok && shadow_ok;
    print_str(" shadow=");
    print_str(if shadow_ok { "bgr" } else { "BAD" });
    println("");
    print_str("gfx: self-test ");
    println(if ok { "ok" } else { "FAILED" });
    ok
}

/// Draw one character with the PROM's raster font.
///
/// `x` and `y` are the top-left corner of the character cell.  Returns the
/// pen advance in pixels.
pub fn draw_char(x: i32, y: i32, ch: u8, r: u32, g: u32, b: u32) -> i32 {
    let idx = ch as usize;
    if !(0x20..=0x7e).contains(&idx) {
        return 0;
    }
    let glyph = idx - 0x20;
    let (off, xsize, ysize, xorig, yorig, xmove) = FONT_INFO[glyph];
    let base = y + FONT_BASELINE as i32 - ysize as i32 - yorig as i32;
    // The IRIX font data stores glyph rows bottom-up.  Reverse them here so
    // the raster is drawn top-down like the rest of the display code.
    for row in 0..ysize as usize {
        let bits = FONT_DATA[off as usize + (ysize as usize - 1 - row)];
        if bits == 0 {
            continue;
        }
        let mut col = 0usize;
        while col < xsize as usize {
            if bits & (1 << (15 - col)) != 0 {
                let start = col;
                while col < xsize as usize && bits & (1 << (15 - col)) != 0 {
                    col += 1;
                }
                let px = x + xorig as i32;
                fill_span(
                    px + start as i32,
                    px + (col - 1) as i32,
                    base + row as i32,
                    r,
                    g,
                    b,
                );
            } else {
                col += 1;
            }
        }
    }
    xmove as i32
}

/// Draw a NUL-terminated string; returns the pen's final x position.
pub fn draw_str(x: i32, y: i32, s: &str, r: u32, g: u32, b: u32) -> i32 {
    let mut pen = x;
    for byte in s.bytes() {
        pen += draw_char(pen, y, byte, r, g, b);
    }
    pen
}

/// Draw a horizontal rule.
pub fn hline(x0: i32, x1: i32, y: i32, r: u32, g: u32, b: u32) {
    fill_span(x0, x1, y, r, g, b);
}

fn print_dec(mut value: u32) {
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    if value == 0 {
        crate::write_byte(b'0');
        return;
    }
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    while count > 0 {
        count -= 1;
        crate::write_byte(digits[count]);
    }
}
