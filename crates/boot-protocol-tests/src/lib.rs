//! Boot protocol (ROM <-> kernel) conformance primitives.
//!
//! Implements the `BootInfo` binary contract from
//! [`docs/spec/boot-protocol.md`](https://github.com/irix7/os/blob/main/docs/spec/boot-protocol.md):
//! the 256-byte `#[repr(C, align(8))]` structure, its field-offset layout, the
//! CRC32C checksum, and the kernel's validation order.
//!
//! The layout, CRC32C and validation logic only use `core`, so they are
//! trivially portable to the o32 ROM and n32 kernel `build-std` contexts. The
//! test harness runs on the host under `std`.

#![cfg_attr(not(test), no_std)]

use core::mem::{offset_of, size_of};

/// BootInfo magic: `"I7BI"` stored big-endian = 0x4937_4249.
pub const BOOTINFO_MAGIC: u32 = 0x4937_4249;
/// BootInfo major version accepted by the kernel.
pub const BOOTINFO_VERSION_MAJOR: u16 = 1;
/// BootInfo size in bytes.
pub const BOOTINFO_SIZE: usize = 256;
/// Maximum number of memory-map entries.
pub const MEM_MAP_COUNT_MAX: u32 = 64;
/// Memory-map entry size in bytes.
pub const MEM_MAP_ENTRY_SIZE: u32 = 24;

/// The binary boot-info contract between the o32 ROM and the n32 kernel.
///
/// Field offsets match [`docs/spec/boot-protocol.md`](docs/spec/boot-protocol.md) §3 exactly
/// and are enforced by `const` assertions below.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default)]
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
    pub reserved: [u8; 16],      // 0xF0
} // 0x100 = 256 bytes

// Compile-time layout assertions mirroring the spec's offset table.
const _: () = {
    assert!(size_of::<BootInfo>() == BOOTINFO_SIZE);
    assert!(offset_of!(BootInfo, magic) == 0x00);
    assert!(offset_of!(BootInfo, version_major) == 0x04);
    assert!(offset_of!(BootInfo, version_minor) == 0x06);
    assert!(offset_of!(BootInfo, size) == 0x08);
    assert!(offset_of!(BootInfo, checksum) == 0x0C);
    assert!(offset_of!(BootInfo, mem_map_phys) == 0x10);
    assert!(offset_of!(BootInfo, mem_map_count) == 0x18);
    assert!(offset_of!(BootInfo, mem_map_entry_size) == 0x1C);
    assert!(offset_of!(BootInfo, kernel_phys) == 0x20);
    assert!(offset_of!(BootInfo, kernel_size) == 0x28);
    assert!(offset_of!(BootInfo, kernel_sha256) == 0x30);
    assert!(offset_of!(BootInfo, bundle_phys) == 0x50);
    assert!(offset_of!(BootInfo, bundle_size) == 0x58);
    assert!(offset_of!(BootInfo, bundle_sha256) == 0x60);
    assert!(offset_of!(BootInfo, cmdline_phys) == 0x80);
    assert!(offset_of!(BootInfo, cmdline_len) == 0x88);
    assert!(offset_of!(BootInfo, console_kind) == 0x8C);
    assert!(offset_of!(BootInfo, console_phys) == 0x90);
    assert!(offset_of!(BootInfo, console_width) == 0x98);
    assert!(offset_of!(BootInfo, console_height) == 0x9C);
    assert!(offset_of!(BootInfo, console_stride) == 0xA0);
    assert!(offset_of!(BootInfo, console_bpp) == 0xA4);
    assert!(offset_of!(BootInfo, console_baud) == 0xA8);
    assert!(offset_of!(BootInfo, mac) == 0xAC);
    assert!(offset_of!(BootInfo, pad0) == 0xB2);
    assert!(offset_of!(BootInfo, ip) == 0xB4);
    assert!(offset_of!(BootInfo, netmask) == 0xB8);
    assert!(offset_of!(BootInfo, gateway) == 0xBC);
    assert!(offset_of!(BootInfo, dns) == 0xC0);
    assert!(offset_of!(BootInfo, dhcp) == 0xC4);
    assert!(offset_of!(BootInfo, rtc_valid) == 0xC8);
    assert!(offset_of!(BootInfo, flags) == 0xCC);
    assert!(offset_of!(BootInfo, r2_url_phys) == 0xD0);
    assert!(offset_of!(BootInfo, r2_url_len) == 0xD8);
    assert!(offset_of!(BootInfo, cpu_hz) == 0xDC);
    assert!(offset_of!(BootInfo, ram_size) == 0xE0);
    assert!(offset_of!(BootInfo, board_id) == 0xE8);
    assert!(offset_of!(BootInfo, prom_version) == 0xEC);
    assert!(offset_of!(BootInfo, reserved) == 0xF0);
};

