// SPDX-License-Identifier: GPL-3.0-only
//! The Apple Desktop Bus transaction format and the standard keyboard
//! scancode map, shared by the CUDA and PMU hosts.
//!
//! An ADB command byte is the device address in bits 7..4, a command in bits
//! 3..2 and a register in bits 1..0.  `talk` asks a device for a register and
//! returns two data bytes; `listen` writes one.  The keyboard's register 0
//! holds the last key transition: bit 7 marks a release and 0x7f is not a
//! valid scancode, so a reply of 0xff, 0xff means no key.
//!
//! The CUDA wraps ADB bytes in a packet whose first byte is `ADB_PACKET`;
//! the PMU carries the same command bytes as arguments to `PMU_ADB_CMD`.
//! Only the outer framing differs, so the command encoding and the scancode
//! map live here.

/// The CUDA packet type that carries an ADB request and reply.
pub const CUDA_ADB_PACKET: u8 = 0x00;

/// The command field values.
pub const COMMAND_TALK: u8 = 0xc;

/// The keyboard's last-key register.
pub const REGISTER_KEY: u8 = 0;

/// The standard keyboard device address used when the firmware's ADB child
/// does not name one.  Firmware normally enumerates the keyboard at 8 or 2;
/// the device tree is the authority and this value is only the fallback for a
/// tree with an `adb` node but no keyboard child.
pub const DEFAULT_KEYBOARD_ADDRESS: u8 = 8;

/// A talk command for `register` on `address`.
pub const fn talk(address: u8, register: u8) -> u8 {
    (address << 4) | COMMAND_TALK | (register & 0x3)
}

/// One key transition decoded from a keyboard reply.
#[derive(Clone, Copy)]
pub struct Key {
    /// The ADB scancode without the release bit.
    pub code: u8,
    /// True for a key press, false for a release.
    pub down: bool,
    /// The US ASCII character, or zero for a key that produces none.
    pub ascii: u8,
}

impl Key {
    pub const EMPTY: Self = Self {
        code: 0,
        down: false,
        ascii: 0,
    };
}

/// The keyboard's shift and caps-lock state.
///
/// The CUDA's direct talk path does not return the modifier register, so the
/// state is tracked from the key transitions themselves.  The standard ADB
/// keyboard reports the left and right shift keys separately, and caps lock
/// toggles on the press.
#[derive(Clone, Copy, Default)]
pub struct Modifiers {
    shift: bool,
    caps: bool,
}

impl Modifiers {
    pub const fn from_bits(shift: bool, caps: bool) -> Self {
        Self { shift, caps }
    }

    pub const fn shift(self) -> bool {
        self.shift
    }

    pub const fn caps(self) -> bool {
        self.caps
    }

    pub fn update(&mut self, byte: u8) -> Key {
        const LEFT_SHIFT: u8 = 0x38;
        const RIGHT_SHIFT: u8 = 0x7b;
        const CAPS_LOCK: u8 = 0x39;

        let code = byte & 0x7f;
        let down = byte & 0x80 == 0;
        match code {
            LEFT_SHIFT | RIGHT_SHIFT => {
                self.shift = down;
                return Key {
                    code,
                    down,
                    ascii: 0,
                };
            }
            CAPS_LOCK => {
                if down {
                    self.caps = !self.caps;
                }
                return Key {
                    code,
                    down,
                    ascii: 0,
                };
            }
            _ => {}
        }
        Key {
            code,
            down,
            ascii: if down {
                ascii(code, self.shift, self.caps)
            } else {
                0
            },
        }
    }
}

/// A bounded wait measured in the decrementer's timebase.
///
/// Every ADB byte is acknowledged by the host controller within a few
/// microseconds on real hardware; the bound only protects a broken or absent
/// device from hanging the boot.  The timebase is the one the device tree
/// reported, so the same code gives the same wall-clock bound on every machine.
pub struct Timeout {
    start: u32,
    span: u32,
}

impl Timeout {
    pub fn after_ms(ms: u32) -> Self {
        Self {
            start: crate::timer::timebase(),
            span: (crate::timer::frequency() / 1000).saturating_mul(ms),
        }
    }

