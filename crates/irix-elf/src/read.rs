// SPDX-License-Identifier: GPL-3.0-only
//! Bounds-checked big-endian field readers.

pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let raw: [u8; 2] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u16::from_be_bytes(raw))
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let raw: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u32::from_be_bytes(raw))
}
