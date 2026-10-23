// SPDX-License-Identifier: GPL-3.0-only
//! Public-API tests for checked segment ranges and alignment.

mod common;

use common::{o32_image, put32, PHOFF, TEXT_ADDRESS};
use irix_elf::Elf;
use irix_elf::Error;

const P_OFFSET: usize = 4;
const P_VADDR: usize = 8;
const P_FILESZ: usize = 16;
const P_MEMSZ: usize = 20;
const P_ALIGN: usize = 28;

fn patch(image: &mut [u8], index: usize, field: usize, value: u32) {
    put32(image, PHOFF + index * 32 + field, value);
}

#[test]
fn rejects_a_file_range_past_the_image() {
    let mut image = o32_image();
    let end = image.len() as u32;
    patch(&mut image, 1, P_OFFSET, end);

    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::SegmentOutOfBounds(1)
    );
}

#[test]
fn rejects_a_32_bit_file_range_overflow() {
    let mut image = o32_image();
    patch(&mut image, 1, P_OFFSET, 0xFFFF_FFF0);
    patch(&mut image, 1, P_FILESZ, 0x100);

    assert_eq!(Elf::parse(&image).unwrap_err(), Error::SegmentOverflow(1));
}

#[test]
fn rejects_filesz_greater_than_memsz() {
    let mut image = o32_image();
    patch(&mut image, 1, P_MEMSZ, 0x100);

    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::FileSizeExceedsMemorySize(1)
    );
}

#[test]
fn rejects_a_32_bit_memory_range_overflow() {
    let mut image = o32_image();
    patch(&mut image, 1, P_VADDR, 0xFFFF_FF00);

    assert_eq!(Elf::parse(&image).unwrap_err(), Error::SegmentOverflow(1));
}

#[test]
fn rejects_a_non_power_of_two_alignment() {
    let mut image = o32_image();
    patch(&mut image, 1, P_ALIGN, 3);

    assert_eq!(Elf::parse(&image).unwrap_err(), Error::InvalidAlignment(1));

    let mut image = o32_image();
    patch(&mut image, 3, P_ALIGN, 6);
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::InvalidAlignment(3));
}

#[test]
fn rejects_a_misaligned_load_segment() {
    let mut image = o32_image();
    patch(&mut image, 1, P_VADDR, TEXT_ADDRESS + 0x100);

    assert_eq!(Elf::parse(&image).unwrap_err(), Error::MisalignedSegment(1));
}

#[test]
fn only_load_segments_need_aligned_offsets() {
    let mut image = o32_image();
    patch(&mut image, 3, P_VADDR, TEXT_ADDRESS);
    patch(&mut image, 3, P_ALIGN, 0x1000);

    let elf = Elf::parse(&image).expect("PT_DYNAMIC has no congruence rule");
    assert_eq!(elf.program_header(3).expect("dynamic").alignment(), 0x1000);
}

#[test]
fn accepts_zero_and_one_alignment() {
    for alignment in [0u32, 1] {
        let mut image = o32_image();
        patch(&mut image, 1, P_VADDR, TEXT_ADDRESS + 0x123);
        patch(&mut image, 1, P_ALIGN, alignment);

        let elf = Elf::parse(&image).expect("alignment 0 and 1 skip the congruence rule");
        assert_eq!(elf.program_header(1).expect("text").alignment(), alignment);
    }
}

#[test]
fn accepts_a_file_range_that_ends_exactly_at_the_image_end() {
    let mut image = o32_image();
    let start = image.len() as u32 - 0x400;
    patch(&mut image, 1, P_OFFSET, start);
    patch(&mut image, 1, P_FILESZ, 0x400);
    patch(&mut image, 1, P_ALIGN, 0);

    let elf = Elf::parse(&image).expect("the last byte is in the image");
    let text = elf.program_header(1).expect("text");
    assert_eq!(text.file().len(), 0x400);
    assert_eq!(text.file()[0x3FF], image[image.len() - 1]);
}

#[test]
fn rejects_a_file_range_one_byte_past_the_end() {
    let mut image = o32_image();
    let start = image.len() as u32 - 0x3FF;
    patch(&mut image, 1, P_OFFSET, start);
    patch(&mut image, 1, P_FILESZ, 0x400);
    patch(&mut image, 1, P_ALIGN, 0);

    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::SegmentOutOfBounds(1)
    );
}

#[test]
fn rejects_a_zero_length_range_past_the_end() {
    let mut image = o32_image();
    let start = image.len() as u32 + 1;
    patch(&mut image, 1, P_OFFSET, start);
    patch(&mut image, 1, P_FILESZ, 0);

    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::SegmentOutOfBounds(1)
    );
}

#[test]
fn accepts_a_zero_length_file_range_at_the_end() {
    let mut image = o32_image();
    let start = image.len() as u32;
    patch(&mut image, 1, P_OFFSET, start);
    patch(&mut image, 1, P_FILESZ, 0);
    patch(&mut image, 1, P_ALIGN, 0);

    let elf = Elf::parse(&image).expect("an empty range at EOF is valid");
    let text = elf.program_header(1).expect("text");
    assert!(text.file().is_empty());
    assert_eq!(text.memory_size(), 0x800);
}
