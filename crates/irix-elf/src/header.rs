// SPDX-License-Identifier: GPL-3.0-only
//! The ELF file header and the ABI values derived from it.

use crate::constants::{
    EF_MIPS_ABI, EF_MIPS_ABI2, EF_MIPS_ABI_EABI32, EF_MIPS_ABI_EABI64, EF_MIPS_ABI_O32,
    EF_MIPS_ABI_O64, EHDR_SIZE, ELFCLASS32, ELFDATA2MSB, ELF_MAGIC, EM_MIPS, EV_CURRENT,
    MAX_SHDR_ENTRY_SIZE, SHDR_SIZE,
};
use crate::error::{Error, Result};
use crate::read::{u16_at, u32_at};

/// The `e_type` of an ELF image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfType {
    /// No file type.
    None,
    /// A relocatable object.
    Rel,
    /// An executable.
    Exec,
    /// A position-independent object or shared library.
    Dyn,
    /// A core file.
    Core,
    /// A type this crate does not name.
    Other(u16),
}

impl ElfType {
    /// Decode an `e_type` field.
    pub const fn from_raw(value: u16) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Rel,
            2 => Self::Exec,
            3 => Self::Dyn,
            4 => Self::Core,
            other => Self::Other(other),
        }
    }

    /// The raw `e_type` value.
    pub const fn raw(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Rel => 1,
            Self::Exec => 2,
            Self::Dyn => 3,
            Self::Core => 4,
            Self::Other(value) => value,
        }
    }
}

/// The `EI_OSABI` value of an ELF image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsAbi {
    /// The System V ABI, which is what most IRIX binaries declare.
    SysV,
    /// HP-UX.
    HpUx,
    /// NetBSD.
    NetBsd,
    /// Linux.
    Linux,
    /// Solaris.
    Solaris,
    /// AIX.
    Aix,
    /// IRIX.
    Irix,
    /// FreeBSD.
    FreeBsd,
    /// An OS ABI this crate does not name.
    Other(u8),
}

impl OsAbi {
    /// Decode an `EI_OSABI` field.
    pub const fn from_raw(value: u8) -> Self {
        match value {
            0 => Self::SysV,
            1 => Self::HpUx,
            2 => Self::NetBsd,
            3 => Self::Linux,
            6 => Self::Solaris,
            7 => Self::Aix,
            8 => Self::Irix,
            9 => Self::FreeBsd,
            other => Self::Other(other),
        }
    }

    /// The raw `EI_OSABI` value.
    pub const fn raw(self) -> u8 {
        match self {
            Self::SysV => 0,
            Self::HpUx => 1,
            Self::NetBsd => 2,
            Self::Linux => 3,
            Self::Solaris => 6,
            Self::Aix => 7,
            Self::Irix => 8,
            Self::FreeBsd => 9,
            Self::Other(value) => value,
        }
    }
}

/// The MIPS ABI declared by `e_flags`.
///
/// IRIX o32 and n32 images are both ELF32.  The ABI is not part of the ELF
/// class: o32 sets the ABI field to `EF_MIPS_ABI_O32`, while n32 sets
/// `EF_MIPS_ABI2` and leaves the ABI field zero.  Anything else is left as
/// [`Abi::Unknown`] rather than guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abi {
    /// The 32-bit o32 ABI.
    O32,
    /// The 32-bit-pointer n32 ABI with 64-bit registers.
    N32,
    /// The o64 ABI.
    O64,
    /// The EABI32 ABI.
    Eabi32,
    /// The EABI64 ABI.
    Eabi64,
    /// No ABI, or an ABI this crate does not name.
    Unknown,
}

impl Abi {
    /// Classify the MIPS ABI bits of an `e_flags` value.
    pub const fn from_flags(flags: u32) -> Self {
        if flags & EF_MIPS_ABI == EF_MIPS_ABI_O32 {
            Self::O32
        } else if flags & EF_MIPS_ABI == EF_MIPS_ABI_O64 {
            Self::O64
        } else if flags & EF_MIPS_ABI == EF_MIPS_ABI_EABI32 {
            Self::Eabi32
        } else if flags & EF_MIPS_ABI == EF_MIPS_ABI_EABI64 {
            Self::Eabi64
        } else if flags & EF_MIPS_ABI == 0 && flags & EF_MIPS_ABI2 != 0 {
            Self::N32
        } else {
            Self::Unknown
        }
    }
}

/// A validated section-header table.
///
/// The loader does not read sections, so this view records only the table
/// location and size.  Section contents and names are not parsed.  Extended
/// numbering (`e_shnum == 0` with a non-zero `e_shoff`) is rejected at parse
/// time because its real count lives in section header zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionTable {
    offset: u32,
    entry_size: u16,
    count: u16,
}

impl SectionTable {
    /// The file offset of the first section header.
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// The size of one section-header entry.
    pub const fn entry_size(&self) -> u16 {
        self.entry_size
    }

    /// The number of section headers.
    pub const fn count(&self) -> u16 {
        self.count
    }
}

/// A validated ELF32 file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    os_abi: OsAbi,
    abi_version: u8,
    elf_type: ElfType,
    machine: u16,
    version: u32,
    entry: u32,
    flags: u32,
    header_size: u16,
    program_header_offset: u32,
    program_header_entry_size: u16,
    program_header_count: u16,
    section_table: Option<SectionTable>,
}

