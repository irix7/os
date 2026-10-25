// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native user ELF loader and anonymous user heap.
//!
//! Reads an n32 ELF image from SCSI target 2 LBA 0, maps its `PT_LOAD`
//! segments into kuseg pages, sets up a fixed user stack and enters through
//! the provisional syscall ABI.  This is an M5/M9 bring-up path: it has no
//! process table, fd table, dynamic linking or libc.

use irix_elf::{Abi, Elf};

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
    cleanup();
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

    let (user_sp, argc, argv, envp) = initial_stack();
    crate::cache::sync();
    crate::print_str("userelf: entry=0x");
    crate::print_hex32(plan.entry());
    crate::println("");
    traps::run_user_abi(user_abi, plan.entry(), user_sp, argc, argv, envp)
}

/// Release every pair the loader mapped and clear the user range table.
///
/// The exit trap calls this before returning to the console; the telnet
/// command calls it again after a load failure.  It is idempotent.
pub fn cleanup() {
    crate::fd::reset();
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
pub fn mmap(length: u32) -> u32 {
    if !map_heap() {
        return u32::MAX;
    }
    let Some(length) = length.checked_add(0xFFF).map(|value| value & !0xFFF) else {
        return u32::MAX;
    };
    if length == 0 {
        return u32::MAX;
    }
    unsafe {
        let start = HEAP_NEXT;
        let Some(end) = start.checked_add(length) else {
            return u32::MAX;
        };
        if end > HEAP_LIMIT {
            return u32::MAX;
        }
        if !record_range(start, end) {
            return u32::MAX;
        }
        HEAP_NEXT = end;
        start
    }
}

/// Build the conventional initial stack:
///
/// ```text
/// sp+0:  argc
/// sp+4:  argv[0] ... argv[argc-1], NULL
/// sp+..: envp[0] ... envp[n-1], NULL
/// ```
///
/// The strings live above the pointer array.  The image is a fixed hello
/// fixture, so the argument and environment contents are fixed for now.
fn initial_stack() -> (u32, u32, u32, u32) {
    let mut cursor = STACK_TOP;
    let home = push_bytes(&mut cursor, b"HOME=/\0");
    let path = push_bytes(&mut cursor, b"PATH=/\0");
    let arg0 = push_bytes(&mut cursor, b"hello\0");

    let argc: u32 = 1;
    let envc: u32 = 2;
    let pointer_words = 1 + argc + 1 + envc + 1;
    let sp = (cursor & !7) - pointer_words * 4;
    write_word(sp, argc);
    write_word(sp + 4, arg0);
    write_word(sp + 8, 0);
    write_word(sp + 12, path);
    write_word(sp + 16, home);
    write_word(sp + 20, 0);
    (sp, argc, sp + 4, sp + 12)
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
fn record_range(start: u32, end: u32) -> bool {
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
fn read_path(address: u32) -> Option<([u8; 64], usize)> {
    let mut path = [0u8; 64];
    let mut length = 0;
    while length < path.len() {
        if !range_ok(address + length as u32, 1) {
            return None;
        }
        let byte = unsafe { core::ptr::read_volatile((address as *const u8).add(length)) };
        if byte == 0 {
            return Some((path, length));
        }
        path[length] = byte;
        length += 1;
    }
    None
}

/// Open a path in the tiny bring-up namespace.
pub fn open_path(address: u32, flags: u32) -> i32 {
    let Some((path, length)) = read_path(address) else {
        return -1;
    };
    crate::fd::open_path(&path[..length], flags)
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
pub fn write_fd(fd: u32, address: u32, length: u32) -> u32 {
    let kind = crate::fd::kind(fd as i32);
    match kind {
        crate::fd::Kind::Console => {
            let mut buffer = [0u8; 512];
            let mut done = 0usize;
            while done < length as usize {
                let chunk = (length as usize - done).min(buffer.len());
                if !range_ok(address + done as u32, chunk as u32) {
                    return u32::MAX;
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
            length
        }
        crate::fd::Kind::Null => length,
        crate::fd::Kind::File(_) | crate::fd::Kind::PipeWrite(_) => {
            let mut buffer = [0u8; 512];
            let mut done = 0usize;
            while done < length as usize {
                let chunk = (length as usize - done).min(buffer.len());
                if !range_ok(address + done as u32, chunk as u32) {
                    return u32::MAX;
                }
                for (index, byte) in buffer[..chunk].iter_mut().enumerate() {
                    *byte = unsafe {
                        core::ptr::read_volatile((address as *const u8).add(done + index))
                    };
                }
                let count = crate::fd::write(fd as i32, &buffer[..chunk]);
                if count < 0 {
                    return u32::MAX;
                }
                done += count as usize;
                if (count as usize) < chunk {
                    break;
                }
            }
            done as u32
        }
        crate::fd::Kind::Dir(_) | crate::fd::Kind::PipeRead(_) => u32::MAX,
        crate::fd::Kind::Closed => u32::MAX,
    }
}

#[repr(C)]
struct UserPollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

/// A tiny poll implementation: console and null descriptors are always ready.
pub fn poll_user(address: u32, count: u32) -> u32 {
    let bytes = count as usize * core::mem::size_of::<UserPollFd>();
    if !range_ok(address, bytes as u32) {
        return u32::MAX;
    }
    let mut ready = 0u32;
    for index in 0..count as usize {
        let base = address + (index * core::mem::size_of::<UserPollFd>()) as u32;
        let fd = unsafe { core::ptr::read_volatile(base as *const i32) };
        let events = unsafe { core::ptr::read_volatile((base + 4) as *const i16) };
        let revents = if crate::fd::kind(fd) == crate::fd::Kind::Closed {
            0x20 // POLLNVAL
        } else {
            ready += 1;
            events
        };
        unsafe {
            core::ptr::write_volatile((base + 6) as *mut i16, revents);
        }
    }
    ready
}

pub fn getcwd_user(address: u32, size: u32) -> u32 {
    if size < 2 || !range_ok(address, 2) {
        return u32::MAX;
    }
    unsafe {
        core::ptr::write_volatile(address as *mut u8, b'/');
        core::ptr::write_volatile((address + 1) as *mut u8, 0);
    }
    0
}

pub fn clock_user(clock: u32, address: u32) -> u32 {
    if !range_ok(address, 16) {
        return u32::MAX;
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
    0
}

pub fn entropy_user(address: u32, length: u32) -> u32 {
    if !range_ok(address, length) {
        return u32::MAX;
    }
    for index in 0..length as usize {
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(index), 0);
        }
    }
    0
}

/// POSIX `tcgetwinsize`: copy the 8-byte winsize from the console.
pub fn tcgetwinsize_user(fd: u32, address: u32) -> u32 {
    if !range_ok(address, 8) {
        return u32::MAX;
    }
    let mut value = [0u16; 4];
    if !crate::fd::get_winsize(fd as i32, &mut value) {
        return u32::MAX;
    }
    for (index, field) in value.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile((address + index as u32 * 2) as *mut u16, *field);
        }
    }
    0
}

/// POSIX `tcsetwinsize`: update the console size from the 8-byte winsize.
pub fn tcsetwinsize_user(fd: u32, address: u32) -> u32 {
    if !range_ok(address, 8) {
        return u32::MAX;
    }
    let mut value = [0u16; 4];
    for (index, field) in value.iter_mut().enumerate() {
        *field = unsafe { core::ptr::read_volatile((address + index as u32 * 2) as *const u16) };
    }
    if crate::fd::set_winsize(fd as i32, &value) {
        0
    } else {
        u32::MAX
    }
}

/// Create a pipe and write `[read_fd, write_fd]` to the user buffer.
pub fn pipe_user(address: u32) -> u32 {
    if !range_ok(address, 8) {
        return u32::MAX;
    }
    let (read, write) = crate::fd::pipe();
    if read < 0 || write < 0 {
        return u32::MAX;
    }
    unsafe {
        core::ptr::write_volatile(address as *mut i32, read);
        core::ptr::write_volatile((address + 4) as *mut i32, write);
    }
    0
}

/// Copy file contents from a descriptor into a validated user buffer.
pub fn read_fd(fd: u32, address: u32, length: u32) -> u32 {
    if !range_ok(address, length) {
        return u32::MAX;
    }
    let mut buffer = [0u8; 512];
    let mut total = 0usize;
    while total < length as usize {
        let chunk = (length as usize - total).min(buffer.len());
        let count = crate::fd::read(fd as i32, &mut buffer[..chunk]);
        if count < 0 {
            return u32::MAX;
        }
        if count == 0 {
            break;
        }
        for (index, byte) in buffer[..count as usize].iter().enumerate() {
            unsafe {
                core::ptr::write_volatile((address as *mut u8).add(total + index), *byte);
            }
        }
        total += count as usize;
        if (count as usize) < chunk {
            break;
        }
    }
    total as u32
}

/// Native n32 `struct stat`, matching the ABI used by the std spike.
#[repr(C)]
struct KernelStat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i32,
    _pad: u32,
    st_blocks: i64,
    st_atim: [i64; 2],
    st_mtim: [i64; 2],
    st_ctim: [i64; 2],
    spare: [u8; 16],
}

fn fill_stat(address: u32, mode: u32, size: i64) -> u32 {
    if !range_ok(address, core::mem::size_of::<KernelStat>() as u32) {
        return u32::MAX;
    }
    let stat = KernelStat {
        st_dev: 0,
        st_ino: 1,
        st_mode: mode,
        st_nlink: 1,
        st_uid: 0,
        st_gid: 0,
        st_rdev: 0,
        st_size: size,
        st_blksize: 4096,
        _pad: 0,
        st_blocks: (size + 511) / 512,
        st_atim: [0; 2],
        st_mtim: [0; 2],
        st_ctim: [0; 2],
        spare: [0; 16],
    };
    let source = &stat as *const KernelStat as *const u8;
    for index in 0..core::mem::size_of::<KernelStat>() {
        unsafe {
            core::ptr::write_volatile(
                (address as *mut u8).add(index),
                core::ptr::read_volatile(source.add(index)),
            );
        }
    }
    0
}

pub fn fstat_user(fd: u32, address: u32) -> u32 {
    match crate::fd::metadata(fd as i32) {
        Some((mode, size)) => fill_stat(address, mode, size),
        None => u32::MAX,
    }
}

pub fn stat_user(path_address: u32, stat_address: u32) -> u32 {
    let Some((path, length)) = read_path(path_address) else {
        return u32::MAX;
    };
    match crate::fd::metadata_path(&path[..length]) {
        Some((mode, size)) => fill_stat(stat_address, mode, size),
        None => u32::MAX,
    }
}

/// Copy one static directory entry into a user buffer.  Returns the dirent
/// type (4 directory, 8 regular file, 2 device) or zero at end of directory.
pub fn readdir_user(fd: u32, index: u32, address: u32, length: u32) -> u32 {
    if !range_ok(address, length) {
        return u32::MAX;
    }
    let mut buffer = [0u8; 256];
    let Some((kind, _)) = crate::fd::readdir(fd as i32, index as usize, &mut buffer) else {
        return 0;
    };
    let name_len = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    if name_len + 1 > length as usize {
        return u32::MAX;
    }
    for index in 0..=name_len {
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(index), buffer[index]);
        }
    }
    kind as u32
}

