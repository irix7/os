// SPDX-License-Identifier: GPL-3.0-only
//! Native user ELF loader and per-process address space.
//!
//! A [`Space`] owns the user page pairs, the range list, the anonymous heap and
//! the fixed stack for one cell.  The process table holds one per process; a
//! process switch installs that space's TLB entries with the process's ASID.
//! Fork copies the mapped frames; exec builds a fresh space before replacing
//! the old one, so a failed load leaves the caller's image intact.

use irix_elf::{Abi, Elf};

use crate::syserr::{
    SysResult, CELL_E_2BIG, CELL_E_AGAIN, CELL_E_FAULT, CELL_E_INVAL, CELL_E_NAMETOOLONG,
    CELL_E_NOEXEC, CELL_E_NOMEM, CELL_E_OVERFLOW, CELL_E_SRCH,
};

use crate::{cache, fd, frames, process, ramfs, scsi, tlb, traps};

/// Enough for the static `std` bring-up image and its section table.
const ELF_SIZE: usize = 256 * 1024;
const TARGET: u8 = 2;
/// One TLB index maps an even/odd 4 KiB pair.  Indices 4-45 are available for
/// image and stack pairs; indices 46-47 are reserved for the two 256 KiB heap
/// entries.  A pair maps 8 KiB, so 42 pairs cover 336 KiB of image.
pub const MAX_PAIRS: usize = 42;
const FIRST_TLB: u32 = 4;
const HEAP_TLB_START: u32 = 46;
const STACK_BASE: u32 = 0x7FF0_0000;
const STACK_PAGES: usize = 4;
const STACK_TOP: u32 = STACK_BASE + STACK_PAGES as u32 * 4096;

const HEAP_BASE: u32 = 0x2000_0000;
const HEAP_LIMIT: u32 = HEAP_BASE + 1024 * 1024;
const HEAP_PAGE_MASK_256K: u32 = crate::tlb::PAGE_MASK_256K;
const HEAP_PAGE_SIZE: u32 = 256 * 1024;
const HEAP_TLB_PAGES: usize = 2;

pub const MAX_RANGES: usize = 12;

static mut ELF_BUF: Option<&'static mut [u8]> = None;

/// Argument and environment strings for `execve`, copied out of the old address
/// space before it is replaced.
const ARG_BYTES: usize = 4096;
const MAX_ARGUMENT_POINTERS: usize = 16;
static mut ARG_DATA: [u8; ARG_BYTES] = [0; ARG_BYTES];

/// The package installer's userland staging buffer.
///
/// This is the same 256 KiB buffer the ELF loader uses, so installing an image
/// does not add a second large `.bss` allocation.  It is only valid while the
/// console owns the machine (no user process is running).
pub fn image_buffer() -> &'static mut [u8] {
    // SAFETY: sash is single-threaded; the loader and the installer never run
    // at the same time.
    crate::bigbuf::get(unsafe { &mut *core::ptr::addr_of_mut!(ELF_BUF) }, ELF_SIZE)
}

#[derive(Clone, Copy)]
struct LoadedPair {
    vaddr: u32,
    even: u32,
    odd: u32,
}

/// One cell's user mappings and range list.
#[derive(Clone, Copy)]
pub struct Space {
    pairs: [LoadedPair; MAX_PAIRS],
    pub(crate) pair_count: usize,
    ranges: [(u32, u32); MAX_RANGES],
    range_count: usize,
    heap_next: u32,
    heap_mapped: bool,
    heap_alloc_base: u64,
    heap_alloc_frames: usize,
    heap_phys: u64,
}

impl Space {
    pub const EMPTY: Self = Self {
        pairs: [LoadedPair {
            vaddr: 0,
            even: 0,
            odd: 0,
        }; MAX_PAIRS],
        pair_count: 0,
        ranges: [(0, 0); MAX_RANGES],
        range_count: 0,
        heap_next: HEAP_BASE,
        heap_mapped: false,
        heap_alloc_base: 0,
        heap_alloc_frames: 0,
        heap_phys: 0,
    };

    fn page_of(address: u32) -> u32 {
        address & !0xFFF
    }

    fn pair_of(address: u32) -> u32 {
        address & !0x1FFF
    }

    /// Whether `[address, address + length)` is inside a recorded user range.
    pub fn range_ok(&self, address: u32, length: u32) -> bool {
        let Some(end) = address.checked_add(length) else {
            return false;
        };
        self.ranges[..self.range_count]
            .iter()
            .any(|&(start, stop)| address >= start && end <= stop)
    }

    /// Record `[start, end)` in the user range table, merging adjacent ranges.
    pub fn record_range(&mut self, start: u32, end: u32) -> bool {
        let mut start = start;
        let mut end = end;
        let mut index = 0;
        while index < self.range_count {
            let (existing_start, existing_end) = self.ranges[index];
            if start <= existing_end && end >= existing_start {
                start = start.min(existing_start);
                end = end.max(existing_end);
                for move_index in index..self.range_count - 1 {
                    self.ranges[move_index] = self.ranges[move_index + 1];
                }
                self.range_count -= 1;
            } else {
                index += 1;
            }
        }
        if self.range_count < MAX_RANGES {
            self.ranges[self.range_count] = (start, end);
            self.range_count += 1;
            true
        } else {
            false
        }
    }

    /// The physical frame backing `address`, if the space maps it.
    pub fn physical(&self, address: u32) -> Option<u32> {
        if self.heap_mapped
            && address >= HEAP_BASE
            && address < HEAP_BASE + HEAP_TLB_PAGES as u32 * 2 * HEAP_PAGE_SIZE
        {
            return Some(self.heap_phys as u32 + (address - HEAP_BASE));
        }
        let pair = self.pairs[..self.pair_count]
            .iter()
            .find(|pair| address >= pair.vaddr && address < pair.vaddr + 0x2000)?;
        let frame = if address & 0x1000 == 0 {
            pair.even
        } else {
            pair.odd
        };
        Some(frame + (address & 0xFFF))
    }

    /// Copy bytes through the user virtual alias.  The space must be active.
    pub fn write_bytes(&mut self, address: u32, bytes: &[u8]) -> bool {
        if self.physical(address).is_none() {
            return false;
        }
        for (offset, byte) in bytes.iter().enumerate() {
            // SAFETY: the caller activated this space and the range is mapped.
            unsafe {
                core::ptr::write_volatile((address as *mut u8).add(offset), *byte);
            }
        }
        true
    }

    /// Write one 32-bit word into the space.
    pub fn write_word(&mut self, address: u32, value: u32) -> bool {
        self.write_bytes(address, &value.to_be_bytes())
    }

    fn try_word(&mut self, address: u32, value: u32) -> Option<()> {
        if self.write_word(address, value) {
            Some(())
        } else {
            None
        }
    }

    /// Allocate and zero the pair containing `address`, returning its frames.
    fn ensure_pair(&mut self, address: u32) -> Option<(u32, u32)> {
        let vaddr = Self::pair_of(address);
        if let Some(pair) = self.pairs[..self.pair_count]
            .iter()
            .find(|pair| pair.vaddr == vaddr)
        {
            return Some((pair.even, pair.odd));
        }
        if self.pair_count >= MAX_PAIRS {
            crate::println("userelf: too many page pairs");
            return None;
        }
        let Some(even) = frames::alloc_frame() else {
            crate::println("userelf: out of frames");
            return None;
        };
        let Some(odd) = frames::alloc_frame() else {
            frames::free_frame(even);
            crate::println("userelf: out of frames");
            return None;
        };
        zero_frame(even);
        zero_frame(odd);
        let pair = LoadedPair {
            vaddr,
            even: even as u32,
            odd: odd as u32,
        };
        self.pairs[self.pair_count] = pair;
        self.pair_count += 1;
        Some((pair.even, pair.odd))
    }

