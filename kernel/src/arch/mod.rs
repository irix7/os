// SPDX-License-Identifier: GPL-3.0-only
//! `arch` module: selects the architecture-specific implementation at build
//! time and exposes it to the shared kernel core.
//!
//! MIPS (`mips` target) uses `arch::mips::MipsArch`; x86_64 uses
//! `arch::x86_64::X86_64Arch`.  The shared core depends only on
//! [`Arch`](crate::arch::interface::Arch) and accesses the live instance
//! through [`arch`](crate::arch::arch).

#[cfg(target_arch = "mips64")]
pub mod mips;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

pub use interface::{Arch, ArchCaps, ArchError, TrapFrame};

mod interface;

// The concrete arch type for the current target.  It is a unit struct: all
// mutable arch state lives in `static mut` cells inside the arch module, so
// the value itself carries no data.
#[cfg(target_arch = "mips64")]
pub type ArchImpl = mips::MipsArch;

#[cfg(target_arch = "x86_64")]
pub type ArchImpl = x86_64::X86_64Arch;

/// The live arch instance.  Initialised by the boot entry before any core
/// code calls into it.  The value is a zero-sized unit struct; all mutable
/// arch state lives in `static mut` cells inside the arch module.
#[no_mangle]
static mut ARCH: ArchImpl = unsafe { core::mem::MaybeUninit::<ArchImpl>::zeroed().assume_init() };

/// Return the live arch instance as the shared `Arch` trait.
///
/// # Safety
/// Only call after the boot entry has initialised the arch.  Before that the
/// value is a zeroed unit struct with no usable state.
pub unsafe fn arch() -> &'static mut dyn Arch {
    &mut ARCH
}
