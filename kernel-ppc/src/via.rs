// SPDX-License-Identifier: GPL-3.0-only
//! The VIA used by the CUDA and the PMU.
//!
//! The CUDA's MOS 6522 is the external timer and the ADB host; the PMU in a
//! PowerBook is a 6522-compatible interface to the PMU microcontroller, and
//! its ADB transactions use the same shift register and handshake pins.  Both
//! nodes name a window inside Mac I/O, so the register address is the base plus
//! the register number shifted by nine.
//!
//! Timer 1 counts down at the 6522's PHI2 rate; in continuous mode it reloads
//! from the latch on every underflow, sets the T1 flag in the interrupt flag
//! register and raises the VIA's interrupt line.  Writing the T1 bit back to
//! the flag register clears it and drops the line.

use core::ptr::{read_volatile, write_volatile};

/// VIA register numbers.  Each register is 0x200 bytes apart.
pub const B: u32 = 0;
pub const DIRB: u32 = 2;
pub const SR: u32 = 10;
pub const ACR: u32 = 11;
pub const IFR: u32 = 13;
pub const IER: u32 = 14;
const T1CH: u32 = 5;
const T1LL: u32 = 6;
const T1LH: u32 = 7;

const REGISTER_SHIFT: u32 = 9;

/// Timer 1 continuous mode: reload from the latch on underflow.
const ACR_T1_CONTINUOUS: u8 = 0x40;
/// The two timer-1 mode bits in the auxiliary control register.
const ACR_T1_MODE: u8 = 0xc0;
/// Timer 1 interrupt flag.
const IFR_T1: u8 = 0x40;
/// The set/clear selector in an interrupt-enable write.
const IER_SET: u8 = 0x80;

/// The shift-register interrupt flag, set when a byte has moved.
pub const IFR_SR: u8 = 0x04;
/// Shift register on the external clock, shift out when `SR_OUT` is set.
pub const ACR_SR_EXT: u8 = 0x0c;
pub const ACR_SR_OUT: u8 = 0x10;

/// Timer 1 counts at the CUDA clock, 4.7 MHz divided by six.
pub const T1_HZ: u32 = 4_700_000 / 6;

/// A 6522 register window.  The base comes from the device tree.
#[derive(Clone, Copy)]
pub struct Via {
    base: u32,
}

impl Via {
    pub const fn new(base: u32) -> Self {
        Self { base }
    }

    fn address(self, register: u32) -> u32 {
        self.base + (register << REGISTER_SHIFT)
    }

    pub fn read(self, register: u32) -> u8 {
        let address = self.address(register);
        // SAFETY: the base comes from the VIA's device-tree node and the
        // register number selects one of the chip's byte-wide registers.
        unsafe { read_volatile(address as *const u8) }
    }

    pub fn write(self, register: u32, value: u8) {
        let address = self.address(register);
        // SAFETY: as for `read`.
        unsafe {
            write_volatile(address as *mut u8, value);
        }
    }
}

#[derive(Clone, Copy)]
pub struct Timer {
    via: Via,
}

impl Timer {
    pub fn new(base: u32) -> Self {
        Self {
            via: Via::new(base),
        }
    }

    fn read(self, register: u32) -> u8 {
        self.via.read(register)
    }

    fn write(self, register: u32, value: u8) {
        self.via.write(register, value);
    }

    /// Load the latch and select continuous mode.  The interrupt stays off.
    ///
    /// Only the two timer-1 mode bits change: the PMU path shares this 6522
    /// with the shift register, and clearing its control field would disturb
    /// a byte in flight.
    pub fn program(self, ticks: u16) {
        self.disable();
        let acr = self.read(ACR);
        self.write(ACR, (acr & !ACR_T1_MODE) | ACR_T1_CONTINUOUS);
        self.write(T1LL, ticks as u8);
        self.write(T1LH, (ticks >> 8) as u8);
        self.write(IFR, IFR_T1);
    }

    /// Enable the timer-1 interrupt and start the counter from the latch.
    pub fn start(self, ticks: u16) {
        self.write(IER, IER_SET | IFR_T1);
        self.write(T1CH, (ticks >> 8) as u8);
    }

    /// Stop the timer and clear every flag it raised.
    ///
    /// Disabling the interrupt stops the controller line but not the
    /// counter; a 6522 sets the T1 flag on every underflow whether or not
    /// the interrupt is enabled.  Put T1 into one-shot mode and load the
    /// longest count before clearing the flag, so the caller's read of IFR
    /// cannot race the next underflow.  `program` restores continuous mode.
    pub fn stop(self) {
        self.disable();
        let acr = self.read(ACR);
        self.write(ACR, acr & !ACR_T1_MODE);
        self.write(T1LL, 0xff);
        self.write(T1LH, 0xff);
        self.write(T1CH, 0xff);
        self.write(IFR, 0x7f);
    }

    /// Mask every VIA interrupt source.
    ///
    /// All seven sources share the chip's one interrupt output, so a source
    /// the firmware left enabled could reach the controller line and make
    /// the timer proof count a foreign event.  The ADB paths poll their
    /// shift-register flag rather than enabling its interrupt.
    fn disable(self) {
        self.write(IER, 0x7f);
    }

    /// Clear the timer-1 flag: the source of the current interrupt.
    pub fn acknowledge(self) {
        self.write(IFR, IFR_T1);
    }

    pub fn ifr(self) -> u8 {
        self.read(IFR)
    }
}
