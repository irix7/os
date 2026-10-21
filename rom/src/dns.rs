//! DNS resolution for the ROM's netboot path.
//!
//! Uses smoltcp's poll-based [`DnsSocket`], driven by the same `iface.poll` /
//! `clock_tick` loop that carries DHCP, NTP and HTTP.  A single query slot is
//! enough: the ROM resolves one hostname at a time, and the caller
//! (`http_get`) waits on the resolver inside its existing poll loop.

use alloc::vec::Vec;
use core::fmt;
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::dns::{QueryHandle, Socket as DnsSocket};
use smoltcp::wire::DnsQueryType;
use smoltcp::wire::{IpAddress, Ipv4Address};

const DNS_QUERY_SLOTS: usize = 1;

/// Result of a DNS lookup.
pub struct LookupResult {
    pub addresses: Vec<IpAddress>,
}

/// A poll-driven DNS resolver.
///
/// Created with `new` (which registers the DNS socket in `socket_set` and
/// returns the socket handle).  Start a lookup with `start`, then call `poll`
/// after each `iface.poll` + `clock_tick` round until `is_done` returns true,
/// then take the result with `take`.
pub struct DnsResolver {
    socket_handle: SocketHandle,
    query_handle: Option<QueryHandle>,
    done: bool,
    result: Option<Result<LookupResult, ()>>,
}

impl DnsResolver {
    /// Register the DNS socket in `socket_set` and return a resolver ready for
    /// lookups.  `servers` is the ordered list of DNS servers (e.g. from the
    /// DHCP lease, falling back to public resolvers).
    pub fn new(socket_set: &mut SocketSet, servers: &[IpAddress]) -> Option<Self> {
        // Leaked static query storage so the socket can borrow it for its
        // whole life (smoltcp's Borrowed ManagedSlice variant needs no alloc).
        let queries: &'static mut [Option<smoltcp::socket::dns::DnsQuery>; DNS_QUERY_SLOTS] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([None; DNS_QUERY_SLOTS]));
        let queries: &mut [Option<smoltcp::socket::dns::DnsQuery>] = queries;
        let socket = DnsSocket::new(servers, queries);
        let socket_handle = socket_set.add(socket);
        Some(Self {
            socket_handle,
            query_handle: None,
            done: false,
            result: None,
        })
    }

    /// Start resolving `name` (an A query; AAAA is a follow-up).  Returns the
    /// query handle or an error if no query slot is free.
    pub fn start(
        &mut self,
        iface: &mut Interface,
        socket_set: &mut SocketSet,
        name: &str,
    ) -> Result<QueryHandle, smoltcp::socket::dns::StartQueryError> {
        let socket = socket_set.get_mut::<DnsSocket>(self.socket_handle);
        let handle = socket.start_query(iface.context(), name, DnsQueryType::A)?;
        self.query_handle = Some(handle);
        self.done = false;
        self.result = None;
        Ok(handle)
    }

    /// Drive the resolver forward.  Call after each `iface.poll` +
    /// `clock_tick` round until `is_done` returns true.
    pub fn poll(&mut self, socket_set: &mut SocketSet) {
        if self.done || self.query_handle.is_none() {
            return;
        }
        let socket = socket_set.get_mut::<DnsSocket>(self.socket_handle);
        let handle = self.query_handle.unwrap();
        match socket.get_query_result(handle) {
            Ok(addresses) => {
                // smoltcp returns a heapless::Vec; collect into an owning
                // alloc::vec::Vec so the result outlives the socket borrow.
                let addresses: alloc::vec::Vec<IpAddress> = addresses.into_iter().collect();
                self.result = Some(Ok(LookupResult { addresses }));
                self.done = true;
            }
            Err(smoltcp::socket::dns::GetQueryResultError::Pending) => {
                // Still waiting on the server; the socket's internal timers
                // are advanced by `iface.poll`/`clock_tick` in the caller.
            }
            Err(_) => {
                self.result = Some(Err(()));
                self.done = true;
            }
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn take_result(&mut self) -> Option<Result<LookupResult, ()>> {
        self.result.take()
    }
}

impl fmt::Debug for DnsResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DnsResolver")
            .field("done", &self.done)
            .finish()
    }
}

/// Pick the DNS servers for the resolver, in preference order:
/// 1. the DNS servers from the DHCP lease,
/// 2. the gateway (the iris NAT forwards DNS),
/// 3. well-known public resolvers.
pub fn default_dns_servers(
    lease_dns: &[Ipv4Address],
    gateway: Option<Ipv4Address>,
) -> alloc::vec::Vec<IpAddress> {
    let mut out = alloc::vec::Vec::new();
    for &s in lease_dns {
        let _ = out.push(IpAddress::Ipv4(s));
    }
    if let Some(g) = gateway {
        let _ = out.push(IpAddress::Ipv4(g));
    }
    let _ = out.push(IpAddress::Ipv4(Ipv4Address::new(8, 8, 8, 8)));
    let _ = out.push(IpAddress::Ipv4(Ipv4Address::new(1, 1, 1, 1)));
    out
}
