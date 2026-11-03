// SPDX-License-Identifier: GPL-3.0-only
//! Seeq 80C03 Ethernet controller behind the SGI HPC3 DMA engines (Indy).
//!
//! The register and descriptor layer, the smoltcp `Device` and its tokens.  The
//! kernel and the ROM share it.  The caller supplies the station address and the
//! DMA-visible placement; nothing here reads NVRAM or the kernel's services.

#![no_std]

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use smoltcp::wire::EthernetAddress;

/// Seeq register window, kseg1 virtual base.
pub const SEEQ_BASE: *mut u8 = 0xBFBD_4000 as *mut u8;

/// The Seeq device (DMA descriptors and frame buffers) is placed in the kseg1
/// alias of the reserved window.  kseg0 is cacheable (`Config.K0 = 3`), so
/// keeping the DMA-visible structure uncached avoids cache maintenance on every
/// descriptor and frame.  Physical `0x0860_0000` is immediately after the 2 MiB
/// module area and below the frame allocator's first page.
pub const SEEQ_AREA: usize = 0xA860_0000;

/// HPC3 enet DMA engines (kseg1).
pub const RX_ENGINE: *mut u32 = 0xBFB9_4000 as *mut u32;
pub const TX_ENGINE: *mut u32 = 0xBFB9_6000 as *mut u32;

/// HPC3 enet register offsets (bytes).
pub const ENET_CBP: usize = 0x0000;
pub const ENET_NBDP: usize = 0x0004;
pub const ENET_CTRL: usize = 0x1004;
pub const ENET_CTRL_ACTIVE: u32 = 1 << 9;

/// PDMA descriptor flags, from the Indy PROM `struct xd` / `struct rd`.
pub const DESC_EOX: u32 = 0x8000_0000; // end of ring
pub const DESC_EOP: u32 = 0x4000_0000; // end of packet
pub const DESC_XIE: u32 = 0x2000_0000; // interrupt when this descriptor completes
pub const DESC_ROWN: u32 = 0x0000_4000; // RX: HPC3 owns the buffer
pub const DESC_COUNT_MASK: u32 = 0x3FFF;

/// Original driver's receive buffer size (`MAX_RPKT` in seeq.h).
pub const MAX_RPKT: u32 = 1517;

/// HPC3 interrupt status (`istat0`, physical `0x1FBB_0000`).
///
/// SGI documents a bug: `istat0` reliably reports only bits 4:0, and
/// `istat1` at `+0x3_000C` only bits 9:5.  A driver reads both and merges
/// them; [`net_irq::merge_status`] is that merge.
pub const HPC3_INTSTAT: *mut u32 = 0xBFBB_0000 as *mut u32;
pub const HPC3_INTSTAT1: *mut u32 = 0xBFBB_000C as *mut u32;

/// Ethernet receive channel reset register (HPC3 `+0x1_5014`).  Writing
/// `CLRINT` clears every Ethernet interrupt except a receive-buffer overflow.
pub const ENET_RX_RESET: *mut u32 = 0xBFB9_5014 as *mut u32;
pub const ENET_RX_RESET_CLRINT: u32 = 0x02;

/// Ethernet receive channel control register (HPC3 `+0x1_5004`).
pub const ENET_RX_CTRL: usize = 0x1004;
/// `HPC_RBO`: the receive channel stopped with a buffer overflow.  This is the
/// one Ethernet interrupt that `CLRINT` cannot clear; the channel must be
/// rebuilt.
pub const ENET_CTRL_RBO: u32 = 0x0800;

/// IOC2 local-0 interrupt mask (physical `0x1FBD_9884`), byte-wide.
pub const IOC_L0_MASK: *mut u8 = 0xBFBD_9884 as *mut u8;
/// IOC local 0, bit 3: the Seeq's `ENET_INT` pin.
pub const IOC_L0_ETHERNET: u8 = 0x08;

/// Seeq register numbers.
pub const REG_MAC0: u8 = 0;
pub const REG_RX_CMD: u8 = 6;
pub const REG_TX_CMD: u8 = 7;
pub const RX_CMD_STA_MULTI: u8 = 0xC0;
/// Interrupt on a good end of frame (`SEQ_RC_INTEOF`).
pub const RX_CMD_INT_EOF: u8 = 0x10;

