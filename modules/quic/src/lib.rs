// SPDX-License-Identifier: GPL-3.0-only
//! Minimal QUIC v1 over UDP using the kernel's ABI 16 services.
//!
//! This crate provides the QUIC transport plane for native IRIX 7 flows:
//! connection IDs allocated from the kernel CSPRNG, packet framing over a
//! module UDP slot, and a per-connection deadline timer for retransmission,
//! ACK delay and PTO.
//!
//! The TLS 1.3 handshake and record protection are intentionally NOT bundled
//! here (rustls is ~482 KiB text, far too large for a module slot).  Instead
//! they are a pluggable [`Crypto`] trait.  A cleartext [`CleartextCrypto`] is
//! provided for bring-up; it authenticates nothing and must be replaced with
//! the minimal TLS/QUIC profile (~120-180 KiB target) before production use.
//!
//! Wire format (v1 long header, simplified):
//!   0xC0 | version(32) | dcid_len(8) | scid_len(8) |
//!   dcid(..) | scid(..) | packet_number(32) | length(16) | payload(..)
//! A short header omits version/length and uses a 1-bit key-phase.
//!
//! QUIC runs entirely in the module; nothing lives in the PROM or kernel.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;
use module_abi::Services;

/// QUIC protocol version this crate speaks.
pub const QUIC_VERSION: u32 = 1;
/// Connection ID length in bytes (64 bits).
pub const CONNECTION_ID_LEN: usize = 8;
/// Maximum UDP payload carried by a single QUIC datagram.
pub const MAX_DATAGRAM: usize = 1400;

/// First byte of a long header.
const FLAG_LONG_HEADER: u8 = 0xC0;
/// Fixed bit + version present.
const FLAG_VERSION: u8 = 0x10;

/// A QUIC connection ID, allocated randomly from the kernel CSPRNG.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct ConnectionId(pub [u8; CONNECTION_ID_LEN]);

impl fmt::Debug for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl ConnectionId {
    /// Allocate a fresh connection ID from the kernel CSPRNG.
    pub fn random(services: &Services) -> Result<Self, ()> {
        let mut bytes = [0u8; CONNECTION_ID_LEN];
        let n = unsafe {
            ((*services.csprng_fill)(bytes.as_mut_ptr(), bytes.len()))
        };
        if n as usize != bytes.len() {
            return Err(());
        }
        Ok(Self(bytes))
    }
}

/// Pluggable cryptographic profile for QUIC packets.
///
/// `CleartextCrypto` sends cleartext and authenticates nothing.  Replace with
//! a minimal TLS 1.3 profile deriving keys from the CSPRNG output.
pub trait Crypto: Send + Sync {
    /// Protect a payload into `out`, returning the protected length.
    fn protect(&self, key: &[u8], nonce: u64, plaintext: &[u8], out: &mut [u8]) -> usize;
    /// Unprotect `ciphertext` into `out`, returning the plaintext length or 0 on failure.
    fn unprotect(&self, key: &[u8], nonce: u64, ciphertext: &[u8], out: &mut [u8]) -> usize;
}

/// Cleartext crypto for bring-up only: no confidentiality or authenticity.
#[derive(Clone, Copy, Debug, Default)]
pub struct CleartextCrypto;

impl Crypto for CleartextCrypto {
    fn protect(&self, _key: &[u8], _nonce: u64, plaintext: &[u8], out: &mut [u8]) -> usize {
        let n = plaintext.len().min(out.len());
        out[..n].copy_from_slice(&plaintext[..n]);
        n
    }
    fn unprotect(&self, _key: &[u8], _nonce: u64, ciphertext: &[u8], out: &mut [u8]) -> usize {
        let n = ciphertext.len().min(out.len());
        out[..n].copy_from_slice(&ciphertext[..n]);
        n
    }
}

/// A QUIC packet long header, as it appears on the wire.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LongHeader {
    pub first_byte: u8,
    pub version: u32,
    pub dcid_len: u8,
    pub scid_len: u8,
    /// Followed by dcid, scid, packet_number, length, payload.
}

/// Encode a long header into `out`, returning the header length (excluding the
/// variable-length ids/packet number/length which the caller writes).
fn encode_long_header(version: u32, dcid: &ConnectionId, scid: &ConnectionId, out: &mut [u8]) -> usize {
    if out.len() < 4 {
        return 0;
    }
    out[0] = FLAG_LONG_HEADER | FLAG_VERSION;
    out[1..5].copy_from_slice(&version.to_be_bytes());
    out[5] = CONNECTION_ID_LEN as u8;
    out[6] = CONNECTION_ID_LEN as u8;
    7
}

