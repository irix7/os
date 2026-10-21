//! BootInfo binary contract between the ROM and the kernel.
//!
//! Layout matches [`docs/spec/boot-protocol.md`](docs/spec/boot-protocol.md) §3
//! exactly: a 256-byte `#[repr(C, align(8))]` structure with fixed offsets.
//! Compile-time assertions enforce the offset table.

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

/// Memory-map entry kind values (boot-protocol.md §4).
pub const MEM_MAP_USABLE_RAM: u32 = 0;
pub const MEM_MAP_RESERVED: u32 = 1;

/// Console kind values (boot-protocol.md §3).
pub const CONSOLE_NONE: u32 = 0;
pub const CONSOLE_UART: u32 = 1;
pub const CONSOLE_FRAMEBUFFER: u32 = 2;
pub const CONSOLE_BOTH: u32 = 3;

/// Board ID values (boot-protocol.md §3).
pub const BOARD_INDY: u32 = 0;
pub const BOARD_INDIGO2_IP22: u32 = 1;
pub const BOARD_INDIGO2_IP28: u32 = 2;
pub const BOARD_UNKNOWN: u32 = 3;

/// A single memory-map entry (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MemMapEntry {
    pub base: u64,
    pub size: u64,
    pub kind: u32,
    pub flags: u32,
}

/// The binary boot-info contract between the o32 ROM and the n32 kernel.
///
/// Field offsets match `docs/spec/boot-protocol.md` §3 exactly and are
/// enforced by `const` assertions below.
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
    pub console_poll: u32,       // 0xF0
    pub net_state: u32,          // 0xF4
    pub seeq_state: u32,         // 0xF8
    pub reserved: [u8; 4],       // 0xFC
} // 0x100 = 256 bytes

// Compile-time layout assertions mirroring the spec's offset table.
const _: () = {
    assert!(core::mem::size_of::<BootInfo>() == BOOTINFO_SIZE);
    assert!(core::mem::align_of::<BootInfo>() == 8);
    assert!(core::mem::offset_of!(BootInfo, magic) == 0x00);
    assert!(core::mem::offset_of!(BootInfo, version_major) == 0x04);
    assert!(core::mem::offset_of!(BootInfo, version_minor) == 0x06);
    assert!(core::mem::offset_of!(BootInfo, size) == 0x08);
    assert!(core::mem::offset_of!(BootInfo, checksum) == 0x0C);
    assert!(core::mem::offset_of!(BootInfo, mem_map_phys) == 0x10);
    assert!(core::mem::offset_of!(BootInfo, mem_map_count) == 0x18);
    assert!(core::mem::offset_of!(BootInfo, mem_map_entry_size) == 0x1C);
    assert!(core::mem::offset_of!(BootInfo, kernel_phys) == 0x20);
    assert!(core::mem::offset_of!(BootInfo, kernel_size) == 0x28);
    assert!(core::mem::offset_of!(BootInfo, kernel_sha256) == 0x30);
    assert!(core::mem::offset_of!(BootInfo, bundle_phys) == 0x50);
    assert!(core::mem::offset_of!(BootInfo, bundle_size) == 0x58);
    assert!(core::mem::offset_of!(BootInfo, bundle_sha256) == 0x60);
    assert!(core::mem::offset_of!(BootInfo, cmdline_phys) == 0x80);
    assert!(core::mem::offset_of!(BootInfo, cmdline_len) == 0x88);
    assert!(core::mem::offset_of!(BootInfo, console_kind) == 0x8C);
    assert!(core::mem::offset_of!(BootInfo, console_phys) == 0x90);
    assert!(core::mem::offset_of!(BootInfo, console_width) == 0x98);
    assert!(core::mem::offset_of!(BootInfo, console_height) == 0x9C);
    assert!(core::mem::offset_of!(BootInfo, console_stride) == 0xA0);
    assert!(core::mem::offset_of!(BootInfo, console_bpp) == 0xA4);
    assert!(core::mem::offset_of!(BootInfo, console_baud) == 0xA8);
    assert!(core::mem::offset_of!(BootInfo, mac) == 0xAC);
    assert!(core::mem::offset_of!(BootInfo, pad0) == 0xB2);
    assert!(core::mem::offset_of!(BootInfo, ip) == 0xB4);
    assert!(core::mem::offset_of!(BootInfo, netmask) == 0xB8);
    assert!(core::mem::offset_of!(BootInfo, gateway) == 0xBC);
    assert!(core::mem::offset_of!(BootInfo, dns) == 0xC0);
    assert!(core::mem::offset_of!(BootInfo, dhcp) == 0xC4);
    assert!(core::mem::offset_of!(BootInfo, rtc_valid) == 0xC8);
    assert!(core::mem::offset_of!(BootInfo, flags) == 0xCC);
    assert!(core::mem::offset_of!(BootInfo, r2_url_phys) == 0xD0);
    assert!(core::mem::offset_of!(BootInfo, r2_url_len) == 0xD8);
    assert!(core::mem::offset_of!(BootInfo, cpu_hz) == 0xDC);
    assert!(core::mem::offset_of!(BootInfo, ram_size) == 0xE0);
    assert!(core::mem::offset_of!(BootInfo, board_id) == 0xE8);
    assert!(core::mem::offset_of!(BootInfo, prom_version) == 0xEC);
    assert!(core::mem::offset_of!(BootInfo, console_poll) == 0xF0);
    assert!(core::mem::offset_of!(BootInfo, net_state) == 0xF4);
    assert!(core::mem::offset_of!(BootInfo, seeq_state) == 0xF8);
    assert!(core::mem::offset_of!(BootInfo, reserved) == 0xFC);
};

