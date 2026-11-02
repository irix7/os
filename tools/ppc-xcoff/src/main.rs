// SPDX-License-Identifier: GPL-3.0-only
//! Wrap a flat PowerPC image as the XCOFF32 file Open Firmware 2.x loads.
//!
//! Open Firmware before version 3 loads XCOFF, not ELF.  The g3beige kernel is
//! linked at 0x01000000 and copied there by the firmware; this tool emits the
//! minimum executable XCOFF the firmware needs:
//!
//! * a 72-byte `COFF_aouthdr_t` (`f_opthdr` must be that size),
//! * a `.text` section containing the flat image,
//! * a `.data` section holding one function-descriptor word.
//!
//! XCOFF32's `o_entry` is a *function descriptor* address.  Open Firmware
//! reads the first word at that address and jumps to it, so the descriptor
//! holds the real kernel entry.  Keeping it in `.data` leaves the image bytes
//! untouched and does not overlap the kernel's stack.
//!
//! Usage:
//!   ppc-xcoff <RAW> <ENTRY> <DESCRIPTOR> <OUT>
//!
//! `ENTRY` and `DESCRIPTOR` are hexadecimal addresses.

use std::env;
use std::fs;
use std::path::Path;

const FILE_HEADER: usize = 20;
const AOUT_HEADER: usize = 72;
const SECTION_HEADER: usize = 40;
const SECTIONS: usize = 2;
const TEXT_OFFSET: usize = FILE_HEADER + AOUT_HEADER + SECTIONS * SECTION_HEADER;
const XCOFF_MAGIC: u16 = 0x01df;
const AOUT_MAGIC: u16 = 0x010b;
const TEXT_FLAGS: u32 = 0x0020;
const DATA_FLAGS: u32 = 0x0040;

fn u16be(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn u32be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn name8(out: &mut Vec<u8>, name: &str) {
    let mut field = [0u8; 8];
    let bytes = name.as_bytes();
    field[..bytes.len()].copy_from_slice(bytes);
    out.extend_from_slice(&field);
}

fn parse_hex(value: &str) -> Result<u32, String> {
    u32::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|error| format!("invalid hexadecimal address {value:?}: {error}"))
}

