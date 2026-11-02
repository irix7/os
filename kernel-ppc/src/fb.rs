// SPDX-License-Identifier: GPL-3.0-only
//! The Open Firmware framebuffer as a second console output.
//!
//! Open Firmware leaves the display in a linear framebuffer mode and publishes
//! the mode and the aperture in the device tree.  The kernel does not set a
//! mode; it maps the aperture cache-inhibited through the fourth data BAT and
//! draws an 8x16 text console into it.  The ESCC remains the primary console,
//! so the framebuffer is a mirror and a failure here never stops the boot.
//!
//! The text console is write-only apart from the start-up self-test, which
//! reads the pattern back through the same mapping.  There is no cursor,
//! scrolling by hardware, acceleration or input.
//!
//! Pixel byte order follows the little-endian framebuffer that QEMU's VGA and
//! the Rage LT both present on a big-endian PCI bus: 32 bpp is stored as
//! `B G R X`, 16 bpp as little-endian 5:6:5, and 8 bpp is a VGA palette index.
//! White is `0x00ff_ffff`, `0xffff` and palette entry 15 respectively.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use crate::font::{self, FONT};
use crate::platform::Display;

/// The 8 bpp palette entry the console uses for a lit pixel.
const PALETTE_WHITE: u8 = 0x0f;

/// The 32 bpp white word as it must appear in big-endian memory.
const WHITE32: u32 = 0x00ff_ffffu32.swap_bytes();
/// The 16 bpp white halfword; byte order does not matter for an all-ones value.
const WHITE16: u16 = 0xffff;

/// Bytes of pre-display log kept for replay.
const PENDING_CAPACITY: usize = 4096;
/// The pre-display log.  The framebuffer is initialised after the BAT switch,
/// so every byte printed before then is held here and replayed once the
/// framebuffer is live.
static mut PENDING: [u8; PENDING_CAPACITY] = [0; PENDING_CAPACITY];

static PENDING_LEN: AtomicU32 = AtomicU32::new(0);

static BASE: AtomicU32 = AtomicU32::new(0);
static WIDTH: AtomicU32 = AtomicU32::new(0);
static HEIGHT: AtomicU32 = AtomicU32::new(0);
static LINEBYTES: AtomicU32 = AtomicU32::new(0);
static DEPTH: AtomicU32 = AtomicU32::new(0);
static COLUMNS: AtomicU32 = AtomicU32::new(0);
static ROWS: AtomicU32 = AtomicU32::new(0);
static COLUMN: AtomicU32 = AtomicU32::new(0);
static ROW: AtomicU32 = AtomicU32::new(0);
static READY: AtomicBool = AtomicBool::new(false);
static RENDERED: AtomicU32 = AtomicU32::new(0);

/// Whether the framebuffer mode can be driven by this console.
///
/// The accepted depths are the ones whose pixel format is fixed by the
/// framebuffer rather than by a driver: 8 bpp indexed, 16 bpp 5:6:5 and
/// 32 bpp `XRGB`.  A 15 bpp, 20 bpp or 24 bpp mode is refused rather than
/// guessed at.
pub fn supported(display: &Display) -> bool {
    if !matches!(display.depth, 8 | 16 | 32) {
        return false;
    }
    if display.width < font::WIDTH || display.height < font::HEIGHT {
        return false;
    }
    let bytes_per_pixel = display.depth / 8;
    let visible = match display.width.checked_mul(bytes_per_pixel) {
        Some(visible) => visible,
        None => return false,
    };
    if display.linebytes < visible || !display.linebytes.is_multiple_of(bytes_per_pixel) {
        return false;
    }
    if !display.base.is_multiple_of(bytes_per_pixel) {
        return false;
    }
    display.linebytes.checked_mul(display.height).is_some()
}

/// The number of bytes the console has drawn since it was initialised.
pub fn rendered() -> u32 {
    RENDERED.load(Relaxed)
}

