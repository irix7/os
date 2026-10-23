// SPDX-License-Identifier: GPL-3.0-only
//! Byte-order codec.
//!
//! EROFS is a little-endian format.  The reader uses [`LittleEndian`] for the
//! on-disk structures, but every accessor takes a [`ByteOrder`] type parameter
//! so the same decoder can be exercised against a big-endian copy without
//! changing the format.  The byte order is never chosen at run time.

/// A byte order used by the on-disk decoder.
pub trait ByteOrder: Copy + Clone + Default {
    /// Decode a 16-bit value from its on-disk bytes.
    fn read_u16(bytes: [u8; 2]) -> u16;
    /// Decode a 32-bit value from its on-disk bytes.
    fn read_u32(bytes: [u8; 4]) -> u32;
    /// Decode a 64-bit value from its on-disk bytes.
    fn read_u64(bytes: [u8; 8]) -> u64;
}

/// Little-endian decoding, the native EROFS byte order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LittleEndian;

impl ByteOrder for LittleEndian {
    fn read_u16(bytes: [u8; 2]) -> u16 {
        u16::from_le_bytes(bytes)
    }

    fn read_u32(bytes: [u8; 4]) -> u32 {
        u32::from_le_bytes(bytes)
    }

    fn read_u64(bytes: [u8; 8]) -> u64 {
        u64::from_le_bytes(bytes)
    }
}

/// Big-endian decoding, used only by the byte-order codec tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BigEndian;

impl ByteOrder for BigEndian {
    fn read_u16(bytes: [u8; 2]) -> u16 {
        u16::from_be_bytes(bytes)
    }

    fn read_u32(bytes: [u8; 4]) -> u32 {
        u32::from_be_bytes(bytes)
    }

    fn read_u64(bytes: [u8; 8]) -> u64 {
        u64::from_be_bytes(bytes)
    }
}

/// Read a 16-bit value at `offset`, or `None` if the slice is too short.
#[must_use]
pub fn u16_at<O: ByteOrder>(data: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let bytes = data.get(offset..end)?;
    Some(O::read_u16([bytes[0], bytes[1]]))
}

/// Read a 32-bit value at `offset`, or `None` if the slice is too short.
#[must_use]
pub fn u32_at<O: ByteOrder>(data: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let bytes = data.get(offset..end)?;
    Some(O::read_u32([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Read a 64-bit value at `offset`, or `None` if the slice is too short.
#[must_use]
pub fn u64_at<O: ByteOrder>(data: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let bytes = data.get(offset..end)?;
    Some(O::read_u64([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}