    /// Install one page backed by a caller-owned frame, allocating a zero
    /// partner for the other half of its pair.  Used by the console smoke test.
    pub fn install_page(&mut self, address: u32, frame: u64) -> bool {
        let vaddr = Self::pair_of(address);
        if let Some(pair) = self.pairs[..self.pair_count]
            .iter_mut()
            .find(|pair| pair.vaddr == vaddr)
        {
            if address & 0x1000 == 0 {
                pair.even = frame as u32;
            } else {
                pair.odd = frame as u32;
            }
            return true;
        }
        if self.pair_count >= MAX_PAIRS {
            return false;
        }
        let Some(partner) = frames::alloc_frame() else {
            return false;
        };
        zero_frame(partner);
        cache::invalidate_l2(frame, 4096);
        cache::invalidate_l2(partner, 4096);
        let pair = if address & 0x1000 == 0 {
            LoadedPair {
                vaddr,
                even: frame as u32,
                odd: partner as u32,
            }
        } else {
            LoadedPair {
                vaddr,
                even: partner as u32,
                odd: frame as u32,
            }
        };
        self.pairs[self.pair_count] = pair;
        self.pair_count += 1;
        self.record_range(address & !0xFFF, (address & !0xFFF) + 4096)
    }

    /// Map every page in `[start, end)` and record the range.
    fn map_range(&mut self, start: u32, end: u32) -> bool {
        if !self.record_range(start, end) {
            return false;
        }
        let start_page = Self::page_of(start);
        let end_page = Self::page_of(end.saturating_add(0xFFF));
        for address in (start_page..end_page).step_by(4096) {
            if self.ensure_pair(address).is_none() {
                return false;
            }
        }
        true
    }

    /// Allocate the frames for an ELF load plan.  The caller activates the
    /// space and calls [`fill_plan`](Self::fill_plan) to write the image.
    pub fn allocate_plan(&mut self, plan: &irix_elf::LoadPlan) -> bool {
        if self.range_count != 0 {
            return false;
        }
        for segment in plan.segments() {
            let Some(end) = segment.load_address().checked_add(segment.memory_size()) else {
                crate::println("userelf: segment address overflow");
                return false;
            };
            if !self.map_range(segment.load_address(), end) {
                return false;
            }
        }
        self.map_range(STACK_BASE, STACK_TOP)
    }

    /// Write an allocated load plan through the active user aliases.
    pub fn fill_plan(&mut self, plan: &irix_elf::LoadPlan) -> bool {
        for pair in &self.pairs[..self.pair_count] {
            cache::invalidate_l2(pair.even as u64, 4096);
            cache::invalidate_l2(pair.odd as u64, 4096);
        }
        cache::dcache_invalidate_all();
        // Zero every page the plan will touch before copying any file data.
        // Two `PT_LOAD` segments can share a page (the std image's `.text`
        // ends at `0x1001d4d8` and `.rodata` starts there), and zeroing a
        // page for the second segment after the first segment has been
        // written would erase the first segment's tail.
        for segment in plan.segments() {
            let start = Self::page_of(segment.load_address());
            let end = Self::page_of(
                segment
                    .load_address()
                    .saturating_add(segment.memory_size())
                    .saturating_add(0xFFF),
            );
            for address in (start..end).step_by(4096) {
                zero_user_page(address);
            }
        }
        for address in (STACK_BASE..STACK_TOP).step_by(4096) {
            zero_user_page(address);
        }
        for segment in plan.segments() {
            if !self.write_bytes(segment.load_address(), segment.file_data()) {
                return false;
            }
        }
        true
    }

    /// Build the conventional initial stack from borrowed argument strings.
    ///
    /// ```text
    /// sp+0:  argc
    /// sp+4:  argv[0] ... argv[argc-1], NULL
    /// sp+..: envp[0] ... envp[n-1], NULL
    /// sp+..: auxv[0].a_type, auxv[0].a_val, ..., AT_NULL, 0
    /// ```
    pub fn build_initial_stack(
        &mut self,
        entry: u32,
        arguments: &[&[u8]],
        environment: &[&[u8]],
    ) -> Option<(u32, u32, u32, u32, u32)> {
        let mut cursor = STACK_TOP;
        let mut argument_pointers = [0u32; MAX_ARGUMENT_POINTERS];
        let mut environment_pointers = [0u32; MAX_ARGUMENT_POINTERS];
        if arguments.len() > MAX_ARGUMENT_POINTERS || environment.len() > MAX_ARGUMENT_POINTERS {
            return None;
        }
        for (index, argument) in arguments.iter().enumerate() {
            argument_pointers[index] = self.push_bytes(&mut cursor, argument)?;
        }
        for (index, value) in environment.iter().enumerate() {
            environment_pointers[index] = self.push_bytes(&mut cursor, value)?;
        }

        let execfn = argument_pointers[0];
        let mut random_bytes = [0u8; 16];
        let mut seed = crate::traps::count_read()
            ^ crate::ntp::now_unix().unwrap_or(0) as u32
            ^ entry.rotate_left(11);
        for byte in &mut random_bytes {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed as u8;
        }
        let random = self.push_bytes(&mut cursor, &random_bytes)?;
        let platform = self.push_bytes(&mut cursor, b"mips\0")?;

        let argc = arguments.len() as u32;
        let envc = environment.len() as u32;
        let auxc: u32 = 16;
        let pointer_words = 1 + argc + 1 + envc + 1 + auxc * 2;
        let mut sp = (cursor & !15) - pointer_words * 4;
        sp &= !15;
        let mut at = sp;
        self.try_word(at, argc)?;
        at += 4;
        for pointer in &argument_pointers[..arguments.len()] {
            self.try_word(at, *pointer)?;
            at += 4;
        }
        self.try_word(at, 0)?;
        at += 4;
        let envp = at;
        for pointer in &environment_pointers[..environment.len()] {
            self.try_word(at, *pointer)?;
            at += 4;
        }
        self.try_word(at, 0)?;
        at += 4;
        let auxv = at;
        let aux = [
            (1u32, 4096u32), // AT_PAGESZ
            (2, entry),      // AT_ENTRY
            (3, 0),          // AT_PHDR
            (4, 0),          // AT_PHNUM
            (5, 0),          // AT_BASE
            (6, random),     // AT_RANDOM
            (7, execfn),     // AT_EXECFN
            (8, 0),          // AT_UID
            (9, 0),          // AT_EUID
            (10, 0),         // AT_GID
            (11, 0),         // AT_EGID
            (12, 0),         // AT_HWCAP
            (13, 100),       // AT_CLKTCK
            (14, 0),         // AT_SECURE
            (15, platform),  // AT_PLATFORM
            (0, 0),          // AT_NULL
        ];
        for (key, value) in aux {
            self.try_word(at, key)?;
            self.try_word(at + 4, value)?;
            at += 8;
        }
        Some((sp, argc, sp + 4, envp, auxv))
    }

    fn push_bytes(&mut self, cursor: &mut u32, bytes: &[u8]) -> Option<u32> {
        *cursor = cursor.checked_sub(bytes.len() as u32)?;
        let start = *cursor;
        if !self.write_bytes(start, bytes) {
            return None;
        }
        Some(start)
    }

    /// Allocate and map the fixed anonymous heap for this space.
    ///
    /// The heap uses two 256 KiB page pairs, so a 1 MiB window costs two JTLB
    /// entries instead of 256.  `asid` is the owning process's address-space id;
    /// the space must be the active TLB set.
    fn map_heap(&mut self, _asid: u32) -> bool {
        if self.heap_mapped {
            return true;
        }
        let Some((base, phys, frames_used)) = alloc_heap_window() else {
            return false;
        };
        self.heap_alloc_base = base;
        self.heap_alloc_frames = frames_used;
        self.heap_phys = phys;
        for index in 0..HEAP_TLB_PAGES {
            let vaddr = HEAP_BASE + index as u32 * (2 * HEAP_PAGE_SIZE);
            let paddr = phys as u32 + index as u32 * (2 * HEAP_PAGE_SIZE);
            tlb::map_page_pair_mask(
                vaddr,
                paddr,
                paddr + HEAP_PAGE_SIZE,
                0,
                HEAP_TLB_START + index as u32,
                HEAP_PAGE_MASK_256K,
                3,
                false,
            );
        }
        tlb::set_pagemask_zero();
        cache::invalidate_l2(phys, HEAP_TLB_PAGES * 2 * HEAP_PAGE_SIZE as usize);
        cache::dcache_invalidate_all();
        let heap_pages = HEAP_TLB_PAGES * 2 * (HEAP_PAGE_SIZE as usize / 4096);
        for page in 0..heap_pages {
            zero_user_page(HEAP_BASE + page as u32 * 4096);
        }
        self.heap_next = HEAP_BASE;
        self.heap_mapped = true;
        true
    }

