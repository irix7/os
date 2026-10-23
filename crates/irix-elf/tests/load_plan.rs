// SPDX-License-Identifier: GPL-3.0-only
//! Public-API tests for the load plan and its segments.

mod common;

use common::{build, o32_image, RawPhdr, DATA_ADDRESS, DATA_OFFSET, TEXT_ADDRESS, TEXT_OFFSET};
use irix_elf::constants::{PF_R, PF_W, PF_X, PT_DYNAMIC};
use irix_elf::{Elf, ElfType, Error, SegmentType};

#[test]
fn builds_the_load_plan_for_a_two_segment_image() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let plan = elf.load_plan().expect("load plan");

    assert_eq!(plan.entry(), TEXT_ADDRESS + 0x10);
    assert_eq!(plan.elf().entry(), plan.entry());
    assert_eq!(plan.elf_type(), ElfType::Exec);
    assert_eq!(plan.interp(), Some(&b"/usr/lib/libc.so.1"[..]));
    assert!(plan.dynamic().is_some());

    let segments: Vec<_> = plan.segments().collect();
    assert_eq!(segments.len(), 2);
    assert_eq!(plan.segments().count(), 2);

    let text = segments[0];
    assert_eq!(text.index(), 1);
    assert_eq!(text.header().kind(), SegmentType::Load);
    assert_eq!(text.load_address(), TEXT_ADDRESS);
    assert_eq!(text.physical_address(), TEXT_ADDRESS);
    assert_eq!(text.file_offset(), TEXT_OFFSET);
    assert_eq!(text.file_size(), 0x400);
    assert_eq!(text.memory_size(), 0x800);
    assert_eq!(text.zero_fill(), 0x400);
    assert_eq!(text.file_data().len(), 0x400);
    assert_eq!(text.file_data()[0], 0xAA);
    assert_eq!(text.flags(), PF_R | PF_X);
    assert_eq!(
        text.permissions(),
        irix_elf::Permissions {
            read: true,
            write: false,
            execute: true,
        }
    );
    assert_eq!(text.alignment(), 0x1000);
    assert_eq!(text.memory_end(), 0x0040_0800);
    assert_eq!(text.memory_range(), 0x0040_0000..0x0040_0800);
    assert_eq!(
        text.file_range(),
        TEXT_OFFSET as usize..TEXT_OFFSET as usize + 0x400
    );

    let data = segments[1];
    assert_eq!(data.index(), 2);
    assert_eq!(data.load_address(), DATA_ADDRESS);
    assert_eq!(data.file_offset(), DATA_OFFSET);
    assert_eq!(data.file_size(), 0x100);
    assert_eq!(data.memory_size(), 0x120);
    assert_eq!(data.zero_fill(), 0x20);
    assert_eq!(data.file_data()[0], 0xBB);
    assert_eq!(data.flags(), PF_R | PF_W);
    assert!(!data.permissions().execute);
    assert_eq!(data.memory_range(), 0x0040_1000..0x0040_1120);
}

#[test]
fn reports_the_lowest_and_highest_load_addresses() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let plan = elf.load_plan().expect("load plan");

    assert_eq!(plan.extent(), Some((0x0040_0000, 0x0040_1120)));
}

#[test]
fn extent_is_independent_of_program_header_order() {
    let payload = [0u8; 0x1010];
    let phdrs = [
        RawPhdr::load(0x1000, 0x0040_2000, 0x10, 0x20, PF_R),
        RawPhdr::load(0x2000, 0x0040_1000, 0x10, 0x20, PF_R),
    ];
    let image = build(&phdrs, &payload, 0x0040_1000);
    let elf = Elf::parse(&image).expect("valid image");
    let plan = elf.load_plan().expect("load plan");

    let segments: Vec<_> = plan.segments().collect();
    assert_eq!(segments[0].load_address(), 0x0040_2000);
    assert_eq!(segments[1].load_address(), 0x0040_1000);
    assert_eq!(plan.extent(), Some((0x0040_1000, 0x0040_2020)));
}

#[test]
fn a_bss_only_segment_has_no_file_bytes() {
    let phdrs = [RawPhdr::load(0, TEXT_ADDRESS, 0, 0x400, PF_R | PF_W)];
    let image = build(&phdrs, &[0u8; 4], TEXT_ADDRESS);
    let elf = Elf::parse(&image).expect("valid image");
    let plan = elf.load_plan().expect("load plan");

    let segments: Vec<_> = plan.segments().collect();
    assert_eq!(segments.len(), 1);
    let bss = segments[0];
    assert!(bss.file_data().is_empty());
    assert_eq!(bss.file_size(), 0);
    assert_eq!(bss.memory_size(), 0x400);
    assert_eq!(bss.zero_fill(), 0x400);
    assert_eq!(bss.memory_range(), 0x0040_0000..0x0040_0400);
    assert_eq!(bss.file_range(), 0..0);
}

#[test]
fn rejects_a_load_plan_without_loadable_segments() {
    let phdrs = [RawPhdr {
        p_type: PT_DYNAMIC,
        p_offset: 0,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 0,
        p_memsz: 0,
        p_flags: PF_R,
        p_align: 4,
    }];
    let image = build(&phdrs, &[], 0);
    let elf = Elf::parse(&image).expect("metadata-only image parses");
    assert_eq!(elf.load_plan().unwrap_err(), Error::NoLoadableSegments);
}

#[test]
fn overlapping_load_segments_are_preserved() {
    // Overlap is a mapping policy, not a parse error.  Both ranges lie inside
    // the image and are congruent modulo p_align, so the plan keeps both in
    // program-header order and leaves the winner to the mapper.
    let payload = [0x77u8; 0x200];
    let phdrs = [
        RawPhdr::load(0x1000, TEXT_ADDRESS, 0x100, 0x200, PF_R | PF_X),
        RawPhdr::load(0x1100, TEXT_ADDRESS + 0x100, 0x100, 0x200, PF_R | PF_W),
    ];
    let image = build(&phdrs, &payload, TEXT_ADDRESS);
    let elf = Elf::parse(&image).expect("overlapping segments parse");
    let plan = elf.load_plan().expect("load plan");

    let segments: Vec<_> = plan.segments().collect();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].memory_range(), 0x0040_0000..0x0040_0200);
    assert_eq!(segments[1].memory_range(), 0x0040_0100..0x0040_0300);
    assert_eq!(plan.extent(), Some((0x0040_0000, 0x0040_0300)));
}

#[test]
fn all_segments_carry_the_underlying_header() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let plan = elf.load_plan().expect("load plan");

    for segment in plan.segments() {
        assert_eq!(segment.header().kind(), SegmentType::Load);
        assert_eq!(segment.header().raw_type(), 1);
        assert_eq!(segment.header().virtual_address(), segment.load_address());
    }
}
