// SPDX-License-Identifier: GPL-3.0-only
//! Physical frame allocator shared by the IRIX 7 machines.
//!
//! Frames are 4 KiB.  A list of [`MemDesc`] says which physical ranges are
//! usable RAM; every other descriptor is reserved and its frames start with a
//! reference count of one, so they are never handed out.  Allocation is
//! first-fit over the usable descriptors.  The reference count of each frame is
//! the only per-frame state.
//!
//! The allocator owns no memory.  The machine supplies the descriptor slice and
//! the reference-count array, indexed by absolute frame number, so the Indy
//! kernel (static arrays, MIPS) and the x86_64 machine (its own statics) run the
//! same logic, and the host tests exercise it directly.
//!
//! Frames above the reference-count array are not tracked: they are never
//! allocated and `ref_count` reports zero for them.  The Indy and x86 kernels
//! size the array to 256 MiB, which is the bound they have always had.

#![no_std]

#[cfg(test)]
extern crate std;

/// Bytes per frame.
pub const FRAME_SIZE: u64 = 4096;

/// Descriptor kind for RAM the allocator may hand out.
pub const KIND_USABLE_RAM: u32 = 0;
/// Descriptor kind for memory the machine has reserved for itself.
pub const KIND_RESERVED: u32 = 1;

/// A physical memory descriptor.  The layout matches the ARCS descriptor the
/// Indy ROM passes; the x86_64 machine builds the same records from the PVH
/// memory map.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemDesc {
    pub base: u64,
    pub size: u64,
    pub kind: u32,
    pub flags: u32,
}

/// First-fit allocator over a descriptor list and a reference-count array.
pub struct Allocator<'a> {
    descs: &'a [MemDesc],
    refs: &'a mut [u32],
}

impl<'a> Allocator<'a> {
    /// Build the allocator.  Every frame in a non-usable descriptor is marked
    /// reserved; all other frames start free.
    pub fn new(descs: &'a [MemDesc], refs: &'a mut [u32]) -> Self {
        refs.fill(0);
        let allocator = Self { descs, refs };
        for desc in descs.iter().filter(|d| d.kind != KIND_USABLE_RAM) {
            let range = allocator.frames_of(desc);
            for frame in range {
                allocator.refs[frame] = 1;
            }
        }
        allocator
    }

    /// Attach to state that `new` already initialised.  Unlike `new`, this
    /// leaves the reference counts as they are.
    pub fn resume(descs: &'a [MemDesc], refs: &'a mut [u32]) -> Self {
        Self { descs, refs }
    }

    /// The tracked frame numbers covered by a descriptor.
    fn frames_of(&self, desc: &MemDesc) -> core::ops::Range<usize> {
        let limit = self.refs.len();
        let start = (desc.base / FRAME_SIZE) as usize;
        let end = ((desc.base + desc.size) / FRAME_SIZE) as usize;
        start.min(limit)..end.min(limit)
    }

    fn usable(&self) -> impl Iterator<Item = core::ops::Range<usize>> + '_ {
        self.descs
            .iter()
            .filter(|d| d.kind == KIND_USABLE_RAM)
            .map(move |d| self.frames_of(d))
    }

    /// Allocate one frame, returning its physical address.
    pub fn alloc_frame(&mut self) -> Option<u64> {
        self.alloc_frames(1)
    }

    /// Allocate `count` physically contiguous frames inside one usable
    /// descriptor.  Returns the address of the first frame.
    pub fn alloc_frames(&mut self, count: usize) -> Option<u64> {
        if count == 0 {
            return None;
        }
        for index in 0..self.descs.len() {
            if self.descs[index].kind != KIND_USABLE_RAM {
                continue;
            }
            let range = self.frames_of(&self.descs[index]);
            let mut run_start = range.start;
            let mut run_len = 0usize;
            for frame in range {
                if self.refs[frame] == 0 {
                    if run_len == 0 {
                        run_start = frame;
                    }
                    run_len += 1;
                    if run_len == count {
                        self.refs[run_start..run_start + count].fill(1);
                        return Some(run_start as u64 * FRAME_SIZE);
                    }
                } else {
                    run_len = 0;
                }
            }
        }
        None
    }

    /// Drop one reference to a frame.  Frames at zero stay at zero.
    pub fn free_frame(&mut self, addr: u64) {
        let frame = (addr / FRAME_SIZE) as usize;
        if let Some(refs) = self.refs.get_mut(frame) {
            *refs = refs.saturating_sub(1);
        }
    }

    /// The reference count of the frame at `addr`, or zero if untracked.
    pub fn ref_count(&self, addr: u64) -> u32 {
        self.refs
            .get((addr / FRAME_SIZE) as usize)
            .copied()
            .unwrap_or(0)
    }

    /// Total usable RAM described, in bytes.
    pub fn total_ram(&self) -> u64 {
        self.descs
            .iter()
            .filter(|d| d.kind == KIND_USABLE_RAM)
            .map(|d| d.size)
            .sum()
    }

    /// Free frames inside the usable descriptors.
    pub fn free_frames(&self) -> usize {
        self.usable()
            .map(|range| range.filter(|&frame| self.refs[frame] == 0).count())
            .sum()
    }
}

/// Frame state with fixed capacity, owned by one kernel.
///
/// A kernel declares one `Store` as a static and calls it.  `DESCS` bounds
/// the descriptor list and `FRAMES` bounds the frames tracked, so the whole
/// structure is `.bss` and has no allocation.
pub struct Store<const DESCS: usize, const FRAMES: usize> {
    descs: [MemDesc; DESCS],
    count: usize,
    refs: [u32; FRAMES],
    ready: bool,
}