    /// Allocate zero-filled anonymous memory from the fixed heap window.
    pub fn mmap(&mut self, length: u32, asid: u32) -> SysResult<u32> {
        if !self.map_heap(asid) {
            return Err(CELL_E_NOMEM);
        }
        let Some(length) = length.checked_add(0xFFF).map(|value| value & !0xFFF) else {
            return Err(CELL_E_INVAL);
        };
        if length == 0 {
            return Err(CELL_E_INVAL);
        }
        let start = self.heap_next;
        let Some(end) = start.checked_add(length) else {
            return Err(CELL_E_NOMEM);
        };
        if end > HEAP_LIMIT {
            return Err(CELL_E_NOMEM);
        }
        if !self.record_range(start, end) {
            return Err(CELL_E_NOMEM);
        }
        self.heap_next = end;
        Ok(start)
    }

    /// Copy every mapped frame and the range list from `parent`.
    ///
    /// The parent is active, so its pages are read through the user aliases.
    /// Each child page is then written through the child's own user alias while
    /// that space is temporarily active.  Both sides therefore use the same
    /// cache view the process will use, and no cache flush is needed.
    pub fn copy_from(&mut self, parent: &Space) -> bool {
        *self = Space::EMPTY;
        for index in 0..parent.pair_count {
            let pair = parent.pairs[index];
            let Some(even) = frames::alloc_frame() else {
                parent.activate(0);
                return false;
            };
            let Some(odd) = frames::alloc_frame() else {
                frames::free_frame(even);
                parent.activate(0);
                return false;
            };
            if parent.physical(pair.vaddr).is_none() {
                frames::free_frame(even);
                frames::free_frame(odd);
                parent.activate(0);
                return false;
            }
            let mut even_page = [0u8; 4096];
            let mut odd_page = [0u8; 4096];
            for offset in 0..4096 {
                // SAFETY: the parent is the active space and both pages are
                // mapped by its pair table.
                unsafe {
                    even_page[offset] =
                        core::ptr::read_volatile((pair.vaddr as *const u8).add(offset));
                    odd_page[offset] =
                        core::ptr::read_volatile((pair.vaddr as *const u8).add(4096 + offset));
                }
            }
            self.pairs[self.pair_count] = LoadedPair {
                vaddr: pair.vaddr,
                even: even as u32,
                odd: odd as u32,
            };
            self.pair_count += 1;
            cache::invalidate_l2(even, 4096);
            cache::invalidate_l2(odd, 4096);
            self.activate(0);
            write_user_page(pair.vaddr, &even_page);
            write_user_page(pair.vaddr + 4096, &odd_page);
            parent.activate(0);
        }
        self.ranges = parent.ranges;
        self.range_count = parent.range_count;
        self.heap_next = parent.heap_next;
        if parent.heap_mapped {
            let Some((base, phys, frames_used)) = alloc_heap_window() else {
                parent.activate(0);
                return false;
            };
            self.heap_alloc_base = base;
            self.heap_alloc_frames = frames_used;
            self.heap_phys = phys;
            self.heap_mapped = true;
            cache::invalidate_l2(phys, HEAP_TLB_PAGES * 2 * HEAP_PAGE_SIZE as usize);
            let used = parent.heap_next.saturating_sub(HEAP_BASE);
            let pages = (used as usize + 4095) / 4096;
            for page in 0..pages {
                let address = HEAP_BASE + page as u32 * 4096;
                if parent.physical(address).is_none() {
                    parent.activate(0);
                    return false;
                }
                let mut buffer = [0u8; 4096];
                for offset in 0..4096 {
                    // SAFETY: the parent heap page is mapped.
                    unsafe {
                        buffer[offset] =
                            core::ptr::read_volatile((address as *const u8).add(offset));
                    }
                }
                self.activate(0);
                write_user_page(address, &buffer);
                parent.activate(0);
            }
        }
        true
    }

    /// Install this space's page pairs into the JTLB with `asid`.
    pub fn activate(&self, _asid: u32) {
        for index in FIRST_TLB..48 {
            tlb::invalidate_index(index);
        }
        for (index, pair) in self.pairs[..self.pair_count].iter().enumerate() {
            tlb::map_page_pair(
                pair.vaddr,
                pair.even,
                pair.odd,
                0,
                FIRST_TLB + index as u32,
                3,
                false,
            );
        }
        if self.heap_mapped {
            for index in 0..HEAP_TLB_PAGES {
                let vaddr = HEAP_BASE + index as u32 * (2 * HEAP_PAGE_SIZE);
                let paddr = self.heap_phys as u32 + index as u32 * (2 * HEAP_PAGE_SIZE);
                tlb::map_page_pair_mask(
                    vaddr,
                    paddr,
                    paddr + HEAP_PAGE_SIZE,
                    0,
                    HEAP_TLB_START + index as u32,
                    HEAP_PAGE_MASK_256K,
                    3,
                    false,
                );
            }
        }
        tlb::set_pagemask_zero();
        crate::traps::entryhi_write(0);
        cache::dcache_invalidate_all();
        cache::icache_invalidate_all();
    }

    /// Free every frame this space owns.  The caller must not be using it.
    pub fn release(&mut self) {
        for index in 0..self.pair_count {
            let pair = self.pairs[index];
            frames::free_frame(pair.even as u64);
            frames::free_frame(pair.odd as u64);
        }
        if self.heap_alloc_frames != 0 {
            for index in 0..self.heap_alloc_frames {
                frames::free_frame(self.heap_alloc_base + index as u64 * 4096);
            }
        }
        *self = Space::EMPTY;
    }
}

fn zero_user_page(address: u32) {
    for offset in 0..4096 {
        // SAFETY: the caller activated the space and the page is mapped.
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(offset), 0);
        }
    }
}

fn write_user_page(address: u32, bytes: &[u8; 4096]) {
    for (offset, byte) in bytes.iter().enumerate() {
        // SAFETY: the caller activated the space and the page is mapped.
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(offset), *byte);
        }
    }
}

fn uncached(physical: u64) -> *mut u8 {
    // kseg1 is the kernel's uncached direct map.  User frames are written
    // through it; a full cache flush on the next activation makes the data
    // visible to the user alias.
    ((physical as u32) | 0xA000_0000) as *mut u8
}

fn zero_frame(physical: u64) {
    let page = uncached(physical);
    for offset in 0..4096 {
        // SAFETY: the frame belongs to the caller and is written through the
        // uncached kseg1 alias.
        unsafe {
            core::ptr::write_volatile(page.add(offset), 0);
        }
    }
}

/// Allocate a 1 MiB heap window and return `(allocation base, aligned phys,
/// frame count)`.
fn alloc_heap_window() -> Option<(u64, u64, usize)> {
    const COUNT: usize = HEAP_TLB_PAGES * 128 + 128;
    let base = frames::alloc_frames(COUNT)?;
    let phys = (base + 0x3FFFF) & !0x3FFFF;
    let span = HEAP_TLB_PAGES as u64 * 2 * HEAP_PAGE_SIZE as u64;
    if phys + span > base + (COUNT as u64 * 4096) {
        for index in 0..COUNT {
            frames::free_frame(base + index as u64 * 4096);
        }
        return None;
    }
    Some((base, phys, COUNT))
}

