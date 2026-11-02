// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native user ELF loader and anonymous user heap.
//!
//! Reads an n32 ELF image from SCSI target 2 LBA 0, maps its `PT_LOAD`
//! segments into kuseg pages, sets up a fixed user stack and enters through
//! the provisional syscall ABI.  This is an M5/M9 bring-up path: it has no
//! process table, fd table, dynamic linking or libc.

use irix_elf::{Abi, Elf};

use crate::syserr::{
    SysResult, CELL_E_FAULT, CELL_E_INVAL, CELL_E_NAMETOOLONG, CELL_E_NOMEM, CELL_E_OVERFLOW,
};

use crate::{frames, scsi, tlb, traps};

/// Enough for the static `std` bring-up image and its section table.
const ELF_SIZE: usize = 256 * 1024;
const TARGET: u8 = 2;
/// One TLB index maps an even/odd 4 KiB pair.  Indices 8-47 are available;
/// the loader uses four for the image and stack and the rest for the heap.
const MAX_PAIRS: usize = 44;
const FIRST_TLB: u32 = 4;
const STACK_BASE: u32 = 0x7FF0_0000;
const STACK_PAGES: usize = 4;
const STACK_TOP: u32 = STACK_BASE + STACK_PAGES as u32 * 4096;

const HEAP_BASE: u32 = 0x2000_0000;
const HEAP_LIMIT: u32 = HEAP_BASE + 1024 * 1024;
const HEAP_PAGE_MASK_256K: u32 = crate::tlb::PAGE_MASK_256K;
const HEAP_PAGE_SIZE: u32 = 256 * 1024;
const HEAP_TLB_PAGES: usize = 2;

static mut ELF_BUF: [u8; ELF_SIZE] = [0; ELF_SIZE];

const MAX_RANGES: usize = 8;
static mut USER_RANGES: [(u32, u32); MAX_RANGES] = [(0, 0); MAX_RANGES];
static mut USER_RANGE_COUNT: usize = 0;

#[derive(Clone, Copy)]
struct LoadedPair {
    vaddr: u32,
    even: u32,
    odd: u32,
}

static mut USER_PAGES: [LoadedPair; MAX_PAIRS] = [LoadedPair {
    vaddr: 0,
    even: 0,
    odd: 0,
}; MAX_PAIRS];
static mut USER_PAIR_COUNT: usize = 0;
static mut HEAP_NEXT: u32 = HEAP_BASE;
static mut HEAP_MAPPED: bool = false;
static mut HEAP_TLB_START: u32 = 0;
static mut HEAP_PHYS: u64 = 0;

/// The package installer's userland staging buffer.
///
/// This is the same 256 KiB buffer the ELF loader uses, so installing an image
/// does not add a second large `.bss` allocation.  It is only valid while the
/// console owns the machine (no user process is running).
pub fn image_buffer() -> &'static mut [u8] {
    // SAFETY: sash is single-threaded; the loader and the installer never run
    // at the same time.
    unsafe { &mut *core::ptr::addr_of_mut!(ELF_BUF) }
}

fn page_of(address: u32) -> u32 {
    address & !0xFFF
}

fn pair_of(address: u32) -> u32 {
    address & !0x1FFF
}

