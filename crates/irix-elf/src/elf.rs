// SPDX-License-Identifier: GPL-3.0-only
//! Parsing and validation of an IRIX ELF image.

use core::fmt;

use crate::constants::{MAX_PHDR_ENTRY_SIZE, PHDR_SIZE, PT_LOAD};
use crate::error::{Error, Result};
use crate::header::{Abi, ElfType, Header, OsAbi, SectionTable};
use crate::load::LoadPlan;
use crate::program::{ProgramHeader, ProgramHeaders, RawProgramHeader, SegmentType};

/// A validated ELF32 big-endian MIPS image.
///
/// `Elf` borrows the image; it never copies or allocates.  Construction via
/// [`Elf::parse`] validates the file header, the program-header table and
/// every program header's file and memory range.
#[derive(Clone, Copy)]
pub struct Elf<'a> {
    image: &'a [u8],
    header: Header,
    program_header_offset: usize,
    program_header_entry_size: usize,
    program_header_count: usize,
    interp: Option<&'a [u8]>,
    dynamic: Option<ProgramHeader<'a>>,
}

impl fmt::Debug for Elf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Elf")
            .field("entry", &self.header.entry())
            .field("elf_type", &self.header.elf_type())
            .field("program_header_count", &self.program_header_count)
            .field("interp", &self.interp)
            .field("dynamic", &self.dynamic.is_some())
            .finish()
    }
}

impl<'a> Elf<'a> {
    /// Parse and validate an ELF32 big-endian MIPS image.
    ///
    /// The image must be an executable or shared object (`ET_EXEC` or
    /// `ET_DYN`) for the MIPS machine, with `ELFCLASS32`, `ELFDATA2MSB` and
    /// the current identification and header versions.
    ///
    /// # Errors
    ///
    /// Returns a typed [`Error`] for a malformed or unsupported image.  Every
    /// offset, size and range is checked with checked arithmetic against
    /// `image`; no field is trusted.
    pub fn parse(image: &'a [u8]) -> Result<Self> {
        let header = Header::parse(image)?;
        let program_header_offset = header.program_header_offset() as usize;
        let program_header_entry_size = usize::from(header.program_header_entry_size());
        let program_header_count = usize::from(header.program_header_count());

        if program_header_count != 0 {
            if program_header_entry_size < PHDR_SIZE
                || header.program_header_entry_size() > MAX_PHDR_ENTRY_SIZE
            {
                return Err(Error::InvalidProgramHeaderSize(
                    header.program_header_entry_size(),
                ));
            }
            let table_size = u64::from(header.program_header_entry_size())
                .checked_mul(u64::from(header.program_header_count()))
                .ok_or(Error::ProgramHeaderTableOutOfBounds)?;
            let table_end = u64::from(header.program_header_offset())
                .checked_add(table_size)
                .ok_or(Error::ProgramHeaderTableOutOfBounds)?;
            if table_end > image.len() as u64 {
                return Err(Error::ProgramHeaderTableOutOfBounds);
            }
        }

        let (interp, dynamic) = validate_program_headers(
            image,
            program_header_offset,
            program_header_entry_size,
            program_header_count,
        )?;

        Ok(Self {
            image,
            header,
            program_header_offset,
            program_header_entry_size,
            program_header_count,
            interp,
            dynamic,
        })
    }

