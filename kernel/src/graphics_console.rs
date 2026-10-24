// SPDX-License-Identifier: GPL-3.0-only
//! Text console on the local Newport display.
//!
//! The serial console remains the primary sink, but a machine with no serial
//! terminal attached should still show what the kernel is doing.  This module
//! keeps a small text screen buffer, mirrors every console byte into it, and
//! paints the visible window below the boot menu.  Drawing is deferred and
//! batched: early boot output is buffered before REX3 is initialised, and the
//! idle loop flushes at most one redraw per batch of output.

use crate::gfx;

const COLS: usize = 120;
const ROWS: usize = 18;
const LINE_HEIGHT: i32 = 16;
const PANE_X: i32 = 32;
const PANE_Y: i32 = 700;
const PANE_W: i32 = COLS as i32 * 9 + 16;
const PANE_H: i32 = ROWS as i32 * LINE_HEIGHT + 28;

const BG: (u32, u32, u32) = (0x08, 0x0C, 0x14);
const LABEL: (u32, u32, u32) = (0x60, 0xC0, 0xFF);
const TEXT: (u32, u32, u32) = (0xD0, 0xD8, 0xE8);

static mut LINES: [[u8; COLS]; ROWS] = [[0; COLS]; ROWS];
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

/// Redraw the console pane if output has arrived since the last flush.
pub fn flush() {
    // SAFETY: single-threaded kernel; `gfx` makes no console calls.
    unsafe {
        if !ACTIVE || !DIRTY {
            return;
        }
        DIRTY = false;
        let lines = &*core::ptr::addr_of!(LINES);

        gfx::fill_rect(
            PANE_X - 8,
            PANE_Y - 28,
            PANE_X + PANE_W,
            PANE_Y + PANE_H,
            BG.0,
            BG.1,
            BG.2,
        );
        gfx::draw_str(
            PANE_X,
            PANE_Y - 22,
            "kernel console",
            LABEL.0,
            LABEL.1,
            LABEL.2,
        );
        gfx::hline(
            PANE_X,
            PANE_X + PANE_W - 16,
            PANE_Y - 4,
            LABEL.0,
            LABEL.1,
            LABEL.2,
        );

        for (row, line) in lines.iter().enumerate() {
            let mut len = COLS;
            while len > 0 && line[len - 1] == b' ' {
                len -= 1;
            }
            if len == 0 {
                continue;
            }
            let text = core::str::from_utf8_unchecked(&line[..len]);
            gfx::draw_str(
                PANE_X,
                PANE_Y + row as i32 * LINE_HEIGHT,
                text,
                TEXT.0,
                TEXT.1,
                TEXT.2,
            );
        }
    }
}
