// SPDX-License-Identifier: GPL-3.0-only
//! PCI configuration access through the legacy mechanism #1 ports.
//!
//! The machine scans bus 0, which is where QEMU's `q35` puts the devices the
//! gates use.  Enumerating bridges is a later slice; a device behind one is not
//! found yet.

use crate::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

const COMMAND: u8 = 0x04;
const BAR0: u8 = 0x10;

/// Command register bits: I/O space enable and bus mastering.
const COMMAND_IO_AND_MASTER: u32 = 0x5;

/// A PCI function on bus 0.
#[derive(Clone, Copy, Debug)]
pub struct Function {
    pub slot: u8,
    pub func: u8,
}

fn address(function: Function, offset: u8) -> u32 {
    0x8000_0000
        | (u32::from(function.slot) & 0x1F) << 11
        | (u32::from(function.func) & 0x7) << 8
        | u32::from(offset & 0xFC)
}

pub fn read32(function: Function, offset: u8) -> u32 {
    outl(CONFIG_ADDRESS, address(function, offset));
    inl(CONFIG_DATA)
}

pub fn write32(function: Function, offset: u8, value: u32) {
    outl(CONFIG_ADDRESS, address(function, offset));
    outl(CONFIG_DATA, value);
}

/// Find the first function on bus 0 with the given vendor and device IDs.
pub fn find(vendor: u16, device: u16) -> Option<Function> {
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let function = Function { slot, func };
            let id = read32(function, 0x00);
            if id & 0xFFFF == 0xFFFF {
                if func == 0 {
                    break;
                }
                continue;
            }
            if id as u16 == vendor && (id >> 16) as u16 == device {
                return Some(function);
            }
        }
    }
    None
}

/// The I/O port base of BAR0.  Only meaningful for an I/O-space BAR.
pub fn io_bar0(function: Function) -> u16 {
    (read32(function, BAR0) & 0xFFFC) as u16
}

/// Let the function decode its I/O ports and act as a DMA master.
pub fn enable(function: Function) {
    // The status half is write-one-to-clear, so write it as zero.
    write32(function, COMMAND, COMMAND_IO_AND_MASTER);
}