/// Build the XCOFF file for `raw`, loaded at `entry` with a descriptor at
/// `descriptor`.
fn build(raw: &[u8], entry: u32, descriptor: u32) -> Result<Vec<u8>, String> {
    if raw.is_empty() {
        return Err("the flat image is empty".into());
    }
    if entry & 3 != 0 || descriptor & 3 != 0 {
        return Err("the entry and descriptor must be word-aligned".into());
    }
    let text_size = u32::try_from(raw.len()).map_err(|_| "the image exceeds 4 GiB")?;
    let data_offset = (TEXT_OFFSET + raw.len() + 3) & !3;
    let text_end = u32::try_from(TEXT_OFFSET)
        .map_err(|_| "the image header exceeds 4 GiB")?
        .checked_add(text_size)
        .ok_or("the text section overflows the file offsets")?;
    let data_offset_u32 =
        u32::try_from(data_offset).map_err(|_| "the image is too large for XCOFF32")?;
    if text_end > data_offset_u32 {
        return Err("the text section overlaps the data section".into());
    }

    let mut out = Vec::with_capacity(data_offset + 4);

    // COFF_filehdr_t: 20 bytes.
    u16be(&mut out, XCOFF_MAGIC);
    u16be(&mut out, SECTIONS as u16);
    u32be(&mut out, 0); // f_timdat
    u32be(&mut out, 0); // f_symptr
    u32be(&mut out, 0); // f_nsyms
    u16be(&mut out, AOUT_HEADER as u16);
    u16be(&mut out, 0x0002); // F_EXEC

    // COFF_aouthdr_t: 72 bytes.
    u16be(&mut out, AOUT_MAGIC);
    u16be(&mut out, 0); // vstamp
    u32be(&mut out, text_size); // tsize
    u32be(&mut out, 4); // dsize
    u32be(&mut out, 0); // bsize
    u32be(&mut out, descriptor); // entry: address of the function descriptor
    u32be(&mut out, entry); // text_start
    u32be(&mut out, descriptor); // data_start
    u32be(&mut out, 0); // o_toc
    u16be(&mut out, 2); // o_snentry: .data
    u16be(&mut out, 1); // o_sntext: .text
    u16be(&mut out, 2); // o_sndata: .data
    u16be(&mut out, 0); // o_sntoc
    u16be(&mut out, 0); // o_snloader
    u16be(&mut out, 0); // o_snbss
    u16be(&mut out, 0); // o_algntext
    u16be(&mut out, 0); // o_algndata
    u16be(&mut out, 0); // o_modtype
    u16be(&mut out, 0); // o_cputype
    u32be(&mut out, 0); // o_maxstack
    u32be(&mut out, 0); // o_maxdata
    out.extend_from_slice(&[0u8; 12]); // o_resv2

    // COFF_scnhdr_t .text: 40 bytes.
    name8(&mut out, ".text");
    u32be(&mut out, entry); // s_paddr
    u32be(&mut out, entry); // s_vaddr
    u32be(&mut out, text_size);
    u32be(&mut out, TEXT_OFFSET as u32);
    u32be(&mut out, 0); // s_relptr
    u32be(&mut out, 0); // s_lnnoptr
    u16be(&mut out, 0); // s_nreloc
    u16be(&mut out, 0); // s_nlnno
    u32be(&mut out, TEXT_FLAGS);

    // COFF_scnhdr_t .data: 40 bytes, one descriptor word.
    name8(&mut out, ".data");
    u32be(&mut out, descriptor);
    u32be(&mut out, descriptor);
    u32be(&mut out, 4);
    u32be(&mut out, data_offset_u32);
    u32be(&mut out, 0); // s_relptr
    u32be(&mut out, 0); // s_lnnoptr
    u16be(&mut out, 0); // s_nreloc
    u16be(&mut out, 0); // s_nlnno
    u32be(&mut out, DATA_FLAGS);

    debug_assert_eq!(out.len(), TEXT_OFFSET);
    out.extend_from_slice(raw);
    out.resize(data_offset, 0);
    u32be(&mut out, entry); // the function descriptor
    Ok(out)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 5 {
        return Err(format!(
            "usage: {} <RAW> <ENTRY> <DESCRIPTOR> <OUT>",
            args.first().map(String::as_str).unwrap_or("ppc-xcoff")
        ));
    }
    let raw_path = Path::new(&args[1]);
    let entry = parse_hex(&args[2])?;
    let descriptor = parse_hex(&args[3])?;
    let out_path = Path::new(&args[4]);
    let raw =
        fs::read(raw_path).map_err(|error| format!("read {}: {error}", raw_path.display()))?;
    let image = build(&raw, entry, descriptor)?;
    fs::write(out_path, &image)
        .map_err(|error| format!("write {}: {error}", out_path.display()))?;
    println!(
        "ppc-xcoff: {} bytes, text {:#010x}..{:#010x}, descriptor {:#010x}",
        image.len(),
        entry,
        entry as usize + raw.len(),
        descriptor
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout_matches_open_firmware() {
        let image = build(&[0x60, 0, 0, 0], 0x0100_0000, 0x0200_0000).unwrap();
        assert_eq!(u16::from_be_bytes([image[0], image[1]]), XCOFF_MAGIC);
        assert_eq!(
            u16::from_be_bytes([image[16], image[17]]),
            AOUT_HEADER as u16
        );
        assert_eq!(u16::from_be_bytes([image[20], image[21]]), AOUT_MAGIC);
        assert_eq!(
            u32::from_be_bytes([image[36], image[37], image[38], image[39]]),
            0x0200_0000
        );
        let text = &image[TEXT_OFFSET..TEXT_OFFSET + 4];
        assert_eq!(text, &[0x60, 0, 0, 0]);
        assert_eq!(
            u32::from_be_bytes(image[image.len() - 4..].try_into().unwrap()),
            0x0100_0000
        );
    }

    #[test]
    fn data_section_is_word_aligned() {
        let image = build(&[1, 2, 3, 4, 5], 0x0100_0000, 0x0200_0000).unwrap();
        let data_offset = u32::from_be_bytes([
            image[92 + 40 + 20],
            image[92 + 40 + 21],
            image[92 + 40 + 22],
            image[92 + 40 + 23],
        ]);
        assert_eq!(data_offset % 4, 0);
    }
}
