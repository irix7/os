// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native user ELF loader and anonymous user heap.
//!
//! Reads an n32 ELF image from SCSI target 2 LBA 0, maps its `PT_LOAD`
//! segments into kuseg pages, sets up a fixed user stack and enters through
//! the provisional syscall ABI.  This is an M5/M9 bring-up path: it has no
//! process table, fd table, dynamic linking or libc.

use irix_elf::{Abi, Elf};

use crate::{frames, scsi, tlb, traps};

/// Enough for the small hello fixture and its section table.
const ELF_SIZE: usize = 80 * 1024;
const TARGET: u8 = 2;
/// One TLB index maps an even/odd 4 KiB pair.  Indices 8-47 are available;
/// the loader uses four for the image and stack and the rest for the heap.
const MAX_PAIRS: usize = 40;
const FIRST_TLB: u32 = 8;
const STACK_BASE: u32 = 0x7FF0_0000;
const STACK_PAGES: usize = 4;
const STACK_TOP: u32 = STACK_BASE + STACK_PAGES as u32 * 4096;

const HEAP_BASE: u32 = 0x2000_0000;
const HEAP_LIMIT: u32 = HEAP_BASE + 128 * 1024;

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
    if let Err(error) = scsi::read_blocks(TARGET, 0, image) {
        crate::print_str("userelf: read failed: ");
        crate::println(error);
        return;
    }

    let elf = match Elf::parse(image) {
        Ok(elf) => elf,
        Err(error) => {
            crate::print_str("userelf: parse failed: ");
            crate::println(error_text(error));
            return;
        }
    };
    if elf.abi() != Abi::N32 {
        crate::print_str("userelf: not an n32 ELF\n");
        return;
    }
    let plan = match elf.load_plan() {
        Ok(plan) => plan,
        Err(error) => {
            crate::print_str("userelf: load plan failed: ");
            crate::println(error_text(error));
            return;
        }
    };

    unsafe {
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
            let Some(phys) = ensure_page(address) else {
                return;
            };
            unsafe {
                core::ptr::write_volatile(
                    ((phys | 0x8000_0000) as *mut u8).add((address & 0xFFF) as usize),
                    *byte,
                );
            }
        }
    }

    // A fixed four-page user stack, separate from the loaded image.
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
    traps::run_user(plan.entry(), user_sp, argc, argv, envp)
}

/// Release every pair the loader mapped and clear the user range table.
///
/// The exit trap calls this before returning to the console; the telnet
/// command calls it again after a load failure.  It is idempotent.
pub fn cleanup() {
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

/// Map zero-filled anonymous pages below [`HEAP_LIMIT`].
///
/// The provisional syscall ignores the requested address, protection and
/// flags; page-aligned, read-write, uncached memory is the only kind it
/// returns.  Repeated allocations are contiguous and merge into one range.
pub fn mmap(length: u32) -> u32 {
    let Some(length) = length.checked_add(0xFFF).map(|value| value & !0xFFF) else {
        return u32::MAX;
    };
    if length == 0 {
        return u32::MAX;
    }
    let start = unsafe { HEAP_NEXT };
    let Some(end) = start.checked_add(length) else {
        return u32::MAX;
    };
    if end > HEAP_LIMIT {
        return u32::MAX;
    }
    for address in (start..end).step_by(4096) {
        if ensure_page(address).is_none() {
            return u32::MAX;
        }
    }
    if !record_range(start, end) {
        crate::println("userelf: too many user ranges");
        return u32::MAX;
    }
    unsafe {
        HEAP_NEXT = end;
    }
    start
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
    let home = push_bytes(&mut cursor, b"HOME=/");
    let path = push_bytes(&mut cursor, b"PATH=/");
    let arg0 = push_bytes(&mut cursor, b"hello");

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
        if count == MAX_PAIRS {
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
        zero_page(even);
        zero_page(odd);
        tlb::map_page_pair(vaddr, even, odd, 0, FIRST_TLB + count as u32, 0, true);
        pages[count] = LoadedPair { vaddr, even, odd };
        USER_PAIR_COUNT = count + 1;
        Some(if address & 0x1000 == 0 { even } else { odd })
    }
}

fn zero_page(phys: u32) {
    let alias = (phys | 0x8000_0000) as *mut u8;
    for offset in 0..4096 {
        unsafe {
            core::ptr::write_volatile(alias.add(offset), 0);
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
