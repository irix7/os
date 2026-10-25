// SPDX-License-Identifier: GPL-3.0-only
//! `platform-api` — the native contract between a target shim and the
//! architecture-independent core.
//!
//! The WebAssembly core never calls this trait directly.  A shim implements
//! the trait, and a small generated host-ABI layer (`wasm-host`) exposes the
//! subset the core is allowed to use as `host_submit` opcodes.  Keeping the
//! trait native means:
//!
//! * the same shim can run the core, a recovery shell, or a test harness;
//! * device drivers can be unit-tested against a mock implementation;
//! * the Wasm import surface stays a small, versioned integer ABI.
//!
//! ## Threading and blocking
//!
//! Every method is called from the shim's single event-loop thread unless a
//! target explicitly documents otherwise.  Methods that can block are named
//! `wait` and `console_write`; all I/O submission methods are non-blocking and
//! report completion through [`CompletionSink`] or [`EventId`].
//!
//! ## Safety
//!
//! The trait is safe Rust.  Target implementations contain the `unsafe`
//! register and DMA code; the trait boundary is the audit seam.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use core::fmt;
use sys_primitives::{Completion, CompletionId, EventId, ObjectId};

/// Physical address.
pub type PhysAddr = u64;
/// DMA address as seen by a device.
pub type DmaAddr = u64;
/// Virtual address in the shim's direct map.
pub type VirtAddr = usize;
/// Monotonic nanoseconds.
pub type Nanos = u64;

/// A physical page run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysFrame {
    /// Base physical address.
    pub addr: PhysAddr,
    /// Order: 0 = 4 KiB, 1 = 8 KiB, ... 10 = 4 MiB.
    pub order: u8,
}

/// A DMA-capable buffer owned by the shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBuffer {
    /// Physical base.
    pub phys: PhysAddr,
    /// Shim virtual base (kseg0 on MIPS, direct map on x86/ARM).
    pub virt: VirtAddr,
    /// Length in bytes.
    pub len: usize,
    /// Allocation order.
    pub order: u8,
    /// Allocation flags.
    pub flags: DmaFlags,
}

/// DMA buffer properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct DmaFlags(pub u32);

impl DmaFlags {
    /// The buffer is cacheable and needs explicit sync.
    pub const CACHEABLE: Self = Self(1 << 0);
    /// The buffer is mapped uncached.
    pub const UNCACHED: Self = Self(1 << 1);
    /// The buffer is physically contiguous.
    pub const CONTIGUOUS: Self = Self(1 << 2);
    /// The buffer is device memory, not RAM.
    pub const DEVICE: Self = Self(1 << 3);
    /// The buffer is pinned for the lifetime of the grant.
    pub const PINNED: Self = Self(1 << 4);
}

/// Page-allocation flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct AllocFlags(pub u32);

impl AllocFlags {
    /// Zero the allocation before returning it.
    pub const ZERO: Self = Self(1 << 0);
    /// The allocation must be DMA-visible.
    pub const DMA: Self = Self(1 << 1);
    /// The allocation must be below 4 GiB (legacy devices).
    pub const LOW: Self = Self(1 << 2);
    /// The allocation must be physically contiguous.
    pub const CONTIGUOUS: Self = Self(1 << 3);
}

/// Memory protection bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct MapProt(pub u32);

impl MapProt {
    /// Readable.
    pub const READ: Self = Self(1 << 0);
    /// Writable.
    pub const WRITE: Self = Self(1 << 1);
    /// Executable.
    pub const EXECUTE: Self = Self(1 << 2);
    /// Accessible from user mode (legacy/POSIX domains).
    pub const USER: Self = Self(1 << 3);
    /// Cacheable.
    pub const CACHEABLE: Self = Self(1 << 4);
    /// Device/uncached.
    pub const DEVICE: Self = Self(1 << 5);
    /// Global mapping (not flushed on address-space switch).
    pub const GLOBAL: Self = Self(1 << 6);
}

/// An address-space or protection domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DomainId(pub u32);

/// A hardware interrupt line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct IrqId(pub u32);

/// A registered interrupt binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct IrqHandle(pub u64);

/// A registered one-shot timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct TimerHandle(pub u64);

/// A shared-memory ring identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ShmemId(pub u32);

/// A peer node or protection domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PeerId(pub u32);

/// A DMA direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DmaDir {
    /// CPU to device.
    ToDevice = 0,
    /// Device to CPU.
    FromDevice = 1,
    /// Both directions.
    Bidirectional = 2,
}

/// A block operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockOp {
    /// Read sectors.
    Read = 0,
    /// Write sectors.
    Write = 1,
    /// Flush the device cache.
    Flush = 2,
    /// Discard sectors.
    Trim = 3,
}