/// Load and enter the user ELF on target 2.
///
/// On success this never returns: a user exit comes back through the console
/// trampoline in `traps.rs`.  A load failure returns to the console wrapper.
#[no_mangle]
pub extern "C" fn user_elf() {
    let image = unsafe { &mut *core::ptr::addr_of_mut!(ELF_BUF) };
    // The WD33C93 READ(6) path used by the bring-up SCSI driver is limited to
    // 255 sectors, so pull the image in 64 KiB chunks.
    let mut offset = 0usize;
    while offset < image.len() {
        let chunk = (image.len() - offset).min(64 * 1024);
        if let Err(error) = scsi::read_blocks(
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
pub fn run_image(image: &[u8]) {
    cleanup();
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

    unsafe {
        if HEAP_MAPPED {
            for index in 0..HEAP_TLB_PAGES {
                tlb::invalidate_index(HEAP_TLB_START + index as u32);
            }
            for index in 0..HEAP_TLB_PAGES * 128 {
                frames::free_frame(HEAP_PHYS + index as u64 * 4096);
            }
            HEAP_MAPPED = false;
            HEAP_PHYS = 0;
        }
        USER_PAIR_COUNT = 0;
        USER_RANGE_COUNT = 0;
        HEAP_NEXT = HEAP_BASE;
    }

    for segment in plan.segments() {
        let Some(end) = segment.load_address().checked_add(segment.memory_size()) else {
            crate::println("userelf: segment address overflow");
            return;
        };
        record_range(segment.load_address(), end);
        let start_page = page_of(segment.load_address());
        let end_page = page_of(end.saturating_add(0xFFF));
        for vaddr in (start_page..end_page).step_by(4096) {
            if ensure_page(vaddr).is_none() {
                return;
            }
        }

        for (offset, byte) in segment.file_data().iter().enumerate() {
            let address = segment.load_address().wrapping_add(offset as u32);
            if ensure_page(address).is_none() {
                return;
            }
            // Write through the same cached user alias that the program will
            // use.  A second cached alias would require cache-index care on
            // the R4000; the kseg1 alias is deliberately not used here.
            unsafe {
                core::ptr::write_volatile(address as *mut u8, *byte);
            }
        }

        // The entry point may have been fetched into the instruction cache by
        // an earlier attempt, or hold stale lines from the kernel.  Invalidate
        // the whole mapped segment before entering it.
        crate::cache::icache_invalidate(
            start_page as usize,
            end_page.saturating_sub(start_page) as usize,
        );
    }

    // A fixed user stack, separate from the loaded image.
    for index in 0..STACK_PAGES {
        let address = STACK_BASE + index as u32 * 4096;
        if ensure_page(address).is_none() {
            return;
        }
    }
    record_range(STACK_BASE, STACK_TOP);

    let (user_sp, argc, argv, envp, auxv) = initial_stack(plan.entry());
    crate::cache::sync();
    crate::print_str("userelf: entry=0x");
    crate::print_hex32(plan.entry());
    crate::println("");
    traps::run_user_abi(user_abi, plan.entry(), user_sp, argc, argv, envp, auxv)
}

/// Release every pair the loader mapped and clear the user range table.
///
/// The exit trap calls this before returning to the console; the telnet
/// command calls it again after a load failure.  It is idempotent.
pub fn cleanup() {
    crate::fd::reset();
    crate::thread::reset();
    unsafe {
        let pages = &*core::ptr::addr_of!(USER_PAGES);
        for index in 0..USER_PAIR_COUNT {
            tlb::invalidate_index(FIRST_TLB + index as u32);
            frames::free_frame(pages[index].even as u64);
            frames::free_frame(pages[index].odd as u64);
        }
        USER_PAIR_COUNT = 0;
        USER_RANGE_COUNT = 0;
        HEAP_NEXT = HEAP_BASE;
    }
}

/// Map the zero-filled anonymous heap with 256 KiB pages.
///
/// A 4 KiB mapping for every heap page exhausts the 48-entry JTLB once std's
/// allocator and the ELF image are both resident.  The heap is a fixed 1 MiB
/// window backed by two 256 KiB page-pair entries; allocations are a bump
/// within that window.
fn map_heap() -> bool {
    unsafe {
        if HEAP_MAPPED {
            return true;
        }
        let Some(base) = frames::alloc_frames(HEAP_TLB_PAGES * 128 + 128) else {
            return false;
        };
        // The allocator is first-fit, not aligned.  Take an aligned 1 MiB
        // window inside a slightly larger run and leave the unused frames
        // allocated; this is a bring-up allocator, not the final VM.
        let phys = (base + 0x3FFFF) & !0x3FFFF;
        if phys + (HEAP_TLB_PAGES as u64 * 2 * HEAP_PAGE_SIZE as u64) > base + 0x160000 {
            return false;
        }
        HEAP_PHYS = phys;
        HEAP_TLB_START = FIRST_TLB + USER_PAIR_COUNT as u32;
        if USER_PAIR_COUNT + HEAP_TLB_PAGES > MAX_PAIRS {
            return false;
        }
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
                true,
            );
        }
        // Anonymous memory must read as zero.  The page frames may have been
        // used by the kernel before this mapping.
        for offset in 0..(HEAP_TLB_PAGES as u32 * 2 * HEAP_PAGE_SIZE) {
            unsafe {
                core::ptr::write_volatile((HEAP_BASE + offset) as *mut u8, 0);
            }
        }
        HEAP_NEXT = HEAP_BASE;
        HEAP_MAPPED = true;
        true
    }
}

/// Allocate zero-filled anonymous memory from the fixed heap window.
pub fn mmap(length: u32) -> SysResult<u32> {
    if !map_heap() {
        return Err(CELL_E_NOMEM);
    }
    let Some(length) = length.checked_add(0xFFF).map(|value| value & !0xFFF) else {
        return Err(CELL_E_INVAL);
    };
    if length == 0 {
        return Err(CELL_E_INVAL);
    }
    unsafe {
        let start = HEAP_NEXT;
        let Some(end) = start.checked_add(length) else {
            return Err(CELL_E_NOMEM);
        };
        if end > HEAP_LIMIT {
            return Err(CELL_E_NOMEM);
        }
        if !record_range(start, end) {
            return Err(CELL_E_NOMEM);
        }
        HEAP_NEXT = end;
        Ok(start)
    }
}

/// Build the conventional initial stack:
///
/// ```text
/// sp+0:  argc
/// sp+4:  argv[0] ... argv[argc-1], NULL
/// sp+..: envp[0] ... envp[n-1], NULL
/// sp+..: auxv[0].a_type, auxv[0].a_val, ..., AT_NULL, 0
/// ```
///
/// The strings live above the pointer array.  The image is a fixed hello
/// fixture, so the argument and environment contents are fixed for now.
/// `$a3` carries the auxv pointer in addition to the System V stack shape.
fn initial_stack(entry: u32) -> (u32, u32, u32, u32, u32) {
    let mut cursor = STACK_TOP;
    let home = push_bytes(&mut cursor, b"HOME=/\0");
    let path = push_bytes(&mut cursor, b"PATH=/\0");
    let arg0 = push_bytes(&mut cursor, b"hello\0");
    let platform = push_bytes(&mut cursor, b"mips\0");
    let execfn = push_bytes(&mut cursor, b"hello\0");
    let mut random_bytes = [0u8; 16];
    let mut seed = crate::traps::count_read()
        ^ crate::ntp::now_unix().unwrap_or(0) as u32
        ^ (entry.rotate_left(11));
    for byte in &mut random_bytes {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        *byte = seed as u8;
    }
    let random = push_bytes(&mut cursor, &random_bytes);

    let argc: u32 = 1;
    let envc: u32 = 2;
    let auxc: u32 = 16;
    let pointer_words = 1 + argc + 1 + envc + 1 + auxc * 2;
    let mut sp = (cursor & !15) - pointer_words * 4;
    sp &= !15;
    let mut at = sp;
    write_word(at, argc);
    at += 4;
    write_word(at, arg0);
    at += 4;
    write_word(at, 0);
    at += 4;
    let envp = at;
    write_word(at, path);
    at += 4;
    write_word(at, home);
    at += 4;
    write_word(at, 0);
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
        write_word(at, key);
        write_word(at + 4, value);
        at += 8;
    }
    (sp, argc, sp + 4, envp, auxv)
}

