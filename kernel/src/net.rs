// SPDX-License-Identifier: GPL-3.0-only
//! Kernel network stack — Seeq 8003 + HPC3 PDMA with smoltcp.
//!
//! Register and descriptor layout follows the original IP22 ARCS PROM driver
//! (`stand/arcs/lib/libsk/net/if_ec2.c`, `include/net/seeq.h`):
//!
//! - Seeq 8003 at kseg1 `0xBFBD_4000`, registers at byte lane 0
//!   (`reg << 2`).  Registers 0-5 are the station address in bank 0,
//!   register 6 is RX command/status and register 7 is TX command/status.
//! - HPC3 enet RX engine at `0xBFB9_4000`, TX engine at `0xBFB9_6000`:
//!   CBP = 0x000, NBDP = 0x004, CTRL = 0x1004; ACTIVE = bit 9.
//! - PDMA descriptor (IP22 layout, 16 bytes): word 0 is the buffer physical
//!   address, word 1 is EOX(31) | EOP(30) | XIE(29) | ... | ROWN(14) | byte
//!   count(13:0), word 2 is the next descriptor's physical address.
//! - RX buffer layout: two pad bytes, the Ethernet frame, one SEEQ status
//!   byte.
//!
//! The MAC address lives in the Dallas DS1386 NVRAM at physical
//! `0x1FBE_04E8`, one byte per 32-bit word in the low byte lane.  On the
//! 80C03 the Seeq station-address registers read back collision counters,
//! not the MAC, so the address has to be read from NVRAM and written into
//! bank 0.

use core::mem::MaybeUninit;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address, Ipv4Cidr};

/// Seeq register window, kseg1 virtual base.
const SEEQ_BASE: *mut u8 = 0xBFBD_4000 as *mut u8;

/// The Seeq device (DMA descriptors and frame buffers) is placed in the kseg1
/// alias of the reserved window.  kseg0 is cacheable (`Config.K0 = 3`), so
/// keeping the DMA-visible structure uncached avoids cache maintenance on every
/// descriptor and frame.  Physical `0x0860_0000` is immediately after the 2 MiB
/// module area and below the frame allocator's first page.
const SEEQ_AREA: usize = 0xA860_0000;

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
const DESC_ROWN: u32 = 0x0000_4000; // RX: HPC3 owns the buffer
const DESC_COUNT_MASK: u32 = 0x3FFF;

/// Original driver's receive buffer size (`MAX_RPKT` in seeq.h).
const MAX_RPKT: u32 = 1517;

/// Seeq register numbers.
const REG_MAC0: u8 = 0;
const REG_RX_CMD: u8 = 6;
const REG_TX_CMD: u8 = 7;
const RX_CMD_STA_MULTI: u8 = 0xC0;

/// MAC slot in the Dallas NVRAM: physical `0x1FBE_04E8`, one byte per word.
const NVRAM_MAC: *const u32 = 0xBFBE_04E8 as *const u32;

/// Fallback station address when the NVRAM battery is dead (all 0xFF).
const FALLBACK_MAC: [u8; 6] = [0x02, 0x00, 0x69, 0x12, 0x34, 0x56];

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

/// Convert a kseg0/kseg1 pointer to the physical address DMA engines need.
fn phys<T>(p: *const T) -> u32 {
    (p as u32) & 0x1FFF_FFFF
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

/// Read the station address left by the firmware in NVRAM.
fn read_nvram_mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        let word = unsafe { read_volatile(NVRAM_MAC.add(i)) };
        *b = (word & 0xFF) as u8;
    }
    mac
}

/// Number of receive descriptors in the ring.  A ring lets the HPC3 move on
/// to the next buffer while the CPU processes the previous frame; a single
/// descriptor forces a stop/re-arm cycle and drops frames on bulk transfers.
const RX_RING: usize = 8;

/// Set when the DS1386 environment disables networking for an offline boot.
static mut NETWORK_OFF: bool = false;

/// Whether networking was deliberately disabled at boot.
pub fn disabled() -> bool {
    // SAFETY: single-threaded kernel; written once by init().
    unsafe { NETWORK_OFF }
}