/// Load and enter the user ELF on target 2.
///
/// On success this never returns: a user exit comes back through the console
/// trampoline in `traps.rs`.  A load failure returns to the console wrapper.
#[no_mangle]
pub extern "C" fn user_elf() {
    let image = image_buffer();
    // READ(6) is limited to 255 sectors.  The loader uses the polled PIO path
    // in 16 KiB chunks so a failed DMA descriptor chain cannot make the retry
    // return the previous command's block.
    let mut offset = 0usize;
    while offset < image.len() {
        let chunk = (image.len() - offset).min(16 * 1024);
        if let Err(error) = scsi::read_blocks_pio(
            TARGET,
            (offset / 512) as u32,
            &mut image[offset..offset + chunk],
        ) {
            crate::print_str("userelf: read failed: ");
            crate::println(error);
            return;
        }
        offset += chunk;
    }

    run_image(image);
}

/// Load and enter an image already staged in [`ELF_BUF`].
///
/// `image` may be shorter than the buffer; only the supplied bytes are parsed.
/// On success this does not return; on a load failure it returns to the
/// caller.  The caller must have set up the console trampoline when entering
/// from a console command.
#[no_mangle]
pub extern "C" fn user_loaded() {
    let image = image_buffer();
    run_image(image);
}

/// Shared loader body for target-2 and in-RAM userland images.
/// Stage the child executable from SCSI target 3 into `/bin/child`.
///
/// The image is optional: a missing target leaves the node empty and `execve`
/// of `/bin/child` reports the failure, but every other userland probe and the
/// boot gate continue.  The ramfs node is created by `ramfs::reset`; this only
/// fills the backing buffer.  It runs after the target-2 image has been read,
/// so a failed target-3 selection cannot disturb that transfer.
pub fn stage_child_image() {
    let image = ramfs::image_buffer();
    let mut offset = 0usize;
    while offset < image.len() {
        let chunk = (image.len() - offset).min(16 * 1024);
        if scsi::read_blocks_pio(3, (offset / 512) as u32, &mut image[offset..offset + chunk])
            .is_err()
        {
            crate::print_str("kernel: no child image on SCSI target 3\n");
            return;
        }
        offset += chunk;
    }
    if ramfs::set_image_size(image.len()) && ramfs::install_image() {
        crate::print_str("kernel: staged /bin/child from SCSI target 3\n");
    }
}

pub fn run_image(image: &[u8]) {
    cleanup();
    stage_child_image();
    let elf = match Elf::parse(image) {
        Ok(elf) => elf,
        Err(error) => {
            crate::print_str("userelf: parse failed: ");
            crate::println(error_text(error));
            return;
        }
    };
    let user_abi = match elf.abi() {
        Abi::N32 => crate::traps::UserAbi::N32,
        Abi::O32 => crate::traps::UserAbi::O32,
        _ => {
            crate::print_str("userelf: not an o32/n32 ELF\n");
            return;
        }
    };
    let plan = match elf.load_plan() {
        Ok(plan) => plan,
        Err(error) => {
            crate::print_str("userelf: load plan failed: ");
            crate::println(error_text(error));
            return;
        }
    };

    let mut space = Space::EMPTY;
    if !space.allocate_plan(&plan) {
        space.release();
        crate::println("userelf: load failed");
        return;
    }
    space.activate(0);
    if !space.fill_plan(&plan) {
        space.release();
        crate::println("userelf: image fill failed");
        return;
    }
    let arguments: [&[u8]; 1] = [b"hello\0"];
    let environment: [&[u8]; 2] = [b"HOME=/\0", b"PATH=/bin\0"];
    let Some((user_sp, argc, argv, envp, auxv)) =
        space.build_initial_stack(plan.entry(), &arguments, &environment)
    else {
        space.release();
        crate::println("userelf: stack build failed");
        return;
    };
    cache::sync();
    crate::print_str("userelf: entry=0x");
    crate::print_hex32(plan.entry());
    crate::println("");
    traps::run_user_abi(
        space,
        user_abi,
        plan.entry(),
        user_sp,
        argc,
        argv,
        envp,
        auxv,
    )
}

/// Release every frame the current address space owns and clear the user TLB
/// set.  The process table calls this at exit and after a failed load.
pub fn release_current_space() {
    if let Some(space) = process::current_space_mut() {
        space.release();
    }
}

/// Reset the user world and the console descriptor machinery.
///
/// The exit trap and the console call this before the kernel console runs
/// again.  It is idempotent.
pub fn cleanup() {
    if let Some(space) = process::current_space_mut() {
        space.release();
    }
    process::reset();
    fd::reset();
    for index in 4..48 {
        tlb::invalidate_index(index);
    }
    tlb::set_pagemask_zero();
    cache::sync();
}

/// Copy a NUL-terminated path out of user memory.
fn read_path(address: u32) -> SysResult<([u8; 64], usize)> {
    let mut path = [0u8; 64];
    let mut length = 0;
    while length < path.len() {
        if !range_ok(address + length as u32, 1) {
            return Err(CELL_E_FAULT);
        }
        let byte = unsafe { core::ptr::read_volatile((address as *const u8).add(length)) };
        if byte == 0 {
            return Ok((path, length));
        }
        path[length] = byte;
        length += 1;
    }
    Err(CELL_E_NAMETOOLONG)
}

/// Open a path in the ramfs namespace.  `mode` applies only to `O_CREAT`.
pub fn open_path(address: u32, flags: u32, mode: u32) -> SysResult<i32> {
    let (path, length) = read_path(address)?;
    crate::fd::open_path(&path[..length], flags, mode)
}

/// Remove a name; `remove_dir` selects `rmdir` semantics.
pub fn unlink_user(address: u32, remove_dir: bool) -> SysResult<()> {
    let (path, length) = read_path(address)?;
    crate::fd::unlink(&path[..length], remove_dir)
}

/// Rename `old` to `new`.
pub fn rename_user(old_address: u32, new_address: u32) -> SysResult<()> {
    let (old, old_length) = read_path(old_address)?;
    let (new, new_length) = read_path(new_address)?;
    crate::fd::rename(&old[..old_length], &new[..new_length])
}

/// Create a directory.
pub fn mkdir_user(address: u32, mode: u32) -> SysResult<()> {
    let (path, length) = read_path(address)?;
    crate::fd::mkdir(&path[..length], mode)
}

/// `openat(dirfd, path, flags, mode)`.
pub fn openat_user(dirfd: u32, address: u32, flags: u32, mode: u32) -> SysResult<i32> {
    let start = crate::fd::dir_node(dirfd as i32)?;
    let (path, length) = read_path(address)?;
    crate::fd::open_path_at(start, &path[..length], flags, mode)
}

/// `fstatat(dirfd, path, stat_block, flags)`.  `flags` has no bits defined in
/// v0: there are no symlinks, so `AT_SYMLINK_NOFOLLOW` has no effect, and an
/// empty path with `AT_EMPTY_PATH` is not implemented.
pub fn fstatat_user(dirfd: u32, address: u32, stat_address: u32, _flags: u32) -> SysResult<()> {
    let start = crate::fd::dir_node(dirfd as i32)?;
    let (path, length) = read_path(address)?;
    let (mode, size, ino) = crate::fd::metadata_path_at(start, &path[..length])?;
    fill_stat(stat_address, mode, size, ino)
}

/// `mkdirat(dirfd, path, mode)`.
pub fn mkdirat_user(dirfd: u32, address: u32, mode: u32) -> SysResult<()> {
    let start = crate::fd::dir_node(dirfd as i32)?;
    let (path, length) = read_path(address)?;
    crate::fd::mkdir_at(start, &path[..length], mode)
}

