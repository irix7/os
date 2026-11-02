// SPDX-License-Identifier: GPL-3.0-only
//! ADB through the CUDA's MOS 6522.
//!
//! The CUDA is a 6522 whose shift register is the ADB data path and whose
//! port B carries three handshake pins: TREQ (input, active low) is asserted
//! when the CUDA has a byte for the host or has accepted one, TACK (output,
//! active low) acknowledges each byte, and TIP (output, active low) marks a
//! transfer.  The sequence below is the one the Linux `via-cuda` driver uses
//! for a big-endian Mac and the one QEMU's `cuda` model implements:
//!
//! * sync: negate TIP/TACK, pulse TACK once and read back the two shift
//!   register interrupts the CUDA raises in response;
//! * send: shift each packet byte out with `SR_OUT`, toggling TACK after each
//!   byte so the CUDA latches it, then negate TIP/TACK to close the packet;
//! * receive: assert TIP in input mode, read each byte the CUDA loads into the
//!   shift register and toggle TACK until TREQ deasserts.
//!
//! Everything is polled.  The VIA's shift-register interrupt is only used as
//! a status flag; the CUDA's controller line is left alone, so this path does
//! not disturb the timer interrupt that the boot probe already owns.

use crate::adb::{self, Error, Timeout};
use crate::via::{self, Via};

/// Port B handshake bits, active low.
const TREQ: u8 = 0x08;
const TACK: u8 = 0x10;
const TIP: u8 = 0x20;

/// The shift register control field in ACR.
const SR_CTRL: u8 = 0x1c;

#[derive(Clone, Copy)]
pub struct CudaAdb {
    via: Via,
}

impl CudaAdb {
    pub const fn new(base: u32) -> Self {
        Self {
            via: Via::new(base),
        }
    }

    fn read_b(self) -> u8 {
        self.via.read(via::B)
    }

    fn write_b(self, value: u8) {
        self.via.write(via::B, value);
    }

    fn treq_asserted(self) -> bool {
        self.read_b() & TREQ == 0
    }

    fn wait_for(self, mask: u8, set: bool, timeout: &Timeout) -> bool {
        loop {
            if (self.read_b() & mask != 0) == set {
                return true;
            }
            if timeout.expired() {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    fn wait_shift(self, timeout: &Timeout) -> bool {
        loop {
            if self.via.read(via::IFR) & via::IFR_SR != 0 {
                return true;
            }
            if timeout.expired() {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    /// Read the shift register and clear its interrupt flag.
    fn clear_shift(self) {
        let _ = self.via.read(via::SR);
        self.via.write(via::IFR, via::IFR_SR);
    }

    /// Bring the CUDA's ADB state machine to idle.
    ///
    /// The sync handshake is the sequence the firmware leaves behind: assert
    /// TACK without TIP, wait for the CUDA's TREQ and shift-register
    /// interrupt, then release TACK and wait for TREQ to drop.  The driver
    /// deliberately does not issue an ADB bus reset: a reset returns every
    /// device to its default address, and the address the kernel uses comes
    /// from the firmware's device tree.  Preserving it keeps the tree and the
    /// bus in agreement without an enumeration pass.
    pub fn reset(self) -> Result<(), Error> {
        let dirb = self.via.read(via::DIRB);
        self.via.write(via::DIRB, (dirb | TACK | TIP) & !TREQ);
        let acr = self.via.read(via::ACR);
        self.via.write(via::ACR, (acr & !SR_CTRL) | via::ACR_SR_EXT);
        let _ = self.via.read(via::SR);

        // Negate TIP and TACK, let any byte in flight drain, then clear the
        // stale shift-register flag before the handshake starts.
        self.write_b(self.read_b() | TIP | TACK);
        adb::spin_ms(4);
        self.clear_shift();

        // Assert TACK without TIP and wait for the CUDA to request a byte.
        self.write_b((self.read_b() | TIP) & !TACK);
        let timeout = Timeout::after_ms(50);
        if !self.wait_for(TREQ, false, &timeout) {
            return Err(Error::Timeout);
        }
        if !self.wait_shift(&timeout) {
            return Err(Error::Timeout);
        }
        self.clear_shift();

        // Release TACK and wait for the CUDA to withdraw TREQ.
        self.write_b(self.read_b() | TACK);
        if !self.wait_for(TREQ, true, &timeout) {
            return Err(Error::Timeout);
        }
        if !self.wait_shift(&timeout) {
            return Err(Error::Timeout);
        }
        self.clear_shift();
        Ok(())
    }

    /// Talk to `address` register 0 and return the two reply bytes.
    pub fn talk(self, address: u8, register: u8, reply: &mut [u8]) -> Result<usize, Error> {
        let packet = [adb::CUDA_ADB_PACKET, adb::talk(address, register)];
        self.send(&packet)?;
        self.receive(reply)
    }

    /// Shift a CUDA packet out of the host into the CUDA.
    fn send(self, bytes: &[u8]) -> Result<(), Error> {
        let acr = self.via.read(via::ACR);
        self.via.write(via::ACR, acr | via::ACR_SR_OUT);
        self.via.write(via::SR, bytes[0]);
        // Assert TIP: the first byte is latched by the CUDA on this edge.
        self.write_b((self.read_b() | TACK) & !TIP);

        let timeout = Timeout::after_ms(50);
        for byte in &bytes[1..] {
            if !self.wait_shift(&timeout) {
                return Err(Error::Timeout);
            }
            let _ = self.via.read(via::SR);
            self.via.write(via::SR, *byte);
            // Toggle TACK so the CUDA latches the byte just written.
            self.write_b(self.read_b() ^ TACK);
        }

        if !self.wait_shift(&timeout) {
            return Err(Error::Timeout);
        }
        let _ = self.via.read(via::SR);
        self.via.write(via::ACR, acr & !via::ACR_SR_OUT);
        let _ = self.via.read(via::SR);
        // Negate TIP and TACK: the packet is complete.
        self.write_b(self.read_b() | TIP | TACK);
        Ok(())
    }

    /// Read the CUDA's reply packet into `reply`.
    ///
    /// The reply is `[ADB_PACKET, status, data..]`.  Status zero carries the
    /// device's two data bytes; a nonzero status is the CUDA's no-response
    /// indication and yields zero bytes.  A real CUDA may also echo the
    /// command byte in front of the data; the caller strips it.
    fn receive(self, reply: &mut [u8]) -> Result<usize, Error> {
        let timeout = Timeout::after_ms(50);
        if !self.wait_shift(&timeout) {
            return Err(Error::Timeout);
        }
        let _ = self.via.read(via::SR);
        // Assert TIP in input mode: the CUDA loads the first byte into SR.
        self.write_b((self.read_b() | TACK) & !TIP);

        let mut bytes = [0u8; 10];
        let mut count = 0;
        while count < bytes.len() {
            if !self.wait_shift(&timeout) {
                return Err(Error::Timeout);
            }
            bytes[count] = self.via.read(via::SR);
            count += 1;
            if !self.treq_asserted() {
                break;
            }
            // Ask for the next byte.
            self.write_b(self.read_b() ^ TACK);
        }

        // Close the transfer whether it ended early or at the limit.
        self.write_b(self.read_b() | TIP | TACK);
        self.clear_shift();

        if count < 2 || bytes[0] != adb::CUDA_ADB_PACKET {
            return Err(Error::Protocol);
        }
        if bytes[1] != 0 {
            return Ok(0);
        }
        let length = (count - 2).min(reply.len());
        reply[..length].copy_from_slice(&bytes[2..2 + length]);
        Ok(length)
    }
}
