// SPDX-License-Identifier: GPL-3.0-only
//! WD33C93 SCSI controller bring-up.
//!
//! The controller is in the HPC3 external window at kseg1 `0xBFBC_4000`; the
//! indirect address register is byte lane 3 (`+3`) and the data register is
//! byte lane 3 of the next word (`+7`), matching Linux `sgiwd93.c`
//! (`regs.SASR = wdregs + 3`, `regs.SCMD = wdregs + 7`).
//!
//! The address register auto-increments after most data-port accesses, so
//! every data byte must reselect the DATA register (0x19).  Writing a byte
//! with the address register left at COMMAND (0x18) is interpreted as a
//! command; the identify byte 0x80 would reset the chip.  The console command
//! resets the chip, selects target 1, issues a polled INQUIRY and reads block 0
//! with READ(6).
//!
//! READ(6) data phases use the HPC3 SCSI DMA engine with a bounce buffer from
//! the frame allocator and a descriptor chain built by the `hpc3` crate
//! (`crates/hpc3`).  The polled PIO data path remains as the fallback: it is
//! used when no DMA buffer can be allocated, when the transfer is larger than
//! the bounded chain, or after a DMA transfer fails.  Setup and error paths
//! always stop the channel and clear `ctrl`; a channel that will not stop is
//! reset and reported rather than left active.
//!
//! ## Direction bit
//!
//! HPC3 `ctrl` bit 2 is 1 for memory-to-device.  NetBSD's `hpcreg.h` states
//! this directly (`0=dev->mem, 1=mem->dev`) and Linux's `sgiwd93.c` sets the
//! bit only for data-out, even though the comment beside the definition in
//! Linux's `hpc3.h` says the opposite.  A READ therefore leaves bit 2 clear.
//! See `docs/research/indy-hardware.md` §5.3 and §5.4.1.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};

use hpc3::{build_chain, DmaDesc, DESC_SIZE, MAX_DESC_LEN};

/// WD33C93 external register window (kseg1).
const BASE: usize = 0xBFBC_4000;
/// HPC3 SCSI channel 0 next-descriptor pointer (kseg1, HPC3 + 0x1_0004).
const HPC3_SCSI_NDPTR: *mut u32 = 0xBFB9_0004 as *mut u32;
/// HPC3 SCSI channel 0 byte count and flags (kseg1, HPC3 + 0x1_1000).
const HPC3_SCSI_BCD: *mut u32 = 0xBFB9_1000 as *mut u32;
/// HPC3 SCSI channel 0 DMA control (kseg1, HPC3 + 0x1_1004).
const HPC3_SCSI_CTRL: *mut u32 = 0xBFB9_1004 as *mut u32;

/// `bcd`: byte count.
const HPC3_SBCD_COUNT_MASK: u32 = 0x0000_3FFF;
/// `bcd`: interrupt at the end of this descriptor.
const HPC3_SBCD_XIE: u32 = 0x0000_4000;
/// `bcd`: last descriptor in the chain.
const HPC3_SBCD_EOX: u32 = 0x0000_8000;

/// `ctrl`: channel active.
const HPC3_SCTRL_ACTIVE: u32 = 0x10;
/// `ctrl`: flush the SCSI FIFO; used to drain a completed read.
const HPC3_SCTRL_FLUSH: u32 = 0x08;
/// `ctrl`: memory-to-device direction.
const HPC3_SCTRL_DIR: u32 = 0x04;
/// `ctrl`: reset the DMA channel and the external controller.
const HPC3_SCTRL_CRESET: u32 = 0x40;
/// `ctrl`: parity error on the HPC-to-WD interface.
const HPC3_SCTRL_PERR: u32 = 0x80;

const ASR_INT: u8 = 0x80;
const ASR_CIP: u8 = 0x10;
const ASR_DBR: u8 = 0x01;

const REG_OWN_ID: u8 = 0x00;
const REG_CONTROL: u8 = 0x01;
const REG_TIMEO: u8 = 0x02;
const REG_CDB1: u8 = 0x03;
const REG_CMD_PHASE: u8 = 0x10;
const REG_SYN: u8 = 0x11;
const REG_COUNT_HI: u8 = 0x12;
const REG_COUNT_MED: u8 = 0x13;
const REG_COUNT_LO: u8 = 0x14;
const REG_SELID: u8 = 0x15;
const REG_RSELID: u8 = 0x16;
const REG_CSR: u8 = 0x17;
const REG_COMMAND: u8 = 0x18;
const REG_DATA: u8 = 0x19;

