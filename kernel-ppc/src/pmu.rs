// SPDX-License-Identifier: GPL-3.0-only
//! ADB through the PMU.
//!
//! The PowerBook G3 M4753 has no CUDA.  Its 6522 presents the PMU
//! microcontroller on port B: TREQ is an output asserted to start a byte,
//! TACK is an input the PMU pulls low to acknowledge it, and the shift
//! register carries the byte.  The PMU multiplexes ADB and housekeeping over
//! that byte channel, so an ADB request is a `PMU_ADB_CMD` command whose
//! arguments are the ADB bytes, followed by a `PMU_INT_ACK` read that returns
//! the pending interrupt packet, including any ADB reply.
//!
//! The command layout and the polled handshake follow the Linux `via-pmu`
//! driver.  QEMU has no PMU model, so this path cannot be exercised by the
//! gate; it is written for the hardware and marked pending verification in
//! `docs/research/powerbook-g3.md`.

use crate::adb::{self, Error, Timeout};
use crate::via::{self, Via};

/// PMU command bytes.
const PMU_ADB_CMD: u8 = 0x20;
const PMU_ADB_POLL_OFF: u8 = 0x21;
const PMU_INT_ACK: u8 = 0x78;

/// PMU interrupt bits.
const PMU_INT_ADB: u8 = 0x10;
const PMU_INT_ADB_AUTO: u8 = 0x04;

/// Port B handshake bits.  On the PMU TACK is the input and TREQ the output,
/// the opposite direction to the CUDA's pair, but both are active low.
const TACK: u8 = 0x08;
const TREQ: u8 = 0x10;

#[derive(Clone, Copy)]
pub struct PmuAdb {
    via: Via,
}

impl PmuAdb {
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

    fn wait_tack(self, asserted: bool, timeout: &Timeout) -> bool {
        loop {
            if (self.read_b() & TACK == 0) == asserted {
                return true;
            }
            if timeout.expired() {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    /// The PMU's byte handshake: assert TREQ, wait for TACK low, release
    /// TREQ and wait for TACK high.
    fn handshake(self, timeout: &Timeout) -> bool {
        self.write_b(self.read_b() & !TREQ);
        if !self.wait_tack(true, timeout) {
            return false;
        }
        self.write_b(self.read_b() | TREQ);
        self.wait_tack(false, timeout)
    }

    fn send_byte(self, byte: u8, timeout: &Timeout) -> bool {
        let acr = self.via.read(via::ACR);
        self.via
            .write(via::ACR, acr | via::ACR_SR_OUT | via::ACR_SR_EXT);
        self.via.write(via::SR, byte);
        self.handshake(timeout)
    }

    fn receive_byte(self, timeout: &Timeout) -> Option<u8> {
        let acr = self.via.read(via::ACR);
        self.via
            .write(via::ACR, (acr & !via::ACR_SR_OUT) | via::ACR_SR_EXT);
        // Reading the shift register resets it before the handshake.
        let _ = self.via.read(via::SR);
        if !self.handshake(timeout) {
            return None;
        }
        Some(self.via.read(via::SR))
    }

    /// Send a PMU command.  The ADB and interrupt-acknowledge commands carry
    /// a length byte followed by their arguments; the poll-off command has
    /// neither.
    fn command(self, command: u8, arguments: &[u8], timeout: &Timeout) -> Result<(), Error> {
        // Only PMU_ADB_CMD has a variable-length argument list; the
        // interrupt-acknowledge command takes none, and its reply carries its
        // own length byte.
        let variable = command == PMU_ADB_CMD;
        if !self.send_byte(command, timeout) {
            return Err(Error::Timeout);
        }
        if variable && !self.send_byte(arguments.len() as u8, timeout) {
            return Err(Error::Timeout);
        }
        for argument in arguments {
            if !self.send_byte(*argument, timeout) {
                return Err(Error::Timeout);
            }
        }
        Ok(())
    }

    /// Read a length-prefixed reply packet.
    fn reply(self, buffer: &mut [u8], timeout: &Timeout) -> Result<usize, Error> {
        let Some(length) = self.receive_byte(timeout) else {
            return Err(Error::Timeout);
        };
        let length = usize::from(length).min(buffer.len());
        for slot in buffer.iter_mut().take(length) {
            let Some(byte) = self.receive_byte(timeout) else {
                return Err(Error::Timeout);
            };
            *slot = byte;
        }
        Ok(length)
    }

    /// Bring the PMU's ADB channel to a known state.
    ///
    /// The bus is not reset: as on the CUDA path, a reset would move every
    /// device to its default address and invalidate the firmware's tree.
    /// Autopoll is turned off so a direct talk owns the bus.
    pub fn reset(self) -> Result<(), Error> {
        let timeout = Timeout::after_ms(100);
        // Negate TREQ, make TREQ an output and TACK an input.
        self.write_b(self.read_b() | TREQ);
        let dirb = self.via.read(via::DIRB);
        self.via.write(via::DIRB, (dirb | TREQ) & !TACK);
        self.command(PMU_ADB_POLL_OFF, &[], &timeout)
    }

    /// Talk to `address` register 0 and return the reply bytes.
    ///
    /// The PMU does not answer a talk command in the command reply; it queues
    /// an interrupt packet.  Poll `PMU_INT_ACK` until the ADB bit appears,
    /// bounded by the same timeout as the byte handshake.
    pub fn talk(self, address: u8, register: u8, reply: &mut [u8]) -> Result<usize, Error> {
        let timeout = Timeout::after_ms(100);
        let command = adb::talk(address, register);
        // The PMU_ADB_CMD arguments are the first ADB byte, the ADB flags
        // byte and the number of ADB bytes that follow.  One talk command has
        // no following bytes.
        self.command(PMU_ADB_CMD, &[command, 0x00, 0x00], &timeout)?;

        loop {
            self.command(PMU_INT_ACK, &[], &timeout)?;
            let mut packet = [0u8; 32];
            let length = self.reply(&mut packet, &timeout)?;
            if length >= 1 && packet[0] & PMU_INT_ADB != 0 {
                // Autopoll packets are not the reply to this request.
                if packet[0] & PMU_INT_ADB_AUTO == 0 && length >= 2 {
                    if packet[1] != 0 {
                        return Ok(0);
                    }
                    let payload = &packet[2..length];
                    let length = payload.len().min(reply.len());
                    reply[..length].copy_from_slice(&payload[..length]);
                    return Ok(length);
                }
            }
            if timeout.expired() {
                return Err(Error::Timeout);
            }
        }
    }
}
