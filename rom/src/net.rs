//! ROM network stack: Seeq 8003 as a smoltcp `Device` + DHCPv4 client.
//!
//! Hardware facts (docs/research/indy-hardware.md §6):
//! - Seeq 8003 at kseg1 0xBFBD_4000, 8 registers at byte lane 0 (`reg << 2`).
//!   Regs 0-5: station MAC (latched from DS1386 NVRAM at reset). Reg 6: RX
//!   command/status. Reg 7: TX command/status.
//! - HPC3 PBUS DMA: TX engine at kseg1 0xBFB9_6000, RX engine at 0xBFB9_4000.
//!   TX_CBPTR=0x000, TX_NDPTR=0x004, TX_CTRL=0x0104 (ACTIVE=bit 9).
//!
//! The RX engine and DMA descriptor flag values (XIE/EOP/EOX) are flagged TBD
//! pending confirmation against the live hardware on iris.

use core::ptr::{read_volatile, write_volatile};
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::dhcpv4;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, Ipv4Cidr};

/// Seeq register window, kseg1 virtual base.
const SEEQ_BASE: *mut u8 = 0xBFBD_4000 as *mut u8;

/// HPC3 TX DMA engine base (HPC3 + 0x16000), kseg1.
const TX_ENGINE: *mut u32 = 0xBFB9_6000 as *mut u32;

/// Seeq register numbers.
const REG_MAC0: u8 = 0;
const REG_RX_CMD: u8 = 6;
const REG_TX_CMD: u8 = 7;

/// HPC3 TX engine register offsets (bytes from TX_ENGINE).
const TX_CBPTR: usize = 0x0000;
const TX_NDPTR: usize = 0x0004;
const TX_CTRL: usize = 0x0104;
const TX_CTRL_ACTIVE: u32 = 1 << 9;

/// Read one byte from a Seeq register (lane 0).
fn seeq_rd8(reg: u8) -> u8 {
    unsafe { read_volatile(SEEQ_BASE.add((reg as usize) << 2)) }
}

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

/// The Seeq device as a smoltcp `Device`.
pub struct SeeqDevice {
    /// Station MAC address.
    mac: EthernetAddress,
    /// Transmit buffer (physically contiguous DMA buffer).
    tx_buf: [u8; 1514],
    /// Receive buffer (physically contiguous DMA buffer).
    rx_buf: [u8; 1514],
    /// A frame is ready in `rx_buf`.
    rx_ready: bool,
    /// Length of the received frame.
    rx_len: usize,
}

impl SeeqDevice {
    /// Probe the Seeq: latch the MAC from the station-address registers.
    pub fn new() -> Option<Self> {
        let mut octets = [0u8; 6];
        for (i, octet) in octets.iter_mut().enumerate() {
            *octet = seeq_rd8(REG_MAC0.wrapping_add(i as u8));
        }
        if octets == [0xFF; 6] {
            return None; // dead RTC battery
        }
        // Soft-reset RX/TX.
        seeq_wr8(REG_RX_CMD, 0x00);
        seeq_wr8(REG_TX_CMD, 0x00);
        Some(Self {
            mac: EthernetAddress::from_bytes(&octets),
            tx_buf: [0; 1514],
            rx_buf: [0; 1514],
            rx_ready: false,
            rx_len: 0,
        })
    }

    /// MAC address.
    pub fn mac_address(&self) -> EthernetAddress {
        self.mac
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
        if !self.rx_ready {
            return None;
        }
        self.rx_ready = false;
        let len = self.rx_len;
        Some((
            SeeqRxToken(&mut self.rx_buf[..len]),
            SeeqTxToken(&mut self.tx_buf),
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(SeeqTxToken(&mut self.tx_buf))
    }
}

/// Receive token: on consume, copy the received frame into `f`.
pub struct SeeqRxToken<'a>(&'a mut [u8]);

impl<'a> smoltcp::phy::RxToken for SeeqRxToken<'a> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.0)
    }
}

/// Transmit token: on consume, `f` fills the buffer, then the frame is sent via DMA.
pub struct SeeqTxToken<'a>(&'a mut [u8]);

impl<'a> smoltcp::phy::TxToken for SeeqTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let result = f(&mut self.0[..len]);
        // Program the TX descriptor and kick the engine.
        // TODO: confirm descriptor flag values (XIE/EOP/EOX) on iris.
        let desc_addr: u32 = 0; // physical address of a TxDesc (TBD)
        hpc3_wr32(TX_ENGINE, TX_CBPTR, desc_addr);
        hpc3_wr32(TX_ENGINE, TX_NDPTR, 0);
        hpc3_wr32(TX_ENGINE, TX_CTRL, TX_CTRL_ACTIVE);
        // Poll until ACTIVE clears.
        for _ in 0..1_000_000 {
            if hpc3_rd32(TX_ENGINE, TX_CTRL) & TX_CTRL_ACTIVE == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        result
    }
}

/// Run DHCPv4 and return the assigned `Ipv4Cidr`.
///
/// Polls the Seeq interface and the DHCPv4 socket until an address is
/// acquired, or `timeout_ms` elapses.
pub fn dhcpv4_configure(device: &mut SeeqDevice) -> Option<Ipv4Cidr> {
    let mac = device.mac_address();
    let mut sockets: [_; 4] = Default::default();
    let mut socket_set = SocketSet::new(&mut sockets[..]);
    let dhcpv4_socket = dhcpv4::Socket::new();
    let dhcpv4_handle = socket_set.add(dhcpv4_socket);
    let mut iface = Interface::new(
        Config::new(HardwareAddress::Ethernet(mac)),
        device,
        Instant::from_millis(0),
    );

    let mut timestamp = Instant::from_millis(0);
    let start = timestamp;
    let timeout_ms = 10_000u64;

    loop {
        // Drive the interface (sends/receives frames through the Seeq device).
        let _ = iface.poll(timestamp, device, &mut socket_set);

        // Check the DHCPv4 socket for a configuration event.
        let socket = socket_set.get_mut::<dhcpv4::Socket>(dhcpv4_handle);
        match socket.poll() {
            Some(dhcpv4::Event::Configured(config)) => return Some(config.address),
            Some(dhcpv4::Event::Deconfigured) => {
                // rediscovering; keep polling
            }
            None => {}
        }

        // Advance time until the next scheduled event, up to the timeout.
        let delay = iface.poll_delay(timestamp, &socket_set);
        let next = match delay {
            Some(d) => timestamp + d,
            None => timestamp + Duration::from_millis(10),
        };
        if next.millis() - start.millis() > timeout_ms as i64 {
            return None;
        }
        timestamp = next;
    }
}
# rom: M1 netboot CI trigger
