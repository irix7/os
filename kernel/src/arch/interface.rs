// SPDX-License-Identifier: GPL-3.0-only
//! `arch` — the architecture boundary between the shared kernel core and the
//! machine-specific bring-up code.
//!
//! The shared core (frames, heap, module loader, service ABI, scheduler, net
//! stack) never touches a CPU register, a page table, an IDT entry or a device
//! port directly.  It talks to the machine through this `Arch` trait.  Each
//! supported machine (MIPS Indy, x86_64 QEMU) provides one implementation.
//!
//! # Design rules
//!
//! * One `Arch` value per machine, created at boot and passed to the core.
//! * Methods that touch hardware are `unsafe` only inside the impl; the trait
//!   itself is safe Rust so the core can call it without `unsafe`.
//! * The trait is deliberately small: if the core needs a new primitive, add it
//!   here and implement it for every arch before using it.
//! * No compatibility shims or dual-arch wire formats.  The ABI contract
//!   (module-abi v15) is identical on every arch.

use crate::frames::MemDesc;

/// The trap frame presented to the arch-specific trap handler.
///
/// Every arch lays this out to match its own exception frame.  The core owns
/// the value and passes a mutable reference into [`Arch::trap`]; the arch
/// fills/reads the fields it needs.  Fields the core reads after a trap
/// (`cause`, `epc`, `badvaddr`, `status`) are documented below.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    /// Exception cause / vector number (arch-defined encoding).
    pub cause: u64,
    /// Program counter at the time of the exception.
    pub epc: u64,
    /// Faulting virtual address, when the exception carries one.
    pub badvaddr: u64,
    /// Processor status / flags at the time of the exception.
    pub status: u64,
    /// Scratch storage the arch may use between trap entry and the Rust handler.
    pub scratch: [u64; 8],
}

impl TrapFrame {
    pub const fn new() -> Self {
        TrapFrame {
            cause: 0,
            epc: 0,
            badvaddr: 0,
            status: 0,
            scratch: [0; 8],
        }
    }
}

/// Hardware capabilities reported by the arch at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchCaps(pub u32);

impl ArchCaps {
    pub const MMU: Self = Self(1 << 0);
    pub const VIRTIO: Self = Self(1 << 1);
    pub const BIG_ENDIAN: Self = Self(1 << 2);
    pub const RTC: Self = Self(1 << 3);
    pub const HW_RNG: Self = Self(1 << 4);
}

/// The arch boundary.  Implement once per supported machine.
pub trait Arch {
    /// Return the capabilities this machine reports.
    fn caps(&self) -> ArchCaps;

    /// Early arch initialisation, called before the frame allocator and the
    /// service table are live.  Set up the boot stack, early console and any
    /// state the panic path needs.
    fn init(&mut self) -> Result<(), ArchError>;

    /// Install the exception / interrupt vectors.  Called after `init`.
    fn install_vectors(&mut self);

    /// Initialise the periodic timer.  The core calls this once during boot.
    fn init_timer(&mut self);

    /// Number of timer ticks since `init_timer` (used by the idle loop and
    /// service clock).
    fn ticks(&self) -> u32;

    /// Park the CPU until the next interrupt.  Must not spin busy.
    fn wait(&mut self);

    /// Console output.  May block for a bounded time.
    fn console_write(&mut self, bytes: &[u8]);

    /// Non-blocking console input.  Returns the number of bytes read.
    fn console_read(&mut self, out: &mut [u8]) -> usize;

    /// Handle a trap/exception.  The assembly vector has saved registers into
    /// `tf` and switched to the arch's trap stack; this is the Rust handler.
    /// Return `true` if the trap was handled and execution should resume at
    /// `tf.epc` (possibly advanced by the arch), `false` for a fatal trap.
    fn trap(&mut self, tf: &mut TrapFrame) -> bool;

    /// Print a fatal trap and park.  Called when [`trap`] returns false.
    fn fatal_trap(&mut self, tf: &TrapFrame) -> !;

    /// Reboot the machine.  Does not return.
    fn reboot(&mut self) -> !;

    /// Power off the machine.  Does not return.
    fn poweroff(&mut self) -> !;

    /// Read the memory map from firmware/ROM and fill `descs`.  Returns the
    /// number of descriptors written.  The core passes usable descriptors to
    /// the frame allocator.
    fn memory_map(&mut self, descs: &mut [MemDesc]) -> usize;

    /// Cache maintenance: write back and invalidate the data cache over
    /// `[addr, addr + len)`.  No-op on architectures with coherent caches.
    fn dcache_writeback_invalidate(&mut self, addr: usize, len: usize);

    /// Cache maintenance: invalidate the instruction cache over
    /// `[addr, addr + len)`.  No-op on architectures with coherent caches.
    fn icache_invalidate(&mut self, addr: usize, len: usize);

    /// Complete all outstanding memory operations.
    fn sync(&mut self);

    /// Offset added to a physical address to obtain its kernel direct-map
    /// virtual address (0x8000_0000 on MIPS kseg0, 0 on identity-mapped x86).
    fn direct_map_offset(&self) -> u64;

    /// Read the battery-backed wall clock, if any.
    fn rtc_read_unix(&mut self) -> Option<u64>;
}

/// Error returned by the arch initialisation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchError {
    NoMemory,
    NoDevice,
    Initialisation,
}

impl core::fmt::Display for ArchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ArchError::NoMemory => "no memory",
            ArchError::NoDevice => "no device",
            ArchError::Initialisation => "initialisation error",
        })
    }
}
