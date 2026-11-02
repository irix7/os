// SPDX-License-Identifier: GPL-3.0-only
//! Boot protocol (ROM <-> kernel) contract and conformance checks.
//!
//! This crate is the single implementation of the `BootInfo` binary contract
//! from [`docs/spec/boot-protocol.md`](https://github.com/irix7/os/blob/main/docs/spec/boot-protocol.md):
//! the 256-byte `#[repr(C, align(8))]` structure, its big-endian field
//! encoding, the CRC32C checksum, the bounded memory-map walk and the kernel's
//! validation order.
//!
//! The kernel links this crate and reads the structure through
//! [`load`]. The ROM is assembly and does not produce a `BootInfo`; when a
//! loader does, [`BootInfo::to_bytes`] is the producer-side encoding it must
//! match. The layout constants are tied to the struct with
//! [`core::mem::offset_of`] and checked against the specification table by
//! compile-time assertions and host tests.
//!
//! Only `core` is used outside the test harness, so the same code compiles in
//! the n32 kernel's `build-std` context and on the host.

#![cfg_attr(not(test), no_std)]

use core::mem::{offset_of, size_of};

/// BootInfo magic: `"I7BI"` stored big-endian = 0x4937_4249.
pub const BOOTINFO_MAGIC: u32 = 0x4937_4249;
/// BootInfo major version accepted by the kernel.
pub const BOOTINFO_VERSION_MAJOR: u16 = 1;
/// BootInfo size in bytes for version 1.
pub const BOOTINFO_SIZE: usize = 256;
/// Largest record a version-1 kernel will read. Minor versions may grow the
/// record at the end; the checksum covers the declared `size`.
pub const BOOTINFO_MAX_SIZE: usize = 4096;
/// Maximum number of memory-map entries.
pub const MEM_MAP_COUNT_MAX: u32 = 64;
/// Memory-map entry size in bytes.
pub const MEM_MAP_ENTRY_SIZE: u32 = 24;
/// Bytes in the largest possible memory map.
pub const MEM_MAP_MAX_BYTES: usize = MEM_MAP_COUNT_MAX as usize * MEM_MAP_ENTRY_SIZE as usize;
/// MIPS physical address space: `base + size` must not exceed this.
pub const PHYSICAL_LIMIT: u64 = 0x1_0000_0000;
/// Largest NUL-terminated string field the kernel will read.
pub const STRING_MAX: usize = 4096;

/// Memory-map kind: usable RAM.
pub const MEM_KIND_USABLE_RAM: u32 = 0;
/// Memory-map kind: reserved (not usable).
pub const MEM_KIND_RESERVED: u32 = 1;

/// The binary boot-info contract between the loader and the n32 kernel.
///
/// Field offsets match [`docs/spec/boot-protocol.md`](docs/spec/boot-protocol.md) §3 exactly
/// and are enforced by the `const` assertions below. The associated `OFF_*`
/// constants are derived from the struct with `offset_of!`, so the parser and
/// serialiser cannot silently use a different layout from the declared struct.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootInfo {
    pub magic: u32,              // 0x00
    pub version_major: u16,      // 0x04
    pub version_minor: u16,      // 0x06
    pub size: u32,               // 0x08
    pub checksum: u32,           // 0x0C
    pub mem_map_phys: u64,       // 0x10
    pub mem_map_count: u32,      // 0x18
    pub mem_map_entry_size: u32, // 0x1C
    pub kernel_phys: u64,        // 0x20
    pub kernel_size: u64,        // 0x28
    pub kernel_sha256: [u8; 32], // 0x30
    pub bundle_phys: u64,        // 0x50
    pub bundle_size: u64,        // 0x58
    pub bundle_sha256: [u8; 32], // 0x60
    pub cmdline_phys: u64,       // 0x80
    pub cmdline_len: u32,        // 0x88
    pub console_kind: u32,       // 0x8C
    pub console_phys: u64,       // 0x90
    pub console_width: u32,      // 0x98
    pub console_height: u32,     // 0x9C
    pub console_stride: u32,     // 0xA0
    pub console_bpp: u32,        // 0xA4
    pub console_baud: u32,       // 0xA8
    pub mac: [u8; 6],            // 0xAC
    pub pad0: u16,               // 0xB2
    pub ip: u32,                 // 0xB4
    pub netmask: u32,            // 0xB8
    pub gateway: u32,            // 0xBC
    pub dns: u32,                // 0xC0
    pub dhcp: u32,               // 0xC4
    pub rtc_valid: u32,          // 0xC8
    pub flags: u32,              // 0xCC
    pub r2_url_phys: u64,        // 0xD0
    pub r2_url_len: u32,         // 0xD8
    pub cpu_hz: u32,             // 0xDC
    pub ram_size: u64,           // 0xE0
    pub board_id: u32,           // 0xE8
    pub prom_version: u32,       // 0xEC
    pub console_poll: u32,       // 0xF0
    pub net_state: u32,          // 0xF4
    pub seeq_state: u32,         // 0xF8
    pub reserved: [u8; 4],       // 0xFC
} // 0x100 = 256 bytes