// CRC32C (Castagnoli, reflected, poly 0x1EDC6F41, init/xorout 0xFFFFFFFF).
// Used for the BootInfo checksum field.
fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0x82F63B78 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Compute the BootInfo checksum: CRC32C over all 256 bytes with the
/// checksum field zeroed.
pub fn bootinfo_checksum(info: &BootInfo) -> u32 {
    let bytes =
        unsafe { core::slice::from_raw_parts(info as *const BootInfo as *const u8, BOOTINFO_SIZE) };
    let mut buf = [0u8; BOOTINFO_SIZE];
    buf.copy_from_slice(bytes);
    // Zero the checksum field (offset 0x0C, 4 bytes).
    buf[0x0C..0x10].copy_from_slice(&0u32.to_le_bytes());
    crc32c(&buf)
}

/// Build a `BootInfo` from the gathered boot parameters. The caller must fill
/// in `kernel_sha256`, `bundle_sha256`, `mac`, `ip`, etc. The checksum field
/// is computed automatically.
#[allow(clippy::too_many_arguments)]
pub fn build_bootinfo(
    mem_map_phys: u64,
    mem_map_count: u32,
    kernel_phys: u64,
    kernel_size: u64,
    kernel_sha256: [u8; 32],
    bundle_phys: u64,
    bundle_size: u64,
    bundle_sha256: [u8; 32],
    cmdline_phys: u64,
    cmdline_len: u32,
    console_kind: u32,
    console_phys: u64,
    console_width: u32,
    console_height: u32,
    console_stride: u32,
    console_bpp: u32,
    console_baud: u32,
    mac: [u8; 6],
    ip: u32,
    netmask: u32,
    gateway: u32,
    dns: u32,
    dhcp: u32,
    rtc_valid: u32,
    flags: u32,
    r2_url_phys: u64,
    r2_url_len: u32,
    cpu_hz: u32,
    ram_size: u64,
    board_id: u32,
    prom_version: u32,
) -> BootInfo {
    let mut info = BootInfo::default();
    info.magic = BOOTINFO_MAGIC;
    info.version_major = BOOTINFO_VERSION_MAJOR;
    info.version_minor = 0;
    info.size = BOOTINFO_SIZE as u32;
    info.mem_map_phys = mem_map_phys;
    info.mem_map_count = mem_map_count;
    info.mem_map_entry_size = MEM_MAP_ENTRY_SIZE;
    info.kernel_phys = kernel_phys;
    info.kernel_size = kernel_size;
    info.kernel_sha256 = kernel_sha256;
    info.bundle_phys = bundle_phys;
    info.bundle_size = bundle_size;
    info.bundle_sha256 = bundle_sha256;
    info.cmdline_phys = cmdline_phys;
    info.cmdline_len = cmdline_len;
    info.console_kind = console_kind;
    info.console_phys = console_phys;
    info.console_width = console_width;
    info.console_height = console_height;
    info.console_stride = console_stride;
    info.console_bpp = console_bpp;
    info.console_baud = console_baud;
    info.mac = mac;
    info.ip = ip;
    info.netmask = netmask;
    info.gateway = gateway;
    info.dns = dns;
    info.dhcp = dhcp;
    info.rtc_valid = rtc_valid;
    info.flags = flags;
    info.r2_url_phys = r2_url_phys;
    info.r2_url_len = r2_url_len;
    info.cpu_hz = cpu_hz;
    info.ram_size = ram_size;
    info.board_id = board_id;
    info.prom_version = prom_version;
    info.console_poll = 0;
    info.net_state = 0;
    info.seeq_state = 0;
    info.checksum = bootinfo_checksum(&info);
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_offsets_match_spec() {
        assert_eq!(core::mem::size_of::<BootInfo>(), BOOTINFO_SIZE);
        assert_eq!(core::mem::offset_of!(BootInfo, magic), 0x00);
        assert_eq!(core::mem::offset_of!(BootInfo, checksum), 0x0C);
        assert_eq!(core::mem::offset_of!(BootInfo, kernel_sha256), 0x30);
        assert_eq!(core::mem::offset_of!(BootInfo, console_poll), 0xF0);
        assert_eq!(core::mem::offset_of!(BootInfo, net_state), 0xF4);
        assert_eq!(core::mem::offset_of!(BootInfo, seeq_state), 0xF8);
        assert_eq!(core::mem::offset_of!(BootInfo, reserved), 0xFC);
    }

    #[test]
    fn crc32c_known_answer() {
        // CRC32C (Castagnoli) of "123456789" = 0xE306_9283.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0x0000_0000);
    }

    #[test]
    fn bootinfo_checksum_is_deterministic() {
        let info1 = BootInfo::default();
        let info2 = BootInfo::default();
        assert_eq!(bootinfo_checksum(&info1), bootinfo_checksum(&info2));
    }
}