const CMD_ABORT: u8 = 0x01;
const CMD_RESET: u8 = 0x00;
const CMD_SEL_ATN: u8 = 0x06;
const CMD_SEL_ATN_XFER: u8 = 0x08;
const CMD_XFER_INFO: u8 = 0x20;

const ID_EAF: u8 = 0x08;
const ID_RAF: u8 = 0x20;
const OWN_ID: u8 = 7;
const CONTROL_EDI_IDI: u8 = 0x0c;
/// `DM2..DM0 = 001`: burst DMA.  The HPC3 services DREQ from the WD33C93.
const CONTROL_BURST_DMA: u8 = 0x20;
const RSELID_ER: u8 = 0x80;
/// `SBIC_TIMEOUT(250, 20)`: 250 ms at a 20 MHz input clock.
const TIMEOUT: u8 = 63;

const CSR_PHASE_MASK: u8 = 0x07;
const PHASE_DATA_OUT: u8 = 0x00;
const PHASE_DATA_IN: u8 = 0x01;
const PHASE_CMD: u8 = 0x02;
const PHASE_MESG_OUT: u8 = 0x06;
const CSR_PHASE_MISMATCH: u8 = 0x88;
const CSR_SEL_TIMEO: u8 = 0x42;
const CSR_S_XFERRED: u8 = 0x16;
const CSR_DISC: u8 = 0x41;
const CSR_DISC_1: u8 = 0x85;
const CMD_PHASE_COMPLETE: u8 = 0x46;

/// SCSI block size.
const SECTOR: usize = 512;
/// READ(6) transfer length is one byte; 255 avoids the 256-block encoding.
const MAX_RD6_BLOCKS: usize = 255;
/// READ(6) logical block address is 21 bits.
const MAX_RD6_LBA: u32 = 0x1F_FFFF;
/// Descriptors in one DMA chain, excluding the EOX terminator.
const MAX_DMA_DESCS: usize = 16;
/// Largest transfer this driver hands to the HPC3 engine.
const MAX_DMA_LEN: usize = MAX_DMA_DESCS * MAX_DESC_LEN;
/// Bounded spin counts for channel state changes and the data phase.
const DMA_STOP_SPINS: u32 = 1_000_000;
const DMA_XFER_SPINS: u32 = 1_000_000;

/// Set after a DMA transfer failure, so later reads go straight to PIO.
///
/// The kernel is single-threaded and SCSI requests are serialised, so a plain
/// static is sufficient.
static mut DMA_DISABLED: bool = false;

/// Result of the controller probe.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    /// Auxiliary status before reset.
    pub asr_before: u8,
    /// Status after the chip reset.
    pub csr: u8,
    /// Microcode revision (CDB1 after reset).
    pub revision: u8,
    /// Own SCSI ID read back.
    pub own_id: u8,
    /// Auxiliary status after initialisation.
    pub asr_after: u8,
}

fn delay() {
    for _ in 0..10_000 {
        core::hint::spin_loop();
    }
}

fn set_ar(register: u8) {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 3) as *mut u8, register);
    }
    crate::cache::sync();
    for _ in 0..8 {
        core::hint::spin_loop();
    }
}

fn read_asr() -> u8 {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 3) as *const u8) }
}

fn write_reg(register: u8, value: u8) {
    set_ar(register);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 7) as *mut u8, value);
    }
}

fn read_reg(register: u8) -> u8 {
    set_ar(register);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 7) as *const u8) }
}

fn write_data(value: u8) {
    // The data register is 0x19, not necessarily the current AR after a
    // status read or a command write.
    set_ar(REG_DATA);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 7) as *mut u8, value);
    }
}

fn read_data() -> u8 {
    set_ar(REG_DATA);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 7) as *const u8) }
}

