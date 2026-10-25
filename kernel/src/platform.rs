// SPDX-License-Identifier: GPL-3.0-only
//! Indy native platform shim.
//!
//! The ROM and the existing kernel bring-up have already programmed the
//! memory controller, mapped DRAM at its low-memory alias and started the
//! serial console.  This module presents those resources through the same
//! `PlatformShim` contract the architecture core will use; it does not
//! duplicate hardware initialisation.
//!
//! The first slice covers the resources the native kernel genuinely owns
//! today: physical frames, the kseg0 direct map, DMA cache maintenance, the
//! polled console and keyboard, the battery-backed clock, entropy and reset.
//! Asynchronous device submission stays unsupported until the completion
//! engine replaces the current polled drivers; it returns `NotSupported`
//! rather than pretending a request completed.

use core::ptr::{addr_of_mut, read_volatile, write_volatile};

use platform_api::{
    AllocFlags, BlockInfo, BlockOp, CompletionSink, DmaAddr, DmaBuffer, DmaDir, DmaFlags, DomainId,
    IrqHandle, IrqId, MapProt, Nanos, NicFeatures, NicInfo, PlatformError, PlatformShim, ShimCaps,
    TimerHandle, VirtAddr,
};
use sys_primitives::EventId;

/// Base of the cached MIPS direct map.
const KSEG0_BASE: u64 = 0x8000_0000;
/// Largest page order the frame allocator can serve contiguously.
const MAX_ORDER: u8 = 10;
/// CP0 Count frequency used by the kernel network clock.
const COUNT_HZ: u64 = 33_000_000;

static mut LAST_COUNT: u32 = 0;
static mut COUNT_WRAPS: u64 = 0;
static mut RNG_KEY: [u8; 32] = [0; 32];
static mut RNG_COUNTER: u64 = 0;

/// The Indy platform shim.
#[derive(Clone, Copy, Debug)]
pub struct IndyShim;

impl IndyShim {
    /// Create a shim over the already-initialised hardware.
    pub const fn new() -> Self {
        Self
    }

    /// Calibrate the monotonic clock and seed the boot entropy pool.
    pub fn start(&mut self) -> Result<(), PlatformError> {
        unsafe {
            LAST_COUNT = crate::traps::count_read();
            COUNT_WRAPS = 0;
        }
        seed_entropy();
        Ok(())
    }
}

fn seed_entropy() {
    use crate::sha256::Hasher;
    let mut hasher = Hasher::new();
    hasher.update(b"IRIX7-INDY-RNG-v1");
    hasher.update(&crate::traps::count_read().to_be_bytes());
    hasher.update(&crate::rtc::read_unix().unwrap_or(0).to_be_bytes());
    hasher.update(&crate::net::mac_address());
    hasher.update(&(unsafe { addr_of_mut!(RNG_KEY) } as usize).to_be_bytes());
    unsafe {
        RNG_KEY = hasher.finalize().0;
        RNG_COUNTER = 0;
    }
}

fn entropy_bytes(out: &mut [u8]) {
    use crate::sha256::Hasher;
    for chunk in out.chunks_mut(32) {
        unsafe {
            RNG_COUNTER = RNG_COUNTER.wrapping_add(1);
            let mut hasher = Hasher::new();
            hasher.update(&*core::ptr::addr_of!(RNG_KEY));
            hasher.update(&RNG_COUNTER.to_be_bytes());
            let digest = hasher.finalize().0;
            RNG_KEY = digest;
            chunk.copy_from_slice(&digest[..chunk.len()]);
        }
    }
}

fn monotonic_nanos() -> Nanos {
    unsafe {
        let now = crate::traps::count_read();
        if now < LAST_COUNT {
            COUNT_WRAPS = COUNT_WRAPS.saturating_add(1);
        }
        LAST_COUNT = now;
        let total = (COUNT_WRAPS << 32) | now as u64;
        total.saturating_mul(1_000_000_000) / COUNT_HZ
    }
}

fn page_count(order: u8) -> Option<usize> {
    if order > MAX_ORDER {
        None
    } else {
        Some(1usize << order)
    }
}

impl PlatformShim for IndyShim {
    fn init(&mut self) -> Result<(), PlatformError> {
        self.start()
    }

    fn caps(&self) -> ShimCaps {
        ShimCaps(ShimCaps::MMU.0 | ShimCaps::BIG_ENDIAN.0 | ShimCaps::RTC.0)
    }

    fn now(&self) -> Nanos {
        monotonic_nanos()
    }

