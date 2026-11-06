// SPDX-License-Identifier: GPL-3.0-only
//! Architecture-neutral kernel core.
//!
//! The machines (`kernel`, `kernel-x86`, `kernel-ppc`) own boot, traps, timers,
//! buses, devices and cache maintenance.  They do not own the module loader
//! protocol or the module service table: those are the same on every machine
//! and live here.
//!
//! This is the first slice of the boundary described in
//! [`docs/plan/x86.md`].  It covers:
//!
//! * [`service`] — the one [`module_abi::Services`] table, its argument guards,
//!   refusals and the [`service::Platform`] trait a machine implements.
//!
//! The machines still own the fetch transport (HTTP, TLS, SCSI or ATA), the
//! module window layout, cache maintenance and the tick loop.  Those are
//! machine resources, not policy.
//!
//! [`docs/plan/x86.md`]: https://github.com/irix7/os/blob/main/docs/plan/x86.md

#![no_std]
#![deny(missing_docs)]

pub mod service;
