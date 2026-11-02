// SPDX-License-Identifier: GPL-3.0-only
//! The Heathrow ATA channel, polled PIO.
//!
//! Mac I/O ATA is a plain task-file interface with a 16-byte register
//! stride, not the ISA `0x1f0` map.  The register window comes from the
//! `ata-3` node's `reg` property; only the offsets inside that window are
//! fixed by the chip.
//!
//! The data register is a 16-bit little-endian port on a big-endian bus, so a
//! halfword load returns the two wire bytes in memory order.  That is the
//! order a sector buffer needs.  Identify word fields are little-endian on
//! the wire, so `identify` byte-swaps each word before reading the feature
//! word and the LBA capacities; the model string then comes out in character
//! order.
//!
//! No interrupt is used: the drive's `nIEN` bit is set and every wait is
//! bounded by two seconds of timebase.

use core::ptr::{read_volatile, write_volatile};

use crate::{print_hex, serial, timer};

/// Each task-file register is 16 bytes apart in the Mac I/O window.
const REGISTER_STRIDE: u32 = 16;

const DATA: u32 = 0x00;
const ERROR: u32 = 0x01;
const NSECTOR: u32 = 0x02;
const LBA0: u32 = 0x03;
const LBA1: u32 = 0x04;
const LBA2: u32 = 0x05;
const DRIVE: u32 = 0x06;
const STATUS: u32 = 0x07;
const COMMAND: u32 = 0x07;
const CONTROL: u32 = 0x08;

pub const SECTOR_SIZE: usize = 512;
/// The largest LBA a single `READ SECTORS` command can address.
const LBA28_LIMIT: u32 = 1 << 28;

const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_DF: u8 = 0x20;
const STATUS_DRDY: u8 = 0x40;
const STATUS_BSY: u8 = 0x80;

const COMMAND_READ_SECTORS: u8 = 0x20;
const COMMAND_IDENTIFY: u8 = 0xec;

/// Device control: disable the drive's interrupt line.
const CONTROL_NIEN: u8 = 0x02;

/// LBA addressing, device 0.
const DRIVE_LBA: u8 = 0xe0;

/// Every task-file wait is bounded by this many seconds of timebase.
const WAIT_SECONDS: u32 = 2;

#[derive(Clone, Copy)]
pub struct Channel {
    base: u32,
}

#[derive(Clone, Copy)]
pub struct Identify {
    pub model: [u8; 40],
    pub sectors: u64,
    pub lba48: bool,
}

#[derive(Clone, Copy)]
pub struct Error {
    pub phase: &'static str,
    pub status: u8,
    pub error: u8,
}

impl Error {
    pub fn report(self) {
        serial::write(b"ppc: ata error phase=");
        serial::write(self.phase.as_bytes());
        serial::write(b" status=");
        print_hex(u32::from(self.status));
        serial::write(b" error=");
        print_hex(u32::from(self.error));
        serial::write_byte(b'\n');
    }
}

impl Channel {
    pub fn new(base: u32) -> Self {
        Self { base }
    }

    fn port(self, register: u32) -> u32 {
        self.base + register * REGISTER_STRIDE
    }

    fn read8(self, register: u32) -> u8 {
        // SAFETY: the register window comes from the device tree and the
        // caller only uses offsets defined by the Mac I/O ATA interface.
        unsafe { read_volatile(self.port(register) as *const u8) }
    }

    fn write8(self, register: u32, value: u8) {
        // SAFETY: as for `read8`; these are the task-file registers.
        unsafe {
            write_volatile(self.port(register) as *mut u8, value);
        }
    }

    fn read_data(self) -> u16 {
        // SAFETY: the data register is the one 16-bit port on this channel.
        unsafe { read_volatile(self.port(DATA) as *const u16) }
    }

    fn status(self) -> u8 {
        self.read8(STATUS)
    }

    fn alt_status(self) -> u8 {
        self.read8(CONTROL)
    }

