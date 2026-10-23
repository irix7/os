// SPDX-License-Identifier: GPL-3.0-only
//! Public-API tests for program headers, `PT_INTERP` and `PT_DYNAMIC`.

mod common;

use common::{
    build, o32_image, RawPhdr, DATA_ADDRESS, DATA_OFFSET, INTERP, INTERP_OFFSET, TEXT_ADDRESS,
    TEXT_OFFSET, UNNAMED_TYPE,
};
use irix_elf::constants::{PF_R, PF_W, PF_X, PT_INTERP, PT_MIPS_REGINFO};
use irix_elf::{Elf, Error, Permissions, SegmentType};

#[test]
fn iterates_every_program_header() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let headers: Vec<_> = elf.program_headers().collect();

    assert_eq!(headers.len(), 5);
    assert_eq!(headers[0].kind(), SegmentType::Interp);
    assert_eq!(headers[1].kind(), SegmentType::Load);
    assert_eq!(headers[2].kind(), SegmentType::Load);
    assert_eq!(headers[3].kind(), SegmentType::Dynamic);
    assert_eq!(headers[4].kind(), SegmentType::Other(UNNAMED_TYPE));

    assert_eq!(headers[0].file(), INTERP);
    assert_eq!(headers[1].virtual_address(), TEXT_ADDRESS);
    assert_eq!(headers[1].physical_address(), TEXT_ADDRESS);
    assert_eq!(headers[1].file_offset(), TEXT_OFFSET);
    assert_eq!(headers[1].file_size(), 0x400);
    assert_eq!(headers[1].memory_size(), 0x800);
    assert_eq!(headers[1].flags(), PF_R | PF_X);
    assert_eq!(
        headers[1].permissions(),
        Permissions {
            read: true,
            write: false,
            execute: true,
        }
    );
    assert_eq!(headers[1].alignment(), 0x1000);
    assert_eq!(headers[1].file().len(), 0x400);
    assert_eq!(headers[1].file()[0], 0xAA);
    assert_eq!(
        headers[1].file_range(),
        TEXT_OFFSET as usize..TEXT_OFFSET as usize + 0x400
    );

    assert_eq!(headers[2].virtual_address(), DATA_ADDRESS);
    assert_eq!(headers[2].file_offset(), DATA_OFFSET);
    assert_eq!(headers[2].file()[0], 0xBB);
}

#[test]
fn program_header_iterator_reports_its_length() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let mut headers = elf.program_headers();

    assert_eq!(headers.len(), 5);
    assert_eq!(headers.size_hint(), (5, Some(5)));
    assert!(headers.next().is_some());
    assert_eq!(headers.len(), 4);
    assert_eq!(headers.size_hint(), (4, Some(4)));
    assert_eq!(headers.count(), 4);
}

#[test]
fn preserves_unknown_and_named_segment_types() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let last = elf.program_header(4).expect("last header");

    assert_eq!(last.kind(), SegmentType::Other(UNNAMED_TYPE));
    assert_eq!(last.raw_type(), UNNAMED_TYPE);

    for value in [
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        PT_MIPS_REGINFO,
        0x7000_0001,
        0x7000_0002,
        UNNAMED_TYPE,
    ] {
        assert_eq!(SegmentType::from_raw(value).raw(), value);
    }
    assert_eq!(
        SegmentType::from_raw(PT_MIPS_REGINFO),
        SegmentType::MipsReginfo
    );
}

#[test]
fn reads_the_interpreter_path() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");

    assert_eq!(elf.interp(), Some(&b"/usr/lib/libc.so.1"[..]));

    let plan = elf.load_plan().expect("load plan");
    assert_eq!(plan.interp(), elf.interp());
}

#[test]
fn stops_the_interpreter_path_at_the_first_nul() {
    let mut payload = vec![0u8; 0x810];
    payload[0x800..0x80A].copy_from_slice(b"/bin/sh\0xy");
    let phdrs = [RawPhdr {
        p_type: PT_INTERP,
        p_offset: INTERP_OFFSET,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 10,
        p_memsz: 10,
        p_flags: PF_R,
        p_align: 1,
    }];
    let image = build(&phdrs, &payload, 0);

    let elf = Elf::parse(&image).expect("valid interpreter");
    assert_eq!(elf.interp(), Some(&b"/bin/sh"[..]));
}