/// Block-device geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInfo {
    /// Logical sector size in bytes.
    pub sector_size: u32,
    /// Number of logical sectors.
    pub sectors: u64,
    /// Whether writes are refused.
    pub read_only: bool,
    /// Maximum outstanding requests.
    pub queue_depth: u16,
}

/// Network-device features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct NicFeatures(pub u32);

impl NicFeatures {
    /// Checksum offload.
    pub const CSUM: Self = Self(1 << 0);
    /// TCP segmentation offload.
    pub const TSO: Self = Self(1 << 1);
    /// MSI-X or equivalent per-queue interrupts.
    pub const MSIX: Self = Self(1 << 2);
    /// Multiple RX/TX queues.
    pub const MULTIQUEUE: Self = Self(1 << 3);
    /// VLAN insertion/stripping.
    pub const VLAN: Self = Self(1 << 4);
    /// Receive-side scaling.
    pub const RSS: Self = Self(1 << 5);
}

/// Network-device description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NicInfo {
    /// Station address.
    pub mac: [u8; 6],
    /// Maximum transmission unit.
    pub mtu: u16,
    /// Link is up.
    pub link: bool,
    /// Number of RX queues.
    pub rx_queues: u8,
    /// Number of TX queues.
    pub tx_queues: u8,
    /// Negotiated link speed in Mbit/s (0 = unknown).
    pub speed_mbps: u32,
    /// Feature bits.
    pub features: NicFeatures,
}

/// Capabilities discovered by the shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ShimCaps(pub u32);

impl ShimCaps {
    /// The target has an MMU.
    pub const MMU: Self = Self(1 << 0);
    /// The target has an IOMMU (VT-d, SMMU, or equivalent).
    pub const IOMMU: Self = Self(1 << 1);
    /// The target supports DMA-buf-style export/import.
    pub const DMA_BUF: Self = Self(1 << 2);
    /// The target supports MSI-X.
    pub const MSIX: Self = Self(1 << 3);
    /// The target has virtio devices.
    pub const VIRTIO: Self = Self(1 << 4);
    /// The target has RDMA.
    pub const RDMA: Self = Self(1 << 5);
    /// The target is big-endian.
    pub const BIG_ENDIAN: Self = Self(1 << 6);
    /// The NIC has multiple queues.
    pub const NIC_MULTIQUEUE: Self = Self(1 << 7);
    /// A hardware random source is present.
    pub const HW_RNG: Self = Self(1 << 8);
    /// A battery-backed wall clock is present.
    pub const RTC: Self = Self(1 << 9);
    /// The target can migrate actors to another node.
    pub const MIGRATION: Self = Self(1 << 10);
}

/// A shared-memory ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shmem {
    /// Ring identity.
    pub id: ShmemId,
    /// Physical base.
    pub phys: PhysAddr,
    /// Length in bytes (two rings plus doorbells, by convention).
    pub len: usize,
    /// Peer identity.
    pub peer: PeerId,
    /// Whether the memory is cacheable.
    pub cacheable: bool,
}

/// A target-shim error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum PlatformError {
    /// The operation is not supported on this target.
    NotSupported = -1,
    /// A parameter is invalid.
    Invalid = -2,
    /// Out of memory.
    NoMemory = -3,
    /// The device or domain is busy.
    Busy = -4,
    /// The device is gone.
    NoDevice = -5,
    /// An I/O error occurred.
    Io = -6,
    /// The operation timed out.
    Timeout = -7,
    /// The caller lacks authority.
    Permission = -8,
    /// The buffer is not DMA-capable.
    NotDma = -9,
    /// The address is not mapped.
    Fault = -10,
    /// The link is down.
    LinkDown = -11,
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotSupported => "not supported",
            Self::Invalid => "invalid",
            Self::NoMemory => "out of memory",
            Self::Busy => "busy",
            Self::NoDevice => "no device",
            Self::Io => "I/O error",
            Self::Timeout => "timeout",
            Self::Permission => "permission denied",
            Self::NotDma => "not DMA-capable",
            Self::Fault => "fault",
            Self::LinkDown => "link down",
        })
    }
}

/// A bounded completion sink.  The shim pushes completions; the core drains
/// them from its completion ring.  No allocation is performed here.
pub struct CompletionSink<'a> {
    buf: &'a mut [Completion],
    len: usize,
}

impl<'a> CompletionSink<'a> {
    /// Create a sink over `buf`.
    pub fn new(buf: &'a mut [Completion]) -> Self {
        Self { buf, len: 0 }
    }

    /// Push one completion.  Returns `false` when the sink is full.
    pub fn push(&mut self, completion: Completion) -> bool {
        if self.len == self.buf.len() {
            return false;
        }
        self.buf[self.len] = completion;
        self.len += 1;
        true
    }

