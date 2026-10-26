// SPDX-License-Identifier: GPL-3.0-only
//! The real M2 record format, wired to host TCP sockets.
//!
//! A record is `u32 little-endian length` followed by one `MeshEnvelope`,
//! produced by `mesh-transport` and `sys-primitives`.  Nothing here invents a
//! framing: the benchmark measures the shipped one.

use std::io::{Read, Write};
use std::net::TcpStream;

use mesh_transport::{decode_record, encode_record, record_len, StreamDecoder};
use sys_primitives::{
    mesh_flags, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, OpCode, StreamId, Tag,
};

/// A stable fake source node.
pub const CLIENT: NodeId = NodeId([0x11; 16]);
/// A stable fake destination node.
pub const SERVER: NodeId = NodeId([0x22; 16]);

/// Build one Ping request record with a body of `body_len` bytes.
pub fn ping_request(tag: u32, body_len: usize) -> Vec<u8> {
    let body = vec![0xa5u8; body_len];
    let frame = MessageFrame::inline(
        MessageKind::Request,
        OpCode::Ping,
        StreamId(1),
        Tag(tag),
        &body,
    )
    .expect("frame");
    let mut header = MeshHeader::new(CLIENT, SERVER, SERVER);
    header.flags = mesh_flags::PROBE;
    header.request_id = u64::from(tag);
    header.ttl = MeshHeader::MAX_HOPS;
    envelope_record(MeshEnvelope { header, frame })
}

/// Build one Ping response record answering `tag`.
pub fn ping_response(tag: u32, body_len: usize) -> Vec<u8> {
    let body = vec![0x5au8; body_len];
    let frame = MessageFrame::inline(
        MessageKind::Response,
        OpCode::Ping,
        StreamId(1),
        Tag(tag),
        &body,
    )
    .expect("frame");
    let mut header = MeshHeader::new(SERVER, CLIENT, CLIENT);
    header.flags = mesh_flags::PROBE | mesh_flags::REPLY;
    header.request_id = 0;
    header.reply_to = u64::from(tag);
    header.ttl = MeshHeader::MAX_HOPS;
    envelope_record(MeshEnvelope { header, frame })
}

fn envelope_record(envelope: MeshEnvelope<'_>) -> Vec<u8> {
    let mut out = vec![0u8; record_len(&envelope).expect("record length")];
    let written = encode_record(&envelope, &mut out).expect("encode");
    out.truncate(written);
    out
}

/// The encoded record length for one envelope, using the real encoder.
pub fn envelope_record_len(envelope: &MeshEnvelope<'_>) -> usize {
    record_len(envelope).expect("record length")
}

/// A blocking record reader over a TCP stream.
pub struct RecordReader {
    stream: TcpStream,
    decoder: StreamDecoder,
    chunk: [u8; 4096],
}

impl RecordReader {
    /// Wrap a stream.
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            decoder: StreamDecoder::new(),
            chunk: [0u8; 4096],
        }
    }

    /// Read until exactly one record is buffered; return its byte length.
    ///
    /// The decoded envelope borrows the decoder, so the caller reads the body
    /// through [`Self::body`] or copies the record with [`Self::record`].
    pub fn read_record_len(&mut self) -> std::io::Result<usize> {
        loop {
            if self.decoder.next_record().expect("decoder").is_some() {
                return Ok(self.decoder.next_record().expect("decoder").unwrap().len());
            }
            let read = self.stream.read(&mut self.chunk)?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "peer closed",
                ));
            }
            let mut offset = 0;
            while offset < read {
                let used = self
                    .decoder
                    .push(&self.chunk[offset..read])
                    .expect("decoder push");
                if used == 0 {
                    break;
                }
                offset += used;
                if self.decoder.next_record().expect("decoder").is_some() {
                    break;
                }
            }
        }
    }

    /// The buffered record bytes, including the length prefix.
    pub fn record(&self) -> &[u8] {
        self.decoder.next_record().expect("decoder").unwrap_or(&[])
    }

    /// Release the buffered record.
    pub fn consume(&mut self) {
        self.decoder.consume_record().expect("consume");
    }

    /// The body of the buffered record.
    pub fn body(&self) -> Vec<u8> {
        let record = self.record();
        let envelope = decode_record(record).expect("decode");
        envelope.frame.body.to_vec()
    }
}

/// Write one whole record, tolerating short writes.
pub fn write_record(stream: &mut TcpStream, record: &[u8]) -> std::io::Result<()> {
    stream.write_all(record)
}

/// A non-blocking record reader used by the tick emulation.
pub struct NonBlockingReader {
    stream: TcpStream,
    decoder: StreamDecoder,
    chunk: [u8; 1024],
}

impl NonBlockingReader {
    /// Wrap a stream that is already non-blocking.
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            decoder: StreamDecoder::new(),
            chunk: [0u8; 1024],
        }
    }

    /// Run up to `reads` reads, copying bytes into the decoder.
    ///
    /// Returns `false` when the peer closed.  This mirrors the module's
    /// `READ_CHUNK = 1024` and `READS_PER_TICK = 8`.
    pub fn pump(&mut self, reads: usize) -> std::io::Result<bool> {
        for _ in 0..reads {
            match self.stream.read(&mut self.chunk) {
                Ok(0) => return Ok(false),
                Ok(count) => {
                    let mut offset = 0;
                    while offset < count {
                        let used = self
                            .decoder
                            .push(&self.chunk[offset..count])
                            .expect("decoder push");
                        if used == 0 {
                            break;
                        }
                        offset += used;
                        if self.decoder.next_record().expect("decoder").is_some() {
                            break;
                        }
                    }
                    if self.decoder.next_record().expect("decoder").is_some() {
                        // One record per tick is the M2-2 note in
                        // modules/mcp/src/mesh.rs; the module drains the rest
                        // on the next tick.  Keep the same shape.
                        return Ok(true);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
                Err(_) => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Whether a complete record is buffered.
    pub fn ready(&self) -> bool {
        self.decoder.next_record().expect("decoder").is_some()
    }

    /// The buffered record.
    pub fn record(&self) -> &[u8] {
        self.decoder.next_record().expect("decoder").unwrap_or(&[])
    }

    /// Release the buffered record.
    pub fn consume(&mut self) {
        self.decoder.consume_record().expect("consume");
    }
}