/// `unlinkat(dirfd, path, flags)`; `AT_REMOVEDIR` selects `rmdir` semantics.
pub fn unlinkat_user(dirfd: u32, address: u32, flags: u32) -> SysResult<()> {
    let start = crate::fd::dir_node(dirfd as i32)?;
    let (path, length) = read_path(address)?;
    crate::fd::unlink_at(
        start,
        &path[..length],
        flags & native_abi::AT_REMOVEDIR as u32 != 0,
    )
}

/// `renameat2(olddirfd, oldpath, rename_at *, flags)`.  `flags` must be 0:
/// `RENAME_NOREPLACE`/`RENAME_EXCHANGE`/`RENAME_WHITEOUT` are not implemented.
pub fn renameat2_user(old_dirfd: u32, old_address: u32, block_address: u32) -> SysResult<()> {
    if !range_ok(
        block_address,
        core::mem::size_of::<native_abi::rename_at>() as u32,
    ) {
        return Err(CELL_E_FAULT);
    }
    let new_dirfd = unsafe { core::ptr::read_volatile(block_address as *const u32) };
    let new_address = unsafe { core::ptr::read_volatile((block_address + 4) as *const u32) };
    let flags = unsafe { core::ptr::read_volatile((block_address + 8) as *const u32) };
    if flags != 0 {
        return Err(crate::syserr::CELL_E_INVAL);
    }
    let old_start = crate::fd::dir_node(old_dirfd as i32)?;
    let new_start = crate::fd::dir_node(new_dirfd as i32)?;
    let (old, old_length) = read_path(old_address)?;
    let (new, new_length) = read_path(new_address)?;
    crate::fd::rename_at(old_start, &old[..old_length], new_start, &new[..new_length])
}

/// Assemble the 64-bit offset split across two argument registers.
fn split_offset(low: u32, high: u32) -> i64 {
    (((high as u64) << 32) | low as u64) as i64
}

/// Set the length of an open regular file.
pub fn ftruncate_user(fd: u32, low: u32, high: u32) -> SysResult<()> {
    crate::fd::ftruncate(fd as i32, split_offset(low, high))
}

/// Set the length of a regular file by path.
pub fn truncate_user(address: u32, low: u32, high: u32) -> SysResult<()> {
    let (path, length) = read_path(address)?;
    crate::fd::truncate(&path[..length], split_offset(low, high))
}

/// Copy up to `limit` bytes from a validated user range.
pub fn read_user(address: u32, length: u32, buffer: &mut [u8]) -> Option<usize> {
    let count = (length as usize).min(buffer.len());
    if !range_ok(address, count as u32) {
        return None;
    }
    for (index, byte) in buffer[..count].iter_mut().enumerate() {
        *byte = unsafe { core::ptr::read_volatile((address as *const u8).add(index)) };
    }
    Some(count)
}

/// Whether `[address, address + length)` is inside the current process's
/// mapped ranges.
pub fn range_ok(address: u32, length: u32) -> bool {
    process::current_space()
        .map(|space| space.range_ok(address, length))
        .unwrap_or(false)
}

/// Allocate anonymous memory from the current process's heap.
pub fn mmap(length: u32) -> SysResult<u32> {
    let asid = process::current_asid();
    process::with_current_space_mut(|space| space.mmap(length, asid)).ok_or(CELL_E_NOMEM)?
}

/// Write to a bring-up descriptor.  `Console` goes to the serial/graphics
/// console; `Null` accepts and discards.
pub fn write_fd(fd: u32, address: u32, length: u32) -> SysResult<u32> {
    let kind = crate::fd::kind(fd as i32);
    match kind {
        crate::fd::Kind::Console => {
            let mut buffer = [0u8; 512];
            let mut done = 0usize;
            while done < length as usize {
                let chunk = (length as usize - done).min(buffer.len());
                if !range_ok(address + done as u32, chunk as u32) {
                    return Err(CELL_E_FAULT);
                }
                for (index, byte) in buffer[..chunk].iter_mut().enumerate() {
                    *byte = unsafe {
                        core::ptr::read_volatile((address as *const u8).add(done + index))
                    };
                }
                crate::write_bytes(&buffer[..chunk]);
                done += chunk;
            }
            crate::graphics_console::flush();
            Ok(length)
        }
        crate::fd::Kind::Null => Ok(length),
        crate::fd::Kind::File(_) | crate::fd::Kind::PipeWrite(_) => {
            let mut buffer = [0u8; 512];
            let mut done = 0usize;
            while done < length as usize {
                let chunk = (length as usize - done).min(buffer.len());
                if !range_ok(address + done as u32, chunk as u32) {
                    return Err(CELL_E_FAULT);
                }
                for (index, byte) in buffer[..chunk].iter_mut().enumerate() {
                    *byte = unsafe {
                        core::ptr::read_volatile((address as *const u8).add(done + index))
                    };
                }
                let count = crate::fd::write(fd as i32, &buffer[..chunk])?;
                done += count;
                if count < chunk {
                    break;
                }
            }
            Ok(done as u32)
        }
        crate::fd::Kind::Dir(_) => Err(crate::syserr::CELL_E_ISDIR),
        crate::fd::Kind::Logical(_) => Err(crate::syserr::CELL_E_ROFS),
        crate::fd::Kind::PipeRead(_) => Err(crate::syserr::CELL_E_BADF),
        crate::fd::Kind::Closed => Err(crate::syserr::CELL_E_BADF),
    }
}

/// `poll(fds, count, timeout)` over the console, pipes and files.
pub fn poll_user(address: u32, count: u32, timeout: i32) -> SysResult<u32> {
    use native_abi::pollfd;

    let record = core::mem::size_of::<pollfd>() as u32;
    let bytes = count.checked_mul(record).ok_or(CELL_E_INVAL)?;
    // A count of zero has no descriptor array; the pointer is not inspected.
    if bytes != 0 && !range_ok(address, bytes) {
        return Err(CELL_E_FAULT);
    }
    let wait_ms = if timeout < 0 {
        crate::fd::WAIT_LIMIT_MS
    } else {
        (timeout as u32).min(crate::fd::WAIT_LIMIT_MS)
    };
    let limit = wait_ms.saturating_mul(crate::fd::TICKS_PER_MS);
    let start = crate::traps::count_read();
    loop {
        let mut ready = 0u32;
        for index in 0..count as usize {
            let base = address + (index as u32) * record;
            let fd = unsafe { core::ptr::read_volatile(base as *const i32) };
            let events = unsafe { core::ptr::read_volatile((base + 4) as *const i16) };
            let revents = crate::fd::poll_events(fd, events);
            unsafe {
                core::ptr::write_volatile((base + 6) as *mut i16, revents);
            }
            if revents != 0 {
                ready += 1;
            }
        }
        if ready != 0 {
            return Ok(ready);
        }
        let elapsed = crate::traps::count_read().wrapping_sub(start);
        if elapsed >= limit {
            return Ok(0);
        }
        // Park the CPU on the timer instead of spinning.  This wait is
        // syscall-local: the gate has no continuation, so it cannot install
        // another cell and resume the scan afterwards.  The bound keeps the
        // call finite and the timeout still means "no readiness in this
        // window".
        let slice = crate::fd::WAIT_SLICE_TICKS.min(limit - elapsed);
        crate::traps::wait_until(crate::traps::count_read().wrapping_add(slice));
    }
}

pub fn getcwd_user(address: u32, size: u32) -> SysResult<()> {
    if size < 2 || !range_ok(address, 2) {
        return Err(CELL_E_FAULT);
    }
    unsafe {
        core::ptr::write_volatile(address as *mut u8, b'/');
        core::ptr::write_volatile((address + 1) as *mut u8, 0);
    }
    Ok(())
}