    /// Number of completions written.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no completions were written.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The written completions.
    pub fn as_slice(&self) -> &[Completion] {
        &self.buf[..self.len]
    }
}

/// The native platform contract.
///
/// An implementation owns the hardware and the TCB.  The core owns policy:
/// which actor runs, which capability is checked, which object is read.  The
/// shim enforces the capability decision but never interprets the actor's
/// bytecode.
pub trait PlatformShim {
    /// Initialise the shim.  Called once before any other method.
    fn init(&mut self) -> Result<(), PlatformError>;

    /// Hardware capabilities discovered at boot.
    fn caps(&self) -> ShimCaps;

    /// Monotonic time in nanoseconds.
    fn now(&self) -> Nanos;

    // -----------------------------------------------------------------
    // Physical memory and MMU
    // -----------------------------------------------------------------

    /// Allocate a physically contiguous page run.
    fn alloc_pages(&mut self, order: u8, flags: AllocFlags) -> Result<DmaBuffer, PlatformError>;

    /// Free a page run previously returned by [`alloc_pages`].
    fn free_pages(&mut self, buffer: DmaBuffer);

    /// Map `len` bytes at `virt` to `phys` in `domain`.
    fn map(
        &mut self,
        domain: DomainId,
        virt: VirtAddr,
        phys: PhysAddr,
        len: usize,
        prot: MapProt,
    ) -> Result<(), PlatformError>;

    /// Remove a mapping.
    fn unmap(&mut self, domain: DomainId, virt: VirtAddr, len: usize) -> Result<(), PlatformError>;

    /// Change protection on an existing mapping.
    fn protect(
        &mut self,
        domain: DomainId,
        virt: VirtAddr,
        len: usize,
        prot: MapProt,
    ) -> Result<(), PlatformError>;

    /// Switch to `domain` on the current CPU.
    fn activate(&mut self, domain: DomainId) -> Result<(), PlatformError>;

    // -----------------------------------------------------------------
    // Interrupts and timers
    // -----------------------------------------------------------------

    /// Bind `irq` to a completion event.  The shim acknowledges the device and
    /// posts a [`Completion`] with `event`; it never calls back into the core.
    fn irq_register(&mut self, irq: IrqId, event: EventId) -> Result<IrqHandle, PlatformError>;

    /// Acknowledge an interrupt at the device.
    fn irq_ack(&mut self, handle: IrqHandle);

    /// Mask an interrupt.
    fn irq_mask(&mut self, handle: IrqHandle);

    /// Unmask an interrupt.
    fn irq_unmask(&mut self, handle: IrqHandle);

    /// Arm a one-shot timer.  The shim posts `event` at or after `deadline`.
    fn timer_arm(&mut self, deadline: Nanos, event: EventId) -> Result<TimerHandle, PlatformError>;

    /// Cancel a timer.  Cancelling an expired timer is not an error.
    fn timer_cancel(&mut self, handle: TimerHandle);

    // -----------------------------------------------------------------
    // Network
    // -----------------------------------------------------------------

    /// Describe the primary NIC.
    fn nic_info(&self) -> NicInfo;

    /// Post receive buffers to `queue`.  The shim owns the descriptors.
    fn nic_rx_post(&mut self, queue: u16, buffers: &[DmaBuffer]) -> Result<(), PlatformError>;

    /// Submit a transmit buffer.  Ownership passes to the shim until the
    /// completion event fires.
    fn nic_tx(
        &mut self,
        queue: u16,
        buffer: DmaBuffer,
        event: EventId,
    ) -> Result<(), PlatformError>;