/// Write the 24-bit transfer count.  Each byte reselects its register instead
/// of relying on the chip's auto-increment.
fn put_count(count: u32) {
    write_reg(REG_COUNT_HI, (count >> 16) as u8);
    write_reg(REG_COUNT_MED, (count >> 8) as u8);
    write_reg(REG_COUNT_LO, count as u8);
}

/// Read the 24-bit residual transfer count.
fn read_count() -> u32 {
    let hi = read_reg(REG_COUNT_HI) as u32;
    let med = read_reg(REG_COUNT_MED) as u32;
    let lo = read_reg(REG_COUNT_LO) as u32;
    (hi << 16) | (med << 8) | lo
}

fn wait_cip_clear() -> Result<(), &'static str> {
    for _ in 0..200_000 {
        if read_asr() & ASR_CIP == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("CIP timeout")
}

fn wait_interrupt() -> Result<(), &'static str> {
    for _ in 0..200_000 {
        if read_asr() & ASR_INT != 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("interrupt timeout")
}

fn read_csr_ack() -> u8 {
    let mut csr = read_reg(REG_CSR);
    // A status read acknowledges the current interrupt.  If another status is
    // queued the chip re-asserts INT immediately; drain it before returning.
    for _ in 0..8 {
        delay();
        if read_asr() & ASR_INT == 0 {
            break;
        }
        csr = read_reg(REG_CSR);
    }
    csr
}

fn xfer(len: usize, buf: &mut [u8], outbound: bool) -> Result<u8, &'static str> {
    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    put_count(len as u32);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_XFER_INFO);
    wait_cip_clear()?;

    let mut done = 0usize;
    let mut spins = 0u32;
    loop {
        let asr = read_asr();
        if asr & ASR_DBR != 0 {
            if done < len {
                if outbound {
                    write_data(buf[done]);
                } else {
                    buf[done] = read_data();
                }
                done += 1;
            } else if !outbound {
                let _ = read_data();
            }
            spins = 0;
            continue;
        }
        if asr & ASR_INT != 0 {
            break;
        }
        spins += 1;
        if spins > 1_000_000 {
            put_count(0);
            return Err("data timeout");
        }
        core::hint::spin_loop();
    }

    let csr = read_csr_ack();
    delay();
    put_count(0);
    if done < len {
        return Err("short data transfer");
    }
    Ok(csr)
}

fn select(target: u8) -> Result<u8, &'static str> {
    put_count(0);
    write_reg(REG_SELID, target);
    write_reg(REG_TIMEO, TIMEOUT);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_SEL_ATN);
    wait_cip_clear()?;

    let mut interrupts = 0u32;
    let mut spins = 0u32;
    loop {
        let asr = read_asr();
        if asr & ASR_INT != 0 {
            let csr = read_csr_ack();
            if csr == CSR_SEL_TIMEO {
                return Err("selection timeout");
            }
            if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT)
                || csr == (CSR_PHASE_MISMATCH | PHASE_CMD)
            {
                return Ok(csr);
            }
            interrupts += 1;
            if interrupts > 8 {
                return Err("unexpected selection status");
            }
            delay();
            continue;
        }
        spins += 1;
        if spins > 2_000_000 {
            return Err("selection interrupt timeout");
        }
        core::hint::spin_loop();
    }
}

fn send_identify() -> Result<(), &'static str> {
    let mut identify = [0x80u8]; // identify, LUN 0, no disconnect
    let csr = xfer(1, &mut identify, true)?;
    if csr & CSR_PHASE_MASK != PHASE_CMD {
        return Err("no command phase after identify");
    }
    Ok(())
}

fn send_cdb() -> Result<(), &'static str> {
    // INQUIRY, allocation length 36.
    let mut cdb = [0x12u8, 0, 0, 0, 36, 0];
    let csr = xfer(cdb.len(), &mut cdb, true)?;
    if csr & CSR_PHASE_MASK != PHASE_DATA_IN {
        return Err("no data-in phase after INQUIRY");
    }
    Ok(())
}

fn finish() -> Result<(), &'static str> {
    put_count(0);
    write_reg(REG_CMD_PHASE, CMD_PHASE_COMPLETE);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_SEL_ATN_XFER);
    wait_cip_clear()?;

    for _ in 0..8 {
        wait_interrupt()?;
        let csr = read_csr_ack();
        delay();
        if csr == CSR_DISC || csr == CSR_DISC_1 || csr == CSR_S_XFERRED {
            return Ok(());
        }
    }
    Err("no disconnect after status")
}