fn push_bytes(cursor: &mut u32, bytes: &[u8]) -> u32 {
    *cursor -= bytes.len() as u32;
    let start = *cursor;
    for (offset, byte) in bytes.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile((start as *mut u8).add(offset), *byte);
        }
    }
    start
}

fn write_word(address: u32, value: u32) {
    unsafe {
        core::ptr::write_volatile(address as *mut u32, value);
    }
}

/// Record `[start, end)` in the user range table, merging adjacent ranges.
pub(crate) fn record_range(start: u32, end: u32) -> bool {
    unsafe {
        let ranges = &mut *core::ptr::addr_of_mut!(USER_RANGES);
        let mut count = USER_RANGE_COUNT;
        let mut start = start;
        let mut end = end;
        let mut index = 0;
        while index < count {
            let (existing_start, existing_end) = ranges[index];
            if start <= existing_end && end >= existing_start {
                start = start.min(existing_start);
                end = end.max(existing_end);
                for move_index in index..count - 1 {
                    ranges[move_index] = ranges[move_index + 1];
                }
                count -= 1;
                USER_RANGE_COUNT = count;
            } else {
                index += 1;
            }
        }
        if count < MAX_RANGES {
            ranges[count] = (start, end);
            USER_RANGE_COUNT = count + 1;
            true
        } else {
            false
        }
    }
}

