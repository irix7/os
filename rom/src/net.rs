//! ROM network stack: Seeq 8003 + HPC3 PDMA as a smoltcp `Device`, plus a
//! DHCPv4 client.
//!
//! Register and descriptor layout is taken from the original IP22 ARCS PROM
//! driver (`stand/arcs/lib/libsk/net/if_ec2.c` and `include/net/seeq.h`):
//! - Seeq 8003 at kseg1 0xBFBD_4000, 8 registers at byte lane 0 (`reg << 2`).
//!   Regs 0-5 are the station address in bank 0, reg 6 is RX command/status,
//!   reg 7 is TX command/status.
//! - HPC3 enet RX engine at 0xBFB9_4000, TX engine at 0xBFB9_6000.
//!   CBP=0x000, NBDP=0x004, CTRL=0x1004; ACTIVE=bit 9.
//! - PDMA descriptor (IP22 layout): word0 = buffer physical address,
//!   word1 = EOR(31) | EORP(30) | XIE(29) | ... | ROWN(14) | byte count(13:0),
//!   word2 = next descriptor physical address.
//! - RX buffer layout: 2 pad bytes, Ethernet frame, 1 SEEQ status byte.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::dhcpv4;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, Ipv4Address, Ipv4Cidr};

/// Seeq register window, kseg1 virtual base.
const SEEQ_BASE: *mut u8 = 0xBFBD_4000 as *mut u8;

/// HPC3 enet DMA engines (kseg1).
const RX_ENGINE: *mut u32 = 0xBFB9_4000 as *mut u32;
const TX_ENGINE: *mut u32 = 0xBFB9_6000 as *mut u32;

/// HPC3 enet register offsets (bytes).
const ENET_CBP: usize = 0x0000;
const ENET_NBDP: usize = 0x0004;
const ENET_CTRL: usize = 0x1004;
const ENET_CTRL_ACTIVE: u32 = 1 << 9;

/// PDMA descriptor flags, from IP22 `struct xd` / `struct rd`.
const DESC_EOX: u32 = 0x8000_0000; // end of ring
const DESC_EOP: u32 = 0x4000_0000; // end of packet
const DESC_ROWN: u32 = 0x0000_4000; // RX: HPC owns the buffer
const DESC_COUNT_MASK: u32 = 0x3FFF;

/// Original driver's receive buffer size (`MAX_RPKT` in seeq.h).
const MAX_RPKT: u32 = 1517;

/// Seeq register numbers.
const REG_MAC0: u8 = 0;
const REG_RX_CMD: u8 = 6;
const REG_TX_CMD: u8 = 7;
const RX_CMD_STA_MULTI: u8 = 0xC0;

/// MAC is stored by the PROM in the Dallas NVRAM at physical
/// 0x1FBE04E8..0x1FBE04FC, one byte every four bytes (SGI's documented
/// `fill -w -v` recovery layout).  The HPC3 sparse-packs one NVRAM byte per
/// 32-bit word in the low byte lane, so read whole words and take bits 7:0.
const NVRAM_MAC: *const u32 = 0xBFBE_04E8 as *const u32;

/// Write one byte to a Seeq register (lane 0).
fn seeq_wr8(reg: u8, data: u8) {
    unsafe {
        write_volatile(SEEQ_BASE.add((reg as usize) << 2), data);
    }
}

/// Write one 32-bit word to an HPC3 engine register.
fn hpc3_wr32(base: *mut u32, offset: usize, value: u32) {
    unsafe {
        write_volatile(base.add(offset >> 2), value);
    }
}

/// Read one 32-bit word from an HPC3 engine register.
fn hpc3_rd32(base: *mut u32, offset: usize) -> u32 {
    unsafe { read_volatile(base.add(offset >> 2)) }
}

/// Convert a kseg1 pointer to the physical address the DMA engines need.
fn phys<T>(p: *const T) -> u32 {
    (p as u32).wrapping_sub(0xA000_0000)
}

/// HPC3 PDMA descriptor (IP22 layout).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct DmaDesc {
    addr: u32,
    flags: u32,
    next: u32,
    pad: u32,
}

/// Read the station address left by the firmware/NVRAM.
fn read_nvram_mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        let word = unsafe { read_volatile(NVRAM_MAC.add(i)) };
        *b = (word & 0xFF) as u8;
    }
    mac
}

/// The Seeq device as a smoltcp `Device`.
pub struct SeeqDevice {
    mac: EthernetAddress,
    /// Transmit frame buffer (DMA-visible).
    tx_buf: [u8; 1514],
    /// Receive DMA buffer: 2 pad + frame + status.
    rx_buf: [u8; 1520],
    /// Copy of the last received frame handed to smoltcp.
    rx_frame: [u8; 1514],
    tx_desc: DmaDesc,
    rx_desc: DmaDesc,
}

impl SeeqDevice {
    /// Probe the Seeq and start its receive DMA.
    pub fn new() -> Option<Self> {
        let mac_bytes = read_nvram_mac();
        if mac_bytes == [0xFF; 6] {
            return None;
        }

        // Bank 0 exposes the station-address registers.
        seeq_wr8(REG_TX_CMD, 0x00);
        for (i, b) in mac_bytes.iter().enumerate() {
            seeq_wr8(REG_MAC0.wrapping_add(i as u8), *b);
        }
        // Station + broadcast + multicast, interrupts left to polling.
        seeq_wr8(REG_RX_CMD, RX_CMD_STA_MULTI);

        let dev = Self {
            mac: EthernetAddress::from_bytes(&mac_bytes),
            tx_buf: [0; 1514],
            rx_buf: [0; 1520],
            rx_frame: [0; 1514],
            tx_desc: DmaDesc {
                addr: 0,
                flags: 0,
                next: 0,
                pad: 0,
            },
            rx_desc: DmaDesc {
                addr: 0,
                flags: 0,
                next: 0,
                pad: 0,
            },
        };
        Some(dev)
    }