// ---------------------------------------------------------------------------
// HPC3 SCSI descriptor DMA
// ---------------------------------------------------------------------------

/// Why a DMA read did not complete.
#[derive(Clone, Copy, Debug)]
enum DmaError {
    /// No DMA buffer or chain could be prepared; no SCSI traffic was issued.
    Unavailable,
    /// The SCSI command itself failed; the reply is already the best error.
    Scsi(&'static str),
    /// The data phase failed after a DMA start; the channel is stopped.
    Transfer(&'static str),
    /// The DMA channel would not stop; it has been reset and must not be used.
    Fatal(&'static str),
}

/// A bounce buffer and its descriptor frame, freed when the read returns.
struct DmaBuffer {
    data_phys: u64,
    pages: usize,
    desc_phys: u64,
}

impl DmaBuffer {
    fn alloc(len: usize) -> Option<Self> {
        let pages = len.div_ceil(4096);
        let data_phys = crate::frames::alloc_frames(pages)?;
        let desc_phys = match crate::frames::alloc_frame() {
            Some(phys) => phys,
            None => {
                free_run(data_phys, pages);
                return None;
            }
        };
        Some(Self {
            data_phys,
            pages,
            desc_phys,
        })
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        free_run(self.data_phys, self.pages);
        crate::frames::free_frame(self.desc_phys);
    }
}

fn free_run(base: u64, pages: usize) {
    for page in 0..pages {
        crate::frames::free_frame(base + (page as u64) * 4096);
    }
}

/// kseg0 alias of a physical address the frame allocator returned.
fn kseg0(phys: u64) -> usize {
    (phys + crate::KSEG0_OFFSET) as usize
}

fn dma_ctrl() -> u32 {
    // SAFETY: fixed HPC3 MMIO register.
    unsafe { read_volatile(HPC3_SCSI_CTRL) }
}

fn dma_write_ctrl(value: u32) {
    // SAFETY: fixed HPC3 MMIO register.
    unsafe {
        write_volatile(HPC3_SCSI_CTRL, value);
    }
    crate::cache::sync();
}

fn dma_wait_inactive(spins: u32) -> bool {
    for _ in 0..spins {
        if dma_ctrl() & HPC3_SCTRL_ACTIVE == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    dma_ctrl() & HPC3_SCTRL_ACTIVE == 0
}

/// Stop the channel, draining a read FIFO first.
///
/// On the way out `ctrl` is always zero.  If the channel refuses to stop it is
/// reset (which also resets the WD33C93) and the caller must not fall back to
/// PIO without reinitialising the controller.
fn dma_stop() -> Result<(), &'static str> {
    if dma_ctrl() & HPC3_SCTRL_ACTIVE != 0 {
        // Preserve the mode bits while adding FLUSH, as NetBSD's hpcdma_flush
        // does.  FLUSH lets the engine drain its FIFO into memory.
        dma_write_ctrl(dma_ctrl() | HPC3_SCTRL_FLUSH);
        if !dma_wait_inactive(DMA_STOP_SPINS) {
            dma_write_ctrl(HPC3_SCTRL_CRESET);
            delay();
            dma_write_ctrl(0);
            return Err("SCSI DMA channel would not stop");
        }
    }
    dma_write_ctrl(0);
    if dma_ctrl() & HPC3_SCTRL_ACTIVE != 0 {
        dma_write_ctrl(HPC3_SCTRL_CRESET);
        delay();
        dma_write_ctrl(0);
        return Err("SCSI DMA channel restarted after stop");
    }
    Ok(())
}

/// Point the channel at `desc_phys` and start a device-to-memory transfer.
fn dma_arm(desc_phys: u32, first_len: usize, outbound: bool) -> Result<(), &'static str> {
    dma_write_ctrl(0);
    if !dma_wait_inactive(DMA_STOP_SPINS) {
        dma_write_ctrl(HPC3_SCTRL_CRESET);
        delay();
        dma_write_ctrl(0);
        return Err("SCSI DMA channel is stuck active");
    }

    // bcd mirrors the first descriptor: count plus the low-order XIE/EOX
    // bits.  The descriptor itself uses the high-order flag bits.  Linux and
    // NetBSD rely on the descriptor fetch and do not write bcd, but the HPC3
    // specification programs it, so write both.
    let bcd = (first_len as u32 & HPC3_SBCD_COUNT_MASK) | HPC3_SBCD_XIE | HPC3_SBCD_EOX;
    // SAFETY: fixed HPC3 MMIO registers.
    unsafe {
        write_volatile(HPC3_SCSI_NDPTR, desc_phys);
        write_volatile(HPC3_SCSI_BCD, bcd);
    }
    // Descriptor writes were cleaned by the caller; order them (and the two
    // register writes) before ACTIVE becomes visible to the engine.
    crate::cache::sync();
    compiler_fence(Ordering::SeqCst);

    // Big-endian. Bit 2 is 1 for memory-to-device, 0 for device-to-memory.
    let mut ctrl = HPC3_SCTRL_ACTIVE;
    if outbound {
        ctrl |= HPC3_SCTRL_DIR;
    }
    dma_write_ctrl(ctrl);
    Ok(())
}

/// Run the WD33C93 data phase under HPC3 DMA.
///
/// The chip's own DMA-mode bits must select burst DMA; leaving them at the
/// polled setting (as the PIO path does) means the chip never asserts DREQ,
/// so the HPC3 engine has nothing to service.  `run_dma_scsi` restores the
/// polled control value once the channel is stopped.
fn dma_transfer(len: usize) -> Result<u8, &'static str> {
    write_reg(REG_CONTROL, CONTROL_EDI_IDI | CONTROL_BURST_DMA);
    put_count(len as u32);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_XFER_INFO);
    wait_cip_clear()?;