/// Initialise the console and replay the log that was printed before the
/// framebuffer was mapped.
///
/// Returns `false` when the geometry was already refused by [`supported`]; the
/// caller reports the failure and continues on the serial console.
pub fn init(display: &Display) -> bool {
    if !supported(display) {
        return false;
    }
    BASE.store(display.base, Relaxed);
    WIDTH.store(display.width, Relaxed);
    HEIGHT.store(display.height, Relaxed);
    LINEBYTES.store(display.linebytes, Relaxed);
    DEPTH.store(display.depth, Relaxed);
    COLUMNS.store(display.width / font::WIDTH, Relaxed);
    ROWS.store(display.height / font::HEIGHT, Relaxed);
    COLUMN.store(0, Relaxed);
    ROW.store(0, Relaxed);
    clear();
    READY.store(true, Relaxed);

    let length = (PENDING_LEN.load(Relaxed) as usize).min(PENDING_CAPACITY);
    let mut index = 0;
    while index < length {
        // SAFETY: the index is bounded by the buffer length and this runs on
        // the single kernel context before interrupts are enabled.
        let byte = unsafe { PENDING[index] };
        text_write(byte);
        index += 1;
    }
    PENDING_LEN.store(0, Relaxed);
    true
}

/// Add one byte to the framebuffer console.  Before [`init`] the byte is kept
/// for replay; after it, the byte is drawn.
pub fn write_byte(byte: u8) {
    if READY.load(Relaxed) {
        text_write(byte);
        return;
    }
    let index = PENDING_LEN.load(Relaxed) as usize;
    if index < PENDING_CAPACITY {
        // SAFETY: the index is bounded by the buffer length and this runs on
        // the single kernel context before interrupts are enabled.
        unsafe {
            PENDING[index] = byte;
        }
        PENDING_LEN.store((index + 1) as u32, Relaxed);
    }
}

fn text_write(byte: u8) {
    match byte {
        b'\r' => COLUMN.store(0, Relaxed),
        b'\n' => {
            COLUMN.store(0, Relaxed);
            newline();
        }
        b'\t' => {
            let column = COLUMN.load(Relaxed);
            let next = (column + 8) & !7;
            let columns = COLUMNS.load(Relaxed);
            let row = ROW.load(Relaxed);
            let mut column = column;
            while column < next && column < columns {
                draw_cell(column, row, b' ');
                column += 1;
            }
            if column >= columns {
                COLUMN.store(0, Relaxed);
                newline();
            } else {
                COLUMN.store(column, Relaxed);
            }
        }
        font::FIRST..=font::LAST => {
            let column = COLUMN.load(Relaxed);
            let row = ROW.load(Relaxed);
            draw_cell(column, row, byte);
            advance();
        }
        _ => {}
    }
    RENDERED.fetch_add(1, Relaxed);
}

fn advance() {
    let column = COLUMN.load(Relaxed) + 1;
    if column >= COLUMNS.load(Relaxed) {
        COLUMN.store(0, Relaxed);
        newline();
    } else {
        COLUMN.store(column, Relaxed);
    }
}

fn newline() {
    let rows = ROWS.load(Relaxed);
    let row = ROW.load(Relaxed) + 1;
    if row >= rows {
        scroll();
        ROW.store(rows.saturating_sub(1), Relaxed);
    } else {
        ROW.store(row, Relaxed);
    }
}

fn draw_cell(column: u32, row: u32, byte: u8) {
    if column >= COLUMNS.load(Relaxed) || row >= ROWS.load(Relaxed) {
        return;
    }
    let index = if (font::FIRST..=font::LAST).contains(&byte) {
        usize::from(byte - font::FIRST)
    } else {
        usize::from(b'?' - font::FIRST)
    };
    let glyph = &FONT[index];
    let left = column * font::WIDTH;
    let top = row * font::HEIGHT;
    for (offset, bits) in glyph.iter().enumerate() {
        let y = top + offset as u32;
        for x in 0..font::WIDTH {
            let lit = bits & (0x80 >> x) != 0;
            put_pixel(left + x, y, lit);
        }
    }
}

/// Draw one pixel.  `x` and `y` are inside the visible area.
fn put_pixel(x: u32, y: u32, lit: bool) {
    let base = BASE.load(Relaxed);
    let depth = DEPTH.load(Relaxed);
    let offset = base + y * LINEBYTES.load(Relaxed) + x * (depth / 8);
    // SAFETY: the display geometry was checked by `supported`, the aperture is
    // mapped cache-inhibited, and every coordinate is inside the visible area.
    unsafe {
        match depth {
            32 => core::ptr::write_volatile(offset as *mut u32, if lit { WHITE32 } else { 0 }),
            16 => core::ptr::write_volatile(offset as *mut u16, if lit { WHITE16 } else { 0 }),
            _ => core::ptr::write_volatile(offset as *mut u8, if lit { PALETTE_WHITE } else { 0 }),
        }
    }
}