    fn alloc_pages(&mut self, order: u8, flags: AllocFlags) -> Result<DmaBuffer, PlatformError> {
        let pages = page_count(order).ok_or(PlatformError::Invalid)?;
        let len = pages * 4096;
        if flags.0
            & !(AllocFlags::ZERO.0
                | AllocFlags::DMA.0
                | AllocFlags::LOW.0
                | AllocFlags::CONTIGUOUS.0)
            != 0
        {
            return Err(PlatformError::Invalid);
        }
        let phys = crate::frames::alloc_frames(pages).ok_or(PlatformError::NoMemory)?;
        let virt = KSEG0_BASE.wrapping_add(phys) as VirtAddr;
        if flags.0 & AllocFlags::ZERO.0 != 0 {
            for offset in 0..len {
                // SAFETY: the frame allocator returned this physical run, and
                // its kseg0 alias is the kernel's direct map.
                unsafe {
                    write_volatile((virt + offset) as *mut u8, 0);
                }
            }
        }
        Ok(DmaBuffer {
            phys,
            virt,
            len,
            order,
            flags: DmaFlags(DmaFlags::CACHEABLE.0 | DmaFlags::CONTIGUOUS.0),
        })
    }

    fn free_pages(&mut self, buffer: DmaBuffer) {
        let Some(pages) = page_count(buffer.order) else {
            return;
        };
        for page in 0..pages {
            crate::frames::free_frame(buffer.phys + (page as u64) * 4096);
        }
    }

    fn map(
        &mut self,
        domain: DomainId,
        virt: VirtAddr,
        phys: platform_api::PhysAddr,
        len: usize,
        prot: MapProt,
    ) -> Result<(), PlatformError> {
        // The kernel and its native tasks run through the kseg0 direct map.
        // A user protection domain still needs the TLB owner; refuse it here
        // rather than silently installing a mapping the old loader will reuse.
        if domain != DomainId(0)
            || prot.0 & MapProt::USER.0 != 0
            || len == 0
            || virt % 4096 != 0
            || phys % 4096 != 0
            || len % 4096 != 0
            || virt as u64 != KSEG0_BASE.wrapping_add(phys)
        {
            return Err(PlatformError::NotSupported);
        }
        Ok(())
    }

    fn unmap(&mut self, domain: DomainId, virt: VirtAddr, len: usize) -> Result<(), PlatformError> {
        if domain == DomainId(0) && virt % 4096 == 0 && len % 4096 == 0 {
            Ok(())
        } else {
            Err(PlatformError::NotSupported)
        }
    }

    fn protect(
        &mut self,
        domain: DomainId,
        virt: VirtAddr,
        len: usize,
        _prot: MapProt,
    ) -> Result<(), PlatformError> {
        self.unmap(domain, virt, len)
    }

    fn activate(&mut self, domain: DomainId) -> Result<(), PlatformError> {
        if domain == DomainId(0) {
            Ok(())
        } else {
            Err(PlatformError::NotSupported)
        }
    }

    fn irq_register(&mut self, _irq: IrqId, _event: EventId) -> Result<IrqHandle, PlatformError> {
        // The current fast-path timer and polled drivers have no general IRQ
        // dispatcher yet.  Do not hand the core a handle that will never fire.
        Err(PlatformError::NotSupported)
    }

    fn irq_ack(&mut self, _handle: IrqHandle) {}

    fn irq_mask(&mut self, _handle: IrqHandle) {}

    fn irq_unmask(&mut self, _handle: IrqHandle) {}

    fn timer_arm(
        &mut self,
        _deadline: Nanos,
        _event: EventId,
    ) -> Result<TimerHandle, PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn timer_cancel(&mut self, _handle: TimerHandle) {}

    fn nic_info(&self) -> NicInfo {
        let mac = crate::net::mac_address();
        NicInfo {
            mac,
            mtu: 1514,
            link: crate::net::ip_addr().is_some(),
            rx_queues: 1,
            tx_queues: 1,
            speed_mbps: 10,
            features: NicFeatures(0),
        }
    }