fn configured_off() -> bool {
    let mut value = [0u8; 8];
    match crate::nvram::get("net", &mut value) {
        Some(n) => match &value[..n] {
            b"off" | b"none" | b"0" => true,
            _ => false,
        },
        None => false,
    }
}

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
    pub fn new() -> Option<Self> {
        let mut mac_bytes = read_nvram_mac();
        if mac_bytes == [0xFF; 6] || mac_bytes == [0x00; 6] {
            mac_bytes = FALLBACK_MAC;
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

    /// Build the descriptor ring and activate the RX engine.
    fn start_rx(&mut self) {
        let mut buf_phys = [0u32; RX_RING];
        for i in 0..RX_RING {
            buf_phys[i] = phys(self.rx_bufs[i].as_ptr());
        }
        let desc_base = self.rx_descs.as_ptr();
        for i in 0..RX_RING {
            let next = phys(unsafe { desc_base.add((i + 1) % RX_RING) });
            unsafe {
                write_volatile(&mut self.rx_descs[i].addr, buf_phys[i]);
                write_volatile(&mut self.rx_descs[i].flags, DESC_ROWN | MAX_RPKT);
                write_volatile(&mut self.rx_descs[i].next, next);
            }
        }

        let first = phys(&self.rx_descs[0]);
        hpc3_wr32(RX_ENGINE, ENET_CBP, first);
        hpc3_wr32(RX_ENGINE, ENET_NBDP, first);
        // Drop the channel first: CTRL writes only change ACTIVE on a 0->1
        // transition in the HPC3 model.
        hpc3_wr32(RX_ENGINE, ENET_CTRL, 0);
        hpc3_wr32(RX_ENGINE, ENET_CTRL, ENET_CTRL_ACTIVE);
    }

    /// Re-arm one receive descriptor after its frame has been consumed.  The
    /// channel is already working on the next descriptor, so it must not be
    /// stopped here.
    fn rearm_rx(&mut self, index: usize) {
        let buf_phys = phys(self.rx_bufs[index].as_ptr());
        unsafe {
            write_volatile(&mut self.rx_descs[index].addr, buf_phys);
            write_volatile(&mut self.rx_descs[index].flags, DESC_ROWN | MAX_RPKT);
        }
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
        for (i, slot) in self.rx_frame.iter_mut().take(len).enumerate() {
            *slot = unsafe { read_volatile(buf.add(2 + i)) };
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

/// Monotonic millisecond clock shared by DHCP and TCP.  The interface keeps
/// timers across all of them, so rewinding the time per request breaks TCP
/// retransmission and neighbour expiry.
static mut CLOCK_MS: u64 = 0;
static mut LAST_COUNT: u32 = 0;
static mut CLOCK_REMAINDER: u32 = 0;

/// CP0 Count frequency used by iris (and close to the R4400's /2 timer on a
/// 66 MHz part).  A real-hardware boot can calibrate this later; for the
/// kernel console it is accurate enough to keep uptime and network timers
/// honest.
const COUNT_HZ: u64 = 33_000_000;
const COUNTS_PER_MS: u32 = (COUNT_HZ / 1000) as u32;

/// Current network stack time.
pub fn clock_now() -> Instant {
    Instant::from_millis(unsafe { CLOCK_MS } as i64)
}

/// Advance the virtual clock from the CP0 Count register.  Polling loops call
/// this instead of adding a fixed millisecond per iteration, so a fast loop
/// does not race the wall clock.
fn clock_tick() {
    let now = crate::traps::count_read();
    let delta = if unsafe { LAST_COUNT } == 0 {
        0
    } else {
        now.wrapping_sub(unsafe { LAST_COUNT })
    };
    unsafe {
        LAST_COUNT = now;
    }
    let total = unsafe { CLOCK_REMAINDER }.saturating_add(delta);
    let ms = total / COUNTS_PER_MS;
    unsafe {
        CLOCK_REMAINDER = total % COUNTS_PER_MS;
    }
    if ms > 0 {
        unsafe {
            CLOCK_MS = CLOCK_MS.wrapping_add(ms as u64);
        }
    }
}

/// Result of a successful DHCPv4 configuration.
struct DhcpLease {
    address: Ipv4Cidr,
    router: Option<Ipv4Address>,
    dns: Option<Ipv4Address>,
    handle: SocketHandle,
}

/// Run DHCPv4 on an existing interface and return the assigned lease.
fn dhcpv4_configure(
    iface: &mut Interface,
    socket_set: &mut SocketSet,
    device: &mut SeeqDevice,
) -> (Option<DhcpLease>, SocketHandle) {
    let dhcpv4_socket = dhcpv4::Socket::new();
    let dhcpv4_handle = socket_set.add(dhcpv4_socket);

    let start = clock_now();
    let mut timestamp = start;
    let timeout_ms = 15_000i64;

    loop {
        // Drive the interface (sends and receives frames through Seeq).
        let _ = iface.poll(timestamp, device, socket_set);

        // Check the DHCPv4 socket for a configuration event.
        let socket = socket_set.get_mut::<dhcpv4::Socket>(dhcpv4_handle);
        match socket.poll() {
            Some(dhcpv4::Event::Configured(config)) => {
                return (
                    Some(DhcpLease {
                        address: config.address,
                        router: config.router,
                        dns: config.dns_servers.first().copied(),
                        handle: dhcpv4_handle,
                    }),
                    dhcpv4_handle,
                );
            }
            Some(dhcpv4::Event::Deconfigured) => {
                // Rediscovering; keep polling.
            }
            None => {}
        }

        // Advance the interface clock from CP0 Count.  Using poll_delay()
        // here is wrong: a zero delay would freeze the clock and make the
        // DHCP socket retransmit on every loop.
        clock_tick();
        timestamp = clock_now();
        if timestamp.total_millis() - start.total_millis() > timeout_ms {
            return (None, dhcpv4_handle);
        }
    }
}

/// Socket storage backing the shared `SocketSet`.  It must be static because
/// the set is stored in `STACK` and used from `poll()`.
const SOCKET_SLOTS: usize = 12;

static mut SOCKET_STORAGE: MaybeUninit<[SocketStorage<'static>; SOCKET_SLOTS]> =
    MaybeUninit::uninit();

#[allow(static_mut_refs)]
fn socket_storage() -> &'static mut [SocketStorage<'static>] {
    unsafe {
        let p =
            core::ptr::addr_of_mut!(SOCKET_STORAGE) as *mut [SocketStorage<'static>; SOCKET_SLOTS];
        let slots = p as *mut SocketStorage<'static>;
        for i in 0..SOCKET_SLOTS {
            slots.add(i).write(SocketStorage::EMPTY);
        }
        core::slice::from_raw_parts_mut(slots, SOCKET_SLOTS)
    }
}

/// The live network state: interface plus socket set.
struct Stack {
    iface: Interface,
    sockets: SocketSet<'static>,
    ip: Option<Ipv4Address>,
    dns: Option<Ipv4Address>,
    dhcp: Option<SocketHandle>,
}

/// The Seeq device lives at [`SEEQ_AREA`], not in .bss: its descriptors and
/// buffers are DMA-visible and must be uncached.
static mut SEEQ: *mut SeeqDevice = core::ptr::null_mut();
static mut STACK: Option<Stack> = None;

/// The live Seeq device.
fn seeq() -> Option<&'static mut SeeqDevice> {
    // SAFETY: the pointer is written once during init() and the kernel is
    // single-threaded.
    unsafe { SEEQ.as_mut() }
}

/// Bring up the Seeq, run DHCP and publish the live stack.
pub fn init() {
    if configured_off() {
        // SAFETY: single-threaded boot; this is the only writer.
        unsafe {
            NETWORK_OFF = true;
        }
        crate::println("network: disabled by environment");
        return;
    }

    let dev = match SeeqDevice::new() {
        Some(d) => d,
        None => {
            crate::println("network: no Seeq device");
            return;
        }
    };

    // Copy the device into the uncached alias and use only that copy from now
    // on.  The stack copy is dead after this point.
    let ptr = SEEQ_AREA as *mut SeeqDevice;
    // SAFETY: the area is reserved for this device, aligned, and written once.
    unsafe {
        ptr.write(dev);
        SEEQ = ptr;
    }
    let seeq = seeq().unwrap();
    seeq.start();

    crate::print_str("network: mac ");
    for (i, b) in seeq.mac_address().as_bytes().iter().enumerate() {
        if i > 0 {
            crate::write_byte(b':');
        }
        crate::print_hex8(*b);
    }
    crate::println("");

    let mut socket_set = SocketSet::new(socket_storage());
    let mut iface = Interface::new(
        Config::new(HardwareAddress::Ethernet(seeq.mac_address())),
        seeq,
        clock_now(),
    );

    crate::print_str("network: dhcp...");
    let (lease, dhcp_handle) = dhcpv4_configure(&mut iface, &mut socket_set, seeq);
    match lease {
        Some(lease) => {
            let ip = lease.address.address();
            crate::print_str("ok, ip ");
            print_ipv4(ip);
            crate::println("");

            iface.update_ip_addrs(|addrs| {
                let _ = addrs.push(IpCidr::Ipv4(lease.address));
            });
            if let Some(gateway) = lease.router {
                let _ = iface.routes_mut().add_default_ipv4_route(gateway);
            }
            if let Some(dns) = lease.dns {
                crate::print_str("network: dns ");
                print_ipv4(dns);
                crate::println("");
            }

            unsafe {
                STACK = Some(Stack {
                    iface,
                    sockets: socket_set,
                    ip: Some(ip),
                    dns: lease.dns,
                    dhcp: Some(dhcp_handle),
                });
            }
        }
        None => {
            crate::println("failed");
            unsafe {
                STACK = Some(Stack {
                    iface,
                    sockets: socket_set,
                    ip: None,
                    dns: None,
                    dhcp: Some(dhcp_handle),
                });
            }
        }
    }
}

/// The DHCP-provided DNS server, if any.
pub fn dns_server() -> Option<Ipv4Address> {
    unsafe { STACK.as_ref().and_then(|s| s.dns) }
}

/// Drive the interface.  Call from the idle loop.
pub fn poll() {
    clock_tick();
    let now = clock_now();
    // SAFETY: single-threaded kernel; both statics are initialised by init().
    unsafe {
        if let (Some(stack), Some(dev)) = (STACK.as_mut(), seeq()) {
            let _ = stack.iface.poll(now, dev, &mut stack.sockets);

            // Service the DHCP socket so renewals and rebinds are applied to
            // the live interface rather than only handled at boot.
            if let Some(handle) = stack.dhcp {
                let event = stack.sockets.get_mut::<dhcpv4::Socket>(handle).poll();
                match event {
                    Some(dhcpv4::Event::Configured(config)) => {
                        let ip = config.address.address();
                        stack.ip = Some(ip);
                        stack.dns = config.dns_servers.first().copied();
                        stack.iface.update_ip_addrs(|addrs| {
                            addrs.clear();
                            let _ = addrs.push(IpCidr::Ipv4(config.address));
                        });
                        if let Some(gateway) = config.router {
                            let _ = stack.iface.routes_mut().add_default_ipv4_route(gateway);
                        }
                        crate::print_str("network: lease ");
                        print_ipv4(ip);
                        crate::println("");
                    }
                    Some(dhcpv4::Event::Deconfigured) => {
                        stack.ip = None;
                        crate::println("network: lease lost");
                    }
                    None => {}
                }
            }
        }
    }
}

/// The DHCP-assigned address, if any.
pub fn ip_addr() -> Option<Ipv4Address> {
    unsafe { STACK.as_ref().and_then(|s| s.ip) }
}

/// Run `f` with the live socket set.  Returns `None` before [`init`].
pub fn sockets_with<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
    // SAFETY: single-threaded kernel; the stack is set once by init().
    unsafe { STACK.as_mut().map(|s| f(&mut s.sockets)) }
}

/// Run `f` with the live interface and socket set.  Returns `None` before
/// [`init`].
pub fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
    // SAFETY: single-threaded kernel; the stack is set once by init().
    unsafe { STACK.as_mut().map(|s| f(&mut s.iface, &mut s.sockets)) }
}

/// Raw pointers to the live stack pieces, for adapters (TLS) that must poll
/// across several calls.  The caller must not alias these with any other
/// borrow; there is one kernel thread.
///
/// # Safety
/// The pointers remain valid for the lifetime of the machine.  They must not
/// be used concurrently with `poll`, `sockets_with` or `with_stack`.
pub unsafe fn raw_stack() -> Option<(*mut Interface, *mut SocketSet<'static>, *mut SeeqDevice)> {
    let stack = STACK.as_mut()?;
    let dev = seeq()?;
    Some((
        &mut stack.iface as *mut Interface,
        &mut stack.sockets as *mut SocketSet<'static>,
        dev as *mut SeeqDevice,
    ))
}

/// Drive the interface from raw pointers (see [`raw_stack`]).
///
/// # Safety
/// The pointers must come from [`raw_stack`] and must not alias another
/// borrow.
pub unsafe fn poll_raw(
    iface: *mut Interface,
    sockets: *mut SocketSet<'static>,
    dev: *mut SeeqDevice,
) {
    clock_tick();
    let now = clock_now();
    let _ = (*iface).poll(now, &mut *dev, &mut *sockets);
}

/// The Seeq station address.
///
/// Before the Seeq is brought up, or when `net=off` disables it, fall back to
/// the station address in NVRAM so the node identity remains stable.
pub fn mac_address() -> [u8; 6] {
    match seeq() {
        Some(dev) => {
            let mut mac = [0u8; 6];
            mac.copy_from_slice(dev.mac_address().as_bytes());
            mac
        }
        None => {
            let mac = read_nvram_mac();
            if mac == [0; 6] || mac == [0xFF; 6] {
                FALLBACK_MAC
            } else {
                mac
            }
        }
    }
}

/// Print an IPv4 address in dotted-decimal form.
fn print_ipv4(addr: Ipv4Address) {
    let octets = addr.octets();
    for (i, octet) in octets.iter().enumerate() {
        if i > 0 {
            crate::write_byte(b'.');
        }
        print_dec(*octet as u32);
    }
}

/// Print an unsigned integer in decimal.
fn print_dec(mut v: u32) {
    let mut buf = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        crate::write_byte(b'0');
        return;
    }
    while v > 0 && n < buf.len() {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        crate::write_byte(buf[n]);
    }
}