    let mut finished = false;
    for _ in 0..DMA_XFER_SPINS {
        if dma_ctrl() & HPC3_SCTRL_PERR != 0 {
            return Err("SCSI DMA parity error");
        }
        if read_asr() & ASR_INT != 0 {
            finished = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !finished {
        return Err("SCSI DMA data timeout");
    }
    if dma_ctrl() & HPC3_SCTRL_PERR != 0 {
        return Err("SCSI DMA parity error");
    }

    // The chip's residual count is the only evidence the CPU has that the
    // HPC3 moved every byte: the DMA engine reports completion, not the
    // amount.  Reject a short chain rather than returning half-filled data.
    let residual = read_count();
    let csr = read_csr_ack();
    delay();
    put_count(0);
    if residual != 0 {
        return Err("short SCSI DMA transfer");
    }
    Ok(csr)
}

/// Put the controller back to an idle command state after a failed command.
fn abort_command() {
    put_count(0);
    write_reg(REG_COMMAND, CMD_ABORT);
    let _ = wait_cip_clear();
    if read_asr() & ASR_INT != 0 {
        let _ = read_csr_ack();
    }
    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    delay();
}

fn dma_disabled() -> bool {
    // SAFETY: single-threaded kernel; the flag is only written from here.
    unsafe { DMA_DISABLED }
}

fn disable_dma(reason: &str) {
    // SAFETY: single-threaded kernel.
    unsafe {
        DMA_DISABLED = true;
    }
    crate::print_str("scsi: DMA transfer failed (");
    crate::print_str(reason);
    crate::println("); using polled PIO");
}

/// DMA read of a validated READ(6) request.
fn read_dma(target: u8, lba: u32, blocks: usize, buf: &mut [u8]) -> Result<(), DmaError> {
    if dma_disabled() {
        return Err(DmaError::Unavailable);
    }
    let len = blocks * SECTOR;
    let dma = DmaBuffer::alloc(len).ok_or(DmaError::Unavailable)?;
    let data_vaddr = kseg0(dma.data_phys);
    let desc_vaddr = kseg0(dma.desc_phys);

    // The frame is at least 4 KiB, so `MAX_DMA_DESCS + 1` descriptors (144
    // bytes) fit.  This also covers the EOX terminator.
    let descs =
        unsafe { core::slice::from_raw_parts_mut(desc_vaddr as *mut DmaDesc, MAX_DMA_DESCS + 1) };
    let written = build_chain(descs, dma.data_phys as u32, len, dma.desc_phys as u32)
        .ok_or(DmaError::Unavailable)?;

    // The CPU wrote the descriptors and may hold dirty lines over the bounce
    // buffer from an earlier user of the frame.  Clean and invalidate both
    // before the device reads or writes them.
    crate::cache::dcache_writeback_invalidate(desc_vaddr, written * DESC_SIZE);
    crate::cache::dcache_writeback_invalidate(data_vaddr, len);
    crate::cache::sync();
    compiler_fence(Ordering::SeqCst);

    run_dma_scsi(target, lba, blocks, &dma, len)?;

    // The device wrote the bounce buffer.  Drop any cached copy before the
    // CPU reads it, then copy into the caller's buffer.
    crate::cache::dcache_writeback_invalidate(data_vaddr, len);
    crate::cache::sync();
    // SAFETY: the frame covers `len` bytes and is mapped in kseg0.  The
    // channel is stopped at this point, so the device is no longer writing.
    let source = unsafe { core::slice::from_raw_parts(data_vaddr as *const u8, len) };
    buf.copy_from_slice(source);
    Ok(())
}

/// DMA write of a validated WRITE(6) request.
fn write_dma(target: u8, lba: u32, blocks: usize, buf: &[u8]) -> Result<(), DmaError> {
    if dma_disabled() {
        return Err(DmaError::Unavailable);
    }
    let len = blocks * SECTOR;
    let dma = DmaBuffer::alloc(len).ok_or(DmaError::Unavailable)?;
    let data_vaddr = kseg0(dma.data_phys);
    let desc_vaddr = kseg0(dma.desc_phys);

    // Copy the caller's bytes into the DMA bounce buffer and clean both the
    // data and descriptors before the device reads them.
    let data = unsafe { core::slice::from_raw_parts_mut(data_vaddr as *mut u8, len) };
    data.copy_from_slice(buf);
    let descs =
        unsafe { core::slice::from_raw_parts_mut(desc_vaddr as *mut DmaDesc, MAX_DMA_DESCS + 1) };
    let written = build_chain(descs, dma.data_phys as u32, len, dma.desc_phys as u32)
        .ok_or(DmaError::Unavailable)?;
    crate::cache::dcache_writeback_invalidate(data_vaddr, len);
    crate::cache::dcache_writeback_invalidate(desc_vaddr, written * DESC_SIZE);
    crate::cache::sync();
    compiler_fence(Ordering::SeqCst);

    run_dma_scsi_write(target, lba, blocks, &dma, len)?;
    crate::cache::dcache_writeback_invalidate(data_vaddr, len);
    crate::cache::sync();
    Ok(())
}

fn run_dma_scsi_write(
    target: u8,
    lba: u32,
    blocks: usize,
    dma: &DmaBuffer,
    len: usize,
) -> Result<(), DmaError> {
    let csr = select(target).map_err(DmaError::Scsi)?;
    if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT) {
        if let Err(error) = send_identify() {
            abort_command();
            return Err(DmaError::Scsi(error));
        }
    }

    let mut cdb = [
        0x0a, // WRITE(6)
        ((lba >> 16) & 0x1f) as u8,
        (lba >> 8) as u8,
        lba as u8,
        blocks as u8,
        0,
    ];
    let csr = match xfer(cdb.len(), &mut cdb, true) {
        Ok(csr) => csr,
        Err(error) => {
            abort_command();
            return Err(DmaError::Scsi(error));
        }
    };
    if csr & CSR_PHASE_MASK != PHASE_DATA_OUT {
        abort_command();
        return Err(DmaError::Scsi("no data-out phase after WRITE(6)"));
    }

    if let Err(error) = dma_arm(dma.desc_phys as u32, len.min(MAX_DESC_LEN), true) {
        return Err(DmaError::Fatal(error));
    }

    let outcome = dma_transfer(len);
    let stopped = dma_stop();
    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    crate::cache::dcache_writeback_invalidate(kseg0(dma.data_phys), len);
    crate::cache::sync();
    stopped.map_err(DmaError::Fatal)?;

    let csr = outcome.map_err(DmaError::Transfer)?;
    if csr == CSR_S_XFERRED || csr == CSR_DISC || csr == CSR_DISC_1 {
        finish().map_err(DmaError::Scsi)?;
        return Ok(());
    }
    finish().map_err(DmaError::Scsi)?;
    Ok(())
}

fn run_dma_scsi(
    target: u8,
    lba: u32,
    blocks: usize,
    dma: &DmaBuffer,
    len: usize,
) -> Result<(), DmaError> {
    let csr = select(target).map_err(DmaError::Scsi)?;
    if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT) {
        if let Err(error) = send_identify() {
            abort_command();
            return Err(DmaError::Scsi(error));
        }
    }

