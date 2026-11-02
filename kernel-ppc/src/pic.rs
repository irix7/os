// SPDX-License-Identifier: GPL-3.0-only
//! The Heathrow interrupt controller.
//!
//! The selected Mac I/O node is `compatible = "heathrow"` on the beige G3 and
//! the PowerBook's primary controller, and `"gatwick"` on the PowerBook's
//! second controller; both put the interrupt controller in the first register
//! block of the window.  It has two banks of four 32-bit registers: the
//! pending events, the mask, the acknowledge port and the raw input levels.  A
//! source raises an event on its rising edge and stays pending until software
//! writes the same bit to the acknowledge port; that write is the end of
//! interrupt.
//!
//! Sources 0..31 are the second bank and 32..63 the first, which is the Mac
//! I/O interrupt numbering.  The registers are little-endian on the Mac I/O
//! bus, so every access is byte-reversed.

use core::ptr::{read_volatile, write_volatile};

const EVENTS: u32 = 0x00;
const MASK: u32 = 0x04;
const ACK: u32 = 0x08;

/// Bank 0 starts at 0x10 and bank 1 at 0x20, four registers each.
const BANK_BASE: u32 = 0x10;
const BANK_STRIDE: u32 = 0x10;

#[derive(Clone, Copy)]
pub struct Controller {
    base: u32,
}

impl Controller {
    pub fn new(base: u32) -> Self {
        Self { base }
    }

    fn address(self, bank: u32, offset: u32) -> u32 {
        self.base + BANK_BASE + bank * BANK_STRIDE + offset
    }

    fn read(self, bank: u32, offset: u32) -> u32 {
        let address = self.address(bank, offset);
        // SAFETY: the caller passes a base inside the mapped Mac I/O window,
        // and the offset is one of the controller's own registers.
        let value = unsafe { read_volatile(address as *const u32) };
        value.swap_bytes()
    }

    fn write(self, bank: u32, offset: u32, value: u32) {
        let address = self.address(bank, offset);
        // SAFETY: as for `read`; the register is write-only or read/write.
        unsafe {
            write_volatile(address as *mut u32, value.swap_bytes());
        }
    }

    /// The bank and bit that carry `line`.
    fn decode(line: u32) -> (u32, u32) {
        assert!(line < 64, "interrupt line outside the Heathrow controller");
        (1 - (line >> 5), line & 31)
    }

    /// Mask every source and drop any event the firmware left pending.
    pub fn reset(self) {
        self.write(0, MASK, 0);
        self.write(1, MASK, 0);
        let events0 = self.read(0, EVENTS);
        let events1 = self.read(1, EVENTS);
        self.write(0, ACK, events0);
        self.write(1, ACK, events1);
    }

    pub fn enable(self, line: u32) {
        let (bank, bit) = Self::decode(line);
        let mask = self.read(bank, MASK);
        self.write(bank, MASK, mask | (1 << bit));
    }

    pub fn disable(self, line: u32) {
        let (bank, bit) = Self::decode(line);
        let mask = self.read(bank, MASK);
        self.write(bank, MASK, mask & !(1 << bit));
    }

    pub fn mask(self, line: u32) -> u32 {
        let (bank, _) = Self::decode(line);
        self.read(bank, MASK)
    }

    /// Every pending source in the bank that carries `line`, masked or not.
    pub fn events(self, line: u32) -> u32 {
        let (bank, _) = Self::decode(line);
        self.read(bank, EVENTS)
    }

    /// Clear `bits` in the bank that carries `line`: the end of interrupt.
    pub fn acknowledge(self, line: u32, bits: u32) {
        let (bank, _) = Self::decode(line);
        self.write(bank, ACK, bits);
    }
}
