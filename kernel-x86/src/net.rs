// SPDX-License-Identifier: GPL-3.0-only
//! The x86_64 network stack: smoltcp over the polled virtio-net driver, with
//! DHCP for the address.  The stack lives for the whole boot: `init` brings it
//! up, and `poll` drives it from the idle loop and from module services.

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, Ipv4Address, Ipv4Cidr};

use crate::timer;
use crate::virtio_net::{Nic, MAX_FRAME};

/// How long DHCP may take before the machine reports it and carries on.
const DHCP_TICKS: u32 = 500; // 5 s at 100 Hz

/// Socket slots: the DHCP client plus the module TCP slots.
const SOCKET_SLOTS: usize = 12;

/// smoltcp's clock, from the PIT tick count.
pub fn now() -> Instant {
    Instant::from_millis(i64::from(timer::ticks()) * 10)
}

/// smoltcp's view of the NIC.  The receive scratch is owned here so a receive
/// token can borrow it while the transmit token borrows the NIC.
pub struct NicDevice {
    nic: Nic,
    rx: [u8; MAX_FRAME],
}

pub struct NicRxToken<'a>(&'a [u8]);

impl smoltcp::phy::RxToken for NicRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.0)
    }
}

pub struct NicTxToken<'a>(&'a mut Nic);

impl smoltcp::phy::TxToken for NicTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut frame = [0u8; MAX_FRAME];
        let len = len.min(MAX_FRAME);
        let result = f(&mut frame[..len]);
        self.0.send(&frame[..len]);
        result
    }
}

impl Device for NicDevice {
    type RxToken<'a>
        = NicRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = NicTxToken<'a>
    where
        Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = MAX_FRAME;
        caps.medium = Medium::Ethernet;
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let len = self.nic.poll_rx(&mut self.rx)?;
        let rx = &self.rx[..len];
        let nic = &mut self.nic;
        Some((NicRxToken(rx), NicTxToken(nic)))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(NicTxToken(&mut self.nic))
    }
}

/// The NIC's MAC address, recorded when the stack comes up.
static mut MAC: Option<[u8; 6]> = None;

/// The NIC's MAC address, once the stack is up.
pub fn mac() -> Option<[u8; 6]> {
    // SAFETY: read-only access on the single thread.
    unsafe { *core::ptr::addr_of!(MAC) }
}

/// Everything the stack owns for the life of the boot.
pub struct Stack {
    device: NicDevice,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    address: Option<Ipv4Cidr>,
    gateway: Option<Ipv4Address>,
}

static mut SOCKET_STORAGE: [SocketStorage<'static>; SOCKET_SLOTS] =
    [SocketStorage::EMPTY; SOCKET_SLOTS];
static mut STACK: Option<Stack> = None;

fn dotted(address: Ipv4Address) {
    let octets = address.octets();
    for (i, octet) in octets.iter().enumerate() {
        if i > 0 {
            crate::serial::write_byte(b'.');
        }
        crate::print_dec(u64::from(*octet));
    }
}

/// Bring up the NIC and configure the interface from DHCP.  Prints the lease
/// and returns the address, or `None` when the device is absent or DHCP does
/// not answer in time.  The stack stays up either way.
pub fn init() -> Option<Ipv4Address> {
    let Some(nic) = crate::virtio_net::probe() else {
        crate::print_line(&[b"x86: net: no virtio-net device"]);
        return None;
    };
    let mac = nic.mac();
    // SAFETY: written once, before any reader.
    unsafe { *core::ptr::addr_of_mut!(MAC) = Some(mac) };
    crate::serial::write(b"x86: virtio-net mac=");
    for (i, byte) in mac.iter().enumerate() {
        if i > 0 {
            crate::serial::write_byte(b':');
        }
        crate::print_hex8(*byte);
    }
    crate::serial::write_byte(b'\n');

    let mut device = NicDevice {
        nic,
        rx: [0; MAX_FRAME],
    };
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let iface = Interface::new(config, &mut device, now());

    // SAFETY: the machine is single-threaded and the storage is used only by
    // the stack created here.
    let storage: &'static mut [SocketStorage<'static>] =
        unsafe { &mut *core::ptr::addr_of_mut!(SOCKET_STORAGE) }.as_mut_slice();
    let mut sockets = SocketSet::new(storage);
    let dhcp = sockets.add(dhcpv4::Socket::new());

    // SAFETY: as above; this is the one write of the stack.
    unsafe {
        *core::ptr::addr_of_mut!(STACK) = Some(Stack {
            device,
            iface,
            sockets,
            dhcp,
            address: None,
            gateway: None,
        });
    }

    crate::print_line(&[b"x86: net: dhcp started"]);
    let start = timer::ticks();
    while timer::ticks().wrapping_sub(start) < DHCP_TICKS {
        poll();
        if let Some(address) = configured() {
            crate::serial::write(b"x86: net ip=");
            dotted(address.address());
            crate::serial::write_byte(b'\n');
            if let Some(router) = gateway() {
                crate::serial::write(b"x86: net gateway=");
                dotted(router);
                crate::serial::write_byte(b'\n');
            }
            return Some(address.address());
        }
    }
    crate::print_line(&[b"x86: net: dhcp timed out"]);
    None
}

/// Drive the stack once: move frames, run sockets, and apply any DHCP lease.
/// Cheap when there is nothing to do.
pub fn poll() {
    // SAFETY: single-threaded; the stack is created once by `init` and not
    // borrowed across this call.
    let Some(stack) = (unsafe { (*core::ptr::addr_of_mut!(STACK)).as_mut() }) else {
        return;
    };
    stack
        .iface
        .poll(now(), &mut stack.device, &mut stack.sockets);

    if let Some(dhcpv4::Event::Configured(lease)) =
        stack.sockets.get_mut::<dhcpv4::Socket>(stack.dhcp).poll()
    {
        irix7_netif::apply_lease(&mut stack.iface, lease.address, lease.router);
        stack.address = Some(lease.address);
        stack.gateway = lease.router;
    }
}

/// The leased address, once DHCP has configured the interface.
pub fn configured() -> Option<Ipv4Cidr> {
    // SAFETY: read-only access on the same single thread.
    unsafe { (*core::ptr::addr_of!(STACK)).as_ref()?.address }
}

fn gateway() -> Option<Ipv4Address> {
    // SAFETY: read-only access on the same single thread.
    unsafe { (*core::ptr::addr_of!(STACK)).as_ref()?.gateway }
}

/// Run `f` against the socket set, or return `None` before `init`.
pub fn with_sockets<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
    // SAFETY: single-threaded; the stack is not borrowed elsewhere during `f`.
    unsafe {
        (*core::ptr::addr_of_mut!(STACK))
            .as_mut()
            .map(|s| f(&mut s.sockets))
    }
}

/// Run `f` against the interface and the socket set together.
pub fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
    // SAFETY: as for `with_sockets`.
    unsafe {
        (*core::ptr::addr_of_mut!(STACK))
            .as_mut()
            .map(|s| f(&mut s.iface, &mut s.sockets))
    }
}
