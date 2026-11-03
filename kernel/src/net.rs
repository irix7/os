// SPDX-License-Identifier: GPL-3.0-only
//! Kernel network stack — Seeq 8003 + HPC3 PDMA with smoltcp.
//!
//! Register and descriptor layout follows the original Indy ARCS PROM driver
//! (`stand/arcs/lib/libsk/net/if_ec2.c`, `include/net/seeq.h`):
//!
//! - Seeq 8003 at kseg1 `0xBFBD_4000`, registers at byte lane 0
//!   (`reg << 2`).  Registers 0-5 are the station address in bank 0,
//!   register 6 is RX command/status and register 7 is TX command/status.
//! - HPC3 enet RX engine at `0xBFB9_4000`, TX engine at `0xBFB9_6000`:
//!   CBP = 0x000, NBDP = 0x004, CTRL = 0x1004; ACTIVE = bit 9.
//! - PDMA descriptor (Indy layout, 16 bytes): word 0 is the buffer physical
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
//!
//! ## Receive interrupt
//!
//! `SeeqDevice::new` enables `INTEOF` in the receive command and every
//! descriptor carries `XIE`, so a good frame raises the Seeq's INT pin and
//! the HPC3 `INTSTAT.ENET_DEV` bit.  That line reaches CPU IP2 through the
//! IOC's local-0 mask, which [`init`] unmasks only after [`interrupt`] has
//! passed its boot self-test.
//!
//! The handler is deliberately tiny: read the split HPC3 status, read the two
//! Seeq status registers, write `CLRINT`, acknowledge the DMA status bits and
//! set one deferred flag.  Frames are taken by [`drain`] on the idle thread.
//! The ordering matters — `CLRINT` also marks both Seeq status registers OLD,
//! so the chip is read first.  The Seeq's own status bit is cleared only by
//! `CLRINT`; writing it back would clear the status while the pin is still
//! asserted (see [`net_irq::ENET_W1C`]).

use core::mem::MaybeUninit;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};

use crate::net_irq;
pub use irix7_seeq::SeeqDevice;
use irix7_seeq::*;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address, Ipv4Cidr};