impl BootInfo {
    pub const OFF_MAGIC: usize = offset_of!(BootInfo, magic);
    pub const OFF_VERSION_MAJOR: usize = offset_of!(BootInfo, version_major);
    pub const OFF_VERSION_MINOR: usize = offset_of!(BootInfo, version_minor);
    pub const OFF_SIZE: usize = offset_of!(BootInfo, size);
    pub const OFF_CHECKSUM: usize = offset_of!(BootInfo, checksum);
    pub const OFF_MEM_MAP_PHYS: usize = offset_of!(BootInfo, mem_map_phys);
    pub const OFF_MEM_MAP_COUNT: usize = offset_of!(BootInfo, mem_map_count);
    pub const OFF_MEM_MAP_ENTRY_SIZE: usize = offset_of!(BootInfo, mem_map_entry_size);
    pub const OFF_KERNEL_PHYS: usize = offset_of!(BootInfo, kernel_phys);
    pub const OFF_KERNEL_SIZE: usize = offset_of!(BootInfo, kernel_size);
    pub const OFF_KERNEL_SHA256: usize = offset_of!(BootInfo, kernel_sha256);
    pub const OFF_BUNDLE_PHYS: usize = offset_of!(BootInfo, bundle_phys);
    pub const OFF_BUNDLE_SIZE: usize = offset_of!(BootInfo, bundle_size);
    pub const OFF_BUNDLE_SHA256: usize = offset_of!(BootInfo, bundle_sha256);
    pub const OFF_CMDLINE_PHYS: usize = offset_of!(BootInfo, cmdline_phys);
    pub const OFF_CMDLINE_LEN: usize = offset_of!(BootInfo, cmdline_len);
    pub const OFF_CONSOLE_KIND: usize = offset_of!(BootInfo, console_kind);
    pub const OFF_CONSOLE_PHYS: usize = offset_of!(BootInfo, console_phys);
    pub const OFF_CONSOLE_WIDTH: usize = offset_of!(BootInfo, console_width);
    pub const OFF_CONSOLE_HEIGHT: usize = offset_of!(BootInfo, console_height);
    pub const OFF_CONSOLE_STRIDE: usize = offset_of!(BootInfo, console_stride);
    pub const OFF_CONSOLE_BPP: usize = offset_of!(BootInfo, console_bpp);
    pub const OFF_CONSOLE_BAUD: usize = offset_of!(BootInfo, console_baud);
    pub const OFF_MAC: usize = offset_of!(BootInfo, mac);
    pub const OFF_PAD0: usize = offset_of!(BootInfo, pad0);
    pub const OFF_IP: usize = offset_of!(BootInfo, ip);
    pub const OFF_NETMASK: usize = offset_of!(BootInfo, netmask);
    pub const OFF_GATEWAY: usize = offset_of!(BootInfo, gateway);
    pub const OFF_DNS: usize = offset_of!(BootInfo, dns);
    pub const OFF_DHCP: usize = offset_of!(BootInfo, dhcp);
    pub const OFF_RTC_VALID: usize = offset_of!(BootInfo, rtc_valid);
    pub const OFF_FLAGS: usize = offset_of!(BootInfo, flags);
    pub const OFF_R2_URL_PHYS: usize = offset_of!(BootInfo, r2_url_phys);
    pub const OFF_R2_URL_LEN: usize = offset_of!(BootInfo, r2_url_len);
    pub const OFF_CPU_HZ: usize = offset_of!(BootInfo, cpu_hz);
    pub const OFF_RAM_SIZE: usize = offset_of!(BootInfo, ram_size);
    pub const OFF_BOARD_ID: usize = offset_of!(BootInfo, board_id);
    pub const OFF_PROM_VERSION: usize = offset_of!(BootInfo, prom_version);
    pub const OFF_CONSOLE_POLL: usize = offset_of!(BootInfo, console_poll);
    pub const OFF_NET_STATE: usize = offset_of!(BootInfo, net_state);
    pub const OFF_SEEQ_STATE: usize = offset_of!(BootInfo, seeq_state);
    pub const OFF_RESERVED: usize = offset_of!(BootInfo, reserved);

    /// Decode a big-endian `BootInfo` record and run the header checks.
    ///
    /// The checks are, in order: magic, major version, size (256..=4096 and
    /// fully present), CRC32C over `bytes[..size]` with the checksum field
    /// zeroed, `mem_map_count <= 64`, and `mem_map_entry_size == 24`. The map
    /// contents are checked by [`MemMap::parse`] / [`load`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BootInfoError> {
        if bytes.len() < BOOTINFO_SIZE {
            return Err(BootInfoError::BadSize);
        }
        if get_u32(bytes, Self::OFF_MAGIC) != BOOTINFO_MAGIC {
            return Err(BootInfoError::BadMagic);
        }
        if get_u16(bytes, Self::OFF_VERSION_MAJOR) != BOOTINFO_VERSION_MAJOR {
            return Err(BootInfoError::BadMajorVersion);
        }
        let size = get_u32(bytes, Self::OFF_SIZE) as usize;
        if !(BOOTINFO_SIZE..=BOOTINFO_MAX_SIZE).contains(&size) || bytes.len() < size {
            return Err(BootInfoError::BadSize);
        }
        if bootinfo_checksum_bytes(&bytes[..size]) != get_u32(bytes, Self::OFF_CHECKSUM) {
            return Err(BootInfoError::BadChecksum);
        }
        let mem_map_count = get_u32(bytes, Self::OFF_MEM_MAP_COUNT);
        if mem_map_count > MEM_MAP_COUNT_MAX {
            return Err(BootInfoError::MemMapCountTooHigh);
        }
        if get_u32(bytes, Self::OFF_MEM_MAP_ENTRY_SIZE) != MEM_MAP_ENTRY_SIZE {
            return Err(BootInfoError::BadMemMapEntrySize);
        }