pub fn clock_user(clock: u32, address: u32) -> SysResult<()> {
    if clock > 1 {
        return Err(CELL_E_INVAL);
    }
    if !range_ok(address, 16) {
        return Err(CELL_E_FAULT);
    }
    let (seconds, nanos) = match clock {
        // CLOCK_REALTIME: the RTC/NTP wall clock.
        0 => (crate::ntp::now_unix().unwrap_or(0), 0),
        // CLOCK_MONOTONIC: CP0 Count at the Indy's 33 MHz rate.
        _ => {
            let count = crate::traps::count_read() as u64;
            (
                count / 33_000_000,
                (count % 33_000_000) * 1_000_000_000 / 33_000_000,
            )
        }
    };
    unsafe {
        core::ptr::write_volatile(address as *mut u64, seconds);
        core::ptr::write_volatile((address + 8) as *mut i32, nanos as i32);
        core::ptr::write_volatile((address + 12) as *mut u32, 0);
    }
    Ok(())
}

pub fn entropy_user(address: u32, length: u32) -> SysResult<()> {
    if !range_ok(address, length) {
        return Err(CELL_E_FAULT);
    }
    for index in 0..length as usize {
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(index), 0);
        }
    }
    Ok(())
}

/// POSIX `tcgetwinsize`: copy the 8-byte winsize from the console.
pub fn tcgetwinsize_user(fd: u32, address: u32) -> SysResult<()> {
    if !range_ok(address, 8) {
        return Err(CELL_E_FAULT);
    }
    let mut value = [0u16; 4];
    crate::fd::get_winsize(fd as i32, &mut value)?;
    for (index, field) in value.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile((address + index as u32 * 2) as *mut u16, *field);
        }
    }
    Ok(())
}

/// POSIX `tcsetwinsize`: update the console size from the 8-byte winsize.
pub fn tcsetwinsize_user(fd: u32, address: u32) -> SysResult<()> {
    if !range_ok(address, 8) {
        return Err(CELL_E_FAULT);
    }
    let mut value = [0u16; 4];
    for (index, field) in value.iter_mut().enumerate() {
        *field = unsafe { core::ptr::read_volatile((address + index as u32 * 2) as *const u16) };
    }
    crate::fd::set_winsize(fd as i32, &value)
}

/// `ioctl(fd, request, arg)`: the terminal requests on a console descriptor.
pub fn ioctl_user(fd: u32, request: u32, argument: u32) -> SysResult<()> {
    use native_abi::{
        termios, TCGETS, TCSETS, TCSETSF, TCSETSW, TIOCGPGRP, TIOCGWINSZ, TIOCSPGRP, TIOCSWINSZ,
    };

    match crate::fd::kind(fd as i32) {
        crate::fd::Kind::Console => {}
        crate::fd::Kind::Closed => return Err(crate::syserr::CELL_E_BADF),
        _ => return Err(crate::syserr::CELL_E_NOTTY),
    }
    let size = core::mem::size_of::<termios>();
    match request {
        TCGETS => {
            if !range_ok(argument, size as u32) {
                return Err(CELL_E_FAULT);
            }
            let value = crate::tty::termios();
            let source = &value as *const termios as *const u8;
            for index in 0..size {
                unsafe {
                    core::ptr::write_volatile(
                        (argument as *mut u8).add(index),
                        core::ptr::read(source.add(index)),
                    );
                }
            }
            Ok(())
        }
        TCSETS | TCSETSW | TCSETSF => {
            if !range_ok(argument, size as u32) {
                return Err(CELL_E_FAULT);
            }
            let mut value = crate::tty::termios();
            let target = &mut value as *mut termios as *mut u8;
            for index in 0..size {
                unsafe {
                    core::ptr::write(
                        target.add(index),
                        core::ptr::read_volatile((argument as *const u8).add(index)),
                    );
                }
            }
            crate::tty::set_termios(value, request == TCSETSF);
            Ok(())
        }
        TIOCGWINSZ => tcgetwinsize_user(fd, argument),
        TIOCSWINSZ => tcsetwinsize_user(fd, argument),
        TIOCGPGRP => {
            if !range_ok(argument, 4) {
                return Err(CELL_E_FAULT);
            }
            unsafe {
                core::ptr::write_volatile(argument as *mut u32, crate::tty::foreground());
            }
            Ok(())
        }
        TIOCSPGRP => {
            if !range_ok(argument, 4) {
                return Err(CELL_E_FAULT);
            }
            let group = unsafe { core::ptr::read_volatile(argument as *const u32) };
            if !process::group_exists(group) {
                return Err(crate::syserr::CELL_E_PERM);
            }
            crate::tty::set_foreground(group);
            Ok(())
        }
        _ => Err(crate::syserr::CELL_E_NOTTY),
    }
}

/// `sigaction(sig, disposition, old)`: both pointers name a 32-bit word.
pub fn sigaction_user(signal: u32, new_address: u32, old_address: u32) -> SysResult<()> {
    if old_address != 0 && !range_ok(old_address, 4) {
        return Err(CELL_E_FAULT);
    }
    let new = if new_address != 0 {
        if !range_ok(new_address, 4) {
            return Err(CELL_E_FAULT);
        }
        Some(unsafe { core::ptr::read_volatile(new_address as *const u32) })
    } else {
        None
    };
    let old = process::set_disposition(signal, new)?;
    if old_address != 0 {
        unsafe {
            core::ptr::write_volatile(old_address as *mut u32, old);
        }
    }
    Ok(())
}

/// Create a pipe and write `[read_fd, write_fd]` to the user buffer.
pub fn pipe_user(address: u32) -> SysResult<()> {
    pipe2_user(address, 0)
}

/// Create a pipe with `flags` (`O_CLOEXEC`, `O_NONBLOCK`) applied.
pub fn pipe2_user(address: u32, flags: u32) -> SysResult<()> {
    if !range_ok(address, 8) {
        return Err(CELL_E_FAULT);
    }
    let (read, write) = crate::fd::pipe2(flags)?;
    unsafe {
        core::ptr::write_volatile(address as *mut i32, read);
        core::ptr::write_volatile((address + 4) as *mut i32, write);
    }
    Ok(())
}

/// Duplicate a descriptor onto a chosen number.
pub fn dup2_user(fd: u32, new_fd: u32) -> SysResult<i32> {
    crate::fd::dup2(fd as i32, new_fd as i32)
}

/// `fcntl` with the v0 descriptor-flag commands.
pub fn fcntl_user(fd: u32, cmd: u32, arg: u32) -> SysResult<i32> {
    crate::fd::fcntl(fd as i32, cmd, arg)
}

/// `lseek(fd, offset, whence)` with the 64-bit offset split across `$a1`/`$a2`.
pub fn lseek_user(fd: u32, low: u32, high: u32, whence: u32) -> SysResult<i64> {
    crate::fd::lseek(fd as i32, split_offset(low, high), whence)
}

/// Read the 16-byte `io_at` block `pread`/`pwrite` take in `$a1`.
fn read_io_at(address: u32) -> SysResult<(u32, u32, i64)> {
    if !range_ok(address, core::mem::size_of::<native_abi::io_at>() as u32) {
        return Err(CELL_E_FAULT);
    }
    let buffer = unsafe { core::ptr::read_volatile(address as *const u32) };
    let length = unsafe { core::ptr::read_volatile((address + 4) as *const u32) };
    let offset = unsafe { core::ptr::read_volatile((address + 8) as *const i64) };
    if !range_ok(buffer, length) {
        return Err(CELL_E_FAULT);
    }
    Ok((buffer, length, offset))
}

/// `pread(fd, io_at *)`.
pub fn pread_user(fd: u32, block: u32) -> SysResult<u32> {
    let (address, length, offset) = read_io_at(block)?;
    let mut buffer = [0u8; 512];
    let mut total = 0usize;
    while total < length as usize {
        let chunk = (length as usize - total).min(buffer.len());
        let Some(position) = offset.checked_add(total as i64) else {
            return Err(CELL_E_OVERFLOW);
        };
        let count = crate::fd::pread(fd as i32, position, &mut buffer[..chunk])?;
        if count == 0 {
            break;
        }
        for (index, byte) in buffer[..count].iter().enumerate() {
            unsafe {
                core::ptr::write_volatile((address as *mut u8).add(total + index), *byte);
            }
        }
        total += count;
        if count < chunk {
            break;
        }
    }
    Ok(total as u32)
}