    /// Poll the NIC and push receive/transmit completions.  Returns the number
    /// of completions pushed.  `budget` is a hard upper bound.
    fn nic_poll(&mut self, budget: usize, out: &mut CompletionSink<'_>) -> usize;

    // -----------------------------------------------------------------
    // Block
    // -----------------------------------------------------------------

    /// Describe the primary block device.
    fn block_info(&self) -> BlockInfo;

    /// Submit a block operation.  `buffer` must be DMA-capable and large
    /// enough for `sectors * sector_size`.  The completion event fires when
    /// the device has finished.
    fn block_submit(
        &mut self,
        op: BlockOp,
        lba: u64,
        sectors: u32,
        buffer: DmaBuffer,
        event: EventId,
    ) -> Result<(), PlatformError>;

    /// Flush the block device.
    fn block_flush(&mut self, event: EventId) -> Result<(), PlatformError>;

    // -----------------------------------------------------------------
    // DMA and shared memory
    // -----------------------------------------------------------------

    /// Map a buffer for device access in `domain` and return the DMA address.
    fn dma_map(
        &mut self,
        domain: DomainId,
        buffer: &DmaBuffer,
        dir: DmaDir,
    ) -> Result<DmaAddr, PlatformError>;

    /// Tear down a DMA mapping.
    fn dma_unmap(
        &mut self,
        domain: DomainId,
        buffer: &DmaBuffer,
        dir: DmaDir,
    ) -> Result<(), PlatformError>;

    /// Write back or invalidate caches around a DMA transfer.
    fn dma_sync(&mut self, buffer: &DmaBuffer, dir: DmaDir);

    /// Create a shared-memory ring for a peer.
    fn shmem_create(&mut self, len: usize, align: usize) -> Result<Shmem, PlatformError>;

    /// Attach to a peer's shared-memory ring.
    fn shmem_attach(&mut self, id: ShmemId) -> Result<Shmem, PlatformError>;

    /// Ring the peer's doorbell.
    fn shmem_notify(&mut self, id: ShmemId, doorbell: u32);

    // -----------------------------------------------------------------
    // Console, entropy and lifecycle
    // -----------------------------------------------------------------

    /// Write bytes to the boot/console sink.  May block for a bounded time.
    fn console_write(&mut self, bytes: &[u8]);

    /// Read console input without blocking.  Returns the number of bytes read.
    fn console_read(&mut self, out: &mut [u8]) -> usize;

    /// Fill `out` with entropy.  A target without a hardware RNG must mix
    /// timing, device state and persistent seed material; it must not return
    /// an all-zero buffer.
    fn random(&mut self, out: &mut [u8]) -> Result<(), PlatformError>;

    /// Park the CPU until the next interrupt.  Must not spin.
    fn wait(&mut self);

    /// Reset the machine.  Does not return.
    fn reset(&mut self) -> !;
}

/// A trait for targets that can report a stable machine identifier.
pub trait MachineId {
    /// Return a stable 16-byte machine identifier.
    fn machine_id(&self) -> [u8; 16];
}

/// A trait for targets that can name an object's storage location.
pub trait ObjectLocation {
    /// Return the shim's location hint for `object`.
    fn location(&self, object: ObjectId) -> Option<u64>;
}

/// A native task identity.  This is deliberately not an `ActorId`: a native
/// POSIX/IRIX task has a register context, an address space and a kernel
/// stack, none of which the Wasm core can manipulate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct NativeTaskId(pub u32);

/// The initial register state of a native task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEntry {
    /// Entry program counter.
    pub pc: VirtAddr,
    /// Initial stack pointer.
    pub sp: VirtAddr,
    /// Architecture-specific argument (for example, `argc` or a pointer).
    pub arg: u64,
    /// Address space the task runs in.
    pub domain: DomainId,
}

/// Why a native task blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// Waiting for a device or completion.
    Io,
    /// Waiting for a timer.
    Timer,
    /// Waiting for an IPC reply.
    Ipc,
    /// Voluntary yield.
    Yield,
    /// Waiting for a capability.
    Capability,
}

/// The native context-switch bridge.  The core decides *which* task runs; the
/// shim saves and restores the registers.  This trait is separate from
/// [`PlatformShim`] because a target without native tasks (a pure Wasm
/// microkernel) need not implement it.
pub trait NativeTaskBridge {
    /// Create a native task.
    fn spawn(
        &mut self,
        entry: NativeEntry,
        stack: DmaBuffer,
        priority: u8,
    ) -> Result<NativeTaskId, PlatformError>;

    /// Yield the current native task.
    fn yield_now(&mut self);

    /// Block the current native task.
    fn block(&mut self, reason: BlockReason);

    /// Wake a native task.
    fn wake(&mut self, task: NativeTaskId);

    /// Kill a native task.
    fn kill(&mut self, task: NativeTaskId);

    /// Set an absolute deadline for a native task.
    fn set_deadline(&mut self, task: NativeTaskId, deadline: Nanos);

    /// Set a fair-scheduling weight.
    fn set_weight(&mut self, task: NativeTaskId, weight: u32);

    /// Switch from one native task to another.  Must be called with interrupts
    /// disabled or with a documented preemption rule.
    fn context_switch(&mut self, from: NativeTaskId, to: NativeTaskId);
}

// A few compile-time checks that keep the ABI types small and stable.
const _: () = assert!(core::mem::size_of::<DmaBuffer>() <= 48);
const _: () = assert!(core::mem::size_of::<Shmem>() <= 48);
const _: () = assert!(core::mem::size_of::<NicInfo>() <= 32);
const _: () = assert!(core::mem::size_of::<BlockInfo>() <= 24);
const _: () = assert!(core::mem::size_of::<Completion>() == 32);
const _: () = assert!(core::mem::size_of::<CompletionId>() == 8);