    pub fn expired(&self) -> bool {
        crate::timer::timebase().wrapping_sub(self.start) > self.span
    }
}

/// Spin for `ms` milliseconds of timebase, with no interrupt dependency.
pub fn spin_ms(ms: u32) {
    let timeout = Timeout::after_ms(ms);
    while !timeout.expired() {
        core::hint::spin_loop();
    }
}

/// A failed ADB transaction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The host controller did not acknowledge a byte in time.
    Timeout,
    /// A reply arrived with a framing byte or length the protocol cannot use.
    Protocol,
}

/// The unshifted and shifted US ASCII characters for an ADB scancode.
///
/// The table is the Apple Standard Keyboard layout as published in the ADB
/// specification.  Keys that produce no ASCII character, such as the arrows,
/// function keys and the modifier keys, return `(0, 0)`.
const fn characters(code: u8) -> (u8, u8) {
    match code {
        0x00 => (b'a', b'A'),
        0x01 => (b's', b'S'),
        0x02 => (b'd', b'D'),
        0x03 => (b'f', b'F'),
        0x04 => (b'h', b'H'),
        0x05 => (b'g', b'G'),
        0x06 => (b'z', b'Z'),
        0x07 => (b'x', b'X'),
        0x08 => (b'c', b'C'),
        0x09 => (b'v', b'V'),
        0x0b => (b'b', b'B'),
        0x0c => (b'q', b'Q'),
        0x0d => (b'w', b'W'),
        0x0e => (b'e', b'E'),
        0x0f => (b'r', b'R'),
        0x10 => (b'y', b'Y'),
        0x11 => (b't', b'T'),
        0x12 => (b'1', b'!'),
        0x13 => (b'2', b'@'),
        0x14 => (b'3', b'#'),
        0x15 => (b'4', b'$'),
        0x16 => (b'6', b'^'),
        0x17 => (b'5', b'%'),
        0x18 => (b'=', b'+'),
        0x19 => (b'9', b'('),
        0x1a => (b'7', b'&'),
        0x1b => (b'-', b'_'),
        0x1c => (b'8', b'*'),
        0x1d => (b'0', b')'),
        0x1e => (b']', b'}'),
        0x1f => (b'o', b'O'),
        0x20 => (b'u', b'U'),
        0x21 => (b'[', b'{'),
        0x22 => (b'i', b'I'),
        0x23 => (b'p', b'P'),
        0x24 => (b'\r', b'\r'),
        0x25 => (b'l', b'L'),
        0x26 => (b'j', b'J'),
        0x27 => (b'\'', b'"'),
        0x28 => (b'k', b'K'),
        0x29 => (b';', b':'),
        0x2a => (b'\\', b'|'),
        0x2b => (b',', b'<'),
        0x2c => (b'/', b'?'),
        0x2d => (b'n', b'N'),
        0x2e => (b'm', b'M'),
        0x2f => (b'.', b'>'),
        0x30 => (b'\t', b'\t'),
        0x31 => (b' ', b' '),
        0x32 => (b'`', b'~'),
        0x33 => (0x08, 0x08),
        0x34 => (b'\r', b'\r'),
        0x35 => (0x1b, 0x1b),
        0x41 => (b'.', b'.'),
        0x43 => (b'*', b'*'),
        0x45 => (b'+', b'+'),
        0x4b => (b'/', b'/'),
        0x4e => (b'-', b'-'),
        0x51 => (b'=', b'='),
        0x52 => (b'0', b'0'),
        0x53 => (b'1', b'1'),
        0x54 => (b'2', b'2'),
        0x55 => (b'3', b'3'),
        0x56 => (b'4', b'4'),
        0x57 => (b'5', b'5'),
        0x58 => (b'6', b'6'),
        0x59 => (b'7', b'7'),
        0x5b => (b'8', b'8'),
        0x5c => (b'9', b'9'),
        _ => (0, 0),
    }
}

fn ascii(code: u8, shift: bool, caps: bool) -> u8 {
    let (normal, shifted) = characters(code);
    if normal.is_ascii_alphabetic() {
        if shift ^ caps {
            normal.to_ascii_uppercase()
        } else {
            normal
        }
    } else if shift {
        shifted
    } else {
        normal
    }
}
