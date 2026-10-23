// SPDX-License-Identifier: GPL-3.0-only
//! Public-API tests for the ELF file header and ABI decoding.

mod common;

use common::{build, o32_image, put16, put32, RawPhdr, DATA_ADDRESS};
use irix_elf::constants::{
    EF_MIPS_ABI2, EF_MIPS_ABI_EABI32, EF_MIPS_ABI_EABI64, EF_MIPS_ABI_O32, EF_MIPS_ABI_O64,
    EHDR_SIZE, EM_MIPS, ET_CORE, ET_DYN, ET_REL,
};
use irix_elf::{Abi, Elf, ElfType, Error, OsAbi, SegmentType};

#[test]
fn parses_o32_header_fields() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("valid o32 image");
    let header = elf.header();

    assert_eq!(header.elf_type(), ElfType::Exec);
    assert_eq!(header.machine(), EM_MIPS);
    assert_eq!(header.version(), 1);
    assert_eq!(header.entry(), 0x0040_0010);
    assert_eq!(elf.entry(), 0x0040_0010);
    assert_eq!(header.flags(), EF_MIPS_ABI_O32);
    assert_eq!(header.abi(), Abi::O32);
    assert_eq!(elf.abi(), Abi::O32);
    assert_eq!(header.os_abi(), OsAbi::SysV);
    assert_eq!(header.abi_version(), 0);
    assert_eq!(header.header_size(), EHDR_SIZE as u16);
    assert_eq!(header.program_header_offset(), 52);
    assert_eq!(header.program_header_entry_size(), 32);
    assert_eq!(header.program_header_count(), 5);
    assert_eq!(elf.program_headers().len(), 5);
    assert_eq!(elf.bytes().len(), image.len());
}

#[test]
fn classifies_n32_flags() {
    let mut image = o32_image();
    put32(&mut image, 36, EF_MIPS_ABI2);

    let elf = Elf::parse(&image).expect("valid n32-like image");
    assert_eq!(elf.abi(), Abi::N32);
    assert_eq!(elf.header().flags(), EF_MIPS_ABI2);
}

#[test]
fn decodes_all_known_mips_abis() {
    assert_eq!(Abi::from_flags(0), Abi::Unknown);
    assert_eq!(Abi::from_flags(EF_MIPS_ABI_O32), Abi::O32);
    assert_eq!(Abi::from_flags(EF_MIPS_ABI_O64), Abi::O64);
    assert_eq!(Abi::from_flags(EF_MIPS_ABI_EABI32), Abi::Eabi32);
    assert_eq!(Abi::from_flags(EF_MIPS_ABI_EABI64), Abi::Eabi64);
    assert_eq!(Abi::from_flags(EF_MIPS_ABI2), Abi::N32);
    assert_eq!(Abi::from_flags(EF_MIPS_ABI_O32 | EF_MIPS_ABI2), Abi::O32);
    assert_eq!(Abi::from_flags(0x0000_5000), Abi::Unknown);
    assert_eq!(Abi::from_flags(0x0000_5000 | EF_MIPS_ABI2), Abi::Unknown);
}

#[test]
fn exposes_irix_osabi_and_abi_version() {
    let mut image = o32_image();
    image[7] = 8;
    image[8] = 3;

    let elf = Elf::parse(&image).expect("valid image");
    assert_eq!(elf.os_abi(), OsAbi::Irix);
    assert_eq!(elf.os_abi().raw(), 8);
    assert_eq!(elf.abi_version(), 3);
}

#[test]
fn preserves_unknown_osabi() {
    let mut image = o32_image();
    image[7] = 42;

    let elf = Elf::parse(&image).expect("unknown OSABI is not rejected");
    assert_eq!(elf.os_abi(), OsAbi::Other(42));
    assert_eq!(elf.os_abi().raw(), 42);
    assert_eq!(OsAbi::from_raw(0).raw(), 0);
    assert_eq!(OsAbi::from_raw(8).raw(), 8);
}

#[test]
fn accepts_shared_object_type() {
    let mut image = o32_image();
    put16(&mut image, 16, ET_DYN);

    let elf = Elf::parse(&image).expect("ET_DYN is loadable");
    assert_eq!(elf.elf_type(), ElfType::Dyn);
    assert!(elf.load_plan().is_ok());
}

#[test]
fn preserves_unknown_elf_type_value_but_rejects_it() {
    // The parser rejects non-loadable types, but the decoder still preserves
    // the raw value for diagnostics and future use.
    assert_eq!(ElfType::from_raw(0x1234), ElfType::Other(0x1234));
    assert_eq!(ElfType::Other(0x1234).raw(), 0x1234);
    assert_eq!(ElfType::from_raw(0).raw(), 0);
    assert_eq!(ElfType::from_raw(1).raw(), 1);
    assert_eq!(ElfType::from_raw(2).raw(), 2);
    assert_eq!(ElfType::from_raw(3).raw(), 3);
    assert_eq!(ElfType::from_raw(4).raw(), 4);
}

#[test]
fn rejects_short_image() {
    assert_eq!(Elf::parse(&[]).unwrap_err(), Error::Truncated);
    assert_eq!(Elf::parse(&[0u8; 51]).unwrap_err(), Error::Truncated);
}

#[test]
fn rejects_bad_magic() {
    let image = [0u8; EHDR_SIZE];
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::BadMagic);
}

#[test]
fn rejects_elf64_class() {
    let mut image = o32_image();
    image[4] = 2;
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::NotElf32);
}