/// MAC slot in the Dallas NVRAM: physical `0x1FBE_04E8`, one byte per word.
const NVRAM_MAC: *const u32 = 0xBFBE_04E8 as *const u32;
/// Fallback station address when the NVRAM battery is dead (all 0xFF).
const FALLBACK_MAC: [u8; 6] = [0x02, 0x00, 0x69, 0x12, 0x34, 0x56];
/// The station address the device uses: NVRAM, or the fallback when the
/// battery is dead (all 0xFF or all zero).
fn station_mac() -> [u8; 6] {
    let mac = read_nvram_mac();
    if mac == [0xFF; 6] || mac == [0x00; 6] {
        FALLBACK_MAC
    } else {
        mac
    }
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

/// Whether the environment asks for the polled receive path.
///
/// `net.rx=off` keeps the Seeq interrupt masked and the IOC's Ethernet line
/// disabled, so the driver is exactly the `net=poll` path.  It is the A/B
/// control for the interrupt measurement and the fallback if a real machine's
/// acknowledge ordering turns out to differ.
fn configured_rx_off() -> bool {
    let mut value = [0u8; 8];
    match crate::nvram::get("net.rx", &mut value) {
        Some(n) => matches!(&value[..n], b"off" | b"none" | b"0"),
        None => false,
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
const SOCKET_SLOTS: usize = 28;

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

/// Register values at the last parked interrupt, for the recovery line.
static mut PARK_STATUS: (u32, u32) = (0, 0);

/// Whether the receive interrupt is unmasked (set once, after the boot test).
static mut ARMED: bool = false;
/// Whether the first real interrupt has been reported on the console.
static mut FIRST_IRQ_LOGGED: bool = false;

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

    let dev = match SeeqDevice::new(station_mac()) {
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

    // Exercise the handler before the line can reach the CPU.  The self-test
    // runs the real decision path twice: a status with no owned source, which
    // must be counted and rejected without touching the tick, and the Ethernet
    // bit, which must defer one wake.  Hardware registers are left alone, so
    // this cannot consume a frame that arrived during the test.
    if configured_rx_off() {
        crate::println("network: rx irq disabled by environment; polling");
    } else {
        let armed = arm_rx_interrupt();
        crate::println(if armed {
            "network: rx irq armed (ip2, selftest ok)"
        } else {
            "network: rx irq unavailable (selftest FAILED); polling"
        });
    }

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

            irix7_netif::apply_lease(&mut iface, lease.address, lease.router);
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
            service_dhcp(stack);
        }
    }
    after_frame_service();
}

/// Thread-side bookkeeping shared by [`poll`] and [`drain`].
///
/// The streak is the handler's count of entries since a thread serviced
/// frames; a poll or drain of any kind clears it.  A parked line is the one
/// case the interrupt path could not clear, and rebuilding the ring is the
/// documented recovery.
fn after_frame_service() {
    net_irq::reset_streak();
    if net_irq::take_parked() {
        recover_rx();
    }
}

/// Apply DHCP renewals and rebinds to the live interface.
fn service_dhcp(stack: &mut Stack) {
    let Some(handle) = stack.dhcp else {
        return;
    };
    match stack.sockets.get_mut::<dhcpv4::Socket>(handle).poll() {
        Some(dhcpv4::Event::Configured(config)) => {
            let ip = config.address.address();
            stack.ip = Some(ip);
            stack.dns = config.dns_servers.first().copied();
            irix7_netif::apply_lease(&mut stack.iface, config.address, config.router);
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

/// Frames one interrupt-driven pass takes from the ring.
///
/// The ring holds eight descriptors, so one pass drains at most one full ring
/// and returns to the timer, telnet and the module ticks.  A burst larger than
/// the budget is picked up by the next wake: the Seeq line stays asserted and
/// the handler sets the deferred flag again.
pub const RX_DRAIN_BUDGET: usize = RX_RING;

/// Take at most `limit` received frames, then run egress.
///
/// This is the bounded half of the interrupt path: frames copied by the hard
/// handler are processed on the thread that owns the socket set.  `net=poll`
/// keeps the unbounded [`poll`] path.
pub fn drain(limit: usize) -> usize {
    clock_tick();
    let now = clock_now();
    let mut taken = 0;
    // SAFETY: single-threaded kernel; the drain runs on the idle thread.
    unsafe {
        if let (Some(stack), Some(dev)) = (STACK.as_mut(), seeq()) {
            stack.iface.poll_maintenance(now);
            while taken < limit {
                match stack
                    .iface
                    .poll_ingress_single(now, dev, &mut stack.sockets)
                {
                    smoltcp::iface::PollIngressSingleResult::None => break,
                    _ => taken += 1,
                }
            }
            let _ = stack.iface.poll_egress(now, dev, &mut stack.sockets);
            service_dhcp(stack);
        }
    }
    // The streak counts interrupts serviced without a thread-side poll; this
    // pass is one.
    after_frame_service();
    taken
}

/// Whether the Ethernet receive interrupt is unmasked.
pub fn armed() -> bool {
    // SAFETY: single-threaded; written once by init() after the boot test.
    unsafe { ARMED }
}

/// Interrupt counters for the status page and the gate.
pub fn irq_stats() -> net_irq::Stats {
    net_irq::stats(armed())
}

/// Whether a deferred wake is waiting for the drain.
pub fn pending() -> bool {
    net_irq::pending()
}

/// Take the deferred-wake flag set by [`interrupt`].
pub fn take_pending() -> bool {
    net_irq::take_pending()
}

/// True once, on the first drain after a real Ethernet interrupt.
///
/// The boot self-test clears the counters, so this cannot fire from the test.
pub fn first_irq() -> bool {
    // SAFETY: single-threaded idle thread.
    unsafe {
        if FIRST_IRQ_LOGGED || net_irq::stats(false).rx == 0 {
            return false;
        }
        FIRST_IRQ_LOGGED = true;
    }
    true
}

/// The hard interrupt handler for IP2.
///
/// Runs from `trap_handler` on the trap stack with `EXL` set.  It reads the
/// split HPC3 status, acknowledges the source and sets one deferred flag.  No
/// allocation, console output, smoltcp call or module call is permitted here;
/// [`drain`] does the work on the idle thread.
pub fn interrupt() {
    service(read_intstat(), true);
}

/// Read and merge the HPC3's split interrupt-status registers.
fn read_intstat() -> u32 {
    // SAFETY: HPC3 registers are memory-mapped and readable at any time.
    let (istat0, istat1) = unsafe { (read_volatile(HPC3_INTSTAT), read_volatile(HPC3_INTSTAT1)) };
    net_irq::merge_status(istat0, istat1)
}

/// Read the receive channel control register.
///
/// The read has side effects in the hardware model (it clears `RBO`, mirrors
/// the Seeq status in the low byte and lets the Seeq retry a held frame), so
/// it is only read on the one path that has to tell an overflow from a new
/// frame.
fn rx_ctrl() -> u32 {
    // SAFETY: RX_ENGINE is the HPC3 receive-channel register block.
    unsafe { hpc3_rd32(RX_ENGINE, ENET_RX_CTRL) }
}

/// Mask IP2 and record that the source needs the drain path to clear it.
fn park_im2(ctrl: u32, status: u32) {
    // SAFETY: single-threaded; interrupt context writes, the drain reads.
    unsafe {
        PARK_STATUS = (ctrl, status);
    }
    mask_im2();
    net_irq::note_parked();
}

/// The handler's policy, with the device acknowledge optional so the boot
/// self-test can run it without touching a device.
fn service(status: u32, ack: bool) {
    match net_irq::source(status) {
        net_irq::Source::Ethernet => {
            net_irq::note_rx();
            if ack {
                ack_enet(status);
                // The Seeq pin is level-triggered.  A surviving source is a
                // new frame (handled on the next entry) unless the receive
                // channel stopped with an overflow, which CLRINT cannot
                // clear: park the CPU mask so the drain can rebuild the ring.
                let after = read_intstat();
                let ctrl = rx_ctrl();
                let survives = net_irq::source(after) == net_irq::Source::Ethernet;
                if survives && ctrl & ENET_CTRL_RBO != 0 {
                    park_im2(ctrl, after);
                }
                if net_irq::must_park(net_irq::bump_streak()) {
                    park_im2(ctrl, after);
                }
            }
        }
        net_irq::Source::Spurious => {
            net_irq::note_spurious();
            // A level that survives an entry with no readable source would
            // otherwise trap forever; the drain path re-arms after the next
            // pass and the line is re-checked.
            if ack && net_irq::must_park(net_irq::bump_streak()) {
                park_im2(0, status);
            }
        }
    }
}

/// Acknowledge the Ethernet source in the documented order.
fn ack_enet(status: u32) {
    // 1. The Seeq's own status registers.  Reading RX then TX clears both NEW
    //    bits and drops the chip's INT pin.
    let _ = seeq_rd8(REG_RX_CMD);
    let _ = seeq_rd8(REG_TX_CMD);
    // 2. The HPC3 latch.  CLRINT is a write strobe on the receive channel's
    //    reset register; it drops ENET_DEV and the IOC's Ethernet line.  It
    //    must follow the status reads, because it also marks them OLD.
    unsafe {
        write_volatile(ENET_RX_RESET, ENET_RX_RESET_CLRINT);
    }
    // 3. The DMA status bits this pass serviced (W1C).  The device bit is
    //    deliberately absent: CLRINT owns it, and a W1C here could clear the
    //    status while the Seeq pin is still high (see `net_irq::ENET_W1C`).
    let bits = net_irq::ack_bits(status);
    if bits != 0 {
        unsafe {
            write_volatile(HPC3_INTSTAT, bits);
        }
    }
}

/// Rebuild the receive ring after the handler parked the line.
fn recover_rx() {
    // SAFETY: single-threaded; written by the handler, read on the drain path.
    let (ctrl, status) = unsafe { PARK_STATUS };
    if let Some(dev) = seeq() {
        dev.restart_rx();
        crate::print_str("network: rx ring reset ctrl=0x");
        crate::print_hex32(ctrl);
        crate::print_str(" status=0x");
        crate::print_hex32(status);
        crate::println("");
    }
    if armed() {
        unmask_im2();
    }
}

fn mask_im2() {
    let status = crate::traps::status_read();
    crate::traps::status_write(status & !crate::traps::STATUS_IM2);
}

fn unmask_im2() {
    let status = crate::traps::status_read();
    crate::traps::status_write(status | crate::traps::STATUS_IM2);
}

/// Run the handler's self-test, then unmask the Seeq line and CPU IP2.
///
/// `Status.IM2` is set only after the decision path has been exercised, so a
/// bug in classify/ack cannot turn the timer's fast path into a storm at
/// boot.  The IOC's local-0 Ethernet bit is the controller-side enable; both
/// are set here and in [`recover_rx`].
fn arm_rx_interrupt() -> bool {
    net_irq::reset_stats();
    let ticks_before = crate::idle::ticks();

    // A status with no owned source is counted, rejected, and must leave the
    // 10 ms tick untouched.
    service(0, false);
    let rejected = net_irq::stats(false).spurious == 1 && crate::idle::ticks() == ticks_before;

    // The Ethernet bit defers exactly one wake.
    service(net_irq::ENET_SOURCE, false);
    let deferred = net_irq::take_pending();
    let counted = net_irq::stats(false).rx == 1;
    net_irq::reset_stats();

    let ok = rejected && deferred && counted;
    if ok {
        // SAFETY: the IOC register is byte-wide and only this path writes it.
        unsafe {
            let mask = read_volatile(IOC_L0_MASK);
            write_volatile(IOC_L0_MASK, mask | IOC_L0_ETHERNET);
        }
        unmask_im2();
        // SAFETY: single-threaded boot; this is the only writer.
        unsafe {
            ARMED = true;
        }
    }
    ok
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
    // The TLS client pumps the interface through these raw pointers, so this
    // is a thread-side service too: a long HTTPS fetch must clear the
    // handler's undrained streak exactly like `poll` and `drain` do.
    after_frame_service();
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
    irix7_fmt::dotted(addr.octets(), &mut |b| crate::write_byte(b));
}

/// Print an unsigned integer in decimal.
fn print_dec(v: u32) {
    irix7_fmt::decimal(u64::from(v), &mut |b| crate::write_byte(b));
}
