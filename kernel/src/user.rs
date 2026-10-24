// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native user ELF loader.
//!
//! Reads an n32 ELF image from SCSI target 2 LBA 0, maps its `PT_LOAD`
//! segments into kuseg pages, sets up a fixed user stack and enters through
//! the provisional syscall ABI.  This is an M5/M9 bring-up path: it has no
//! argv/envp, dynamic linking, process table, fd table or libc.

use irix_elf::{Abi, Elf};

use crate::{frames, scsi, tlb, traps};

/// Enough for the small hello fixture and its section table.
const ELF_SIZE: usize = 80 * 1024;
const TARGET: u8 = 2;
/// One TLB index maps an even/odd 4 KiB pair.
const MAX_PAIRS: usize = 8;
const FIRST_TLB: u32 = 40;
const STACK_BASE: u32 = 0x7FF0_0000;
const STACK_PAGES: usize = 4;
const STACK_TOP: u32 = STACK_BASE + STACK_PAGES as u32 * 4096;

static mut ELF_BUF: [u8; ELF_SIZE] = [0; ELF_SIZE];

#[derive(Clone, Copy)]
struct LoadedPair {
    vaddr: u32,
    even: u32,
    odd: u32,
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

    let mut pairs = [LoadedPair {
        vaddr: 0,
        even: 0,
        odd: 0,
    }; MAX_PAIRS];
    let mut pair_count = 0usize;

    for segment in plan.segments() {
        let Some(end) = segment.load_address().checked_add(segment.memory_size()) else {
            crate::println("userelf: segment address overflow");
            return;
        };
        let start_page = page_of(segment.load_address());
        let end_page = page_of(end.saturating_add(0xFFF));
        for vaddr in (start_page..end_page).step_by(4096) {
            if ensure_page(&mut pairs, &mut pair_count, vaddr).is_none() {
                return;
            }
        }

        for (offset, byte) in segment.file_data().iter().enumerate() {
            let address = segment.load_address().wrapping_add(offset as u32);
            let Some(phys) = ensure_page(&mut pairs, &mut pair_count, address) else {
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
        if ensure_page(&mut pairs, &mut pair_count, address).is_none() {
            return;
        }
    }

    crate::cache::sync();
    crate::print_str("userelf: entry=0x");
    crate::print_hex32(plan.entry());
    crate::println("");
    traps::run_user(plan.entry(), STACK_TOP)
}

/// Map and zero the even/odd pair containing `address`, then return the
/// physical frame for that specific 4 KiB page.
fn ensure_page(
    pairs: &mut [LoadedPair; MAX_PAIRS],
    pair_count: &mut usize,
    address: u32,
) -> Option<u32> {
    let vaddr = pair_of(address);
    let pair = match pairs[..*pair_count].iter().find(|pair| pair.vaddr == vaddr) {
        Some(pair) => *pair,
        None => {
            if *pair_count == MAX_PAIRS {
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
            tlb::map_page_pair(vaddr, even, odd, 0, FIRST_TLB + *pair_count as u32, 0, true);
            let pair = LoadedPair { vaddr, even, odd };
            pairs[*pair_count] = pair;
            *pair_count += 1;
            pair
        }
    };

    Some(if address & 0x1000 == 0 {
        pair.even
    } else {
        pair.odd
    })
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