        Ok(Self {
            magic: get_u32(bytes, Self::OFF_MAGIC),
            version_major: get_u16(bytes, Self::OFF_VERSION_MAJOR),
            version_minor: get_u16(bytes, Self::OFF_VERSION_MINOR),
            size: size as u32,
            checksum: get_u32(bytes, Self::OFF_CHECKSUM),
            mem_map_phys: get_u64(bytes, Self::OFF_MEM_MAP_PHYS),
            mem_map_count,
            mem_map_entry_size: get_u32(bytes, Self::OFF_MEM_MAP_ENTRY_SIZE),
            kernel_phys: get_u64(bytes, Self::OFF_KERNEL_PHYS),
            kernel_size: get_u64(bytes, Self::OFF_KERNEL_SIZE),
            kernel_sha256: get_array(bytes, Self::OFF_KERNEL_SHA256),
            bundle_phys: get_u64(bytes, Self::OFF_BUNDLE_PHYS),
            bundle_size: get_u64(bytes, Self::OFF_BUNDLE_SIZE),
            bundle_sha256: get_array(bytes, Self::OFF_BUNDLE_SHA256),
            cmdline_phys: get_u64(bytes, Self::OFF_CMDLINE_PHYS),
            cmdline_len: get_u32(bytes, Self::OFF_CMDLINE_LEN),
            console_kind: get_u32(bytes, Self::OFF_CONSOLE_KIND),
            console_phys: get_u64(bytes, Self::OFF_CONSOLE_PHYS),
            console_width: get_u32(bytes, Self::OFF_CONSOLE_WIDTH),
            console_height: get_u32(bytes, Self::OFF_CONSOLE_HEIGHT),
            console_stride: get_u32(bytes, Self::OFF_CONSOLE_STRIDE),
            console_bpp: get_u32(bytes, Self::OFF_CONSOLE_BPP),
            console_baud: get_u32(bytes, Self::OFF_CONSOLE_BAUD),
            mac: get_array(bytes, Self::OFF_MAC),
            pad0: get_u16(bytes, Self::OFF_PAD0),
            ip: get_u32(bytes, Self::OFF_IP),
            netmask: get_u32(bytes, Self::OFF_NETMASK),
            gateway: get_u32(bytes, Self::OFF_GATEWAY),
            dns: get_u32(bytes, Self::OFF_DNS),
            dhcp: get_u32(bytes, Self::OFF_DHCP),
            rtc_valid: get_u32(bytes, Self::OFF_RTC_VALID),
            flags: get_u32(bytes, Self::OFF_FLAGS),
            r2_url_phys: get_u64(bytes, Self::OFF_R2_URL_PHYS),
            r2_url_len: get_u32(bytes, Self::OFF_R2_URL_LEN),
            cpu_hz: get_u32(bytes, Self::OFF_CPU_HZ),
            ram_size: get_u64(bytes, Self::OFF_RAM_SIZE),
            board_id: get_u32(bytes, Self::OFF_BOARD_ID),
            prom_version: get_u32(bytes, Self::OFF_PROM_VERSION),
            console_poll: get_u32(bytes, Self::OFF_CONSOLE_POLL),
            net_state: get_u32(bytes, Self::OFF_NET_STATE),
            seeq_state: get_u32(bytes, Self::OFF_SEEQ_STATE),
            reserved: get_array(bytes, Self::OFF_RESERVED),
        })
    }

    /// Encode the version-1 record as big-endian bytes.
    ///
    /// The checksum field is written as stored; call [`Self::seal`] before
    /// serialising a producer-owned record, or use [`bootinfo_checksum`].
    pub fn to_bytes(&self) -> [u8; BOOTINFO_SIZE] {
        let mut out = [0u8; BOOTINFO_SIZE];
        put_u32(&mut out, Self::OFF_MAGIC, self.magic);
        put_u16(&mut out, Self::OFF_VERSION_MAJOR, self.version_major);
        put_u16(&mut out, Self::OFF_VERSION_MINOR, self.version_minor);
        put_u32(&mut out, Self::OFF_SIZE, self.size);
        put_u32(&mut out, Self::OFF_CHECKSUM, self.checksum);
        put_u64(&mut out, Self::OFF_MEM_MAP_PHYS, self.mem_map_phys);
        put_u32(&mut out, Self::OFF_MEM_MAP_COUNT, self.mem_map_count);
        put_u32(
            &mut out,
            Self::OFF_MEM_MAP_ENTRY_SIZE,
            self.mem_map_entry_size,
        );
        put_u64(&mut out, Self::OFF_KERNEL_PHYS, self.kernel_phys);
        put_u64(&mut out, Self::OFF_KERNEL_SIZE, self.kernel_size);
        put_bytes(&mut out, Self::OFF_KERNEL_SHA256, &self.kernel_sha256);
        put_u64(&mut out, Self::OFF_BUNDLE_PHYS, self.bundle_phys);
        put_u64(&mut out, Self::OFF_BUNDLE_SIZE, self.bundle_size);
        put_bytes(&mut out, Self::OFF_BUNDLE_SHA256, &self.bundle_sha256);
        put_u64(&mut out, Self::OFF_CMDLINE_PHYS, self.cmdline_phys);
        put_u32(&mut out, Self::OFF_CMDLINE_LEN, self.cmdline_len);
        put_u32(&mut out, Self::OFF_CONSOLE_KIND, self.console_kind);
        put_u64(&mut out, Self::OFF_CONSOLE_PHYS, self.console_phys);
        put_u32(&mut out, Self::OFF_CONSOLE_WIDTH, self.console_width);
        put_u32(&mut out, Self::OFF_CONSOLE_HEIGHT, self.console_height);
        put_u32(&mut out, Self::OFF_CONSOLE_STRIDE, self.console_stride);
        put_u32(&mut out, Self::OFF_CONSOLE_BPP, self.console_bpp);
        put_u32(&mut out, Self::OFF_CONSOLE_BAUD, self.console_baud);
        put_bytes(&mut out, Self::OFF_MAC, &self.mac);
        put_u16(&mut out, Self::OFF_PAD0, self.pad0);
        put_u32(&mut out, Self::OFF_IP, self.ip);
        put_u32(&mut out, Self::OFF_NETMASK, self.netmask);
        put_u32(&mut out, Self::OFF_GATEWAY, self.gateway);
        put_u32(&mut out, Self::OFF_DNS, self.dns);
        put_u32(&mut out, Self::OFF_DHCP, self.dhcp);
        put_u32(&mut out, Self::OFF_RTC_VALID, self.rtc_valid);
        put_u32(&mut out, Self::OFF_FLAGS, self.flags);
        put_u64(&mut out, Self::OFF_R2_URL_PHYS, self.r2_url_phys);
        put_u32(&mut out, Self::OFF_R2_URL_LEN, self.r2_url_len);
        put_u32(&mut out, Self::OFF_CPU_HZ, self.cpu_hz);
        put_u64(&mut out, Self::OFF_RAM_SIZE, self.ram_size);
        put_u32(&mut out, Self::OFF_BOARD_ID, self.board_id);
        put_u32(&mut out, Self::OFF_PROM_VERSION, self.prom_version);
        put_u32(&mut out, Self::OFF_CONSOLE_POLL, self.console_poll);
        put_u32(&mut out, Self::OFF_NET_STATE, self.net_state);
        put_u32(&mut out, Self::OFF_SEEQ_STATE, self.seeq_state);
        put_bytes(&mut out, Self::OFF_RESERVED, &self.reserved);
        out
    }

    /// Fill in the checksum for the version-1 encoding.
    pub fn seal(&mut self) {
        self.checksum = bootinfo_checksum(self);
    }

    /// Validate this record's own header and checksum.
    pub fn validate(&self) -> Result<(), BootInfoError> {
        Self::from_bytes(&self.to_bytes()).map(|_| ())
    }
}

