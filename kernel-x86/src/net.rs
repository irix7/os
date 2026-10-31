// SPDX-License-Identifier: GPL-3.0-only
//! The x86_64 network stack: smoltcp over the polled virtio-net driver, with
//! DHCP for the address.  This is the same stack the Indy kernel runs, on a
//! different device.

use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address};

use crate::timer;
use crate::virtio_net::{Nic, MAX_FRAME};

/// How long DHCP may take before the machine reports it and carries on.
const DHCP_TICKS: u32 = 500; // 5 s at 100 Hz

/// smoltcp's clock, from the PIT tick count.
fn now() -> Instant {
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

static mut SOCKET_STORAGE: [SocketStorage<'static>; 1] = [SocketStorage::EMPTY; 1];

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
/// not answer in time.
pub fn init() -> Option<Ipv4Address> {
    let Some(nic) = crate::virtio_net::probe() else {
        crate::print_line(&[b"x86: net: no virtio-net device"]);
        return None;
    };
    let mac = nic.mac();
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
    let mut iface = Interface::new(config, &mut device, now());

    // SAFETY: the machine is single-threaded and this runs once, before the
    // socket set is used anywhere else.
    let storage = unsafe { &mut *core::ptr::addr_of_mut!(SOCKET_STORAGE) };
    let mut sockets = SocketSet::new(&mut storage[..]);
    let dhcp = sockets.add(dhcpv4::Socket::new());

    let start = timer::ticks();
    while timer::ticks().wrapping_sub(start) < DHCP_TICKS {
        iface.poll(now(), &mut device, &mut sockets);
        let socket = sockets.get_mut::<dhcpv4::Socket>(dhcp);
        if let Some(dhcpv4::Event::Configured(lease)) = socket.poll() {
            let address = lease.address;
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::Ipv4(address));
            });
            crate::serial::write(b"x86: net ip=");
            dotted(address.address());
            crate::serial::write_byte(b'\n');
            if let Some(router) = lease.router {
                let _ = iface.routes_mut().add_default_ipv4_route(router);
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