/// Error returned by [`BootInfo::validate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootInfoError {
    BadMagic,
    BadMajorVersion,
    BadSize,
    BadChecksum,
    MemMapCountTooHigh,
    BadMemMapEntrySize,
    Overlap,
}

/// CRC32C (Castagnoli, reflected, poly 0x1EDC6F41, init/xorout 0xFFFFFFFF).
pub fn crc32c(data: &[u8]) -> u32 {
    // Static table built at compile time via a const fn would need const loops;
    // build lazily once. For clarity and determinism we compute on demand.
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0x82F63B78 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    crc ^= 0xFFFF_FFFF;
    crc
}

/// Compute the BootInfo checksum field value: CRC32C over all 256 bytes with
/// the checksum field zeroed.
pub fn bootinfo_checksum(info: &BootInfo) -> u32 {
    let bytes =
        unsafe { core::slice::from_raw_parts(info as *const BootInfo as *const u8, BOOTINFO_SIZE) };
    let mut buf = [0u8; BOOTINFO_SIZE];
    buf.copy_from_slice(bytes);
    // Zero the checksum field (offset 0x0C, 4 bytes).
    buf[0x0C..0x10].copy_from_slice(&0u32.to_le_bytes());
    crc32c(&buf)
}

impl BootInfo {
    /// Validate per the kernel's validation order (boot-protocol.md §5).
    /// `usable_ram` is a list of `(base, size)` usable RAM regions used for the
    /// kernel/bundle containment check.
    pub fn validate(&self, usable_ram: &[(u64, u64)]) -> Result<(), BootInfoError> {
        // 1. magic == "I7BI"
        if self.magic != BOOTINFO_MAGIC {
            return Err(BootInfoError::BadMagic);
        }
        // 2. version_major == 1
        if self.version_major != BOOTINFO_VERSION_MAJOR {
            return Err(BootInfoError::BadMajorVersion);
        }
        // 3. size >= 256 and mem_map_entry_size == 24
        if self.size < BOOTINFO_SIZE as u32 {
            return Err(BootInfoError::BadSize);
        }
        if self.mem_map_entry_size != MEM_MAP_ENTRY_SIZE {
            return Err(BootInfoError::BadMemMapEntrySize);
        }
        // 4. CRC32C matches with checksum field zeroed
        if bootinfo_checksum(self) != self.checksum {
            return Err(BootInfoError::BadChecksum);
        }
        // 5. mem_map_count <= 64
        if self.mem_map_count > MEM_MAP_COUNT_MAX {
            return Err(BootInfoError::MemMapCountTooHigh);
        }
        // 6. kernel/bundle inside usable RAM and not overlapping BootInfo/stack.
        //    (BootInfo itself lives in reserved RAM; here we just check containment.)
        if !in_ram(self.kernel_phys, self.kernel_size, usable_ram) {
            return Err(BootInfoError::Overlap);
        }
        if !in_ram(self.bundle_phys, self.bundle_size, usable_ram) {
            return Err(BootInfoError::Overlap);
        }
        Ok(())
    }
}

