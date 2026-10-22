// SPDX-License-Identifier: GPL-3.0-only
//! PS/2 keyboard input.
//!
//! The controller lives in the IOC: the data port is at IOC offset 0x40 and
//! the command/status port at 0x44 (IOC base 0x1FBD_9800, so kseg1
//! 0xBFBD_9840 and 0xBFBD_9844).
//!
//! The keyboard is put into scancode set 1 and scanning is enabled, so the
//! bytes delivered are the familiar set-1 make/break codes with 0xE0 as the
//! extended prefix.

use core::ptr::{read_volatile, write_volatile};

/// Data port (read: scancode; write: keyboard command parameter).
const DATA: u32 = 0xBFBD_9840;
/// Command/status port.
const CMD: u32 = 0xBFBD_9844;

/// Status bit 0: the output buffer holds a byte.
const STATUS_OUTPUT_FULL: u8 = 0x01;

/// A key event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// A printable character, already shifted.
    Char(u8),
    /// Return/Enter.
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
}

/// Set-1 scancode to ASCII (unshifted).
const SET1_MAP: &[(u8, u8)] = &[
    (0x02, b'1'), (0x03, b'2'), (0x04, b'3'), (0x05, b'4'), (0x06, b'5'),
    (0x07, b'6'), (0x08, b'7'), (0x09, b'8'), (0x0A, b'9'), (0x0B, b'0'),
    (0x0C, b'-'), (0x0D, b'='), (0x0E, 0), (0x0F, 0),
    (0x10, b'q'), (0x11, b'w'), (0x12, b'e'), (0x13, b'r'), (0x14, b't'),
    (0x15, b'y'), (0x16, b'u'), (0x17, b'i'), (0x18, b'o'), (0x19, b'p'),
    (0x1A, b'['), (0x1B, b']'), (0x1C, 0), (0x1D, b'\\'),
    (0x1E, b'a'), (0x1F, b's'), (0x20, b'd'), (0x21, b'f'), (0x22, b'g'),
    (0x23, b'h'), (0x24, b'j'), (0x25, b'k'), (0x26, b'l'), (0x27, b';'),
    (0x28, b'\''), (0x29, b'`'), (0x2A, 0), (0x2B, b'\\'),
    (0x2C, b'z'), (0x2D, b'x'), (0x2E, b'c'), (0x2F, b'v'), (0x30, b'b'),
    (0x31, b'n'), (0x32, b'm'), (0x33, b','), (0x34, b'.'), (0x35, b'/'),
    (0x36, 0), (0x37, b'*'), (0x38, b'+'),
    (0x39, b' '), (0x3A, 0), (0x3B, 0), (0x3C, 0), (0x3D, 0), (0x3E, 0),
    (0x3F, 0), (0x40, 0), (0x41, 0), (0x42, 0), (0x43, 0), (0x44, 0),
    (0x45, 0), (0x46, 0), (0x47, 0),
    (0x48, 0), (0x49, 0), (0x4A, 0), (0x4B, 0), (0x4C, 0), (0x4D, 0),
    (0x4E, 0), (0x4F, 0), (0x50, 0), (0x51, 0), (0x52, 0),
];

/// Set-1 scancode to shifted character.
const SET1_SHIFT_MAP: &[(u8, u8)] = &[
    (0x02, b'!'), (0x03, b'@'), (0x04, b'#'), (0x05, b'$'), (0x06, b'%'),
    (0x07, b'^'), (0x08, b'*'), (0x09, b'('), (0x0A, b')'), (0x0B, b'_'),
    (0x0C, b'+'), (0x0D, b'{'),
];

fn read8(addr: u32) -> u8 {
    unsafe { read_volatile(addr as *const u8) }
}

fn write8(addr: u32, val: u8) {
    unsafe { write_volatile(addr as *mut u8, val) }
}

/// Bring the keyboard up: select scancode set 1 and enable scanning.
///
/// The keyboard's own commands go to the data port; the command port
/// belongs to the controller.  0xF0 asks the keyboard which scancode set
/// it uses and the set number follows on the data port; 0xF4 enables
/// scanning so key events are delivered.
pub fn init() {
    write8(DATA, 0xF0);
    write8(DATA, 0x01);
    write8(DATA, 0xF4);
    // Drain the acknowledgements so they are not mistaken for key presses.
    for _ in 0..4 {
        if read8(CMD) & STATUS_OUTPUT_FULL == 0 {
            break;
        }
        let _ = read8(DATA);
    }
}

/// Modifier state tracked between scancodes.
struct Mods {
    shift: bool,
    caps: bool,
    extended: bool,
}

/// Poll for one key event, if any.
pub fn poll_key() -> Option<Key> {
    static mut MODS: Mods = Mods {
        shift: false,
        caps: false,
        extended: false,
    };

    loop {
        if read8(CMD) & STATUS_OUTPUT_FULL == 0 {
            return None;
        }
        let code = read8(DATA);
        let mods = unsafe { &mut MODS };

        // 0xE0 introduces the extended keys (arrows, keypad navigation).
        if code == 0xE0 {
            mods.extended = true;
            continue;
        }

        let make = code & 0x80 == 0;
        let sc = code & 0x7F;

        // 0xE1 pauses the keyboard; skip the rest of that sequence.
        if sc == 0xE1 {
            mods.extended = false;
            continue;
        }

        let extended = mods.extended;
        mods.extended = false;

        if !make {
            // Break codes only update the modifier state.
            match sc {
                0x2A | 0x36 => mods.shift = false,
                0x3A => mods.caps = false,
                _ => {}
            }
            continue;
        }

        match sc {
            0x2A | 0x36 => {
                mods.shift = true;
                continue;
            }
            0x3A => {
                mods.caps = true;
                continue;
            }
            _ => {}
        }

        if extended {
            let key = match sc {
                0x48 => Some(Key::Up),
                0x50 => Some(Key::Down),
                0x4B => Some(Key::Left),
                0x4D => Some(Key::Right),
                _ => None,
            };
            if key.is_some() {
                return key;
            }
            continue;
        }

        // Enter and Escape have no printable mapping.
        if sc == 0x1C {
            return Some(Key::Enter);
        }
        if sc == 0x01 {
            return Some(Key::Escape);
        }
        if sc == 0x0E {
            return Some(Key::Backspace);
        }

        let shifted = mods.shift;
        let caps = mods.caps;
        for (s, c) in SET1_MAP.iter() {
            if *s == sc {
                if *c == 0 {
                    return None;
                }
                let mut ch = *c;
                if shifted {
                    if let Some((_, sc2)) = SET1_SHIFT_MAP.iter().find(|(s2, _)| *s2 == sc) {
                        ch = *sc2;
                    }
                }
                if caps {
                    ch = ch.to_ascii_lowercase();
                } else if !shifted {
                    ch = ch.to_ascii_uppercase();
                }
                return Some(Key::Char(ch));
            }
        }
        // Unmapped scancode.
        continue;
    }
}
