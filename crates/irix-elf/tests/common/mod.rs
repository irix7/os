// SPDX-License-Identifier: GPL-3.0-only
//! Synthetic ELF32 big-endian images for the public-API tests.
//!
//! The layout is fixed: the file header at 0, the program-header table at 52
//! and the payload at 0x1000.  Tests patch individual fields after building a
//! valid image, so a failure names exactly one corrupt field.

#![allow(dead_code)]

use irix_elf::constants::{
    EF_MIPS_ABI_O32, EHDR_SIZE, ELFCLASS32, ELFDATA2MSB, ELF_MAGIC, EM_MIPS, ET_EXEC, EV_CURRENT,
    PF_R, PF_W, PF_X, PT_DYNAMIC, PT_INTERP, PT_LOAD,
};

/// File offset of the program-header table.
pub const PHOFF: usize = EHDR_SIZE;

/// File offset of the payload area.
pub const PAYLOAD: usize = 0x1000;

/// File offset of the text payload.
pub const TEXT_OFFSET: u32 = PAYLOAD as u32;

/// File offset of the data payload.
pub const DATA_OFFSET: u32 = PAYLOAD as u32 + 0x1000;

/// File offset of the dynamic payload.
pub const DYNAMIC_OFFSET: u32 = PAYLOAD as u32 + 0x1100;

/// File offset of the interpreter string.
pub const INTERP_OFFSET: u32 = PAYLOAD as u32 + 0x800;

/// The interpreter path used by the valid image, including its NUL.
pub const INTERP: &[u8] = b"/usr/lib/libc.so.1\0";

/// Text virtual address.
pub const TEXT_ADDRESS: u32 = 0x0040_0000;

/// Data virtual address.
pub const DATA_ADDRESS: u32 = 0x0040_1000;

/// One raw program header for the image builder.
#[derive(Clone, Copy, Debug)]
pub struct RawPhdr {
    pub p_type: u32,
    pub p_offset: u32,
    pub p_vaddr: u32,
    pub p_paddr: u32,
    pub p_filesz: u32,
    pub p_memsz: u32,
    pub p_flags: u32,
    pub p_align: u32,
}

impl RawPhdr {
    /// A loadable segment with `p_paddr == p_vaddr`.
    pub const fn load(offset: u32, address: u32, filesz: u32, memsz: u32, flags: u32) -> Self {
        Self {
            p_type: PT_LOAD,
            p_offset: offset,
            p_vaddr: address,
            p_paddr: address,
            p_filesz: filesz,
            p_memsz: memsz,
            p_flags: flags,
            p_align: 0x1000,
        }
    }
}

/// Build a valid image around `phdrs` and `payload`.
pub fn build(phdrs: &[RawPhdr], payload: &[u8], entry: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; PAYLOAD + payload.len()];
    bytes[..4].copy_from_slice(&ELF_MAGIC);
    bytes[4] = ELFCLASS32;
    bytes[5] = ELFDATA2MSB;
    bytes[6] = u8::try_from(EV_CURRENT).expect("EV_CURRENT fits in a byte");
    bytes[7] = 0;
    bytes[8] = 0;

    put16(&mut bytes, 16, ET_EXEC);
    put16(&mut bytes, 18, EM_MIPS);
    put32(&mut bytes, 20, EV_CURRENT);
    put32(&mut bytes, 24, entry);
    put32(&mut bytes, 28, PHOFF as u32);
    put32(&mut bytes, 32, 0);
    put32(&mut bytes, 36, EF_MIPS_ABI_O32);
    put16(&mut bytes, 40, EHDR_SIZE as u16);
    put16(&mut bytes, 42, 32);
    put16(
        &mut bytes,
        44,
        u16::try_from(phdrs.len()).expect("test image has few headers"),
    );
    put16(&mut bytes, 46, 0);
    put16(&mut bytes, 48, 0);
    put16(&mut bytes, 50, 0);

    for (index, phdr) in phdrs.iter().enumerate() {
        put_phdr(&mut bytes, index, phdr);
    }
    bytes[PAYLOAD..].copy_from_slice(payload);
    bytes
}

/// Write one program header at `index`.
pub fn put_phdr(bytes: &mut [u8], index: usize, phdr: &RawPhdr) {
    let offset = PHOFF + index * 32;
    put32(bytes, offset, phdr.p_type);
    put32(bytes, offset + 4, phdr.p_offset);
    put32(bytes, offset + 8, phdr.p_vaddr);
    put32(bytes, offset + 12, phdr.p_paddr);
    put32(bytes, offset + 16, phdr.p_filesz);
    put32(bytes, offset + 20, phdr.p_memsz);
    put32(bytes, offset + 24, phdr.p_flags);
    put32(bytes, offset + 28, phdr.p_align);
}

/// Overwrite a 16-bit big-endian field.
pub fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

/// Overwrite a 32-bit big-endian field.
pub fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// The valid o32 image used by most tests.
///
/// Program headers: `PT_INTERP`, text `PT_LOAD`, data `PT_LOAD`,
/// `PT_DYNAMIC`, and an unnamed type.  The text has 0x400 bytes of BSS and the
/// data has 0x20.
pub fn o32_image() -> Vec<u8> {
    let mut payload = vec![0u8; 0x2140];
    payload[..0x400].fill(0xAA);
    payload[0x800..0x800 + INTERP.len()].copy_from_slice(INTERP);
    payload[0x1000..0x1100].fill(0xBB);
    payload[0x1100..0x1140].fill(0xCC);

    let phdrs = [
        RawPhdr {
            p_type: PT_INTERP,
            p_offset: INTERP_OFFSET,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: INTERP.len() as u32,
            p_memsz: INTERP.len() as u32,
            p_flags: PF_R,
            p_align: 1,
        },
        RawPhdr::load(TEXT_OFFSET, TEXT_ADDRESS, 0x400, 0x800, PF_R | PF_X),
        RawPhdr::load(DATA_OFFSET, DATA_ADDRESS, 0x100, 0x120, PF_R | PF_W),
        RawPhdr {
            p_type: PT_DYNAMIC,
            p_offset: DYNAMIC_OFFSET,
            p_vaddr: DATA_ADDRESS + 0x100,
            p_paddr: DATA_ADDRESS + 0x100,
            p_filesz: 0x40,
            p_memsz: 0x40,
            p_flags: PF_R | PF_W,
            p_align: 4,
        },
        RawPhdr {
            p_type: 0x6000_1234,
            p_offset: 0,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: 0,
            p_memsz: 0,
            p_flags: 0,
            p_align: 0,
        },
    ];
    build(&phdrs, &payload, TEXT_ADDRESS + 0x10)
}

/// A minimal image with one loadable segment and no interpreter.
pub fn simple_load_image() -> Vec<u8> {
    let payload = [0x5Au8; 0x80];
    let phdrs = [RawPhdr::load(
        TEXT_OFFSET,
        TEXT_ADDRESS,
        payload.len() as u32,
        0x100,
        PF_R | PF_X,
    )];
    build(&phdrs, &payload, TEXT_ADDRESS)
}

/// A named-but-uncommon program type, used to check preservation.
pub const UNNAMED_TYPE: u32 = 0x6000_1234;