impl<const DESCS: usize, const FRAMES: usize> Store<DESCS, FRAMES> {
    /// An empty store.  `init` must run before any allocation.
    pub const fn new() -> Self {
        Self {
            descs: [MemDesc {
                base: 0,
                size: 0,
                kind: KIND_RESERVED,
                flags: 0,
            }; DESCS],
            count: 0,
            refs: [0; FRAMES],
            ready: false,
        }
    }

    /// Copy the descriptor list and mark the frames it reserves.  Returns the
    /// number of descriptors stored.  Panics if the list does not fit.
    pub fn init(&mut self, descs: &[MemDesc]) -> usize {
        assert!(descs.len() <= DESCS, "frame store: too many descriptors");
        self.descs[..descs.len()].copy_from_slice(descs);
        self.count = descs.len();
        Allocator::new(&self.descs[..self.count], &mut self.refs);
        self.ready = true;
        self.count
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut Allocator<'_>) -> R) -> Option<R> {
        if !self.ready {
            return None;
        }
        let mut allocator = Allocator::resume(&self.descs[..self.count], &mut self.refs);
        Some(f(&mut allocator))
    }

    /// Allocate one frame.
    pub fn alloc_frame(&mut self) -> Option<u64> {
        self.with(|a| a.alloc_frame()).flatten()
    }

    /// Allocate `count` contiguous frames.
    pub fn alloc_frames(&mut self, count: usize) -> Option<u64> {
        self.with(|a| a.alloc_frames(count)).flatten()
    }

    /// Drop one reference to a frame.
    pub fn free_frame(&mut self, addr: u64) {
        self.with(|a| a.free_frame(addr));
    }

    /// The reference count of the frame at `addr`.
    pub fn ref_count(&mut self, addr: u64) -> u32 {
        self.with(|a| a.ref_count(addr)).unwrap_or(0)
    }

    /// Usable RAM described, in bytes.
    pub fn total_ram(&mut self) -> u64 {
        self.with(|a| a.total_ram()).unwrap_or(0)
    }

    /// Free frames inside the usable descriptors.
    pub fn free_frames(&mut self) -> usize {
        self.with(|a| a.free_frames()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM: u32 = KIND_USABLE_RAM;

    fn ram(base: u64, size: u64) -> MemDesc {
        MemDesc {
            base,
            size,
            kind: RAM,
            flags: 0,
        }
    }

    fn reserved(base: u64, size: u64) -> MemDesc {
        MemDesc {
            base,
            size,
            kind: KIND_RESERVED,
            flags: 0,
        }
    }

    #[test]
    fn alloc_and_free_frame() {
        let descs = [ram(0x0800_0000, 0x1000_0000)];
        let mut refs = std::vec![0u32; 65536];
        let mut alloc = Allocator::new(&descs, &mut refs);

        let frame = alloc.alloc_frame().expect("a frame");
        assert_eq!(frame, 0x0800_0000);
        assert_eq!(alloc.ref_count(frame), 1);

        alloc.free_frame(frame);
        assert_eq!(alloc.ref_count(frame), 0);
    }

    #[test]
    fn reserved_memory_is_never_allocated() {
        let descs = [reserved(0x0800_0000, 0x1000), ram(0x0800_1000, 0x1000_0000)];
        let mut refs = std::vec![0u32; 65536];
        let mut alloc = Allocator::new(&descs, &mut refs);

        assert_eq!(alloc.ref_count(0x0800_0000), 1);
        assert_eq!(alloc.alloc_frame(), Some(0x0800_1000));
    }

    #[test]
    fn contiguous_runs_skip_reserved_holes() {
        // Frames 0..4 usable, 4 reserved, 5..16 usable.
        let descs = [
            ram(0, 4 * FRAME_SIZE),
            reserved(4 * FRAME_SIZE, FRAME_SIZE),
            ram(5 * FRAME_SIZE, 11 * FRAME_SIZE),
        ];
        let mut refs = std::vec![0u32; 16];
        let mut alloc = Allocator::new(&descs, &mut refs);

        assert_eq!(alloc.alloc_frames(4), Some(0));
        assert_eq!(alloc.alloc_frames(4), Some(5 * FRAME_SIZE));
        assert_eq!(alloc.alloc_frames(8), None, "no run of 8 remains");
        assert_eq!(alloc.alloc_frames(0), None);
    }

    #[test]
    fn frames_beyond_the_array_are_untracked() {
        let descs = [ram(0, 0x1_0000_0000)];
        let mut refs = std::vec![0u32; 4];
        let mut alloc = Allocator::new(&descs, &mut refs);

        assert_eq!(alloc.ref_count(0x10_0000), 0);
        alloc.free_frame(0x10_0000);
        assert_eq!(alloc.free_frames(), 4);
    }

    #[test]
    fn accounting_matches_descriptors() {
        let descs = [ram(0, 8 * FRAME_SIZE), reserved(2 * FRAME_SIZE, FRAME_SIZE)];
        let mut refs = std::vec![0u32; 8];
        let mut alloc = Allocator::new(&descs, &mut refs);

        assert_eq!(alloc.total_ram(), 8 * FRAME_SIZE);
        assert_eq!(alloc.free_frames(), 8 - 1, "the reserved frame is not free");
        alloc.alloc_frame();
        assert_eq!(alloc.free_frames(), 6);
    }
}