    /// The complete image slice supplied to [`Elf::parse`].
    pub const fn bytes(&self) -> &'a [u8] {
        self.image
    }

    /// The validated ELF file header.
    pub const fn header(&self) -> Header {
        self.header
    }

    /// The entry virtual address (`e_entry`).
    pub const fn entry(&self) -> u32 {
        self.header.entry()
    }

    /// The `e_type` of the image.
    pub const fn elf_type(&self) -> ElfType {
        self.header.elf_type()
    }

    /// The raw `e_flags`.
    pub const fn flags(&self) -> u32 {
        self.header.flags()
    }

    /// The MIPS ABI derived from `e_flags`.
    pub const fn abi(&self) -> Abi {
        self.header.abi()
    }

    /// The `EI_OSABI` value.
    pub const fn os_abi(&self) -> OsAbi {
        self.header.os_abi()
    }

    /// The `EI_ABIVERSION` value.
    pub const fn abi_version(&self) -> u8 {
        self.header.abi_version()
    }

    /// The validated section-header table, if the image has one.
    pub const fn section_table(&self) -> Option<SectionTable> {
        self.header.section_table()
    }

    /// Iterate over every program header in table order.
    pub fn program_headers(&self) -> ProgramHeaders<'a> {
        ProgramHeaders::new(
            self.image,
            self.program_header_offset,
            self.program_header_entry_size,
            self.program_header_count,
        )
    }

    /// The program header at `index`, or `None` if it is out of range.
    pub fn program_header(&self, index: usize) -> Option<ProgramHeader<'a>> {
        if index >= self.program_header_count {
            return None;
        }
        let offset = self
            .program_header_offset
            .checked_add(index.checked_mul(self.program_header_entry_size)?)?;
        ProgramHeader::parse(self.image, offset)
    }

    /// The `PT_INTERP` path without its NUL terminator.
    ///
    /// The path is returned as bytes because IRIX paths are not required to be
    /// UTF-8.  Parsing rejects a missing terminator and an empty path.
    pub const fn interp(&self) -> Option<&'a [u8]> {
        self.interp
    }

    /// The `PT_DYNAMIC` program header, if present.
    pub const fn dynamic(&self) -> Option<ProgramHeader<'a>> {
        self.dynamic
    }

    /// Build the load plan for this image.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoLoadableSegments`] when the image has no `PT_LOAD`
    /// segment.  A shared object with only dynamic metadata is not loadable on
    /// its own.
    pub fn load_plan(&self) -> Result<LoadPlan<'a>> {
        if self
            .program_headers()
            .any(|header| header.kind() == SegmentType::Load)
        {
            Ok(LoadPlan::new(*self))
        } else {
            Err(Error::NoLoadableSegments)
        }
    }
}

fn validate_program_headers<'a>(
    image: &'a [u8],
    table_offset: usize,
    entry_size: usize,
    count: usize,
) -> Result<(Option<&'a [u8]>, Option<ProgramHeader<'a>>)> {
    let mut interp = None;
    let mut dynamic = None;

    for index in 0..count {
        let entry_offset = index
            .checked_mul(entry_size)
            .and_then(|relative| table_offset.checked_add(relative))
            .ok_or(Error::ProgramHeaderTableOutOfBounds)?;
        let raw = RawProgramHeader::read(image, entry_offset)
            .ok_or(Error::ProgramHeaderTableOutOfBounds)?;

        let file_start = u64::from(raw.p_offset);
        raw.p_offset
            .checked_add(raw.p_filesz)
            .ok_or(Error::SegmentOverflow(index))?;
        let file_end = file_start
            .checked_add(u64::from(raw.p_filesz))
            .ok_or(Error::SegmentOverflow(index))?;
        if file_end > image.len() as u64 {
            return Err(Error::SegmentOutOfBounds(index));
        }
        if raw.p_filesz > raw.p_memsz {
            return Err(Error::FileSizeExceedsMemorySize(index));
        }
        if raw.p_vaddr.checked_add(raw.p_memsz).is_none() {
            return Err(Error::SegmentOverflow(index));
        }
        if raw.p_align != 0 && !raw.p_align.is_power_of_two() {
            return Err(Error::InvalidAlignment(index));
        }
        if raw.p_type == PT_LOAD
            && raw.p_align > 1
            && raw.p_vaddr % raw.p_align != raw.p_offset % raw.p_align
        {
            return Err(Error::MisalignedSegment(index));
        }

        let start = file_start as usize;
        let end = file_end as usize;
        let file = image
            .get(start..end)
            .ok_or(Error::SegmentOutOfBounds(index))?;
        let header = ProgramHeader::from_raw(raw, file);

        match header.kind() {
            SegmentType::Interp => {
                if interp.is_some() {
                    return Err(Error::DuplicateInterp);
                }
                interp = Some(interp_path(&header, index)?);
            }
            SegmentType::Dynamic if dynamic.is_none() => dynamic = Some(header),
            _ => {}
        }
    }

    Ok((interp, dynamic))
}

fn interp_path<'a>(header: &ProgramHeader<'a>, index: usize) -> Result<&'a [u8]> {
    let bytes = header.file();
    if bytes.is_empty() {
        return Err(Error::EmptyInterp(index));
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(Error::UnterminatedInterp(index))?;
    if end == 0 {
        return Err(Error::EmptyInterp(index));
    }
    Ok(&bytes[..end])
}