/// Busy-wait for a user `timespec`.  The syscall path runs with interrupts
/// masked by EXL, so CP0 Count keeps advancing while no timer interrupt can
/// preempt the wait.  This is a bring-up sleep, not the scheduler.
pub fn sleep_user(address: u32) -> u32 {
    if !range_ok(address, 16) {
        return u32::MAX;
    }
    let seconds = unsafe { core::ptr::read_volatile(address as *const i64) }.max(0) as u64;
    let nanos = unsafe { core::ptr::read_volatile((address + 8) as *const i32) }.max(0) as u64;
    let ticks = seconds
        .saturating_mul(33_000_000)
        .saturating_add(nanos.saturating_mul(33) / 1_000);
    let start = crate::traps::count_read();
    let mut elapsed = 0u64;
    while elapsed < ticks {
        let now = crate::traps::count_read();
        elapsed = now.wrapping_sub(start) as u64;
        core::hint::spin_loop();
    }
    0
}

pub fn chdir_user(address: u32) -> u32 {
    let Some((path, length)) = read_path(address) else {
        return u32::MAX;
    };
    if crate::fd::set_cwd(&path[..length]) {
        0
    } else {
        u32::MAX
    }
}

pub fn getcwd_text(address: u32, size: u32) -> u32 {
    let mut path = [0u8; 64];
    let length = crate::fd::copy_cwd(&mut path);
    if size as usize <= length || !range_ok(address, (length + 1) as u32) {
        return u32::MAX;
    }
    for index in 0..length {
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(index), path[index]);
        }
    }
    unsafe {
        core::ptr::write_volatile((address as *mut u8).add(length), 0);
    }
    0
}
