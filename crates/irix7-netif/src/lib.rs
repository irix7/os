// SPDX-License-Identifier: GPL-3.0-only
//! Interface configuration shared by the IRIX 7 kernels.

#![no_std]

use smoltcp::iface::Interface;
use smoltcp::wire::{IpCidr, Ipv4Address, Ipv4Cidr};

/// Make a DHCP lease the interface's only address, and its gateway the default
/// route.  Renewals call this too, so an address the server withdrew is removed
/// rather than kept beside the new one.
pub fn apply_lease(iface: &mut Interface, address: Ipv4Cidr, router: Option<Ipv4Address>) {
    iface.update_ip_addrs(|addrs| {
        addrs.clear();
        let _ = addrs.push(IpCidr::Ipv4(address));
    });
    if let Some(gateway) = router {
        let _ = iface.routes_mut().add_default_ipv4_route(gateway);
    }
}
