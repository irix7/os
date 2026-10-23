// SPDX-License-Identifier: GPL-3.0-only
//! The load plan: the `PT_LOAD` view a kernel mapper consumes.

use crate::program::{Permissions, ProgramHeader, ProgramHeaders, SegmentType};
use crate::Elf;

/// One `PT_LOAD` segment, viewed for mapping.
///
/// The segment does not describe page rounding or MMU permissions.  A kernel
/// loader maps `load_address` to `load_address + memory_size`, copies
/// [`file_data`](Self::file_data) at the start and zeroes the remaining
/// [`zero_fill`](Self::zero_fill) bytes.  Address translation (for example
/// into kseg0) is the caller's decision.
#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    index: usize,
    header: ProgramHeader<'a>,
}

impl<'a> Segment<'a> {
    /// The segment's index in the program-header table.
    pub const fn index(&self) -> usize {
        self.index
    }

    /// The underlying program header.
    pub const fn header(&self) -> ProgramHeader<'a> {
        self.header
    }

    /// The virtual address to load at (`p_vaddr`).
    pub const fn load_address(&self) -> u32 {
        self.header.virtual_address()
    }

    /// The physical address recorded in the image (`p_paddr`).
    pub const fn physical_address(&self) -> u32 {
        self.header.physical_address()
    }

    /// The file offset of the segment (`p_offset`).
    pub const fn file_offset(&self) -> u32 {
        self.header.file_offset()
    }

    /// The number of bytes to copy from the image (`p_filesz`).
    pub const fn file_size(&self) -> u32 {
        self.header.file_size()
    }

    /// The number of bytes the segment occupies in memory (`p_memsz`).
    pub const fn memory_size(&self) -> u32 {
        self.header.memory_size()
    }

    /// The number of bytes to zero after the copied data.
    ///
    /// This is `p_memsz - p_filesz`; parsing rejects a negative difference.
    pub const fn zero_fill(&self) -> u32 {
        self.header.memory_size() - self.header.file_size()
    }

    /// The segment bytes to copy from the image.
    pub const fn file_data(&self) -> &'a [u8] {
        self.header.file()
    }

    /// The decoded `p_flags` permissions.
    pub const fn permissions(&self) -> Permissions {
        self.header.permissions()
    }

    /// The raw `p_flags`.
    pub const fn flags(&self) -> u32 {
        self.header.flags()
    }

    /// The alignment recorded in `p_align`.
    pub const fn alignment(&self) -> u32 {
        self.header.alignment()
    }

    /// The first address after the segment's memory image.
    pub const fn memory_end(&self) -> u32 {
        self.header.memory_end()
    }

    /// The segment's memory range, `load_address..memory_end`.
    pub const fn memory_range(&self) -> core::ops::Range<u32> {
        self.load_address()..self.memory_end()
    }

    /// The segment's byte range in the image.
    pub fn file_range(&self) -> core::ops::Range<usize> {
        self.header.file_range()
    }
}

/// A cursor over the `PT_LOAD` segments in program-header order.
#[derive(Clone, Debug)]
pub struct LoadSegments<'a> {
    headers: ProgramHeaders<'a>,
    index: usize,
}

impl<'a> Iterator for LoadSegments<'a> {
    type Item = Segment<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let header = self.headers.next()?;
            let index = self.index;
            self.index += 1;
            if header.kind() == SegmentType::Load {
                return Some(Segment { index, header });
            }
        }
    }
}

/// A validated view of an image's loadable segments.
///
/// Construct one with [`Elf::load_plan`].  The plan borrows the image; it does
/// not copy it, allocate, or map anything.
#[derive(Clone, Copy, Debug)]
pub struct LoadPlan<'a> {
    elf: Elf<'a>,
}

impl<'a> LoadPlan<'a> {
    pub(crate) fn new(elf: Elf<'a>) -> Self {
        Self { elf }
    }

    /// The parsed image this plan came from.
    pub const fn elf(&self) -> Elf<'a> {
        self.elf
    }

    /// The entry virtual address (`e_entry`).
    pub const fn entry(&self) -> u32 {
        self.elf.entry()
    }

    /// The `e_type` of the image.
    pub const fn elf_type(&self) -> crate::ElfType {
        self.elf.elf_type()
    }

    /// The program interpreter path, without its NUL terminator.
    pub const fn interp(&self) -> Option<&'a [u8]> {
        self.elf.interp()
    }

    /// The `PT_DYNAMIC` header, if present.
    pub const fn dynamic(&self) -> Option<ProgramHeader<'a>> {
        self.elf.dynamic()
    }

    /// Iterate over the `PT_LOAD` segments in program-header order.
    pub fn segments(&self) -> LoadSegments<'a> {
        LoadSegments {
            headers: self.elf.program_headers(),
            index: 0,
        }
    }

    /// The lowest load address and the highest memory end, exclusive.
    ///
    /// Returns `None` for a plan with no loadable segments, although
    /// [`Elf::load_plan`] rejects such an image before a plan is built.
    pub fn extent(&self) -> Option<(u32, u32)> {
        let mut segments = self.segments();
        let first = segments.next()?;
        let mut low = first.load_address();
        let mut high = first.memory_end();
        for segment in segments {
            low = low.min(segment.load_address());
            high = high.max(segment.memory_end());
        }
        Some((low, high))
    }
}
