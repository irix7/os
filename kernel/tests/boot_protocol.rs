// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the kernel's boot-protocol adapter.
//!
//! The parser itself lives in `boot-protocol-tests` and is exercised by that
//! crate's tests. These tests compile the kernel's adapter directly, so a
//! layout or constant change in the shared contract that the kernel does not
//! use fails here as well as in the crate.

#![allow(dead_code)]

#[path = "../src/boot_protocol.rs"]
mod boot_protocol;

use boot_protocol::BootInfoError;
use boot_protocol_tests::PhysMemory;

#[test]
fn kernel_uses_the_shared_layout_and_constants() {
    assert_eq!(boot_protocol_tests::BOOTINFO_MAGIC, 0x4937_4249);
    assert_eq!(boot_protocol_tests::BOOTINFO_VERSION_MAJOR, 1);
    assert_eq!(boot_protocol_tests::BOOTINFO_SIZE, 256);
    assert_eq!(boot_protocol_tests::MEM_MAP_COUNT_MAX, 64);
    assert_eq!(boot_protocol_tests::MEM_MAP_ENTRY_SIZE, 24);
    assert_eq!(boot_protocol::MEM_KIND_USABLE_RAM, 0);

    assert_eq!(boot_protocol_tests::BootInfo::OFF_MAGIC, 0x00);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_SIZE, 0x08);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_CHECKSUM, 0x0C);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_MEM_MAP_PHYS, 0x10);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_MEM_MAP_COUNT, 0x18);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_MEM_MAP_ENTRY_SIZE, 0x1C);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_KERNEL_PHYS, 0x20);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_CONSOLE_POLL, 0xF0);
    assert_eq!(boot_protocol_tests::BootInfo::OFF_RESERVED, 0xFC);
}

#[test]
fn absent_bootinfo_is_not_a_malformed_record() {
    assert_eq!(boot_protocol::load(0), Err(BootInfoError::Absent));
}

#[test]
fn kseg0_reader_stays_inside_the_512_mib_window() {
    let mut memory = boot_protocol::Kseg0Memory;
    let mut word = [0u8; 4];

    assert!(!memory.read_phys(boot_protocol::KSEG0_PHYS_LIMIT, &mut word));
    assert!(!memory.read_phys(boot_protocol::KSEG0_PHYS_LIMIT - 1, &mut word));
    assert!(!memory.read_phys(u64::MAX, &mut word));
}