// Compile-time layout assertions mirroring the spec's offset table.
const _: () = {
    assert!(size_of::<BootInfo>() == BOOTINFO_SIZE);
    assert!(align_of::<BootInfo>() == 8);
    assert!(BootInfo::OFF_MAGIC == 0x00);
    assert!(BootInfo::OFF_VERSION_MAJOR == 0x04);
    assert!(BootInfo::OFF_VERSION_MINOR == 0x06);
    assert!(BootInfo::OFF_SIZE == 0x08);
    assert!(BootInfo::OFF_CHECKSUM == 0x0C);
    assert!(BootInfo::OFF_MEM_MAP_PHYS == 0x10);
    assert!(BootInfo::OFF_MEM_MAP_COUNT == 0x18);
    assert!(BootInfo::OFF_MEM_MAP_ENTRY_SIZE == 0x1C);
    assert!(BootInfo::OFF_KERNEL_PHYS == 0x20);
    assert!(BootInfo::OFF_KERNEL_SIZE == 0x28);
    assert!(BootInfo::OFF_KERNEL_SHA256 == 0x30);
    assert!(BootInfo::OFF_BUNDLE_PHYS == 0x50);
    assert!(BootInfo::OFF_BUNDLE_SIZE == 0x58);
    assert!(BootInfo::OFF_BUNDLE_SHA256 == 0x60);
    assert!(BootInfo::OFF_CMDLINE_PHYS == 0x80);
    assert!(BootInfo::OFF_CMDLINE_LEN == 0x88);
    assert!(BootInfo::OFF_CONSOLE_KIND == 0x8C);
    assert!(BootInfo::OFF_CONSOLE_PHYS == 0x90);
    assert!(BootInfo::OFF_CONSOLE_WIDTH == 0x98);
    assert!(BootInfo::OFF_CONSOLE_HEIGHT == 0x9C);
    assert!(BootInfo::OFF_CONSOLE_STRIDE == 0xA0);
    assert!(BootInfo::OFF_CONSOLE_BPP == 0xA4);
    assert!(BootInfo::OFF_CONSOLE_BAUD == 0xA8);
    assert!(BootInfo::OFF_MAC == 0xAC);
    assert!(BootInfo::OFF_PAD0 == 0xB2);
    assert!(BootInfo::OFF_IP == 0xB4);
    assert!(BootInfo::OFF_NETMASK == 0xB8);
    assert!(BootInfo::OFF_GATEWAY == 0xBC);
    assert!(BootInfo::OFF_DNS == 0xC0);
    assert!(BootInfo::OFF_DHCP == 0xC4);
    assert!(BootInfo::OFF_RTC_VALID == 0xC8);
    assert!(BootInfo::OFF_FLAGS == 0xCC);
    assert!(BootInfo::OFF_R2_URL_PHYS == 0xD0);
    assert!(BootInfo::OFF_R2_URL_LEN == 0xD8);
    assert!(BootInfo::OFF_CPU_HZ == 0xDC);
    assert!(BootInfo::OFF_RAM_SIZE == 0xE0);
    assert!(BootInfo::OFF_BOARD_ID == 0xE8);
    assert!(BootInfo::OFF_PROM_VERSION == 0xEC);
    assert!(BootInfo::OFF_CONSOLE_POLL == 0xF0);
    assert!(BootInfo::OFF_NET_STATE == 0xF4);
    assert!(BootInfo::OFF_SEEQ_STATE == 0xF8);
    assert!(BootInfo::OFF_RESERVED == 0xFC);
};

use core::mem::align_of;

/// One memory-map entry. Its 24-byte layout is part of the contract.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemMapEntry {
    pub base: u64,
    pub size: u64,
    pub kind: u32,
    pub flags: u32,
}

impl MemMapEntry {
    pub const EMPTY: Self = Self {
        base: 0,
        size: 0,
        kind: 0,
        flags: 0,
    };

    pub const OFF_BASE: usize = offset_of!(MemMapEntry, base);
    pub const OFF_SIZE: usize = offset_of!(MemMapEntry, size);
    pub const OFF_KIND: usize = offset_of!(MemMapEntry, kind);
    pub const OFF_FLAGS: usize = offset_of!(MemMapEntry, flags);

    /// Decode one big-endian entry.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BootInfoError> {
        if bytes.len() < MEM_MAP_ENTRY_SIZE as usize {
            return Err(BootInfoError::MemMapTruncated);
        }
        Ok(Self {
            base: get_u64(bytes, Self::OFF_BASE),
            size: get_u64(bytes, Self::OFF_SIZE),
            kind: get_u32(bytes, Self::OFF_KIND),
            flags: get_u32(bytes, Self::OFF_FLAGS),
        })
    }

    /// Encode one big-endian entry.
    pub fn to_bytes(&self) -> [u8; MEM_MAP_ENTRY_SIZE as usize] {
        let mut out = [0u8; MEM_MAP_ENTRY_SIZE as usize];
        put_u64(&mut out, Self::OFF_BASE, self.base);
        put_u64(&mut out, Self::OFF_SIZE, self.size);
        put_u32(&mut out, Self::OFF_KIND, self.kind);
        put_u32(&mut out, Self::OFF_FLAGS, self.flags);
        out
    }
}

const _: () = {
    assert!(size_of::<MemMapEntry>() == MEM_MAP_ENTRY_SIZE as usize);
    assert!(MemMapEntry::OFF_BASE == 0x00);
    assert!(MemMapEntry::OFF_SIZE == 0x08);
    assert!(MemMapEntry::OFF_KIND == 0x10);
    assert!(MemMapEntry::OFF_FLAGS == 0x14);
};

/// A bounds-checked memory map copied out of the boot record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemMap {
    entries: [MemMapEntry; MEM_MAP_COUNT_MAX as usize],
    len: usize,
}

impl MemMap {
    pub const fn empty() -> Self {
        Self {
            entries: [MemMapEntry::EMPTY; MEM_MAP_COUNT_MAX as usize],
            len: 0,
        }
    }