/// `pwrite(fd, io_at *)`.
pub fn pwrite_user(fd: u32, block: u32) -> SysResult<u32> {
    let (address, length, offset) = read_io_at(block)?;
    let mut buffer = [0u8; 512];
    let mut total = 0usize;
    while total < length as usize {
        let chunk = (length as usize - total).min(buffer.len());
        let Some(position) = offset.checked_add(total as i64) else {
            return Err(CELL_E_OVERFLOW);
        };
        for (index, byte) in buffer[..chunk].iter_mut().enumerate() {
            *byte = unsafe { core::ptr::read_volatile((address as *const u8).add(total + index)) };
        }
        let count = crate::fd::pwrite(fd as i32, position, &buffer[..chunk])?;
        if count == 0 {
            break;
        }
        total += count;
        if count < chunk {
            break;
        }
    }
    Ok(total as u32)
}

/// `getdents(fd, buf, len)`: fill whole `dirent` records and advance the
/// descriptor's cookie.
pub fn getdents_user(fd: u32, address: u32, length: u32) -> SysResult<u32> {
    let record = core::mem::size_of::<native_abi::dirent>();
    if (length as usize) < record {
        return Err(CELL_E_INVAL);
    }
    if !range_ok(address, length) {
        return Err(CELL_E_FAULT);
    }
    let mut scratch = [0u8; 1024];
    let mut total = 0usize;
    while (length as usize) - total >= record {
        let chunk = ((length as usize) - total).min(scratch.len());
        let count = crate::fd::getdents(fd as i32, &mut scratch[..chunk])?;
        if count == 0 {
            break;
        }
        for (index, byte) in scratch[..count].iter().enumerate() {
            unsafe {
                core::ptr::write_volatile((address as *mut u8).add(total + index), *byte);
            }
        }
        total += count;
        if total >= length as usize {
            break;
        }
    }
    Ok(total as u32)
}

/// Read up to `length` bytes from a descriptor, parking the CPU between
/// readiness checks.
///
/// `fd::read` never waits: an empty pipe or idle console is `CELL_E_AGAIN`.
/// A blocking descriptor retries with a parked timer slice until the
/// `WAIT_LIMIT_MS` bound, then reports `EAGAIN`; a non-blocking one reports it
/// at once.  Once any bytes have been read a short count is returned rather
/// than waiting for the rest, so a blocking read cannot lose data it already
/// copied out.  The wait stays inside this gate: it cannot hand the CPU to
/// another cell, which needs the M9.5 readiness wait queue.
pub fn read_fd(fd: u32, address: u32, length: u32) -> SysResult<u32> {
    read_fd_with(fd, address, length, true)
}

/// One read attempt for the native gate.
///
/// An empty pipe or an idle console is `CELL_E_AGAIN` at once; the gate
/// restarts the call for a blocking descriptor so another cell can run.
pub fn read_fd_once(fd: u32, address: u32, length: u32) -> SysResult<u32> {
    read_fd_with(fd, address, length, false)
}

fn read_fd_with(fd: u32, address: u32, length: u32, wait: bool) -> SysResult<u32> {
    if !range_ok(address, length) {
        return Err(CELL_E_FAULT);
    }
    let start = crate::traps::count_read();
    let mut buffer = [0u8; 512];
    let mut total = 0usize;
    while total < length as usize {
        let chunk = (length as usize - total).min(buffer.len());
        let count = match crate::fd::read(fd as i32, &mut buffer[..chunk]) {
            Ok(count) => count,
            Err(CELL_E_AGAIN) => {
                // A short read is legal and avoids re-reading bytes that were
                // already copied into the caller's buffer.
                if total != 0 {
                    break;
                }
                if !wait || crate::fd::nonblocking(fd as i32) {
                    return Err(CELL_E_AGAIN);
                }
                let elapsed = crate::traps::count_read().wrapping_sub(start);
                let limit = crate::fd::WAIT_LIMIT_MS * crate::fd::TICKS_PER_MS;
                if elapsed >= limit {
                    return Err(CELL_E_AGAIN);
                }
                let slice = crate::fd::WAIT_SLICE_TICKS.min(limit - elapsed);
                crate::traps::wait_until(crate::traps::count_read().wrapping_add(slice));
                continue;
            }
            Err(status) => return Err(status),
        };
        if count == 0 {
            break;
        }
        for (index, byte) in buffer[..count].iter().enumerate() {
            unsafe {
                core::ptr::write_volatile((address as *mut u8).add(total + index), *byte);
            }
        }
        total += count;
        if count < chunk {
            break;
        }
    }
    Ok(total as u32)
}

/// Native n32 `struct stat`, shared with relibc through `native-abi`.
fn fill_stat(address: u32, mode: u32, size: i64, ino: u64) -> SysResult<()> {
    if !range_ok(address, core::mem::size_of::<native_abi::stat>() as u32) {
        return Err(CELL_E_FAULT);
    }
    let timestamp = native_abi::timespec {
        tv_sec: 0,
        tv_nsec: 0,
        _pad: 0,
    };
    let stat = native_abi::stat {
        st_dev: 1,
        st_ino: ino,
        st_nlink: 1,
        st_mode: mode,
        st_uid: 0,
        st_gid: 0,
        st_rdev: 0,
        st_size: size,
        st_blksize: 4096,
        _pad0: 0,
        st_blocks: (size + 511) / 512,
        st_atim: timestamp,
        st_mtim: timestamp,
        st_ctim: timestamp,
        _pad: [0; 24],
    };
    let source = &stat as *const native_abi::stat as *const u8;
    for index in 0..core::mem::size_of::<native_abi::stat>() {
        unsafe {
            core::ptr::write_volatile(
                (address as *mut u8).add(index),
                core::ptr::read_volatile(source.add(index)),
            );
        }
    }
    Ok(())
}

pub fn fstat_user(fd: u32, address: u32) -> SysResult<()> {
    let (mode, size, ino) = crate::fd::metadata(fd as i32)?;
    fill_stat(address, mode, size, ino)
}

pub fn stat_user(path_address: u32, stat_address: u32) -> SysResult<()> {
    let (path, length) = read_path(path_address)?;
    let (mode, size, ino) = crate::fd::metadata_path(&path[..length])?;
    fill_stat(stat_address, mode, size, ino)
}

/// Copy the mesh-qualified cell identity into a user buffer.
pub fn cell_id_user(address: u32) -> SysResult<()> {
    let size = core::mem::size_of::<native_abi::cell_id>() as u32;
    if !range_ok(address, size) {
        return Err(CELL_E_FAULT);
    }
    let Some(identity) = process::current_identity() else {
        return Err(CELL_E_SRCH);
    };
    let record = identity.to_abi();
    let source = &record as *const native_abi::cell_id as *const u8;
    for index in 0..size as usize {
        unsafe {
            core::ptr::write_volatile(
                (address as *mut u8).add(index),
                core::ptr::read_volatile(source.add(index)),
            );
        }
    }
    Ok(())
}

/// Copy one static directory entry into a user buffer.  Returns the dirent
/// type (4 directory, 8 regular file, 2 device) or `None` at end of directory.
pub fn readdir_user(fd: u32, index: u32, address: u32, length: u32) -> SysResult<Option<u32>> {
    if !range_ok(address, length) {
        return Err(CELL_E_FAULT);
    }
    let mut buffer = [0u8; 256];
    let Some((kind, _)) = crate::fd::readdir(fd as i32, index as usize, &mut buffer)? else {
        return Ok(None);
    };
    let name_len = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    if name_len + 1 > length as usize {
        return Err(CELL_E_INVAL);
    }
    for index in 0..=name_len {
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(index), buffer[index]);
        }
    }
    Ok(Some(kind as u32))
}