    let mut cdb = [
        0x08, // READ(6)
        ((lba >> 16) & 0x1f) as u8,
        (lba >> 8) as u8,
        lba as u8,
        blocks as u8,
        0,
    ];
    let csr = match xfer(cdb.len(), &mut cdb, true) {
        Ok(csr) => csr,
        Err(error) => {
            abort_command();
            return Err(DmaError::Scsi(error));
        }
    };
    if csr & CSR_PHASE_MASK != PHASE_DATA_IN {
        // The target is in some other phase and still selected; release it.
        abort_command();
        return Err(DmaError::Scsi("no data-in phase after READ(6)"));
    }

    if let Err(error) = dma_arm(dma.desc_phys as u32, len.min(MAX_DESC_LEN), false) {
        // dma_arm resets the channel and the external controller before
        // returning, so PIO cannot continue this command.
        return Err(DmaError::Fatal(error));
    }

    // From here on the channel must be stopped on every path.
    let outcome = dma_transfer(len);
    let stopped = dma_stop();
    // Back to polled control for the status/message finish or a PIO retry.
    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    // Invalidate even on failure: a partial DMA write may have touched the
    // bounce buffer, and the retry will reuse the frame.
    crate::cache::dcache_writeback_invalidate(kseg0(dma.data_phys), len);
    crate::cache::sync();
    stopped.map_err(DmaError::Fatal)?;