    /// Parse a map. `bytes` must contain exactly the declared entries (or
    /// more); every entry is decoded big-endian, checked against the physical
    /// address limit, and checked against every other entry for overlap.
    pub fn parse(bytes: &[u8], count: u32, entry_size: u32) -> Result<Self, BootInfoError> {
        if count > MEM_MAP_COUNT_MAX {
            return Err(BootInfoError::MemMapCountTooHigh);
        }
        if entry_size != MEM_MAP_ENTRY_SIZE {
            return Err(BootInfoError::BadMemMapEntrySize);
        }
        let needed = count as usize * MEM_MAP_ENTRY_SIZE as usize;
        if bytes.len() < needed {
            return Err(BootInfoError::MemMapTruncated);
        }

        let mut map = Self::empty();
        for index in 0..count as usize {
            let start = index * MEM_MAP_ENTRY_SIZE as usize;
            map.entries[index] =
                MemMapEntry::from_bytes(&bytes[start..start + MEM_MAP_ENTRY_SIZE as usize])?;
            map.len += 1;
        }
        map.check_bounds()?;
        map.check_overlap()?;
        Ok(map)
    }

    pub fn entries(&self) -> &[MemMapEntry] {
        &self.entries[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Usable RAM regions, in map order. The caller sorts or clips them.
    pub fn usable_ram(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.entries()
            .iter()
            .filter(|entry| entry.kind == MEM_KIND_USABLE_RAM)
            .map(|entry| (entry.base, entry.size))
    }

    fn check_bounds(&self) -> Result<(), BootInfoError> {
        for entry in self.entries() {
            let end = entry
                .base
                .checked_add(entry.size)
                .ok_or(BootInfoError::MapOutOfBounds)?;
            if end > PHYSICAL_LIMIT {
                return Err(BootInfoError::MapOutOfBounds);
            }
        }
        Ok(())
    }

    fn check_overlap(&self) -> Result<(), BootInfoError> {
        let entries = self.entries();
        for index in 0..entries.len() {
            for other in (index + 1)..entries.len() {
                let a = entries[index];
                let b = entries[other];
                if ranges_overlap(a.base, a.size, b.base, b.size) {
                    return Err(BootInfoError::MapOverlap);
                }
            }
        }
        Ok(())
    }
}

/// Physical-memory reader used by [`load`].
///
/// The kernel supplies a kseg0 reader; host tests supply a flat memory image.
/// A reader returns `false` when the whole range is not readable.
pub trait PhysMemory {
    fn read_phys(&mut self, phys: u64, out: &mut [u8]) -> bool;
}

/// The validated boot record and the map copied out of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedBootInfo {
    pub info: BootInfo,
    pub map: MemMap,
}

/// Read, decode and validate a `BootInfo` record at `bootinfo_phys`.
///
/// This is the live kernel parser. It uses the declared `size` to bound the
/// checksum, reads exactly the declared number of map entries, and never
/// indexes the 256-byte header with a physical address. Validation order:
///
/// 1. non-zero physical address, readable record;
/// 2. magic;
/// 3. major version;
/// 4. size and CRC32C;
/// 5. memory-map count;
/// 6. memory-map entry size;
/// 7. every map entry within the 4 GiB physical space;
/// 8. no two map entries overlap, and the kernel/bundle ranges are usable,
///    within bounds, and do not overlap each other or the boot record.
pub fn load(
    bootinfo_phys: u64,
    memory: &mut impl PhysMemory,
) -> Result<LoadedBootInfo, BootInfoError> {
    if bootinfo_phys == 0 {
        return Err(BootInfoError::Absent);
    }

    let mut header = [0u8; BOOTINFO_SIZE];
    if !memory.read_phys(bootinfo_phys, &mut header) {
        return Err(BootInfoError::Unreadable);
    }

    // Check the fixed-position fields before trusting the declared length, so
    // a corrupt record reports its real defect rather than an incidental size
    // error from bytes that are not a BootInfo at all.
    if get_u32(&header, BootInfo::OFF_MAGIC) != BOOTINFO_MAGIC {
        return Err(BootInfoError::BadMagic);
    }
    if get_u16(&header, BootInfo::OFF_VERSION_MAJOR) != BOOTINFO_VERSION_MAJOR {
        return Err(BootInfoError::BadMajorVersion);
    }

    // Read the declared record length before trusting it. Version 1 is 256
    // bytes; a minor version may append fields, and the checksum covers them.
    let size = get_u32(&header, BootInfo::OFF_SIZE) as usize;
    if !(BOOTINFO_SIZE..=BOOTINFO_MAX_SIZE).contains(&size) {
        return Err(BootInfoError::BadSize);
    }
    if bootinfo_phys
        .checked_add(size as u64)
        .is_none_or(|end| end > PHYSICAL_LIMIT)
    {
        return Err(BootInfoError::BadSize);
    }

    let mut record = [0u8; BOOTINFO_MAX_SIZE];
    record[..BOOTINFO_SIZE].copy_from_slice(&header);
    if size > BOOTINFO_SIZE
        && !memory.read_phys(
            bootinfo_phys + BOOTINFO_SIZE as u64,
            &mut record[BOOTINFO_SIZE..size],
        )
    {
        return Err(BootInfoError::Unreadable);
    }
    let info = BootInfo::from_bytes(&record[..size])?;

    let needed = info.mem_map_count as usize * MEM_MAP_ENTRY_SIZE as usize;
    let mut map_bytes = [0u8; MEM_MAP_MAX_BYTES];
    if needed > 0 {
        let end = info
            .mem_map_phys
            .checked_add(needed as u64)
            .ok_or(BootInfoError::MapOutOfBounds)?;
        if info.mem_map_phys == 0 || end > PHYSICAL_LIMIT {
            return Err(BootInfoError::MapOutOfBounds);
        }
        if !memory.read_phys(info.mem_map_phys, &mut map_bytes[..needed]) {
            return Err(BootInfoError::Unreadable);
        }
    }
    let map = MemMap::parse(
        &map_bytes[..needed],
        info.mem_map_count,
        info.mem_map_entry_size,
    )?;

    validate_image(
        info.kernel_phys,
        info.kernel_size,
        &map,
        bootinfo_phys,
        size as u64,
        BootInfoError::KernelOutsideRam,
    )?;
    validate_image(
        info.bundle_phys,
        info.bundle_size,
        &map,
        bootinfo_phys,
        size as u64,
        BootInfoError::BundleOutsideRam,
    )?;
    if ranges_overlap(
        info.kernel_phys,
        info.kernel_size,
        info.bundle_phys,
        info.bundle_size,
    ) {
        return Err(BootInfoError::ImageOverlap);
    }

    validate_string(info.cmdline_phys, info.cmdline_len, memory)?;
    validate_string(info.r2_url_phys, info.r2_url_len, memory)?;

    Ok(LoadedBootInfo { info, map })
}

/// Error returned by [`BootInfo::from_bytes`], [`MemMap::parse`] and [`load`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootInfoError {
    /// `$a0` was zero: the loader did not produce a record.
    Absent,
    /// The record or map could not be read through the physical window.
    Unreadable,
    BadMagic,
    BadMajorVersion,
    BadSize,
    BadChecksum,
    MemMapCountTooHigh,
    BadMemMapEntrySize,
    MemMapTruncated,
    MapOutOfBounds,
    MapOverlap,
    KernelOutsideRam,
    BundleOutsideRam,
    ImageOverlap,
    BadString,
}