/// Longest a single parked sleep lasts, in milliseconds.
///
/// A caller that asks for longer re-arms with the remainder, which the PAL
/// does so `nanosleep` still honours the full interval.  The bound keeps every
/// interval shorter than half the 32-bit Count range, so the wrap-safe
/// deadline comparison stays unambiguous.
pub const SLEEP_LIMIT_MS: u32 = 60_000;

/// Parse a user `timespec` as a sleep interval in CP0 Count ticks.
///
/// The interval is relative, so a negative field is `CELL_E_INVAL` and an
/// absurd one is capped at [`SLEEP_LIMIT_MS`] rather than overflowing.
pub fn sleep_ticks(address: u32) -> SysResult<u32> {
    if !range_ok(address, 16) {
        return Err(CELL_E_FAULT);
    }
    let seconds = unsafe { core::ptr::read_volatile(address as *const i64) };
    let nanos = unsafe { core::ptr::read_volatile((address + 8) as *const i32) };
    if seconds < 0 || nanos < 0 || nanos >= 1_000_000_000 {
        return Err(CELL_E_INVAL);
    }
    let ticks = (seconds as u64)
        .saturating_mul(33_000_000)
        .saturating_add((nanos as u64).saturating_mul(33) / 1_000);
    let cap = u64::from(SLEEP_LIMIT_MS) * u64::from(crate::fd::TICKS_PER_MS);
    Ok(ticks.min(cap) as u32)
}

/// Legacy `nanosleep`: wait the interval in the kernel without a thread
/// switch.  The legacy gate has no scheduler, so there is no peer to park
/// for; the timer still drives the wait and the CPU is idle rather than
/// spinning.
pub fn sleep_user(address: u32) -> SysResult<()> {
    let limit = sleep_ticks(address)?;
    crate::traps::wait_ticks(limit);
    Ok(())
}

pub fn chdir_user(address: u32) -> SysResult<()> {
    let (path, length) = read_path(address)?;
    crate::fd::set_cwd(&path[..length])
}

pub fn getcwd_text(address: u32, size: u32) -> SysResult<()> {
    let mut path = [0u8; 64];
    let length = crate::fd::copy_cwd(&mut path).ok_or(CELL_E_NAMETOOLONG)?;
    if size as usize <= length || !range_ok(address, (length + 1) as u32) {
        return Err(CELL_E_INVAL);
    }
    for index in 0..length {
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(index), path[index]);
        }
    }
    unsafe {
        core::ptr::write_volatile((address as *mut u8).add(length), 0);
    }
    Ok(())
}

fn error_text(error: irix_elf::Error) -> &'static str {
    match error {
        irix_elf::Error::Truncated => "truncated",
        irix_elf::Error::NoLoadableSegments => "no loadable segments",
        _ => "invalid ELF",
    }
}

/// A staged executable loaded out of the ramfs, ready to be installed.
pub struct Executable {
    pub space: Space,
    pub abi: traps::UserAbi,
    pub entry: u32,
    pub sp: u32,
    pub argc: u32,
    pub argv: u32,
    pub envp: u32,
    pub auxv: u32,
}

/// One `argv` or `envp` vector copied out of the old address space.
struct Vector {
    data: [u8; ARG_BYTES],
    used: usize,
    offsets: [(u32, u32); MAX_ARGUMENT_POINTERS],
    count: usize,
}

impl Vector {
    const EMPTY: Self = Self {
        data: [0; ARG_BYTES],
        used: 0,
        offsets: [(0, 0); MAX_ARGUMENT_POINTERS],
        count: 0,
    };

    /// Copy a user string vector into kernel memory.  A null pointer is an
    /// empty vector; a non-null vector must end in a null pointer.
    fn copy(address: u32) -> SysResult<Self> {
        let mut vector = Self::EMPTY;
        if address == 0 {
            return Ok(vector);
        }
        while vector.count < MAX_ARGUMENT_POINTERS {
            let slot = address + (vector.count as u32) * 4;
            if !range_ok(slot, 4) {
                return Err(CELL_E_FAULT);
            }
            let pointer = unsafe { core::ptr::read_volatile(slot as *const u32) };
            if pointer == 0 {
                return Ok(vector);
            }
            let start = vector.used;
            let mut length = 0usize;
            loop {
                if vector.used >= vector.data.len() {
                    return Err(CELL_E_NAMETOOLONG);
                }
                if !range_ok(pointer + length as u32, 1) {
                    return Err(CELL_E_FAULT);
                }
                let byte = unsafe { core::ptr::read_volatile((pointer as *const u8).add(length)) };
                vector.data[vector.used] = byte;
                vector.used += 1;
                if byte == 0 {
                    break;
                }
                length += 1;
            }
            vector.offsets[vector.count] = (start as u32, length as u32);
            vector.count += 1;
        }
        Err(CELL_E_2BIG)
    }

    fn strings(&self) -> [&[u8]; MAX_ARGUMENT_POINTERS] {
        let mut out: [&[u8]; MAX_ARGUMENT_POINTERS] = [b""; MAX_ARGUMENT_POINTERS];
        for (index, &(start, length)) in self.offsets[..self.count].iter().enumerate() {
            out[index] = &self.data[start as usize..start as usize + length as usize + 1];
        }
        out
    }
}

/// Copy a ramfs file into the shared ELF buffer and return its length.
fn read_image_node(node: u16) -> SysResult<usize> {
    let buffer = image_buffer();
    let mut total = 0usize;
    loop {
        let count = ramfs::read(node, total, &mut buffer[total..])?;
        if count == 0 {
            break;
        }
        total += count;
        if total == buffer.len() {
            break;
        }
    }
    Ok(total)
}

/// Load `path` from the ramfs and build a new address space for `argv`/`envp`.
///
/// The old address space is untouched.  On success the caller installs the
/// returned [`Executable`]; on failure every frame allocated here is freed.
pub fn prepare_exec(
    path_address: u32,
    argv_address: u32,
    envp_address: u32,
) -> SysResult<Executable> {
    let (path, path_length) = read_path(path_address)?;
    let node = match ramfs::resolve(fd::cwd(), &path[..path_length])? {
        ramfs::Resolved::Node(node) => node,
        ramfs::Resolved::Missing { .. } => return Err(crate::syserr::CELL_E_NOENT),
    };
    if ramfs::node_type(node) == ramfs::NodeType::Dir {
        return Err(crate::syserr::CELL_E_ISDIR);
    }

    let arguments = Vector::copy(argv_address)?;
    let environment = Vector::copy(envp_address)?;

    let length = read_image_node(node)?;
    if length == 0 {
        return Err(CELL_E_NOEXEC);
    }
    // The image is in ELF_BUF; parse and copy the load plan before anything
    // else can reuse the buffer.
    let image =
        &crate::bigbuf::get(unsafe { &mut *core::ptr::addr_of_mut!(ELF_BUF) }, ELF_SIZE)[..length];
    let elf = Elf::parse(image).map_err(|_| CELL_E_NOEXEC)?;
    let abi = match elf.abi() {
        Abi::N32 => traps::UserAbi::N32,
        Abi::O32 => traps::UserAbi::O32,
        _ => return Err(CELL_E_NOEXEC),
    };
    let plan = elf.load_plan().map_err(|_| CELL_E_NOEXEC)?;

    let mut space = Space::EMPTY;
    if !space.allocate_plan(&plan) {
        space.release();
        return Err(CELL_E_NOMEM);
    }
    space.activate(0);
    if !space.fill_plan(&plan) {
        space.release();
        return Err(CELL_E_NOMEM);
    }
    let argument_strings = arguments.strings();
    let environment_strings = environment.strings();
    let Some((sp, argc, argv, envp, auxv)) = space.build_initial_stack(
        plan.entry(),
        &argument_strings[..arguments.count],
        &environment_strings[..environment.count],
    ) else {
        space.release();
        return Err(CELL_E_NOMEM);
    };
    cache::sync();
    Ok(Executable {
        space,
        abi,
        entry: plan.entry(),
        sp,
        argc,
        argv,
        envp,
        auxv,
    })
}