#[test]
fn accepts_a_non_utf8_interpreter_path() {
    let mut payload = vec![0u8; 0x804];
    payload[0x800..0x803].copy_from_slice(&[0xFF, b'x', 0]);
    let phdrs = [RawPhdr {
        p_type: PT_INTERP,
        p_offset: INTERP_OFFSET,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 3,
        p_memsz: 3,
        p_flags: PF_R,
        p_align: 1,
    }];
    let image = build(&phdrs, &payload, 0);

    let elf = Elf::parse(&image).expect("paths are byte strings");
    assert_eq!(elf.interp(), Some(&[0xFF, b'x'][..]));
}

#[test]
fn rejects_an_unterminated_interpreter_path() {
    let mut payload = vec![0u8; 0x808];
    payload[0x800..0x808].copy_from_slice(b"/bin/sh!");
    let phdrs = [RawPhdr {
        p_type: PT_INTERP,
        p_offset: INTERP_OFFSET,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 8,
        p_memsz: 8,
        p_flags: PF_R,
        p_align: 1,
    }];
    let image = build(&phdrs, &payload, 0);

    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::UnterminatedInterp(0)
    );
}

#[test]
fn rejects_an_empty_interpreter_path() {
    let phdrs = [RawPhdr {
        p_type: PT_INTERP,
        p_offset: 0,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 0,
        p_memsz: 0,
        p_flags: PF_R,
        p_align: 1,
    }];
    let image = build(&phdrs, &[], 0);
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::EmptyInterp(0));

    let mut payload = vec![0u8; 0x802];
    payload[0x800] = 0;
    payload[0x801] = b'x';
    let phdrs = [RawPhdr {
        p_type: PT_INTERP,
        p_offset: INTERP_OFFSET,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 2,
        p_memsz: 2,
        p_flags: PF_R,
        p_align: 1,
    }];
    let image = build(&phdrs, &payload, 0);
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::EmptyInterp(0));
}

#[test]
fn rejects_two_interpreter_segments() {
    let mut payload = vec![0u8; 0x802];
    payload[0x800..0x802].copy_from_slice(b"/\0");
    let phdrs = [
        RawPhdr {
            p_type: PT_INTERP,
            p_offset: INTERP_OFFSET,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: 2,
            p_memsz: 2,
            p_flags: PF_R,
            p_align: 1,
        },
        RawPhdr {
            p_type: PT_INTERP,
            p_offset: INTERP_OFFSET,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: 2,
            p_memsz: 2,
            p_flags: PF_R,
            p_align: 1,
        },
    ];
    let image = build(&phdrs, &payload, 0);

    assert_eq!(Elf::parse(&image).unwrap_err(), Error::DuplicateInterp);
}

#[test]
fn reads_the_dynamic_segment() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");
    let dynamic = elf.dynamic().expect("PT_DYNAMIC");

    assert_eq!(dynamic.kind(), SegmentType::Dynamic);
    assert_eq!(dynamic.file().len(), 0x40);
    assert_eq!(dynamic.file()[0], 0xCC);
    assert_eq!(dynamic.virtual_address(), DATA_ADDRESS + 0x100);
    assert_eq!(dynamic.flags(), PF_R | PF_W);
    assert!(elf.load_plan().expect("plan").dynamic().is_some());
}

#[test]
fn program_header_index_is_bounded() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid image");

    assert!(elf.program_header(0).is_some());
    assert!(elf.program_header(4).is_some());
    assert!(elf.program_header(5).is_none());
    assert!(elf.program_header(usize::MAX).is_none());
}

#[test]
fn permission_bits_round_trip() {
    let all = Permissions::from_flags(PF_R | PF_W | PF_X);
    assert!(all.read && all.write && all.execute);
    assert_eq!(all.bits(), PF_R | PF_W | PF_X);
    assert!(!all.is_empty());

    let none = Permissions::from_flags(0);
    assert!(none.is_empty());
    assert_eq!(none.bits(), 0);

    // Unknown flag bits do not leak into the permission view, but the raw
    // flags remain available on the header.
    let odd = Permissions::from_flags(0x8000_0000 | PF_R);
    assert_eq!(odd.bits(), PF_R);
}
