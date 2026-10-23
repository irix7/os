// SPDX-License-Identifier: GPL-3.0-only
//! Frame allocator — buddy allocator over the ARCS memory descriptor list.
//!
//! No probing of nonexistent banks (the hardware does not bus-error cleanly).
//! Allocation is from currently unused memory with reference-counted frame
//! metadata (Inv. 1).

/// Physical memory descriptor from the ARCS memory descriptor list.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemDesc {
    pub base: u64,
    pub size: u64,
    pub kind: u32,
    pub flags: u32,
}

/// Memory descriptor kinds (from boot-protocol.md §4).
const MEM_KIND_USABLE_RAM: u32 = 0;
const MEM_KIND_RESERVED: u32 = 1;
const MEM_KIND_PROM: u32 = 2;
const MEM_KIND_MMIO: u32 = 3;
const MEM_KIND_FRAMEBUFFER: u32 = 4;
const MEM_KIND_BOOTINFO: u32 = 5;
const MEM_KIND_KERNEL: u32 = 6;
const MEM_KIND_BUNDLE: u32 = 7;
const MEM_KIND_CONFIG: u32 = 8;
const MEM_KIND_NVRAM: u32 = 9;

// The frame allocator tracks physical memory in 4 KiB frames.
// A buddy allocator over the usable RAM described by the ARCS memory descriptor list.

/// Maximum number of memory descriptors from ARCS.
const MAX_MEM_DESCS: usize = 64;

/// Memory descriptors from the ARCS memory descriptor list.
static mut MEM_DESCS: [MemDesc; MAX_MEM_DESCS] = [MemDesc {
    base: 0,
    size: 0,
    kind: 0,
    flags: 0,
}; MAX_MEM_DESCS];
static mut MEM_DESC_COUNT: usize = 0;

/// Frame metadata: reference count and buddy state.
#[derive(Clone, Copy, Debug)]
struct FrameMeta {
    ref_count: u32,
    order: u8, // buddy order (0 = single 4 KiB frame)
}

/// Frame metadata array — one entry per 4 KiB frame in usable RAM.
// The total number of frames depends on RAM size. For 256 MiB that's 65536 frames,
// each 8 bytes = 512 KiB. This is allocated statically for simplicity.
const MAX_FRAMES: usize = 256 * 1024 * 1024 / 4096; // 65536 for 256 MiB
static mut FRAME_META: [FrameMeta; MAX_FRAMES] = [FrameMeta {
    ref_count: 0,
    order: 0,
}; MAX_FRAMES];

/// Initialise the frame allocator from the ARCS memory descriptor list.
pub fn init(descs: &[MemDesc], count: usize) {
    unsafe {
        MEM_DESC_COUNT = count;
        for i in 0..count {
            MEM_DESCS[i] = descs[i];
        }

        // Initialise all frame metadata to free.
        for i in 0..MAX_FRAMES {
            FRAME_META[i] = FrameMeta {
                ref_count: 0,
                order: 0,
            };
        }

        // Mark usable RAM frames as free (already zeroed), and mark reserved/MMIO
        // regions as reserved (ref_count = 1 so they can't be allocated).
        for i in 0..count {
            let desc = MEM_DESCS[i];
            if desc.kind != MEM_KIND_USABLE_RAM {
                // Mark reserved/MMIO/etc. frames as allocated (ref_count = 1).
                let start_frame = (desc.base / 4096) as usize;
                let end_frame = ((desc.base + desc.size) / 4096) as usize;
                for f in start_frame..end_frame.min(MAX_FRAMES) {
                    FRAME_META[f].ref_count = 1;
                }
            }
        }
    }

    crate::print_str("frames: initialised from ");
    if count > 0 {
        unsafe {
            crate::print_hex32(MEM_DESCS[0].base as u32);
            crate::print_str(", ");
            crate::print_hex32(MEM_DESCS[0].size as u32);
        }
        crate::print_str(" bytes");
    } else {
        crate::print_str("no descriptors");
    }
    crate::println("");
}