    let csr = outcome.map_err(DmaError::Transfer)?;
    if csr == CSR_S_XFERRED || csr == CSR_DISC || csr == CSR_DISC_1 {
        return Ok(());
    }
    let _ = finish();
    Ok(())
}

/// Polled PIO READ(6), used when DMA is unavailable or has failed.
fn read_pio(target: u8, lba: u32, blocks: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    let csr = select(target)?;
    if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT) {
        send_identify()?;
    }

    let mut cdb = [
        0x08, // READ(6)
        ((lba >> 16) & 0x1f) as u8,
        (lba >> 8) as u8,
        lba as u8,
        blocks as u8,
        0,
    ];
    let csr = xfer(cdb.len(), &mut cdb, true)?;
    if csr & CSR_PHASE_MASK != PHASE_DATA_IN {
        return Err("no data-in phase after READ(6)");
    }

    let csr = xfer(buf.len(), buf, false)?;
    if csr == CSR_S_XFERRED || csr == CSR_DISC || csr == CSR_DISC_1 {
        return Ok(());
    }
    let _ = finish();
    Ok(())
}

/// Write `buf.len()` bytes to `lba` with WRITE(6) over HPC3 DMA.
///
/// The length must be a non-zero multiple of 512, at most 255 blocks and at
/// most [`MAX_DMA_LEN`].  The caller is responsible for choosing a writable
/// target; this is used by the optional cache module and never by the boot
/// path.
pub fn write_blocks(target: u8, lba: u32, buf: &[u8]) -> Result<(), &'static str> {
    if buf.is_empty() || buf.len() % SECTOR != 0 {
        return Err("write length is not a whole number of sectors");
    }
    let blocks = buf.len() / SECTOR;
    if blocks > MAX_RD6_BLOCKS {
        return Err("write is larger than WRITE(6) allows");
    }
    if blocks > 0 && lba.saturating_add(blocks as u32 - 1) > MAX_RD6_LBA {
        return Err("LBA is out of range for WRITE(6)");
    }

    if buf.len() > MAX_DMA_LEN {
        return Err("write is larger than the DMA window");
    }
    match write_dma(target, lba, blocks, buf) {
        Ok(()) => Ok(()),
        Err(DmaError::Unavailable) => Err("SCSI DMA is unavailable"),
        Err(DmaError::Scsi(error) | DmaError::Fatal(error) | DmaError::Transfer(error)) => {
            Err(error)
        }
    }
}

