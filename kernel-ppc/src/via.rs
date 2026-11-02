// SPDX-License-Identifier: GPL-3.0-only
//! The CUDA's MOS 6522 VIA, used as the external timer.
//!
//! The CUDA node names its window inside Mac I/O and the interrupt line it
//! asserts.  Timer 1 counts down at the 6522's PHI2 rate; in continuous mode
//! it reloads from the latch on every underflow, sets the T1 flag in the
//! interrupt flag register and raises the CUDA's interrupt line.  Writing the
//! T1 bit back to the flag register clears it and drops the line.
//!
//! The CUDA window is at an offset inside the Mac I/O window the firmware
//! assigns; that offset comes from the device tree, not from a constant.

use core::ptr::{read_volatile, write_volatile};

/// VIA register numbers.  Each register is 0x200 bytes apart.
const T1CH: u32 = 5;
const T1LL: u32 = 6;
const T1LH: u32 = 7;
const ACR: u32 = 11;
const IFR: u32 = 13;
const IER: u32 = 14;

const REGISTER_SHIFT: u32 = 9;

/// Timer 1 continuous mode: reload from the latch on underflow.
const ACR_T1_CONTINUOUS: u8 = 0x40;
/// Timer 1 interrupt flag.
const IFR_T1: u8 = 0x40;
/// The set/clear selector in an interrupt-enable write.
const IER_SET: u8 = 0x80;

/// Timer 1 counts at the CUDA clock, 4.7 MHz divided by six.
pub const T1_HZ: u32 = 4_700_000 / 6;

#[derive(Clone, Copy)]
pub struct Timer {
    base: u32,
}

impl Timer {
    pub fn new(base: u32) -> Self {
        Self { base }
    }

    fn address(self, register: u32) -> u32 {
        self.base + (register << REGISTER_SHIFT)
    }

    fn read(self, register: u32) -> u8 {
        let address = self.address(register);
        // SAFETY: the base comes from the CUDA node and the register number
        // selects one of the VIA's byte-wide registers.
        unsafe { read_volatile(address as *const u8) }
    }

    fn write(self, register: u32, value: u8) {
        let address = self.address(register);
        // SAFETY: as for `read`.
        unsafe {
            write_volatile(address as *mut u8, value);
        }
    }

    /// Load the latch and select continuous mode.  The interrupt stays off.
    pub fn program(self, ticks: u16) {
        self.disable();
        self.write(ACR, ACR_T1_CONTINUOUS);
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
        self.write(ACR, 0);
        self.write(T1LL, 0xff);
        self.write(T1LH, 0xff);
        self.write(T1CH, 0xff);
        self.write(IFR, 0x7f);
    }

    fn disable(self) {
        self.write(IER, IFR_T1);
    }

    /// Clear the timer-1 flag: the source of the current interrupt.
    pub fn acknowledge(self) {
        self.write(IFR, IFR_T1);
    }

    pub fn ifr(self) -> u8 {
        self.read(IFR)
    }
}