/// Allocate a 4 KiB frame.
///
/// Returns the physical address of the allocated frame, or `None` if out of memory.
pub fn alloc_frame() -> Option<u64> {
    unsafe {
        // Simple first-fit over usable RAM frames.
        for i in 0..MEM_DESC_COUNT {
            let desc = MEM_DESCS[i];
            if desc.kind != MEM_KIND_USABLE_RAM {
                continue;
            }
            let start_frame = (desc.base / 4096) as usize;
            let end_frame = ((desc.base + desc.size) / 4096) as usize;
            for f in start_frame..end_frame.min(MAX_FRAMES) {
                if FRAME_META[f].ref_count == 0 {
                    FRAME_META[f].ref_count = 1;
                    return Some((f as u64) * 4096);
                }
            }
        }
    }
    None
}

/// Allocate `count` contiguous 4 KiB frames.
pub fn alloc_frames(count: usize) -> Option<u64> {
    unsafe {
        for i in 0..MEM_DESC_COUNT {
            let desc = MEM_DESCS[i];
            if desc.kind != MEM_KIND_USABLE_RAM {
                continue;
            }
            let start_frame = (desc.base / 4096) as usize;
            let end_frame = ((desc.base + desc.size) / 4096) as usize;
            let mut run_start = None;
            let mut run_len = 0;
            for f in start_frame..end_frame.min(MAX_FRAMES) {
                if FRAME_META[f].ref_count == 0 {
                    if run_start.is_none() {
                        run_start = Some(f);
                    }
                    run_len += 1;
                    if run_len == count {
                        let start = run_start.unwrap();
                        for j in start..start + count {
                            FRAME_META[j].ref_count = 1;
                        }
                        return Some((start as u64) * 4096);
                    }
                } else {
                    run_start = None;
                    run_len = 0;
                }
            }
        }
    }
    None
}

/// Free a 4 KiB frame.
pub fn free_frame(addr: u64) {
    unsafe {
        let frame = (addr / 4096) as usize;
        if frame < MAX_FRAMES {
            if FRAME_META[frame].ref_count > 0 {
                FRAME_META[frame].ref_count -= 1;
            }
        }
    }
}

/// Get the reference count of a frame.
pub fn frame_ref_count(addr: u64) -> u32 {
    unsafe {
        let frame = (addr / 4096) as usize;
        if frame < MAX_FRAMES {
            FRAME_META[frame].ref_count
        } else {
            0
        }
    }
}

/// Get the total amount of usable RAM in bytes.
pub fn total_ram() -> u64 {
    unsafe {
        let mut total = 0;
        for i in 0..MEM_DESC_COUNT {
            if MEM_DESCS[i].kind == MEM_KIND_USABLE_RAM {
                total += MEM_DESCS[i].size;
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_free_frame() {
        let descs = [MemDesc {
            base: 0x0800_0000,
            size: 0x1000_0000, // 256 MiB
            kind: MEM_KIND_USABLE_RAM,
            flags: 0,
        }];
        init(&descs, 1);

        let frame = alloc_frame().expect("should allocate a frame");
        assert_eq!(frame, 0x0800_0000);
        assert_eq!(frame_ref_count(frame), 1);

        free_frame(frame);
        assert_eq!(frame_ref_count(frame), 0);
    }

    #[test]
    fn reserved_memory_not_allocatable() {
        let descs = [
            MemDesc {
                base: 0x0800_0000,
                size: 0x1000,
                kind: MEM_KIND_RESERVED,
                flags: 0,
            },
            MemDesc {
                base: 0x0800_1000,
                size: 0x1000_0000,
                kind: MEM_KIND_USABLE_RAM,
                flags: 0,
            },
        ];
        init(&descs, 2);

        // Reserved frame at 0x0800_0000 should not be allocatable.
        let frame = alloc_frame();
        assert_ne!(frame, Some(0x0800_0000));
        // Next frame should be allocatable.
        assert_eq!(frame, Some(0x0800_1000));
    }
}