    /// Activate the receive engine.  Must be called after the device has come
    /// to rest at its final address: the descriptor's next pointer is a
    /// physical self-reference and would be stale if taken in `new()` (the
    /// struct moves when it is returned).
    pub fn start(&mut self) {
        self.start_rx();
    }

    /// MAC address.
    pub fn mac_address(&self) -> EthernetAddress {
        self.mac
    }

    /// (Re)arm the single receive descriptor and activate the RX engine.
    fn start_rx(&mut self) {
        // The descriptor is shared with the DMA engine, so every field is
        // accessed through volatile operations: the compiler must not cache
        // or constant-fold values the hardware can change.
        let buf_phys = phys(self.rx_buf.as_ptr());
        let desc_phys = phys(&self.rx_desc);
        unsafe {
            write_volatile(&mut self.rx_desc.addr, buf_phys);
            write_volatile(&mut self.rx_desc.flags, DESC_ROWN | MAX_RPKT);
            write_volatile(&mut self.rx_desc.next, desc_phys);
        }
        hpc3_wr32(RX_ENGINE, ENET_CBP, desc_phys);
        hpc3_wr32(RX_ENGINE, ENET_NBDP, desc_phys);
        // Drop the channel first: CTRL writes only change ACTIVE on a 0->1
        // transition in the HPC3 model.
        hpc3_wr32(RX_ENGINE, ENET_CTRL, 0);
        hpc3_wr32(RX_ENGINE, ENET_CTRL, ENET_CTRL_ACTIVE);
    }

    /// Re-arm the receive descriptor after a frame (the HPC3 cleared ROWN and
    /// wrote the remaining byte count back into it).
    fn rearm_rx(&mut self) {
        self.start_rx();
    }
}

impl Device for SeeqDevice {
    type RxToken<'b> = SeeqRxToken<'b>;
    type TxToken<'b> = SeeqTxToken<'b>;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        caps.medium = Medium::Ethernet;
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // ROWN is set while the HPC3 owns the descriptor.  It clears when a
        // frame has been written (along with the remaining byte count).  The
        // descriptor lives in RAM shared with the DMA engine, so read it
        // with volatile semantics.
        let flags = unsafe { read_volatile(&self.rx_desc.flags) };
        if flags & DESC_ROWN != 0 {
            return None;
        }

        let remaining = flags & DESC_COUNT_MASK;
        let total = MAX_RPKT.saturating_sub(remaining);
        if total < 3 {
            self.rearm_rx();
            return None;
        }
        let len = (total - 3) as usize;
        if !(14..=1514).contains(&len) {
            self.rearm_rx();
            return None;
        }

        let buf = self.rx_buf.as_ptr();
        let status = unsafe { read_volatile(buf.add(2 + len)) };
        for (i, slot) in self.rx_frame.iter_mut().take(len).enumerate() {
            *slot = unsafe { read_volatile(buf.add(2 + i)) };
        }
        self.rearm_rx();

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

/// Receive token: copy the received frame into `f`.
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

        hpc3_wr32(TX_ENGINE, ENET_CBP, desc_phys);
        hpc3_wr32(TX_ENGINE, ENET_NBDP, desc_phys);
        hpc3_wr32(TX_ENGINE, ENET_CTRL, 0);
        hpc3_wr32(TX_ENGINE, ENET_CTRL, ENET_CTRL_ACTIVE);

        // Poll until the HPC3 has consumed the descriptor and cleared ACTIVE.
        for _ in 0..10_000_000 {
            if hpc3_rd32(TX_ENGINE, ENET_CTRL) & ENET_CTRL_ACTIVE == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        result
    }
}

/// Result of a successful DHCPv4 configuration.
pub struct DhcpLease {
    pub address: Ipv4Cidr,
    pub router: Option<Ipv4Address>,
}

/// Run DHCPv4 on an existing interface and return the assigned lease.
///
/// The interface and socket set are owned by the caller so the same network
/// stack is used for DHCP, NTP and the kernel fetch.
pub fn dhcpv4_configure(
    iface: &mut Interface,
    socket_set: &mut SocketSet,
    device: &mut SeeqDevice,
) -> Option<DhcpLease> {
    let dhcpv4_socket = dhcpv4::Socket::new();
    let dhcpv4_handle = socket_set.add(dhcpv4_socket);

    let mut timestamp = Instant::from_millis(0);
    let start = timestamp;
    let timeout_ms = 10_000u64;

    loop {
        // Drive the interface (sends/receives frames through the Seeq device).
        let _ = iface.poll(timestamp, device, socket_set);

        // Check the DHCPv4 socket for a configuration event.
        let socket = socket_set.get_mut::<dhcpv4::Socket>(dhcpv4_handle);
        match socket.poll() {
            Some(dhcpv4::Event::Configured(config)) => {
                return Some(DhcpLease {
                    address: config.address,
                    router: config.router,
                });
            }
            Some(dhcpv4::Event::Deconfigured) => {
                // rediscovering; keep polling
            }
            None => {}
        }

        // Advance the interface clock by a fixed millisecond per poll.  Using
        // poll_delay() here is wrong: a zero delay would freeze the clock and
        // make the DHCP socket retransmit on every loop.
        timestamp += Duration::from_millis(1);
        if timestamp.millis() - start.millis() > timeout_ms as i64 {
            return None;
        }
    }
}