#[test]
fn rejects_little_endian_image() {
    let mut image = o32_image();
    image[5] = 1;
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::NotBigEndian);
}

#[test]
fn rejects_bad_identification_version() {
    let mut image = o32_image();
    image[6] = 0;
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::BadIdentVersion);
}

#[test]
fn rejects_bad_header_version() {
    let mut image = o32_image();
    put32(&mut image, 20, 2);
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::BadHeaderVersion);
}

#[test]
fn rejects_small_header_size() {
    let mut image = o32_image();
    put16(&mut image, 40, 51);
    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::InvalidHeaderSize(51)
    );
}

#[test]
fn rejects_non_loadable_elf_types() {
    for (raw_type, expected) in [
        (ET_REL, Error::UnsupportedElfType(ET_REL)),
        (ET_CORE, Error::UnsupportedElfType(ET_CORE)),
        (5, Error::UnsupportedElfType(5)),
    ] {
        let mut image = o32_image();
        put16(&mut image, 16, raw_type);
        assert_eq!(Elf::parse(&image).unwrap_err(), expected);
    }
}

#[test]
fn rejects_non_mips_machine() {
    let mut image = o32_image();
    put16(&mut image, 18, 3);
    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::UnsupportedMachine(3)
    );
}

#[test]
fn accepts_zero_program_headers() {
    let mut image = o32_image();
    put16(&mut image, 42, 0);
    put16(&mut image, 44, 0);

    let elf = Elf::parse(&image).expect("an image with no program headers parses");
    assert_eq!(elf.header().program_header_count(), 0);
    assert_eq!(elf.header().program_header_entry_size(), 0);
    assert_eq!(elf.program_headers().len(), 0);
    assert!(elf.program_header(0).is_none());
    assert_eq!(elf.load_plan().unwrap_err(), Error::NoLoadableSegments);
}

#[test]
fn rejects_invalid_program_header_entry_sizes() {
    for size in [0u16, 1, 31, 1025, u16::MAX] {
        let mut image = o32_image();
        put16(&mut image, 42, size);
        assert_eq!(
            Elf::parse(&image).unwrap_err(),
            Error::InvalidProgramHeaderSize(size)
        );
    }
}

#[test]
fn accepts_padded_program_header_entry_size() {
    let mut image = o32_image();
    put16(&mut image, 42, 64);
    put16(&mut image, 44, 2);

    let elf = Elf::parse(&image).expect("a padded entry is tolerated");
    assert_eq!(elf.header().program_header_entry_size(), 64);
    let headers: Vec<_> = elf.program_headers().collect();
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[0].kind(), SegmentType::Interp);
    // The second entry is read at 52 + 64, which lands on the original
    // third header (the data PT_LOAD).
    assert_eq!(headers[1].kind(), SegmentType::Load);
    assert_eq!(headers[1].virtual_address(), DATA_ADDRESS);
    assert_eq!(
        elf.program_header(1)
            .expect("second header")
            .virtual_address(),
        DATA_ADDRESS
    );
}

#[test]
fn rejects_program_header_table_outside_image() {
    let mut image = o32_image();
    let end = image.len() as u32;
    put32(&mut image, 28, end);
    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::ProgramHeaderTableOutOfBounds
    );

    let mut image = o32_image();
    put16(&mut image, 44, u16::MAX);
    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::ProgramHeaderTableOutOfBounds
    );
}

#[test]
fn parses_valid_section_table() {
    let mut image = o32_image();
    put32(&mut image, 32, 0x3000);
    put16(&mut image, 46, 40);
    put16(&mut image, 48, 2);
    put16(&mut image, 50, 0);

    let elf = Elf::parse(&image).expect("valid section table");
    let table = elf.section_table().expect("section table");
    assert_eq!(table.offset(), 0x3000);
    assert_eq!(table.entry_size(), 40);
    assert_eq!(table.count(), 2);
}

#[test]
fn absence_of_section_table_is_allowed() {
    let image = o32_image();
    let elf = Elf::parse(&image).expect("no section table");
    assert!(elf.section_table().is_none());
}

#[test]
fn rejects_section_table_outside_image() {
    let mut image = o32_image();
    put32(&mut image, 32, 0x3120);
    put16(&mut image, 46, 40);
    put16(&mut image, 48, 1);
    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::SectionHeaderTableOutOfBounds
    );
}

#[test]
fn rejects_inconsistent_section_tables() {
    let mut image = o32_image();
    put16(&mut image, 48, 1);
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::InvalidSectionTable);

    let mut image = o32_image();
    put32(&mut image, 32, 0x3000);
    put16(&mut image, 48, 0);
    assert_eq!(Elf::parse(&image).unwrap_err(), Error::InvalidSectionTable);

    for entry_size in [0u16, 39, 1025] {
        let mut image = o32_image();
        put32(&mut image, 32, 0x3000);
        put16(&mut image, 46, entry_size);
        put16(&mut image, 48, 1);
        assert_eq!(Elf::parse(&image).unwrap_err(), Error::InvalidSectionTable);
    }
}

#[test]
fn rejects_a_program_header_table_that_overruns_the_image() {
    // A table that starts inside the image but overruns it must not be read.
    let phdrs = [RawPhdr::load(0x1000, 0x0040_0000, 1, 1, 0)];
    let mut image = build(&phdrs, &[0u8; 4], 0x0040_0000);
    let overrun = image.len() as u32 - 1;
    put32(&mut image, 28, overrun);
    assert_eq!(
        Elf::parse(&image).unwrap_err(),
        Error::ProgramHeaderTableOutOfBounds
    );
}