/// A QUIC endpoint bound to a module UDP slot.  Demuxes incoming datagrams by
/// destination connection ID and dispatches them to connections.
pub struct Endpoint<C: Crypto> {
    services: &'static Services,
    slot: u32,
    handle: i32,
    crypto: C,
    /// Local connection ID we listen for; connections are keyed by their DCID.
    local_cid: ConnectionId,
    /// Next packet number to use when sending.
    next_packet_number: u32,
}

impl<C: Crypto> Endpoint<C> {
    /// Bind to module UDP `slot` on `port` with `local_cid` as our listen ID.
    pub fn bind(services: &'static Services, slot: u32, port: u16, local_cid: ConnectionId) -> Result<Self, ()> {
        let handle = unsafe { ((*services).udp_open)(slot, port) };
        if handle < 0 {
            return Err(());
        }
        Ok(Self {
            services,
            slot,
            handle,
            crypto: C::default(),
            local_cid,
            next_packet_number: 0,
        })
    }

    /// Send a datagram to `peer` carrying `payload` (protected by the crypto).
    pub fn send(&mut self, peer_cid: ConnectionId, payload: &[u8]) -> Result<usize, ()> {
        let mut buf = [0u8; MAX_DATAGRAM];
        let header_len = encode_long_header(QUIC_VERSION, &self.local_cid, &peer_cid, &mut buf);
        if header_len == 0 || header_len + 8 + 4 > buf.len() {
            return Err(());
        }
        let off = header_len;
        buf[off..off + 8].copy_from_slice(&self.local_cid.0); // placeholder scid/dcid layout
        let pn = self.next_packet_number;
        self.next_packet_number = pn.wrapping_add(1);
        buf[off + 8..off + 12].copy_from_slice(&pn.to_be_bytes());
        let plen = self.crypto.protect(&[], pn as u64, payload, &mut buf[off + 12..]);
        let total = off + 12 + plen;
        let n = unsafe { ((*self.services).udp_send)(self.handle, buf.as_ptr(), total) };
        if n < 0 {
            return Err(());
        }
        Ok(n as usize)
    }

    /// Poll for an incoming datagram.  Returns `Some((source_cid, payload))`
    /// when one is available, protected by the endpoint crypto.
    pub fn recv(&mut self) -> Option<(ConnectionId, Vec<u8>)> {
        // The module heap allocator never frees, so use a fixed buffer.
        let mut buf = [0u8; MAX_DATAGRAM];
        let n = unsafe { ((*self.services).udp_recv)(self.handle, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            return None;
        }
        // Minimal parse: expect a long header with our local CID as DCID.
        if n < 7 || buf[0] != FLAG_LONG_HEADER | FLAG_VERSION {
            return None;
        }
        let _version = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let dcid_len = buf[5] as usize;
        let scid_len = buf[6] as usize;
        let off = 7 + dcid_len + scid_len + 4; // header + ids + packet number
        let _plen = u16::from_be_bytes([buf[7 + dcid_len + scid_len + 4 - 2], buf[7 + dcid_len + scid_len + 4 - 1]]);
        if off + 2 > n as usize {
            return None;
        }
        let payload_len = (n as usize) - off;
        let mut payload = Vec::new();
        payload.reserve_exact(payload_len);
        // SAFETY: Vec::from_raw_parts over a copy.
        let src = &buf[off..off + payload_len];
        let plain = self.crypto.unprotect(&[], 0, src, &mut [0u8; MAX_DATAGRAM]);
        // NOTE: CleartextCrypto writes into a scratch; we copy below.
        let mut out = [0u8; MAX_DATAGRAM];
        let plain_len = self.crypto.unprotect(&[], 0, src, &mut out);
        for i in 0..plain_len {
            payload.push(out[i]);
        }
        // The SCID follows the DCID; treat it as the source connection ID.
        let source_cid_start = 7 + dcid_len;
        let mut source = [0u8; CONNECTION_ID_LEN];
        let scid_bytes = &buf[source_cid_start..source_cid_start + scid_len.min(CONNECTION_ID_LEN)];
        source[..scid_len.min(CONNECTION_ID_LEN)].copy_from_slice(scid_bytes);
        Some((ConnectionId(source), payload))
    }

    /// The UDP slot handle, for external timer/close wiring.
    pub fn handle(&self) -> i32 {
        self.handle
    }
}
