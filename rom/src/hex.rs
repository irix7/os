// SPDX-License-Identifier: GPL-3.0-only
//! Hex encoding helpers (lowercase).
//!
//! Small, dependency-free hex encoding for printing digests and MAC addresses.

/// Encode a single nibble (0-15) as a lowercase hex ASCII digit.
pub const fn nibble(n: u8) -> u8 {
    match n {
        0 => b'0',
        1 => b'1',
        2 => b'2',
        3 => b'3',
        4 => b'4',
        5 => b'5',
        6 => b'6',
        7 => b'7',
        8 => b'8',
        9 => b'9',
        10 => b'a',
        11 => b'b',
        12 => b'c',
        13 => b'd',
        14 => b'e',
        15 => b'f',
        _ => b'?',
    }
}

/// Encode `bytes` as lowercase hex into `out`, which must have at least
/// `2 * bytes.len()` capacity. Returns the number of bytes written.
pub fn encode_into(bytes: &[u8], out: &mut [u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() && (i * 2 + 1) < out.len() {
        out[i * 2] = nibble((bytes[i] >> 4) & 0xF);
        out[i * 2 + 1] = nibble(bytes[i] & 0xF);
        i += 1;
    }
    i * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_abc() {
        let mut out = [0u8; 6];
        let n = encode_into(b"abc", &mut out);
        assert_eq!(n, 6);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "616263");
    }

    #[test]
    fn hex_empty() {
        let mut out = [0u8; 2];
        let n = encode_into(b"", &mut out);
        assert_eq!(n, 0);
    }

    #[test]
    fn hex_truncates() {
        let mut out = [0u8; 4];
        let n = encode_into(b"abcd", &mut out);
        assert_eq!(n, 4);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "6162");
    }

    #[test]
    fn hex_full() {
        let mut out = [0u8; 8];
        let n = encode_into(b"abcd", &mut out);
        assert_eq!(n, 8);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "61626364");
    }
}
