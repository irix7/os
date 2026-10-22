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

/// STATUS bit 3: the drawing engine is busy.
const STATUS_GFXBUSY: u32 = 1 << 3;
/// STATUS bits 9:7: command FIFO level.
const STATUS_GFIFOLEVEL_SHIFT: u32 = 7;
const STATUS_GFIFOLEVEL_MASK: u32 = 0x3F;

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
    loop {
        let status = unsafe { read_volatile((REX3 + STATUS) as *const u32) };
        let busy = status & STATUS_GFXBUSY != 0;
        let level = (status >> STATUS_GFIFOLEVEL_SHIFT) & STATUS_GFIFOLEVEL_MASK;
        if !busy && level == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Encode a screen coordinate for XYSTARTI/XYENDI.
#[inline]
fn xy(x: i32, y: i32) -> u32 {
    let xi = (x + COORD_BIAS) as u16 as u32;
    let yi = (y + COORD_BIAS) as u16 as u32;
    (xi << 16) | yi
}

/// Bring the drawing engine up: reset the pipeline state and leave it
/// configured for 24-bit RGB block and span fills.
pub fn init() {
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
}

/// Fill the rectangle `(x0, y0)..=(x1, y1)` with an RGB colour.
pub fn fill_rect(x0: i32, y0: i32, x1: i32, y1: i32, r: u32, g: u32, b: u32) {
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

/// Draw one character with the PROM's raster font.
///
/// `x` and `y` are the top-left corner of the character cell.  Returns the
/// pen advance in pixels.
pub fn draw_char(x: i32, y: i32, ch: u8, r: u32, g: u32, b: u32) -> i32 {
    let idx = ch as usize;
    if idx < 0x20 || idx - 0x20 >= FONT_INFO.len() {
        return 0;
    }
    let (off, xsize, ysize, xorig, yorig, xmove) = FONT_INFO[idx - 0x20];
    let base = y + FONT_BASELINE as i32 - ysize as i32 - yorig as i32;
    for row in 0..ysize as usize {
        let bits = FONT_DATA[off as usize + row];
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
                fill_span(px + start as i32, px + (col - 1) as i32, base + row as i32, r, g, b);
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