fn in_ram(base: u64, size: u64, ram: &[(u64, u64)]) -> bool {
    if size == 0 {
        return true;
    }
    let end = base.saturating_add(size);
    ram.iter().any(|(rbase, rsize)| {
        let rend = rbase.saturating_add(*rsize);
        base >= *rbase && end <= rend
    })
}

/// Build a minimal valid `BootInfo` with the checksum filled in.
pub fn valid_bootinfo() -> BootInfo {
    let mut info = BootInfo::default();
    info.magic = BOOTINFO_MAGIC;
    info.version_major = BOOTINFO_VERSION_MAJOR;
    info.size = BOOTINFO_SIZE as u32;
    info.mem_map_entry_size = MEM_MAP_ENTRY_SIZE;
    info.kernel_phys = 0x0804_0000;
    info.kernel_size = 0x0040_0000;
    info.bundle_phys = 0x0844_0000;
    info.bundle_size = 0x0100_0000;
    info.console_kind = 1; // UART
    info.cmdline_len = 1;
    info.checksum = bootinfo_checksum(&info);
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_offsets_match_spec() {
        assert_eq!(size_of::<BootInfo>(), BOOTINFO_SIZE);
        assert_eq!(offset_of!(BootInfo, magic), 0x00);
        assert_eq!(offset_of!(BootInfo, checksum), 0x0C);
        assert_eq!(offset_of!(BootInfo, kernel_sha256), 0x30);
        assert_eq!(offset_of!(BootInfo, reserved), 0xF0);
    }

    #[test]
    fn crc32c_known_answer() {
        // CRC32C (Castagnoli) of "123456789" = 0xE306_9283.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0x0000_0000);
    }

    #[test]
    fn valid_bootinfo_passes() {
        let info = valid_bootinfo();
        let ram = [(0x0800_0000u64, 0x1000_0000)]; // 256 MiB from 0x0800_0000
        assert!(info.validate(&ram).is_ok());
    }

    #[test]
    fn corrupt_magic_rejected() {
        let mut info = valid_bootinfo();
        info.magic = 0xDEAD_BEEF;
        assert_eq!(
            info.validate(&[(0x0800_0000, 0x1000_0000)]),
            Err(BootInfoError::BadMagic)
        );
    }

    #[test]
    fn bad_version_rejected() {
        let mut info = valid_bootinfo();
        info.version_major = 2;
        assert_eq!(
            info.validate(&[(0x0800_0000, 0x1000_0000)]),
            Err(BootInfoError::BadMajorVersion)
        );
    }

    #[test]
    fn bad_checksum_rejected() {
        let mut info = valid_bootinfo();
        info.checksum ^= 0xFF;
        assert_eq!(
            info.validate(&[(0x0800_0000, 0x1000_0000)]),
            Err(BootInfoError::BadChecksum)
        );
    }

    #[test]
    fn map_count_too_high_rejected() {
        let mut info = valid_bootinfo();
        info.mem_map_count = MEM_MAP_COUNT_MAX + 1;
        info.checksum = bootinfo_checksum(&info);
        assert_eq!(
            info.validate(&[(0x0800_0000, 0x1000_0000)]),
            Err(BootInfoError::MemMapCountTooHigh)
        );
    }

    #[test]
    fn kernel_outside_ram_rejected() {
        let mut info = valid_bootinfo();
        info.kernel_phys = 0xFFFF_F000; // not in the usable region
        info.checksum = bootinfo_checksum(&info);
        assert_eq!(
            info.validate(&[(0x0800_0000, 0x1000_0000)]),
            Err(BootInfoError::Overlap)
        );
    }

    #[test]
    fn checksum_recomputed_on_mutated_info() {
        let mut info = valid_bootinfo();
        info.cmdline_len = 42;
        info.checksum = bootinfo_checksum(&info);
        assert!(info.validate(&[(0x0800_0000, 0x1000_0000)]).is_ok());
    }
}