    fn error(self, phase: &'static str, status: u8) -> Error {
        Error {
            phase,
            status,
            error: self.read8(ERROR),
        }
    }

    /// Four alternate-status reads are the ATA 400 ns command delay.
    fn command_delay(self) {
        for _ in 0..4 {
            self.alt_status();
        }
    }

    /// Wait until `mask` is set and the drive is neither busy nor faulted.
    fn wait(self, mask: u8, phase: &'static str) -> Result<u8, Error> {
        let start = timer::timebase();
        let limit = timer::frequency().saturating_mul(WAIT_SECONDS);
        loop {
            let status = self.alt_status();
            if status & STATUS_BSY == 0 {
                if status & (STATUS_ERR | STATUS_DF) != 0 {
                    return Err(self.error(phase, status));
                }
                if status & mask == mask {
                    return Ok(status);
                }
            }
            if timer::timebase().wrapping_sub(start) > limit {
                return Err(self.error(phase, status));
            }
            core::hint::spin_loop();
        }
    }

    /// Select device 0 and run `IDENTIFY DEVICE`.
    pub fn identify(self) -> Result<Identify, Error> {
        self.write8(CONTROL, CONTROL_NIEN);
        self.write8(DRIVE, 0xa0);
        self.command_delay();
        if self.status() == 0 {
            return Err(self.error("identify", 0));
        }
        self.write8(NSECTOR, 0);
        self.write8(LBA0, 0);
        self.write8(LBA1, 0);
        self.write8(LBA2, 0);
        self.write8(COMMAND, COMMAND_IDENTIFY);
        self.command_delay();
        self.wait(STATUS_DRQ, "identify")?;

        let mut words = [0u16; 256];
        for word in words.iter_mut() {
            // The data port presents the ATA wire bytes in memory order.
            // Identify word fields are little-endian on the wire, so the
            // halfword the CPU loaded is byte-swapped relative to the value
            // the ATA word denotes.  Sector data needs no such swap: it is a
            // byte stream and is copied in memory order.
            *word = self.read_data().swap_bytes();
        }

        let mut model = [0u8; 40];
        for (index, word) in words[27..47].iter().enumerate() {
            let bytes = word.to_be_bytes();
            model[index * 2] = bytes[0];
            model[index * 2 + 1] = bytes[1];
        }

        let lba48 = words[83] & (1 << 10) != 0;
        let mut sectors = u64::from(words[60]) | (u64::from(words[61]) << 16);
        if lba48 {
            let extended = u64::from(words[100])
                | (u64::from(words[101]) << 16)
                | (u64::from(words[102]) << 32)
                | (u64::from(words[103]) << 48);
            if extended != 0 {
                sectors = extended;
            }
        }

        Ok(Identify {
            model,
            sectors,
            lba48,
        })
    }

    /// Read one sector with `READ SECTORS` (LBA28, PIO).
    pub fn read_sector(self, lba: u32, buffer: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
        if lba >= LBA28_LIMIT {
            return Err(self.error("read range", 0));
        }
        self.write8(CONTROL, CONTROL_NIEN);
        self.wait(STATUS_DRDY, "select")?;
        self.write8(DRIVE, DRIVE_LBA | ((lba >> 24) as u8 & 0x0f));
        self.command_delay();
        self.write8(NSECTOR, 1);
        self.write8(LBA0, lba as u8);
        self.write8(LBA1, (lba >> 8) as u8);
        self.write8(LBA2, (lba >> 16) as u8);
        self.write8(COMMAND, COMMAND_READ_SECTORS);
        self.command_delay();
        self.wait(STATUS_DRQ, "read")?;

        for index in 0..(SECTOR_SIZE / 2) {
            let bytes = self.read_data().to_be_bytes();
            buffer[index * 2] = bytes[0];
            buffer[index * 2 + 1] = bytes[1];
        }

        self.wait(STATUS_DRDY, "read complete")?;
        Ok(())
    }
}