/// Write one byte to a Seeq register (lane 0).
pub fn seeq_wr8(reg: u8, data: u8) {
    unsafe {
        write_volatile(SEEQ_BASE.add((reg as usize) << 2), data);
    }
}

/// Read one byte from a Seeq register in the selected bank.
///
/// Reading the RX (register 6) or TX (register 7) status register clears that
/// register's `NEW` bit and drops the chip's `INT` pin, so the interrupt
/// handler must do it before it writes `CLRINT`.
pub fn seeq_rd8(reg: u8) -> u8 {
    unsafe { read_volatile(SEEQ_BASE.add((reg as usize) << 2)) }
}

/// Write one 32-bit word to an HPC3 engine register.
/// # Safety
/// `base` must point at a live HPC3 engine register block.
pub unsafe fn hpc3_wr32(base: *mut u32, offset: usize, value: u32) {
    write_volatile(base.add(offset >> 2), value);
}

/// Read one 32-bit word from an HPC3 engine register.
/// # Safety
/// `base` must point at a live HPC3 engine register block.
pub unsafe fn hpc3_rd32(base: *mut u32, offset: usize) -> u32 {
    read_volatile(base.add(offset >> 2))
}

/// Convert a kseg0/kseg1 pointer to the physical address DMA engines need.
pub fn phys<T>(p: *const T) -> u32 {
    (p as u32) & 0x1FFF_FFFF
}

/// HPC3 PDMA descriptor (Indy layout).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct DmaDesc {
    addr: u32,
    flags: u32,
    next: u32,
    pad: u32,
}

/// Number of receive descriptors in the ring.  A ring lets the HPC3 move on
/// to the next buffer while the CPU processes the previous frame; a single
/// descriptor forces a stop/re-arm cycle and drops frames on bulk transfers.
///
/// Eight descriptors hold a seven-segment burst (the advertised window below)
/// plus one descriptor owned by the CPU, so a full window can arrive without
/// overwriting the frame being copied.  A 16-descriptor ring was tried and
/// broke the slot-B Wasm autoload on the emulated Seeq; the ring stays at the
/// shipped eight.
pub const RX_RING: usize = 8;

/// The Seeq device as a smoltcp `Device`.
pub struct SeeqDevice {
    mac: EthernetAddress,
    /// Transmit frame buffer (DMA-visible).
    tx_buf: [u8; 1514],
    /// Receive DMA buffers: 2 pad + frame + status each.
    rx_bufs: [[u8; 1520]; RX_RING],
    /// Copy of the last received frame handed to smoltcp.
    rx_frame: [u8; 1514],
    tx_desc: DmaDesc,
    rx_descs: [DmaDesc; RX_RING],
    rx_index: usize,
}

impl SeeqDevice {
    /// Probe the Seeq and start its receive DMA.
    pub fn new(mac_bytes: [u8; 6]) -> Option<Self> {
        // Bank 0 exposes the station-address registers.
        seeq_wr8(REG_TX_CMD, 0x00);
        for (i, b) in mac_bytes.iter().enumerate() {
            seeq_wr8(REG_MAC0.wrapping_add(i as u8), *b);
        }
        // Station + broadcast + multicast, and interrupt on a good end of
        // frame.  The interrupt line is masked at the IOC until the handler
        // has passed its boot self-test (see `init`).
        seeq_wr8(REG_RX_CMD, RX_CMD_STA_MULTI | RX_CMD_INT_EOF);

        let dev = Self {
            mac: EthernetAddress::from_bytes(&mac_bytes),
            tx_buf: [0; 1514],
            rx_bufs: [[0; 1520]; RX_RING],
            rx_frame: [0; 1514],
            tx_desc: DmaDesc {
                addr: 0,
                flags: 0,
                next: 0,
                pad: 0,
            },
            rx_descs: [DmaDesc {
                addr: 0,
                flags: 0,
                next: 0,
                pad: 0,
            }; RX_RING],
            rx_index: 0,
        };
        Some(dev)
    }