/// Whether `[address, address + length)` is inside a recorded user range.
///
/// The syscall layer uses this before copying from user memory.  It is a
/// range check, not a page-table walk: the loader records every mapped
/// segment, the stack and each anonymous heap request.
pub fn range_ok(address: u32, length: u32) -> bool {
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    unsafe {
        let ranges = &*core::ptr::addr_of!(USER_RANGES);
        ranges[..USER_RANGE_COUNT]
            .iter()
            .any(|&(start, stop)| address >= start && end <= stop)
    }
}

/// Map and zero the even/odd pair containing `address`, then return the
/// physical frame for that specific 4 KiB page.
fn ensure_page(address: u32) -> Option<u32> {
    let vaddr = pair_of(address);
    unsafe {
        let pages = &mut *core::ptr::addr_of_mut!(USER_PAGES);
        let count = USER_PAIR_COUNT;
        if let Some(pair) = pages[..count].iter().find(|pair| pair.vaddr == vaddr) {
            return Some(if address & 0x1000 == 0 {
                pair.even
            } else {
                pair.odd
            });
        }
        if count >= MAX_PAIRS {
            crate::println("userelf: too many page pairs");
            return None;
        }
        let Some(even) = frames::alloc_frame() else {
            crate::println("userelf: out of frames");
            return None;
        };
        let Some(odd) = frames::alloc_frame() else {
            crate::println("userelf: out of frames");
            return None;
        };
        let even = even as u32;
        let odd = odd as u32;
        tlb::map_page_pair(vaddr, even, odd, 0, FIRST_TLB + count as u32, 3, true);
        // Write through the user virtual address after mapping.  This keeps
        // the cached user alias authoritative and avoids writing through the
        // uncached kseg1 alias while the user alias is cached.
        zero_user_page(vaddr);
        zero_user_page(vaddr + 4096);
        pages[count] = LoadedPair { vaddr, even, odd };
        USER_PAIR_COUNT = count + 1;
        Some(if address & 0x1000 == 0 { even } else { odd })
    }
}

fn zero_user_page(vaddr: u32) {
    let page = vaddr as *mut u8;
    for offset in 0..4096 {
        unsafe {
            core::ptr::write_volatile(page.add(offset), 0);
        }
    }
}

fn error_text(error: irix_elf::Error) -> &'static str {
    match error {
        irix_elf::Error::Truncated => "truncated",
        irix_elf::Error::NoLoadableSegments => "no loadable segments",
        _ => "invalid ELF",
    }
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
///
/// `timeout` is milliseconds: 0 probes once, a positive value waits that long,
/// and a negative value means "no deadline" in POSIX.  The kernel is
/// single-threaded, so a negative timeout is bounded by
/// [`crate::fd::WAIT_LIMIT_MS`] rather than waiting forever; the same cap
/// applies to a longer positive timeout.  Readiness is re-evaluated while the
/// call waits, so console input arriving during the wait is reported.
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
        if crate::traps::count_read().wrapping_sub(start) >= limit {
            return Ok(0);
        }
        core::hint::spin_loop();
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
/// descriptor's cookie.  The scratch buffer keeps the user pointer out of the
/// fd layer; the loop stops before a record would cross the caller's range.
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
        // A short chunk is not end-of-directory: the scratch buffer simply
        // filled with whole records.  Only a zero-length read ends the walk.
        if total >= length as usize {
            break;
        }
    }
    Ok(total as u32)
}

/// Copy file contents from a descriptor into a validated user buffer.
pub fn read_fd(fd: u32, address: u32, length: u32) -> SysResult<u32> {
    if !range_ok(address, length) {
        return Err(CELL_E_FAULT);
    }
    let mut buffer = [0u8; 512];
    let mut total = 0usize;
    while total < length as usize {
        let chunk = (length as usize - total).min(buffer.len());
        let count = crate::fd::read(fd as i32, &mut buffer[..chunk])?;
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

/// Busy-wait for a user `timespec`.  The syscall path runs with interrupts
/// masked by EXL, so CP0 Count keeps advancing while no timer interrupt can
/// preempt the wait.  This is a bring-up sleep, not the scheduler.
pub fn sleep_user(address: u32) -> SysResult<()> {
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
    let start = crate::traps::count_read();
    let mut elapsed = 0u64;
    while elapsed < ticks {
        let now = crate::traps::count_read();
        elapsed = now.wrapping_sub(start) as u64;
        core::hint::spin_loop();
    }
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