fn read_pixel(x: u32, y: u32) -> bool {
    let base = BASE.load(Relaxed);
    let depth = DEPTH.load(Relaxed);
    let offset = base + y * LINEBYTES.load(Relaxed) + x * (depth / 8);
    // SAFETY: the display geometry was checked by `supported`, the aperture is
    // mapped cache-inhibited, and every coordinate is inside the visible area.
    unsafe {
        match depth {
            32 => core::ptr::read_volatile(offset as *const u32) == WHITE32,
            16 => core::ptr::read_volatile(offset as *const u16) == WHITE16,
            _ => core::ptr::read_volatile(offset as *const u8) == PALETTE_WHITE,
        }
    }
}

fn clear() {
    clear_rows(0, HEIGHT.load(Relaxed));
}

fn clear_rows(first: u32, count: u32) {
    let base = BASE.load(Relaxed);
    let linebytes = LINEBYTES.load(Relaxed);
    let visible = WIDTH.load(Relaxed) * (DEPTH.load(Relaxed) / 8);
    for row in first..first.saturating_add(count) {
        let offset = base + row * linebytes;
        // SAFETY: the rows and the visible width lie inside the aperture that
        // `supported` checked, and the aperture is mapped cache-inhibited.
        unsafe {
            core::ptr::write_bytes(offset as *mut u8, 0, visible as usize);
        }
    }
}

fn scroll() {
    let rows = ROWS.load(Relaxed);
    if rows <= 1 {
        clear_rows(0, rows * font::HEIGHT);
        return;
    }
    let base = BASE.load(Relaxed);
    let linebytes = LINEBYTES.load(Relaxed);
    let visible = WIDTH.load(Relaxed) * (DEPTH.load(Relaxed) / 8);
    // Move every visible row one text line up.  The source starts a whole
    // text line below the destination, so the ranges never overlap.
    for row in 0..(rows - 1) * font::HEIGHT {
        let source = base + (row + font::HEIGHT) * linebytes;
        let destination = base + row * linebytes;
        // SAFETY: both rows are inside the aperture and `visible` is the
        // part of a row the console owns.
        unsafe {
            core::ptr::copy_nonoverlapping(
                source as *const u8,
                destination as *mut u8,
                visible as usize,
            );
        }
    }
    clear_rows((rows - 1) * font::HEIGHT, font::HEIGHT);
}

/// Draw a solid block and a glyph, then read both back through the mapping.
///
/// The block proves the pixel format and the aperture; the glyph is read back
/// against the font table, so a wrong stride or a misread colour would fail.
/// The test is run before the boot log can scroll the bottom row away.
pub fn selftest() -> bool {
    if !READY.load(Relaxed) {
        return false;
    }
    let columns = COLUMNS.load(Relaxed);
    let rows = ROWS.load(Relaxed);
    if columns < 2 || rows == 0 {
        return false;
    }
    let block_column = columns - 2;
    let glyph_column = columns - 1;
    let row = rows - 1;

    for y in 0..font::HEIGHT {
        for x in 0..font::WIDTH {
            put_pixel(block_column * font::WIDTH + x, row * font::HEIGHT + y, true);
        }
    }
    draw_cell(glyph_column, row, b'G');

    let mut ok = true;
    for y in 0..font::HEIGHT {
        for x in 0..font::WIDTH {
            if !read_pixel(block_column * font::WIDTH + x, row * font::HEIGHT + y) {
                ok = false;
            }
        }
    }
    let glyph = &FONT[usize::from(b'G' - font::FIRST)];
    for (offset, bits) in glyph.iter().enumerate() {
        for x in 0..font::WIDTH {
            let expected = bits & (0x80 >> x) != 0;
            let actual = read_pixel(
                glyph_column * font::WIDTH + x,
                row * font::HEIGHT + offset as u32,
            );
            if actual != expected {
                ok = false;
            }
        }
    }
    ok
}
