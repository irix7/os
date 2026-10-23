// SPDX-License-Identifier: GPL-3.0-only
//! The parser and load-plan view for IRIX o32 and n32 ELF images.
//!
//! This crate is the first dependency of the M5 legacy container.  It reads a
//! 32-bit big-endian MIPS ELF image, validates every field and range it
//! consumes, and exposes the program headers and the `PT_LOAD` segments a
//! kernel loader needs.  It does **not** map memory, allocate frames, parse
//! relocations or follow the program interpreter.
//!
//! The crate is `no_std`, has no allocation and no external dependencies, and
//! contains no unsafe code.  All parsing is done directly over the caller's
//! image slice, so an `Elf` borrows the image for as long as it lives.
//!
//! ```text
//! let image = fs::read("hello")?;
//! let elf = irix_elf::Elf::parse(&image)?;
//! let plan = elf.load_plan()?;
//!
//! for segment in plan.segments() {
//!     // Copy segment.file_data() to segment.load_address(), then zero
//!     // segment.zero_fill() bytes after it.
//! }
//! ```
//!
//! The loader contract, including the accepted subset and the deliberate
//! limitations, is documented in
//! [`docs/spec/elf.md`](https://github.com/irix7/os/blob/main/docs/spec/elf.md).

#![no_std]
#![deny(unsafe_code)]

pub mod constants;

mod elf;
mod error;
mod header;
mod load;
mod program;
mod read;

pub use elf::Elf;
pub use error::{Error, Result};
pub use header::{Abi, ElfType, Header, OsAbi, SectionTable};
pub use load::{LoadPlan, LoadSegments, Segment};
pub use program::{Permissions, ProgramHeader, ProgramHeaders, SegmentType};