/// Write one 512-byte sector to `target` with WRITE(6).
pub fn write_sector(target: u8, lba: u32, buf: &[u8; 512]) -> Result<(), &'static str> {
    write_blocks(target, lba, buf)
}

/// Reset the HPC3 SCSI channel and the WD33C93, then read back its identity.
pub fn probe() -> Result<Probe, &'static str> {
    let asr_before = read_asr();

    // Reset the DMA channel and the external controller together.
    // SAFETY: fixed HPC3 MMIO register.
    unsafe {
        write_volatile(HPC3_SCSI_CTRL, HPC3_SCTRL_CRESET);
    }
    delay();
    // SAFETY: fixed HPC3 MMIO register.
    unsafe {
        write_volatile(HPC3_SCSI_CTRL, 0);
    }
    delay();

    write_reg(REG_COMMAND, CMD_ABORT);
    wait_cip_clear()?;

    // Own ID 7; advanced and really-advanced features (as NetBSD does).
    write_reg(REG_OWN_ID, OWN_ID | ID_EAF | ID_RAF);
    write_reg(REG_COMMAND, CMD_RESET);
    delay();
    wait_interrupt()?;
    let csr = read_reg(REG_CSR);
    let revision = read_reg(REG_CDB1);

    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    write_reg(REG_RSELID, RSELID_ER);
    write_reg(REG_SYN, 0);
    write_reg(REG_TIMEO, TIMEOUT);

    let own_id = read_reg(REG_OWN_ID);
    let asr_after = read_asr();
    Ok(Probe {
        asr_before,
        csr,
        revision,
        own_id,
        asr_after,
    })
}

/// Issue a polled INQUIRY to `target` and return the 36-byte response.
pub fn inquiry(target: u8) -> Result<[u8; 36], &'static str> {
    let csr = select(target)?;
    if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT) {
        send_identify()?;
    }
    send_cdb()?;

    let mut data = [0u8; 36];
    let csr = xfer(data.len(), &mut data, false)?;
    if csr == CSR_S_XFERRED || csr == CSR_DISC || csr == CSR_DISC_1 {
        return Ok(data);
    }
    let _ = finish();
    Ok(data)
}

/// Read `buf.len()` bytes from `lba` with READ(6).
///
/// The length must be a non-zero multiple of 512 and at most 255 blocks.
/// Transfers up to [`MAX_DMA_LEN`] use the HPC3 descriptor engine through a
/// bounce buffer; everything else, and every DMA failure, uses the polled PIO
/// data path.
pub fn read_blocks(target: u8, lba: u32, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.is_empty() || buf.len() % SECTOR != 0 {
        return Err("read length is not a whole number of sectors");
    }
    let blocks = buf.len() / SECTOR;
    if blocks > MAX_RD6_BLOCKS {
        return Err("read is larger than READ(6) allows");
    }
    if blocks > 0 && lba.saturating_add(blocks as u32 - 1) > MAX_RD6_LBA {
        return Err("LBA is out of range for READ(6)");
    }

    if buf.len() <= MAX_DMA_LEN && !dma_disabled() {
        match read_dma(target, lba, blocks, buf) {
            Ok(()) => return Ok(()),
            Err(DmaError::Scsi(error)) | Err(DmaError::Fatal(error)) => return Err(error),
            Err(DmaError::Unavailable) => {
                // No DMA buffer: nothing has touched the SCSI bus yet.
            }
            Err(DmaError::Transfer(error)) => {
                // The data phase failed and the channel is stopped.  Abort the
                // command before reselecting for the PIO retry.
                disable_dma(error);
                abort_command();
            }
        }
    }

    read_pio(target, lba, blocks, buf)
}

/// Read one 512-byte sector from `target` with READ(6).
pub fn read_sector(target: u8, lba: u32, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    read_blocks(target, lba, buf)
}
