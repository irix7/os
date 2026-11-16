// SPDX-License-Identifier: GPL-3.0-only
//! ELF32 big-endian MIPS to ECOFF.
//!
//! The output is an `OMAGIC` executable with `.text`, `.data` and `.bss`
//! section headers, the layout Linux's `elf2ecoff` writes for the same PROM.
//! The kernel's loadable segments become one image from the lowest load
//! address; it is split into text and data at the `.rodata` section so both
//! sections are non-empty.

use std::fs;

const FILE_HEADER: usize = 20;
const AOUT_HEADER: usize = 56;
const SECTION_HEADER: usize = 40;
const SECTIONS: usize = 3;
/// `N_TXTOFF` for `OMAGIC` with a version stamp of 23 or later: the headers
/// rounded up to 16 bytes.
const TEXT_OFFSET: usize = (FILE_HEADER + AOUT_HEADER + SECTIONS * SECTION_HEADER + 15) & !15;

const MIPSEB_MAGIC: u16 = 0x0160;
const OMAGIC: u16 = 0x0107;
const VERSION_STAMP: u16 = 200;
/// Relocations, line numbers and local symbols stripped; executable.
const FILE_FLAGS: u16 = 0x100f;
const STYP_TEXT: u32 = 0x20;
const STYP_DATA: u32 = 0x40;
const STYP_BSS: u32 = 0x80;

const PT_LOAD: u32 = 1;

/// The parts of an executable the ECOFF headers describe.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub base: u32,
    pub entry: u32,
    /// Offset in `bytes` where data starts.
    pub split: usize,
    pub bytes: Vec<u8>,
    pub bss: u32,
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, String> {
    data.get(offset..offset + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| "ELF is truncated".to_string())
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, String> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| "ELF is truncated".to_string())
}

/// Read the loadable image out of an ELF32 big-endian MIPS executable.
pub fn parse_elf(elf: &[u8]) -> Result<Image, String> {
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    if elf[4] != 1 || elf[5] != 2 {
        return Err("ELF is not 32-bit big-endian".into());
    }
    if u16_at(elf, 18)? != 8 {
        return Err("ELF is not MIPS".into());
    }
    let entry = u32_at(elf, 24)?;
    let phoff = u32_at(elf, 28)? as usize;
    let shoff = u32_at(elf, 32)? as usize;
    let phentsize = u16_at(elf, 42)? as usize;
    let phnum = u16_at(elf, 44)? as usize;
    let shentsize = u16_at(elf, 46)? as usize;
    let shnum = u16_at(elf, 48)? as usize;
    let shstrndx = u16_at(elf, 50)? as usize;

    // (vaddr, file offset, file size, memory size)
    let mut loads = Vec::new();
    for index in 0..phnum {
        let at = phoff + index * phentsize;
        if u32_at(elf, at)? != PT_LOAD {
            continue;
        }
        let offset = u32_at(elf, at + 4)? as usize;
        let vaddr = u32_at(elf, at + 8)?;
        let filesz = u32_at(elf, at + 16)? as usize;
        let memsz = u32_at(elf, at + 20)?;
        if memsz != 0 {
            loads.push((vaddr, offset, filesz, memsz));
        }
    }
    let base = loads
        .iter()
        .map(|load| load.0)
        .min()
        .ok_or("ELF has no loadable segment")?;
    let file_end = loads
        .iter()
        .filter(|load| load.2 != 0)
        .map(|load| load.0 as u64 + load.2 as u64)
        .max()
        .ok_or("ELF has no file-backed segment")?;
    let mem_end = loads
        .iter()
        .map(|load| load.0 as u64 + load.3 as u64)
        .max()
        .unwrap_or(file_end);

    let mut bytes = vec![0u8; (file_end - base as u64) as usize];
    for &(vaddr, offset, filesz, _) in &loads {
        let source = elf
            .get(offset..offset + filesz)
            .ok_or("ELF segment runs past the end of the file")?;
        let start = (vaddr - base) as usize;
        bytes[start..start + filesz].copy_from_slice(source);
    }
    if entry < base || entry as u64 >= file_end {
        return Err(format!("entry {entry:#x} is outside the loaded image"));
    }

    // Split at `.rodata` when the section table names it, else in the middle.
    let mut split = (bytes.len() / 2) & !15;
    if shoff != 0 && shstrndx < shnum {
        let names_at = u32_at(elf, shoff + shstrndx * shentsize + 16)? as usize;
        for index in 0..shnum {
            let at = shoff + index * shentsize;
            let name_at = names_at + u32_at(elf, at)? as usize;
            let name = elf.get(name_at..).unwrap_or(&[]);
            if name.starts_with(b".rodata\0") {
                let addr = u32_at(elf, at + 12)?;
                if addr > base && (addr as u64) < file_end {
                    split = (addr - base) as usize;
                }
                break;
            }
        }
    }
    if split == 0 || split >= bytes.len() {
        return Err("image is too small to split into text and data".into());
    }

    Ok(Image {
        base,
        entry,
        split,
        bytes,
        bss: (mem_end - file_end) as u32,
    })
}

fn push16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn section(out: &mut Vec<u8>, name: &str, addr: u32, size: u32, file_offset: u32, flags: u32) {
    let mut field = [0u8; 8];
    field[..name.len()].copy_from_slice(name.as_bytes());
    out.extend_from_slice(&field);
    push32(out, addr); // s_paddr
    push32(out, addr); // s_vaddr
    push32(out, size);
    push32(out, file_offset);
    push32(out, 0); // s_relptr
    push32(out, 0); // s_lnnoptr
    push16(out, 0); // s_nreloc
    push16(out, 0); // s_nlnno
    push32(out, flags);
}

