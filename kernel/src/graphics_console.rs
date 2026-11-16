// SPDX-License-Identifier: GPL-3.0-only
//! Text console on the local Newport display.
//!
//! The serial console remains the primary sink, but a machine with no serial
//! terminal attached should still show what the kernel is doing.  This module
//! keeps a text screen buffer, mirrors every console byte into it, and paints
//! the whole display as a boot-log console.  Drawing is deferred and batched:
//! early boot output is buffered before REX3 is initialised.  The first flush
//! paints the whole display; later flushes repaint only the rows whose text
//! changed, so the screen does not flash on every line of output.
//!
//! A `factory-prom` build keeps the video mode the SGI PROM chose, which is
//! 1024x768 on some monitors, so its layout fits that size.

use crate::gfx;

/// True when the SGI PROM chose the video mode.
const PROM_DISPLAY: bool = cfg!(feature = "factory-prom");

const COLS: usize = if PROM_DISPLAY { 104 } else { 120 };
const ROWS: usize = if PROM_DISPLAY { 44 } else { 60 };
const LINE_HEIGHT: i32 = 16;
const TEXT_X: i32 = if PROM_DISPLAY { 16 } else { 92 };
const TEXT_Y: i32 = 36;

const BG: (u32, u32, u32) = (0x08, 0x0C, 0x14);
const LABEL: (u32, u32, u32) = (0x60, 0xC0, 0xFF);
const TEXT: (u32, u32, u32) = (0xD0, 0xD8, 0xE8);

static mut LINES: [[u8; COLS]; ROWS] = [[0; COLS]; ROWS];
/// The text each row showed after the last flush.
static mut PAINTED: [[u8; COLS]; ROWS] = [[b' '; COLS]; ROWS];
/// False until the first flush has painted the background and the title.
static mut BACKGROUND_PAINTED: bool = false;
static mut ROW: usize = 0;
static mut COL: usize = 0;
static mut ACTIVE: bool = false;
static mut DIRTY: bool = true;

/// Initialise the text buffer and enable painting after `gfx::init`.
///
/// Console bytes written before this call are already in `LINES`; any unused
/// cells are filled with spaces here so a redraw has a defined screen.
pub fn init() {
    // SAFETY: single-threaded kernel; no other task touches the buffer.
    unsafe {
        let lines = &mut *core::ptr::addr_of_mut!(LINES);
        for line in lines.iter_mut() {
            for cell in line.iter_mut() {
                if *cell == 0 {
                    *cell = b' ';
                }
            }
        }
        ACTIVE = true;
        DIRTY = true;
    }
}

/// Append one console byte to the text screen.
pub fn write_byte(byte: u8) {
    // SAFETY: single-threaded kernel.
    unsafe {
        let lines = &mut *core::ptr::addr_of_mut!(LINES);
        match byte {
            b'\r' => COL = 0,
            b'\n' => newline(lines),
            b'\t' => {
                let stop = (COL + 8) & !7;
                while COL < stop.min(COLS) {
                    lines[ROW][COL] = b' ';
                    COL += 1;
                }
                if COL >= COLS {
                    newline(lines);
                }
            }
            0x08 => {
                if COL > 0 {
                    COL -= 1;
                    lines[ROW][COL] = b' ';
                }
            }
            0x0c => {
                for line in lines.iter_mut() {
                    line.fill(b' ');
                }
                ROW = 0;
                COL = 0;
                DIRTY = true;
            }
            0x20..=0x7e => {
                lines[ROW][COL] = byte;
                COL += 1;
                if COL >= COLS {
                    newline(lines);
                }
            }
            _ => {
                lines[ROW][COL] = b'?';
                COL += 1;
                if COL >= COLS {
                    newline(lines);
                }
            }
        }
        DIRTY = true;
    }
}

fn newline(lines: &mut [[u8; COLS]; ROWS]) {
    // SAFETY: caller holds the only mutable access to `LINES`.
    unsafe {
        ROW += 1;
        COL = 0;
        if ROW >= ROWS {
            lines.copy_within(1.., 0);
            lines[ROWS - 1].fill(b' ');
            ROW = ROWS - 1;
        }
    }
}

/// Redraw the console if output has arrived since the last flush.
pub fn flush() {
    // SAFETY: single-threaded kernel; `gfx` makes no console calls.
    unsafe {
        if !ACTIVE || !DIRTY {
            return;
        }
        DIRTY = false;
        let lines = &*core::ptr::addr_of!(LINES);

        // Paint the whole display once, not just the text area: the
        // firmware's framebuffer contents are undefined, and there is no boot
        // menu covering them any more.
        if !BACKGROUND_PAINTED {
            BACKGROUND_PAINTED = true;
            gfx::fill_rect(
                0,
                0,
                gfx::SCREEN_WIDTH - 1,
                gfx::SCREEN_HEIGHT - 1,
                BG.0,
                BG.1,
                BG.2,
            );
            gfx::draw_str(TEXT_X, 12, "IRIX 7 console", LABEL.0, LABEL.1, LABEL.2);
            gfx::hline(
                TEXT_X,
                TEXT_X + COLS as i32 * 9 - 1,
                30,
                LABEL.0,
                LABEL.1,
                LABEL.2,
            );
        }

        let painted = &mut *core::ptr::addr_of_mut!(PAINTED);
        for (row, line) in lines.iter().enumerate() {
            if *line == painted[row] {
                continue;
            }
            painted[row] = *line;
            let top = TEXT_Y + row as i32 * LINE_HEIGHT;
            gfx::fill_rect(
                0,
                top,
                gfx::SCREEN_WIDTH - 1,
                top + LINE_HEIGHT - 1,
                BG.0,
                BG.1,
                BG.2,
            );
            let mut len = COLS;
            while len > 0 && line[len - 1] == b' ' {
                len -= 1;
            }
            if len == 0 {
                continue;
            }
            let text = core::str::from_utf8_unchecked(&line[..len]);
            gfx::draw_str(TEXT_X, top, text, TEXT.0, TEXT.1, TEXT.2);
        }

        // The first full-screen fill covers the self-test probe area, so the
        // probe is repainted last: a host framebuffer dump then always sees
        // the engine's fill output rather than the console background.
        gfx::paint_probe();
    }
}