impl BootInfoError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Unreadable => "unreadable",
            Self::BadMagic => "bad magic",
            Self::BadMajorVersion => "bad major version",
            Self::BadSize => "bad size",
            Self::BadChecksum => "bad CRC32C",
            Self::MemMapCountTooHigh => "map count too high",
            Self::BadMemMapEntrySize => "bad map entry size",
            Self::MemMapTruncated => "truncated map",
            Self::MapOutOfBounds => "map entry out of bounds",
            Self::MapOverlap => "overlapping map entries",
            Self::KernelOutsideRam => "kernel outside usable RAM",
            Self::BundleOutsideRam => "bundle outside usable RAM",
            Self::ImageOverlap => "kernel/bundle overlap",
            Self::BadString => "unterminated string",
        }
    }
}

/// CRC32C (Castagnoli, reflected, poly 0x1EDC6F41, init/xorout 0xFFFFFFFF).
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0x82F6_3B78 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// CRC32C over a BootInfo record with its checksum field zeroed.
pub fn bootinfo_checksum_bytes(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for (index, &byte) in bytes.iter().enumerate() {
        let byte = if (BootInfo::OFF_CHECKSUM..BootInfo::OFF_CHECKSUM + 4).contains(&index) {
            0
        } else {
            byte
        };
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0x82F6_3B78 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Compute the checksum field value for a `BootInfo`.
pub fn bootinfo_checksum(info: &BootInfo) -> u32 {
    bootinfo_checksum_bytes(&info.to_bytes())
}

/// Build a minimal valid version-1 `BootInfo` with the checksum filled in.
pub fn valid_bootinfo() -> BootInfo {
    let mut info = BootInfo {
        magic: BOOTINFO_MAGIC,
        version_major: BOOTINFO_VERSION_MAJOR,
        size: BOOTINFO_SIZE as u32,
        mem_map_phys: 0x0800_2000,
        mem_map_count: 1,
        mem_map_entry_size: MEM_MAP_ENTRY_SIZE,
        kernel_phys: 0x0804_0000,
        kernel_size: 0x0040_0000,
        bundle_phys: 0x0844_0000,
        bundle_size: 0x0100_0000,
        console_kind: 1, // UART
        ..BootInfo::default()
    };
    info.seal();
    info
}

fn validate_image(
    base: u64,
    size: u64,
    map: &MemMap,
    bootinfo_phys: u64,
    bootinfo_size: u64,
    outside: BootInfoError,
) -> Result<(), BootInfoError> {
    if size == 0 {
        return Ok(());
    }
    let end = base
        .checked_add(size)
        .ok_or(BootInfoError::MapOutOfBounds)?;
    if end > PHYSICAL_LIMIT {
        return Err(outside);
    }
    if ranges_overlap(base, size, bootinfo_phys, bootinfo_size) {
        return Err(BootInfoError::ImageOverlap);
    }
    if !in_usable_ram(map, base, end) {
        return Err(outside);
    }
    Ok(())
}

fn in_usable_ram(map: &MemMap, base: u64, end: u64) -> bool {
    map.entries().iter().any(|entry| {
        entry.kind == MEM_KIND_USABLE_RAM
            && base >= entry.base
            && end <= entry.base.saturating_add(entry.size)
    })
}

fn validate_string(phys: u64, len: u32, memory: &mut impl PhysMemory) -> Result<(), BootInfoError> {
    if len == 0 {
        return Ok(());
    }
    let len = len as usize;
    if phys == 0 || len > STRING_MAX {
        return Err(BootInfoError::BadString);
    }
    if phys
        .checked_add(len as u64)
        .is_none_or(|end| end > PHYSICAL_LIMIT)
    {
        return Err(BootInfoError::BadString);
    }
    let mut buffer = [0u8; STRING_MAX];
    if !memory.read_phys(phys, &mut buffer[..len]) {
        return Err(BootInfoError::BadString);
    }
    if buffer[len - 1] != 0 || buffer[..len - 1].contains(&0) {
        return Err(BootInfoError::BadString);
    }
    Ok(())
}

fn ranges_overlap(a_base: u64, a_size: u64, b_base: u64, b_size: u64) -> bool {
    if a_size == 0 || b_size == 0 {
        return false;
    }
    let a_end = a_base.saturating_add(a_size);
    let b_end = b_base.saturating_add(b_size);
    a_base < b_end && b_base < a_end
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn get_array<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[offset..offset + N]);
    out
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn put_bytes(bytes: &mut [u8], offset: usize, value: &[u8]) {
    bytes[offset..offset + value.len()].copy_from_slice(value);
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const BOOTINFO_PHYS: u64 = 0x0800_1000;
    const MAP_PHYS: u64 = 0x0800_2000;
    const RAM_BASE: u64 = 0x0800_0000;
    const RAM_SIZE: usize = 0x0200_0000; // 32 MiB

    struct TestMemory {
        base: u64,
        bytes: Vec<u8>,
    }

    impl TestMemory {
        fn new() -> Self {
            Self {
                base: RAM_BASE,
                bytes: std::vec![0u8; RAM_SIZE],
            }
        }

        fn write(&mut self, phys: u64, bytes: &[u8]) {
            assert!(phys >= self.base);
            let offset = (phys - self.base) as usize;
            self.bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl PhysMemory for TestMemory {
        fn read_phys(&mut self, phys: u64, out: &mut [u8]) -> bool {
            if phys < self.base {
                return false;
            }
            let offset = (phys - self.base) as usize;
            if offset + out.len() > self.bytes.len() {
                return false;
            }
            out.copy_from_slice(&self.bytes[offset..offset + out.len()]);
            true
        }
    }

    fn ram_entry() -> MemMapEntry {
        MemMapEntry {
            base: RAM_BASE,
            size: RAM_SIZE as u64,
            kind: MEM_KIND_USABLE_RAM,
            flags: 0,
        }
    }

    fn fixture(entries: &[MemMapEntry]) -> BootInfo {
        let mut info = valid_bootinfo();
        info.mem_map_phys = MAP_PHYS;
        info.mem_map_count = entries.len() as u32;
        info.mem_map_entry_size = MEM_MAP_ENTRY_SIZE;
        info.seal();
        info
    }

    fn load_fixture(
        info: &BootInfo,
        entries: &[MemMapEntry],
    ) -> Result<LoadedBootInfo, BootInfoError> {
        let mut memory = TestMemory::new();
        memory.write(BOOTINFO_PHYS, &info.to_bytes());
        let mut map = [0u8; MEM_MAP_MAX_BYTES];
        for (index, entry) in entries.iter().enumerate() {
            let start = index * MEM_MAP_ENTRY_SIZE as usize;
            map[start..start + MEM_MAP_ENTRY_SIZE as usize].copy_from_slice(&entry.to_bytes());
        }
        if !entries.is_empty() {
            memory.write(
                MAP_PHYS,
                &map[..entries.len() * MEM_MAP_ENTRY_SIZE as usize],
            );
        }
        load(BOOTINFO_PHYS, &mut memory)
    }

    #[test]
    fn size_and_offsets_match_spec() {
        assert_eq!(size_of::<BootInfo>(), BOOTINFO_SIZE);
        assert_eq!(BootInfo::OFF_MAGIC, 0x00);
        assert_eq!(BootInfo::OFF_VERSION_MAJOR, 0x04);
        assert_eq!(BootInfo::OFF_SIZE, 0x08);
        assert_eq!(BootInfo::OFF_CHECKSUM, 0x0C);
        assert_eq!(BootInfo::OFF_MEM_MAP_PHYS, 0x10);
        assert_eq!(BootInfo::OFF_MEM_MAP_COUNT, 0x18);
        assert_eq!(BootInfo::OFF_MEM_MAP_ENTRY_SIZE, 0x1C);
        assert_eq!(BootInfo::OFF_KERNEL_PHYS, 0x20);
        assert_eq!(BootInfo::OFF_KERNEL_SIZE, 0x28);
        assert_eq!(BootInfo::OFF_KERNEL_SHA256, 0x30);
        assert_eq!(BootInfo::OFF_BUNDLE_PHYS, 0x50);
        assert_eq!(BootInfo::OFF_BUNDLE_SIZE, 0x58);
        assert_eq!(BootInfo::OFF_BUNDLE_SHA256, 0x60);
        assert_eq!(BootInfo::OFF_CMDLINE_PHYS, 0x80);
        assert_eq!(BootInfo::OFF_CMDLINE_LEN, 0x88);
        assert_eq!(BootInfo::OFF_CONSOLE_KIND, 0x8C);
        assert_eq!(BootInfo::OFF_CONSOLE_PHYS, 0x90);
        assert_eq!(BootInfo::OFF_CONSOLE_WIDTH, 0x98);
        assert_eq!(BootInfo::OFF_CONSOLE_HEIGHT, 0x9C);
        assert_eq!(BootInfo::OFF_CONSOLE_STRIDE, 0xA0);
        assert_eq!(BootInfo::OFF_CONSOLE_BPP, 0xA4);
        assert_eq!(BootInfo::OFF_CONSOLE_BAUD, 0xA8);
        assert_eq!(BootInfo::OFF_MAC, 0xAC);
        assert_eq!(BootInfo::OFF_PAD0, 0xB2);
        assert_eq!(BootInfo::OFF_IP, 0xB4);
        assert_eq!(BootInfo::OFF_NETMASK, 0xB8);
        assert_eq!(BootInfo::OFF_GATEWAY, 0xBC);
        assert_eq!(BootInfo::OFF_DNS, 0xC0);
        assert_eq!(BootInfo::OFF_DHCP, 0xC4);
        assert_eq!(BootInfo::OFF_RTC_VALID, 0xC8);
        assert_eq!(BootInfo::OFF_FLAGS, 0xCC);
        assert_eq!(BootInfo::OFF_R2_URL_PHYS, 0xD0);
        assert_eq!(BootInfo::OFF_R2_URL_LEN, 0xD8);
        assert_eq!(BootInfo::OFF_CPU_HZ, 0xDC);
        assert_eq!(BootInfo::OFF_RAM_SIZE, 0xE0);
        assert_eq!(BootInfo::OFF_BOARD_ID, 0xE8);
        assert_eq!(BootInfo::OFF_PROM_VERSION, 0xEC);
        assert_eq!(BootInfo::OFF_CONSOLE_POLL, 0xF0);
        assert_eq!(BootInfo::OFF_NET_STATE, 0xF4);
        assert_eq!(BootInfo::OFF_SEEQ_STATE, 0xF8);
        assert_eq!(BootInfo::OFF_RESERVED, 0xFC);
        assert_eq!(size_of::<MemMapEntry>(), MEM_MAP_ENTRY_SIZE as usize);
    }

    #[test]
    fn encoding_is_big_endian() {
        let info = valid_bootinfo();
        let bytes = info.to_bytes();
        assert_eq!(&bytes[0x00..0x04], &[0x49, 0x37, 0x42, 0x49]);
        assert_eq!(&bytes[0x04..0x06], &[0x00, 0x01]);
        assert_eq!(&bytes[0x08..0x0C], &[0x00, 0x00, 0x01, 0x00]);
        assert_eq!(get_u32(&bytes, BootInfo::OFF_MAGIC), BOOTINFO_MAGIC);
    }

    #[test]
    fn roundtrip_through_bytes() {
        let info = valid_bootinfo();
        let bytes = info.to_bytes();
        assert_eq!(BootInfo::from_bytes(&bytes), Ok(info));
    }

    #[test]
    fn crc32c_known_answer() {
        // CRC32C (Castagnoli) of "123456789" = 0xE306_9283.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0x0000_0000);
    }

    #[test]
    fn valid_bootinfo_loads() {
        let info = fixture(&[ram_entry()]);
        let loaded = load_fixture(&info, &[ram_entry()]).expect("valid BootInfo");
        assert_eq!(loaded.info, info);
        assert_eq!(loaded.map.len(), 1);
        assert_eq!(loaded.map.entries()[0], ram_entry());
        assert_eq!(
            loaded.map.usable_ram().collect::<Vec<_>>(),
            vec![(RAM_BASE, RAM_SIZE as u64)]
        );
    }

    #[test]
    fn corrupt_magic_rejected() {
        let mut info = valid_bootinfo();
        info.magic = 0xDEAD_BEEF;
        info.seal();
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::BadMagic)
        );
    }

    #[test]
    fn little_endian_magic_rejected() {
        let info = valid_bootinfo();
        let mut bytes = info.to_bytes();
        bytes[0..4].copy_from_slice(&[0x49, 0x42, 0x37, 0x49]);
        assert_eq!(BootInfo::from_bytes(&bytes), Err(BootInfoError::BadMagic));
    }

    #[test]
    fn bad_version_rejected() {
        let mut info = valid_bootinfo();
        info.version_major = 2;
        info.seal();
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::BadMajorVersion)
        );
    }

    #[test]
    fn bad_size_rejected() {
        let mut info = valid_bootinfo();
        info.size = 128;
        info.seal();
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::BadSize)
        );

        let mut info = valid_bootinfo();
        info.size = (BOOTINFO_MAX_SIZE + 1) as u32;
        info.seal();
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::BadSize)
        );
    }

    #[test]
    fn bad_crc32c_rejected() {
        let mut info = valid_bootinfo();
        info.checksum ^= 0xFF;
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::BadChecksum)
        );
    }

    #[test]
    fn map_count_too_high_rejected() {
        let mut info = valid_bootinfo();
        info.mem_map_count = MEM_MAP_COUNT_MAX + 1;
        info.seal();
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::MemMapCountTooHigh)
        );
    }

    #[test]
    fn bad_entry_size_rejected() {
        let mut info = valid_bootinfo();
        info.mem_map_entry_size = 16;
        info.seal();
        assert_eq!(
            BootInfo::from_bytes(&info.to_bytes()),
            Err(BootInfoError::BadMemMapEntrySize)
        );
    }

    #[test]
    fn truncated_map_rejected() {
        assert_eq!(
            MemMap::parse(&[0u8; MEM_MAP_ENTRY_SIZE as usize], 2, MEM_MAP_ENTRY_SIZE),
            Err(BootInfoError::MemMapTruncated)
        );
    }

    #[test]
    fn out_of_bounds_map_rejected() {
        let entry = MemMapEntry {
            base: PHYSICAL_LIMIT - 0x1000,
            size: 0x2000,
            kind: MEM_KIND_RESERVED,
            flags: 0,
        };
        let info = fixture(&[entry]);
        assert_eq!(
            load_fixture(&info, &[entry]),
            Err(BootInfoError::MapOutOfBounds)
        );
    }

    #[test]
    fn overlapping_map_rejected() {
        let first = ram_entry();
        let second = MemMapEntry {
            base: RAM_BASE + 0x0010_0000,
            size: 0x0010_0000,
            kind: MEM_KIND_RESERVED,
            flags: 0,
        };
        let info = fixture(&[first, second]);
        assert_eq!(
            load_fixture(&info, &[first, second]),
            Err(BootInfoError::MapOverlap)
        );
    }

    #[test]
    fn kernel_outside_ram_rejected() {
        let mut info = fixture(&[ram_entry()]);
        info.kernel_phys = 0x2000_0000;
        info.kernel_size = 0x1000;
        info.seal();
        assert_eq!(
            load_fixture(&info, &[ram_entry()]),
            Err(BootInfoError::KernelOutsideRam)
        );
    }

    #[test]
    fn kernel_over_bootinfo_rejected() {
        let mut info = fixture(&[ram_entry()]);
        info.kernel_phys = BOOTINFO_PHYS;
        info.kernel_size = 0x1000;
        info.seal();
        assert_eq!(
            load_fixture(&info, &[ram_entry()]),
            Err(BootInfoError::ImageOverlap)
        );
    }

    #[test]
    fn bad_string_rejected() {
        let mut info = fixture(&[ram_entry()]);
        info.cmdline_phys = 0x0800_3000;
        info.cmdline_len = 4;
        info.seal();
        let mut memory = TestMemory::new();
        memory.write(BOOTINFO_PHYS, &info.to_bytes());
        let map = [ram_entry()];
        let mut map_bytes = [0u8; MEM_MAP_MAX_BYTES];
        map_bytes[..MEM_MAP_ENTRY_SIZE as usize].copy_from_slice(&map[0].to_bytes());
        memory.write(MAP_PHYS, &map_bytes[..MEM_MAP_ENTRY_SIZE as usize]);
        memory.write(0x0800_3000, b"abcd");
        assert_eq!(
            load(BOOTINFO_PHYS, &mut memory),
            Err(BootInfoError::BadString)
        );
    }

    #[test]
    fn valid_string_accepted() {
        let mut info = fixture(&[ram_entry()]);
        info.cmdline_phys = 0x0800_3000;
        info.cmdline_len = 4;
        info.seal();
        let mut memory = TestMemory::new();
        memory.write(BOOTINFO_PHYS, &info.to_bytes());
        let map = ram_entry();
        memory.write(MAP_PHYS, &map.to_bytes());
        memory.write(0x0800_3000, b"abc\0");
        assert!(load(BOOTINFO_PHYS, &mut memory).is_ok());
    }

    #[test]
    fn absent_bootinfo_is_distinct_from_malformed() {
        let mut memory = TestMemory::new();
        assert_eq!(load(0, &mut memory), Err(BootInfoError::Absent));
    }

    #[test]
    fn load_checks_magic_before_the_declared_size() {
        let mut memory = TestMemory::new();
        memory.write(BOOTINFO_PHYS, &[0u8; BOOTINFO_SIZE]);
        assert_eq!(
            load(BOOTINFO_PHYS, &mut memory),
            Err(BootInfoError::BadMagic)
        );
    }
}