/// Write `image` as an ECOFF executable.
pub fn write_ecoff(image: &Image) -> Vec<u8> {
    let text_size = image.split as u32;
    let data_size = (image.bytes.len() - image.split) as u32;
    let data_start = image.base + text_size;
    let bss_start = data_start + data_size;

    let mut out = Vec::with_capacity(TEXT_OFFSET + image.bytes.len());
    // File header.
    push16(&mut out, MIPSEB_MAGIC);
    push16(&mut out, SECTIONS as u16);
    push32(&mut out, 0); // f_timdat
    push32(&mut out, 0); // f_symptr
    push32(&mut out, 0); // f_nsyms
    push16(&mut out, AOUT_HEADER as u16);
    push16(&mut out, FILE_FLAGS);
    // Optional header.
    push16(&mut out, OMAGIC);
    push16(&mut out, VERSION_STAMP);
    push32(&mut out, text_size);
    push32(&mut out, data_size);
    push32(&mut out, image.bss);
    push32(&mut out, image.entry);
    push32(&mut out, image.base);
    push32(&mut out, data_start);
    push32(&mut out, bss_start);
    push32(&mut out, 0xf3ff_fffe); // gprmask
    for _ in 0..4 {
        push32(&mut out, 0); // cprmask
    }
    push32(&mut out, 0); // gp_value
    section(
        &mut out,
        ".text",
        image.base,
        text_size,
        TEXT_OFFSET as u32,
        STYP_TEXT,
    );
    section(
        &mut out,
        ".data",
        data_start,
        data_size,
        TEXT_OFFSET as u32 + text_size,
        STYP_DATA,
    );
    section(&mut out, ".bss", bss_start, image.bss, 0, STYP_BSS);
    out.resize(TEXT_OFFSET, 0);
    out.extend_from_slice(&image.bytes);
    out
}

/// Convert the ELF at `input` to an ECOFF file at `output`.
pub fn convert(input: &str, output: &str) -> Result<(), String> {
    let elf = fs::read(input).map_err(|error| format!("{input}: {error}"))?;
    let image = parse_elf(&elf)?;
    let ecoff = write_ecoff(&image);
    fs::write(output, &ecoff).map_err(|error| format!("{output}: {error}"))?;
    println!(
        "{output}: {} bytes, load {:#010x}..{:#010x}, bss {} bytes, entry {:#010x}",
        ecoff.len(),
        image.base,
        image.base as u64 + image.bytes.len() as u64,
        image.bss,
        image.entry
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-segment ELF: 64 file bytes at 0x88040000 with 32 more of bss.
    fn sample_elf() -> Vec<u8> {
        let mut elf = vec![0u8; 52 + 32];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1;
        elf[5] = 2;
        elf[18..20].copy_from_slice(&8u16.to_be_bytes());
        elf[24..28].copy_from_slice(&0x8804_0000u32.to_be_bytes());
        elf[28..32].copy_from_slice(&52u32.to_be_bytes());
        elf[42..44].copy_from_slice(&32u16.to_be_bytes());
        elf[44..46].copy_from_slice(&1u16.to_be_bytes());
        let ph = 52;
        elf[ph..ph + 4].copy_from_slice(&PT_LOAD.to_be_bytes());
        elf[ph + 4..ph + 8].copy_from_slice(&84u32.to_be_bytes());
        elf[ph + 8..ph + 12].copy_from_slice(&0x8804_0000u32.to_be_bytes());
        elf[ph + 16..ph + 20].copy_from_slice(&64u32.to_be_bytes());
        elf[ph + 20..ph + 24].copy_from_slice(&96u32.to_be_bytes());
        elf.extend((0..64u8).map(|byte| byte + 1));
        elf
    }

    #[test]
    fn elf_segments_become_one_image() {
        let image = parse_elf(&sample_elf()).unwrap();
        assert_eq!(image.base, 0x8804_0000);
        assert_eq!(image.entry, 0x8804_0000);
        assert_eq!(image.bytes.len(), 64);
        assert_eq!(image.bytes[0], 1);
        assert_eq!(image.bss, 32);
        assert_eq!(image.split, 32);
    }

    #[test]
    fn ecoff_headers_describe_the_image() {
        let image = parse_elf(&sample_elf()).unwrap();
        let out = write_ecoff(&image);
        assert_eq!(TEXT_OFFSET, 208);
        assert_eq!(out.len(), 208 + 64);
        assert_eq!(&out[..2], &[0x01, 0x60]);
        assert_eq!(u16_at(&out, 20).unwrap(), OMAGIC);
        assert_eq!(u32_at(&out, 24).unwrap(), 32); // tsize
        assert_eq!(u32_at(&out, 28).unwrap(), 32); // dsize
        assert_eq!(u32_at(&out, 32).unwrap(), 32); // bsize
        assert_eq!(u32_at(&out, 36).unwrap(), 0x8804_0000); // entry
        assert_eq!(u32_at(&out, 40).unwrap(), 0x8804_0000); // text_start
        assert_eq!(u32_at(&out, 44).unwrap(), 0x8804_0020); // data_start

        // The .data section header: its file offset follows the text.
        let data = FILE_HEADER + AOUT_HEADER + SECTION_HEADER;
        assert_eq!(&out[data..data + 5], b".data");
        assert_eq!(u32_at(&out, data + 20).unwrap(), 208 + 32);
        assert_eq!(&out[208..], &image.bytes[..]);
    }

    #[test]
    fn a_non_elf_file_is_rejected() {
        assert!(parse_elf(b"not an elf").is_err());
    }
}