    fn nic_rx_post(&mut self, _queue: u16, _buffers: &[DmaBuffer]) -> Result<(), PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn nic_tx(
        &mut self,
        _queue: u16,
        _buffer: DmaBuffer,
        _event: EventId,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn nic_poll(&mut self, _budget: usize, _out: &mut CompletionSink<'_>) -> usize {
        0
    }

    fn block_info(&self) -> BlockInfo {
        BlockInfo {
            sector_size: 512,
            sectors: 0,
            read_only: true,
            queue_depth: 1,
        }
    }

    fn block_submit(
        &mut self,
        _op: BlockOp,
        _lba: u64,
        _sectors: u32,
        _buffer: DmaBuffer,
        _event: EventId,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn block_flush(&mut self, _event: EventId) -> Result<(), PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn dma_map(
        &mut self,
        domain: DomainId,
        buffer: &DmaBuffer,
        _dir: DmaDir,
    ) -> Result<DmaAddr, PlatformError> {
        if domain == DomainId(0) {
            Ok(buffer.phys)
        } else {
            Err(PlatformError::NotSupported)
        }
    }

    fn dma_unmap(
        &mut self,
        domain: DomainId,
        _buffer: &DmaBuffer,
        _dir: DmaDir,
    ) -> Result<(), PlatformError> {
        if domain == DomainId(0) {
            Ok(())
        } else {
            Err(PlatformError::NotSupported)
        }
    }

    fn dma_sync(&mut self, buffer: &DmaBuffer, dir: DmaDir) {
        match dir {
            DmaDir::FromDevice => {
                crate::cache::dcache_invalidate(buffer.virt, buffer.len);
            }
            DmaDir::ToDevice | DmaDir::Bidirectional => {
                crate::cache::dcache_writeback_invalidate(buffer.virt, buffer.len);
            }
        }
        crate::cache::sync();
    }

    fn shmem_create(
        &mut self,
        _len: usize,
        _align: usize,
    ) -> Result<platform_api::Shmem, PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn shmem_attach(
        &mut self,
        _id: platform_api::ShmemId,
    ) -> Result<platform_api::Shmem, PlatformError> {
        Err(PlatformError::NotSupported)
    }

    fn shmem_notify(&mut self, _id: platform_api::ShmemId, _doorbell: u32) {}

    fn console_write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            crate::write_byte(byte);
        }
    }

    fn console_read(&mut self, out: &mut [u8]) -> usize {
        let mut written = 0;
        while written < out.len() {
            let Some(key) = crate::kbd::poll_key() else {
                break;
            };
            let byte = match key {
                crate::kbd::Key::Char(byte) => byte,
                crate::kbd::Key::Enter => b'\n',
                crate::kbd::Key::Escape => 0x1b,
                crate::kbd::Key::Backspace => 0x08,
                _ => continue,
            };
            out[written] = byte;
            written += 1;
        }
        written
    }

    fn random(&mut self, out: &mut [u8]) -> Result<(), PlatformError> {
        entropy_bytes(out);
        Ok(())
    }

    fn wait(&mut self) {
        // SAFETY: `wait` parks until an interrupt; the ROM has enabled them.
        unsafe {
            core::arch::asm!("wait", options(nomem, nostack));
        }
    }

    fn reset(&mut self) -> ! {
        crate::reboot()
    }
}

/// Install the Indy shim and run its boot self-test.
///
/// This does not take over any driver.  It checks the resources the core will
/// depend on and leaves the existing native path in place, so a failure is
/// visible at boot instead of surfacing later as a core fault.
pub fn init() {
    let mut shim = IndyShim::new();
    if let Err(error) = shim.init() {
        crate::print_str("platform: indy shim failed: ");
        crate::println(match error {
            PlatformError::NoMemory => "no memory",
            _ => "initialisation error",
        });
        return;
    }

    let caps = shim.caps();
    let mut ok = caps.0 & ShimCaps::MMU.0 != 0
        && caps.0 & ShimCaps::BIG_ENDIAN.0 != 0
        && caps.0 & ShimCaps::RTC.0 != 0;

    let mut entropy = [0u8; 16];
    if shim.random(&mut entropy).is_err() || entropy.iter().all(|&byte| byte == 0) {
        ok = false;
    }

    let page = shim.alloc_pages(0, AllocFlags(AllocFlags::ZERO.0 | AllocFlags::DMA.0));
    if let Ok(buffer) = page {
        let dma = shim.dma_map(DomainId(0), &buffer, DmaDir::ToDevice);
        if dma != Ok(buffer.phys) {
            ok = false;
        }
        shim.dma_sync(&buffer, DmaDir::ToDevice);
        if shim
            .dma_unmap(DomainId(0), &buffer, DmaDir::ToDevice)
            .is_err()
        {
            ok = false;
        }
        let first = unsafe { read_volatile(buffer.virt as *const u8) };
        if first != 0 {
            ok = false;
        }
        shim.free_pages(buffer);
    } else {
        ok = false;
    }

    // Force at least one monotonic read; a wrap on the very first call would
    // mean the seed was taken after the counter had already wrapped.
    let _ = shim.now();

    crate::print_str("platform: indy shim ");
    crate::println(if ok {
        "ok (pages, kseg0, dma, rng)"
    } else {
        "FAILED"
    });
}