    /// Activate the receive engine.  Must be called after the device has come
    /// to rest at its final address: the descriptor next pointers are physical
    /// self-references and would be stale if taken in `new()`.
    pub fn start(&mut self) {
        self.start_rx();
    }

    /// MAC address.
    pub fn mac_address(&self) -> EthernetAddress {
        self.mac
    }

    /// Raw pointer to receive descriptor `index`, taken from the ring base so
    /// the volatile writes go through a bounds-checked element pointer rather
    /// than a reference to one field.
    fn rx_desc_ptr(&mut self, index: usize) -> *mut DmaDesc {
        assert!(index < RX_RING);
        // SAFETY: `index` is below RX_RING, the length of `rx_descs`.
        unsafe { self.rx_descs.as_mut_ptr().add(index) }
    }

    /// Build the descriptor ring and activate the RX engine.
    fn start_rx(&mut self) {
        let mut buf_phys = [0u32; RX_RING];
        for i in 0..RX_RING {
            buf_phys[i] = phys(self.rx_bufs[i].as_ptr());
        }
        let desc_base = self.rx_descs.as_ptr();
        for (i, &buf) in buf_phys.iter().enumerate() {
            let next = phys(unsafe { desc_base.add((i + 1) % RX_RING) });
            let desc = self.rx_desc_ptr(i);
            unsafe {
                write_volatile(&raw mut (*desc).addr, buf);
                // XIE is the HPC3-side receive interrupt: the engine raises
                // its DMA completion when this descriptor fills.  On the Indy
                // both that bit and the Seeq's own INT pin land on the same
                // HPC3 ENET interrupt, so either one is enough to wake.
                write_volatile(&raw mut (*desc).flags, DESC_XIE | DESC_ROWN | MAX_RPKT);
                write_volatile(&raw mut (*desc).next, next);
            }
        }

        let first = phys(&self.rx_descs[0]);
        unsafe {
            hpc3_wr32(RX_ENGINE, ENET_CBP, first);
        }
        unsafe {
            hpc3_wr32(RX_ENGINE, ENET_NBDP, first);
        }
        // Drop the channel first: CTRL writes only change ACTIVE on a 0->1
        // transition in the HPC3 model.
        unsafe {
            hpc3_wr32(RX_ENGINE, ENET_CTRL, 0);
        }
        unsafe {
            hpc3_wr32(RX_ENGINE, ENET_CTRL, ENET_CTRL_ACTIVE);
        }
    }

    /// Re-arm one receive descriptor after its frame has been consumed.  The
    /// channel is already working on the next descriptor, so it must not be
    /// stopped here.
    fn rearm_rx(&mut self, index: usize) {
        let buf_phys = phys(self.rx_bufs[index].as_ptr());
        let desc = self.rx_desc_ptr(index);
        unsafe {
            write_volatile(&raw mut (*desc).addr, buf_phys);
            write_volatile(&raw mut (*desc).flags, DESC_XIE | DESC_ROWN | MAX_RPKT);
        }
    }

    /// Rebuild the ring and restart the receive engine.
    ///
    /// This is the recovery for a source `CLRINT` cannot clear: a receive
    /// buffer overflow leaves the HPC3 re-asserting the Seeq line until the
    /// channel is reset.  Clearing every descriptor also drops any frame the
    /// ring still held, which is the same loss the overflow already caused.
    pub fn restart_rx(&mut self) {
        self.rx_index = 0;
        self.start_rx();
    }
}

impl Device for SeeqDevice {
    type RxToken<'b> = SeeqRxToken<'b>;
    type TxToken<'b> = SeeqTxToken<'b>;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        // smoltcp clamps the advertised TCP window to
        // `max_burst_size * (MTU - IP - TCP)`.  One segment (the M2 value)
        // limits every socket to ~1.4 KiB in flight.  Seven segments is
        // 10,220 bytes, enough for one 8 KiB bulk chunk plus framing; the
        // eight-descriptor ring keeps one descriptor for the CPU, so 7 is the
        // largest safe burst.  This assignment is load-bearing: without it the
        // 16 KiB socket window lets a peer burst past the ring and frames are
        // lost under concurrent connections.
        caps.max_burst_size = Some(7);
        caps.medium = Medium::Ethernet;
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Each descriptor owns one buffer.  ROWN is set while the HPC3 owns
        // it; it clears when a frame has been written.  The descriptor lives
        // in RAM shared with the DMA engine, so use volatile semantics.
        let index = self.rx_index;
        let flags = unsafe { read_volatile(&self.rx_descs[index].flags) };
        if flags & DESC_ROWN != 0 {
            return None;
        }