impl Header {
    pub(crate) fn parse(image: &[u8]) -> Result<Self> {
        if image.len() < EHDR_SIZE {
            return Err(Error::Truncated);
        }
        if image.get(..4) != Some(ELF_MAGIC.as_slice()) {
            return Err(Error::BadMagic);
        }
        if image.get(4).copied() != Some(ELFCLASS32) {
            return Err(Error::NotElf32);
        }
        if image.get(5).copied() != Some(ELFDATA2MSB) {
            return Err(Error::NotBigEndian);
        }
        if u32::from(image.get(6).copied().ok_or(Error::Truncated)?) != EV_CURRENT {
            return Err(Error::BadIdentVersion);
        }

        let os_abi = OsAbi::from_raw(image.get(7).copied().ok_or(Error::Truncated)?);
        let abi_version = image.get(8).copied().ok_or(Error::Truncated)?;
        let raw_type = u16_at(image, 16).ok_or(Error::Truncated)?;
        let elf_type = ElfType::from_raw(raw_type);
        if !matches!(elf_type, ElfType::Exec | ElfType::Dyn) {
            return Err(Error::UnsupportedElfType(raw_type));
        }
        let machine = u16_at(image, 18).ok_or(Error::Truncated)?;
        if machine != EM_MIPS {
            return Err(Error::UnsupportedMachine(machine));
        }
        let version = u32_at(image, 20).ok_or(Error::Truncated)?;
        if version != EV_CURRENT {
            return Err(Error::BadHeaderVersion);
        }
        let entry = u32_at(image, 24).ok_or(Error::Truncated)?;
        let program_header_offset = u32_at(image, 28).ok_or(Error::Truncated)?;
        let section_header_offset = u32_at(image, 32).ok_or(Error::Truncated)?;
        let flags = u32_at(image, 36).ok_or(Error::Truncated)?;
        let header_size = u16_at(image, 40).ok_or(Error::Truncated)?;
        if usize::from(header_size) < EHDR_SIZE {
            return Err(Error::InvalidHeaderSize(header_size));
        }
        let program_header_entry_size = u16_at(image, 42).ok_or(Error::Truncated)?;
        let program_header_count = u16_at(image, 44).ok_or(Error::Truncated)?;
        let section_header_entry_size = u16_at(image, 46).ok_or(Error::Truncated)?;
        let section_header_count = u16_at(image, 48).ok_or(Error::Truncated)?;

        let section_table = Self::parse_section_table(
            image,
            section_header_offset,
            section_header_entry_size,
            section_header_count,
        )?;

        Ok(Self {
            os_abi,
            abi_version,
            elf_type,
            machine,
            version,
            entry,
            flags,
            header_size,
            program_header_offset,
            program_header_entry_size,
            program_header_count,
            section_table,
        })
    }

    fn parse_section_table(
        image: &[u8],
        offset: u32,
        entry_size: u16,
        count: u16,
    ) -> Result<Option<SectionTable>> {
        if offset == 0 {
            if count != 0 {
                return Err(Error::InvalidSectionTable);
            }
            return Ok(None);
        }
        if count == 0 {
            // ELF extended numbering stores the real count in section header
            // zero.  Sections are not needed for loading, so do not chase it.
            return Err(Error::InvalidSectionTable);
        }
        if usize::from(entry_size) < SHDR_SIZE || entry_size > MAX_SHDR_ENTRY_SIZE {
            return Err(Error::InvalidSectionTable);
        }
        let end = u64::from(offset)
            .checked_add(u64::from(entry_size) * u64::from(count))
            .ok_or(Error::SectionHeaderTableOutOfBounds)?;
        if end > image.len() as u64 {
            return Err(Error::SectionHeaderTableOutOfBounds);
        }
        Ok(Some(SectionTable {
            offset,
            entry_size,
            count,
        }))
    }

    /// The `EI_OSABI` value.
    pub const fn os_abi(&self) -> OsAbi {
        self.os_abi
    }

    /// The `EI_ABIVERSION` value.
    pub const fn abi_version(&self) -> u8 {
        self.abi_version
    }

    /// The `e_type` value.
    pub const fn elf_type(&self) -> ElfType {
        self.elf_type
    }

    /// The `e_machine` value, always [`EM_MIPS`](crate::constants::EM_MIPS)
    /// after a successful parse.
    pub const fn machine(&self) -> u16 {
        self.machine
    }

    /// The `e_version` value.
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// The `e_entry` virtual address.
    pub const fn entry(&self) -> u32 {
        self.entry
    }

    /// The `e_flags` value.
    pub const fn flags(&self) -> u32 {
        self.flags
    }

    /// The MIPS ABI derived from `e_flags`.
    pub const fn abi(&self) -> Abi {
        Abi::from_flags(self.flags)
    }

    /// The `e_ehsize` value.
    pub const fn header_size(&self) -> u16 {
        self.header_size
    }

    /// The `e_phoff` value.
    pub const fn program_header_offset(&self) -> u32 {
        self.program_header_offset
    }

    /// The `e_phentsize` value.
    pub const fn program_header_entry_size(&self) -> u16 {
        self.program_header_entry_size
    }

    /// The `e_phnum` value.
    pub const fn program_header_count(&self) -> u16 {
        self.program_header_count
    }

    /// The validated section-header table, if the image has one.
    pub const fn section_table(&self) -> Option<SectionTable> {
        self.section_table
    }
}