        let remaining = flags & DESC_COUNT_MASK;
        let total = MAX_RPKT.saturating_sub(remaining);
        if total < 3 {
            self.rearm_rx(index);
            self.rx_index = (index + 1) % RX_RING;
            return None;
        }
        let len = (total - 3) as usize;
        if !(14..=1514).contains(&len) {
            self.rearm_rx(index);
            self.rx_index = (index + 1) % RX_RING;
            return None;
        }

        let buf = self.rx_bufs[index].as_ptr();
        let status = unsafe { read_volatile(buf.add(2 + len)) };
        // The DMA buffer is uncached kseg1 and the descriptor flag (read
        // volatile above) is the ownership handshake.  Copy the frame in one
        // operation: a per-byte `read_volatile` loop issued one uncached load
        // per byte and was the largest receive-side CPU cost on bulk traffic.
        compiler_fence(Ordering::Acquire);
        unsafe {
            core::ptr::copy_nonoverlapping(buf.add(2), self.rx_frame.as_mut_ptr(), len);
        }
        self.rearm_rx(index);
        self.rx_index = (index + 1) % RX_RING;

        if status & 0x20 == 0 {
            // Not a good frame (CRC/short/overflow); it has been discarded.
            return None;
        }

        let (rx, tx, desc) = (
            &mut self.rx_frame[..len],
            &mut self.tx_buf,
            &mut self.tx_desc,
        );
        Some((SeeqRxToken(rx), SeeqTxToken { buf: tx, desc }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(SeeqTxToken {
            buf: &mut self.tx_buf,
            desc: &mut self.tx_desc,
        })
    }
}

/// Receive token: hand the received frame to `f`.
pub struct SeeqRxToken<'a>(&'a mut [u8]);

impl<'a> smoltcp::phy::RxToken for SeeqRxToken<'a> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.0)
    }
}

/// Transmit token: on consume, fill the buffer and run one descriptor through
/// the HPC3 TX engine.
pub struct SeeqTxToken<'a> {
    buf: &'a mut [u8; 1514],
    desc: &'a mut DmaDesc,
}

impl<'a> smoltcp::phy::TxToken for SeeqTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let result = f(&mut self.buf[..len]);

        // Ethernet minimum payload frame is 60 bytes without FCS.
        let count = len.max(60);
        for b in &mut self.buf[len..count] {
            *b = 0;
        }

        let desc_phys = phys(self.desc as *const DmaDesc);
        let buf_phys = phys(self.buf.as_ptr());
        // Make the frame bytes visible to the DMA engine before it is kicked.
        compiler_fence(Ordering::SeqCst);
        unsafe {
            write_volatile(&mut self.desc.addr, buf_phys);
            write_volatile(&mut self.desc.flags, DESC_EOX | DESC_EOP | count as u32);
            write_volatile(&mut self.desc.next, desc_phys);
        }

        unsafe {
            hpc3_wr32(TX_ENGINE, ENET_CBP, desc_phys);
        }
        unsafe {
            hpc3_wr32(TX_ENGINE, ENET_NBDP, desc_phys);
        }
        unsafe {
            hpc3_wr32(TX_ENGINE, ENET_CTRL, 0);
        }
        unsafe {
            hpc3_wr32(TX_ENGINE, ENET_CTRL, ENET_CTRL_ACTIVE);
        }

        // Poll until the HPC3 has consumed the descriptor and cleared ACTIVE.
        for _ in 0..10_000_000 {
            if unsafe { hpc3_rd32(TX_ENGINE, ENET_CTRL) } & ENET_CTRL_ACTIVE == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        result
    }
}
